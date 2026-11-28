//! Test-only (feature `stand-ins`): a scripted model and the recorder's reads, so a chat scenario recorded from the
//! Python daemon (engine/fixtures/daemon/chat_scenarios.json) replays through this real program.
//!
//! `FERBD_STAND_INS`' `"script"` names a JSON file the test wrote:
//!
//! - `calls`: every recorded model call (`key`, `events`, `tool_bridge_lines`), in the recording's order; `first`:
//!   the index of the first one this process makes (a restart continues the count); `gated`: those that answer only
//!   after the control socket's `release_model`.
//! - `supervise`, `systems`, `contexts`: what the Python supervisor read at each planner call, role instruction and
//!   worker context (engine/fixtures/supervisor/scenarios.json; `skills_line` and `abilities_line` already dumped), as
//!   queues: the environment and the live facts, the facts in every role's instructions, the relevant activity and
//!   the desktop observer's context.
//! - `spans` (`columns`, `rows`, `used`): the desktop observer's rows the recorder's daemon wrote, each written here
//!   once, under the next id, when the supervisor starts or finishes a tree (and at shutdown for a tree still open).
//! - `root`, `root_resolved`: the run's folder (written `/FX` in the recording, both spellings).
//! - `note`: `[path, text]`, the file the owner writes once the first checker has answered.
//! - `by_role` (a hand-written script, T-1277's lifecycle tests): in place of `calls`, one queue of calls per runtime
//!   role (`orchestrator`, `worker:default`, `checker`), each call taken by the next model call FERB makes in that
//!   role, whatever the order across roles. A call may carry `hold`: a name, so it answers only after the control
//!   socket's `release` of that name (or never, when FERB cancels it first); `repeat: true` answers every later call
//!   of its role too (a checker that accepts whatever is asked); `cancel_hold`: a name, so a cancel FERB sends ends
//!   the call only after the `release` of that name (a slow cancel, as a real model's run can take seconds to end). In its events and tool lines `@TREE@` is
//!   the newest task tree's id when the call starts. A role with no call left answers nothing until cancelled (a
//!   problem in the report). Unlike a recorded call, a cancelled one ends there, as a real model's run that FERB
//!   cancelled does: no tool call or answer after the cancel.
//!
//! - `front_door` (a test's script only, ORCH-735): the supervisor's front door decides each chat turn; a planner
//!   call's events may then hold `{"kind": "front_call", "name", "arguments"}`: the call is reported started, made
//!   through the socket as the tool bridge makes it (ORCH-736), and reported completed with its error flag; each reply
//!   is in the `script` report's `front_results`. A Python recording's script has neither.
//!
//! - `recorded_ids` and `db` (tests/front_door_replay.rs's front door run only, journal 265): the recording's ids, so
//!   the ids this run made in their place are written into whatever recorded content names them (see [`Translation`]).
//!   A Python recording's replay has neither, and nothing is translated.
//!
//! The scripted model makes its recorded tool calls as the tool bridge does: a connection to this daemon's socket as
//! `mcp_shim` with its run token, then each `tools.call` line; each reply is compared with the recorded one.
//!
//! Also here: the recorder's file facts (`fs_identity`: a file result's `modified_at`, owner, device, inode and
//! `mtime_ns` as the recorder invented them), the questions' counter ids, the byte-ceiling tokenizer, the
//! untrusted-data wrapper's fixed id and the new chat id.

use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use ferb_core::interfaces::IdSource;
use ferb_core::types::{
    ModelLimits, ModelRequest, RunHandle, RuntimeEvent, RuntimeEventKind, Task, ToolContext,
};
use ferb_gateway::tool::{Args, Tool, ToolError, ToolOutput};
use ferb_orchestrator::ports::BoxFuture;
use ferb_orchestrator::steering::ContinueError;
use ferb_supervisor::flow::Environment;
use ferb_supervisor::prompts::SystemInputs;
use ferb_supervisor::worker_context::Activity;
use ferb_worker::ports::{EventSource, ModelRuntime, RunControl, StartedRun};
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::basics::{lock, SharedDb};
use crate::compose::supervisor::Reads;

fn s(v: &Value) -> String {
    match v {
        Value::String(t) => t.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// What the scripted model and the recorded reads did, read back through the control socket.
#[derive(Default)]
pub struct ScriptState {
    /// `make_runtime`'s arguments, one per call made.
    pub made: Vec<Value>,
    /// Each started call: its index, task, instructions and parts (`/FX` written back).
    pub started: Vec<Value>,
    /// The calls whose final answer was given.
    pub answered: Vec<usize>,
    /// A tool call through the bridge whose reply differed from the recording.
    pub tool_diffs: Vec<String>,
    pub tool_calls: usize,
    pub supervise_used: usize,
    pub systems_used: usize,
    pub contexts_used: usize,
    pub spans_used: BTreeSet<usize>,
    /// The trees the desktop observer watches (a baseline written, no summary yet).
    pub watching: BTreeSet<String>,
    /// A read the recording didn't make.
    pub problems: Vec<String>,
    /// ORCH-736: each front door call a test's script made through the socket: [call, tool, arguments, the reply].
    pub front_results: Vec<Value>,
}

/// The scripted model, the recorded reads and their state.
pub struct Script {
    pub file: Value,
    pub state: Mutex<ScriptState>,
    supervise: Mutex<VecDeque<Value>>,
    /// The memory and browser ports' recorded answers, in order.
    ports: Mutex<VecDeque<Value>>,
    systems: Mutex<VecDeque<Value>>,
    contexts: Mutex<VecDeque<Value>>,
    pub released: AtomicBool,
    /// The questions asked so far in the scenario (the counter's state; `questions_from` at start).
    pub asked: Arc<AtomicU64>,
    pub released_now: tokio::sync::Notify,
    /// `by_role`'s queues and the holds released so far.
    by_role: Mutex<std::collections::BTreeMap<String, VecDeque<Value>>>,
    holds_released: Mutex<BTreeSet<String>>,
    note: Mutex<Option<(String, String)>>,
    /// Filled once the daemon is built.
    pub db: OnceLock<SharedDb>,
    pub ids: OnceLock<Arc<dyn IdSource>>,
    pub socket: OnceLock<PathBuf>,
    root: String,
    resolved: String,
    /// `recorded_ids`: the recording's ids and this run's in their place (None: nothing translated).
    translation: Option<Translation>,
}

/// The ids in a text, as tests/front_door_replay.rs's `Renumber` finds them: a run of letters and digits (not inside a
/// longer word) ending in "-" and six digits, an id counter's (`id-000021`, also inside `tm-id-000021` and
/// `id-000021.worker`). Each as its byte range.
fn id_spans(t: &str) -> Vec<(usize, usize)> {
    let b = t.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if (i == 0 || !b[i - 1].is_ascii_alphanumeric()) && b[i].is_ascii_alphanumeric() {
            let mut j = i;
            while j < b.len() && b[j].is_ascii_alphanumeric() {
                j += 1;
            }
            if j + 7 <= b.len()
                && b[j] == b'-'
                && b[j + 1..j + 7].iter().all(u8::is_ascii_digit)
                && (j + 7 == b.len() || !b[j + 7].is_ascii_alphanumeric())
            {
                out.push((i, j + 7));
                i = j + 7;
            } else {
                i = j;
            }
            continue;
        }
        i += 1;
    }
    out
}

/// A text with each id the map names written as its pair.
fn with_ids(text: &str, map: &std::collections::BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (a, b) in id_spans(text) {
        out.push_str(&text[at..a]);
        out.push_str(map.get(&text[a..b]).map_or(&text[a..b], String::as_str));
        at = b;
    }
    out.push_str(&text[at..]);
    out
}

/// A text with its ids taken out (each one `\u{1}`), and the ids in order.
fn skeleton(t: &str) -> (String, Vec<&str>) {
    let (mut out, mut ids, mut at) = (String::with_capacity(t.len()), Vec::new(), 0);
    for (a, b) in id_spans(t) {
        out.push_str(&t[at..a]);
        out.push('\u{1}');
        ids.push(&t[a..b]);
        at = b;
    }
    out.push_str(&t[at..]);
    (out, ids)
}

/// The ids of two texts paired where the texts are the same but for their ids, line by line: the lines matched in
/// order (the longest common run of lines that are equal with their ids taken out), each matched line's ids paired by
/// position. (recorded, this run's).
fn paired_ids(recorded: &str, actual: &str) -> Vec<(String, String)> {
    let a: Vec<(String, Vec<&str>)> = recorded.split('\n').map(skeleton).collect();
    let b: Vec<(String, Vec<&str>)> = actual.split('\n').map(skeleton).collect();
    let mut matched: Vec<(usize, usize)> = Vec::new();
    // the lines equal at the start and at the end, then the longest common run of the rest
    let mut start = 0;
    while start < a.len() && start < b.len() && a[start].0 == b[start].0 {
        matched.push((start, start));
        start += 1;
    }
    let mut end = 0;
    while end < a.len() - start
        && end < b.len() - start
        && a[a.len() - 1 - end].0 == b[b.len() - 1 - end].0
    {
        end += 1;
    }
    let (am, bm) = (&a[start..a.len() - end], &b[start..b.len() - end]);
    let (n, m) = (am.len(), bm.len());
    // a middle too large to align line by line is left unpaired (the prompts are a few hundred lines)
    if n > 0 && m > 0 && n * m <= 4_000_000 {
        let mut lcs = vec![0u32; (n + 1) * (m + 1)];
        let at = |i: usize, j: usize| i * (m + 1) + j;
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[at(i, j)] = if am[i].0 == bm[j].0 {
                    lcs[at(i + 1, j + 1)] + 1
                } else {
                    lcs[at(i + 1, j)].max(lcs[at(i, j + 1)])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if am[i].0 == bm[j].0 {
                matched.push((start + i, start + j));
                i += 1;
                j += 1;
            } else if lcs[at(i + 1, j)] >= lcs[at(i, j + 1)] {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    for k in 0..end {
        matched.push((a.len() - end + k, b.len() - end + k));
    }
    matched
        .into_iter()
        .flat_map(|(i, j)| {
            a[i].1
                .iter()
                .zip(&b[j].1)
                .map(|(x, y)| (x.to_string(), y.to_string()))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// (recorded id, this run's id) pairs.
type Pairs = Vec<(String, String)>;

/// Journal 265, the front door replay only: the front door's turn writes records the planner's didn't (each call's
/// span, ...), so the store hands out later ids that differ from the recording's, and recorded content naming one (a
/// scripted tool call's arguments, a checker's evidence, the owner's later call, a recorded read) would name an id
/// this run never made. Each such id is written as the one this run made in its place. The pairs come only from what
/// this run made, matched to the recording by order and kind, never guessed from text:
///
/// - the store's rows: each recorded table's ids (`id`) in row order against this run's rows of that table, and each
///   journal record against this run's record of the same kind and number, their ids paired where the two records are
///   the same but for their ids;
/// - what the scripted model was shown: each call's instructions and input against the recorded call's, and each tool
///   reply against the recorded one, line by line where they are the same but for their ids.
///
/// A recorded id paired with two different ids of this run is a problem in the report (a pairing gone wrong).
struct Translation {
    /// Each recorded table's ids in row order.
    tables: Vec<(String, Vec<String>)>,
    /// The recorded journal: each kind's records' data in order.
    journal: std::collections::BTreeMap<String, Vec<String>>,
    /// The recorded spans of a task: (id, task_id, kind) in order.
    spans: Vec<(String, String, String)>,
    db_path: String,
    db: Mutex<Option<rusqlite::Connection>>,
    /// The store's pairs as last worked out, with the store's row counts then.
    cache: Mutex<Option<(String, Pairs)>>,
    /// The pairs from what the scripted model was shown.
    shown: Mutex<std::collections::BTreeMap<String, String>>,
    /// The conflicts already reported.
    reported: Mutex<BTreeSet<String>>,
}

impl Translation {
    fn load(file: &Value) -> Option<Translation> {
        let ids = file.get("recorded_ids").filter(|v| v.is_object())?;
        let tables = ids["tables"]
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(t, v)| {
                        let ids = v
                            .as_array()
                            .map(|a| a.iter().map(s).collect())
                            .unwrap_or_default();
                        (t.clone(), ids)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut journal: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        for r in ids["journal"].as_array().into_iter().flatten() {
            journal.entry(s(&r[0])).or_default().push(s(&r[1]));
        }
        let cols: Vec<String> = file["spans"]["columns"]
            .as_array()
            .map(|a| a.iter().map(s).collect())
            .unwrap_or_default();
        let at = |name: &str| cols.iter().position(|c| c == name);
        let spans = match (at("id"), at("task_id"), at("kind")) {
            (Some(i), Some(t), Some(k)) => file["spans"]["rows"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|r| !r[t].is_null())
                .map(|r| (s(&r[i]), s(&r[t]), s(&r[k])))
                .collect(),
            _ => Vec::new(),
        };
        Some(Translation {
            tables,
            journal,
            spans,
            db_path: s(&file["db"]),
            db: Mutex::new(None),
            cache: Mutex::new(None),
            shown: Mutex::new(Default::default()),
            reported: Mutex::new(BTreeSet::new()),
        })
    }

    /// The store's pairs: this run's rows read through a connection of its own (never the daemon's, which a caller
    /// may hold), as committed.
    fn store_pairs(&self, mask: impl Fn(&str) -> String) -> Vec<(String, String)> {
        let mut db = lock(&self.db);
        if db.is_none() {
            *db = rusqlite::Connection::open_with_flags(
                &self.db_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .ok();
            if let Some(c) = db.as_ref() {
                let _ = c.busy_timeout(std::time::Duration::from_secs(2));
            }
        }
        let Some(conn) = db.as_ref() else {
            return Vec::new();
        };
        let column = |sql: &str| -> Vec<Vec<String>> {
            let Ok(mut st) = conn.prepare(sql) else {
                return Vec::new();
            };
            let n = st.column_count();
            st.query_map([], |r| {
                (0..n)
                    .map(|i| r.get::<_, Option<String>>(i).map(|v| v.unwrap_or_default()))
                    .collect::<Result<Vec<_>, _>>()
            })
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
        };
        // the pairs are worked out again only once a row was added (a row's id never changes)
        let mut mark = String::new();
        for table in self
            .tables
            .iter()
            .map(|(t, _)| t.as_str())
            .chain(["journal", "spans"])
        {
            let got = column(&format!(
                "SELECT COUNT(*) || ':' || COALESCE(MAX(rowid), 0) FROM \"{table}\""
            ));
            mark.push_str(
                got.first()
                    .and_then(|r| r.first())
                    .map_or("-", String::as_str),
            );
            mark.push(' ');
        }
        if let Some((had, pairs)) = lock(&self.cache).as_ref() {
            if *had == mark {
                return pairs.clone();
            }
        }
        let mut out = Vec::new();
        for (table, recorded) in &self.tables {
            let rows = column(&format!("SELECT id FROM \"{table}\" ORDER BY rowid"));
            for (want, got) in recorded.iter().zip(&rows) {
                out.extend(paired_ids(want, &got[0]));
            }
        }
        let rows = column("SELECT kind, data_json FROM journal ORDER BY seq");
        let mut nth: std::collections::BTreeMap<&str, usize> = Default::default();
        for r in &rows {
            let k = nth.entry(r[0].as_str()).or_insert(0);
            let want = self.journal.get(&r[0]).and_then(|all| all.get(*k));
            *k += 1;
            if let Some(data) = want {
                let got = mask(&r[1]);
                let (a, b) = (skeleton(data), skeleton(&got));
                if a.0 == b.0 {
                    out.extend(
                        a.1.iter()
                            .zip(&b.1)
                            .map(|(x, y)| (x.to_string(), y.to_string())),
                    );
                }
            }
        }
        // the spans of a task: each kind's in order against this run's of the same kind and task (the task's id
        // written as this run's by the pairs above); a span of no task (the planner's, the front door's own calls) is
        // left unpaired
        let mut base: std::collections::BTreeMap<String, String> = Default::default();
        for (a, b) in &out {
            base.entry(a.clone()).or_insert_with(|| b.clone());
        }
        let rows =
            column("SELECT id, task_id, kind FROM spans WHERE task_id IS NOT NULL ORDER BY rowid");
        let mut nth: std::collections::BTreeMap<(String, String), usize> = Default::default();
        for r in &rows {
            let key = (r[2].clone(), r[1].clone());
            let k = nth.entry(key.clone()).or_insert(0);
            let want = self
                .spans
                .iter()
                .filter(|(_, task, kind)| *kind == key.0 && with_ids(task, &base) == key.1)
                .nth(*k);
            *k += 1;
            if let Some((id, _, _)) = want {
                out.extend(paired_ids(id, &r[0]));
            }
        }
        *lock(&self.cache) = Some((mark, out.clone()));
        out
    }
}

impl Script {
    pub fn load(path: &str) -> Option<Arc<Script>> {
        let file: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        let queue = |k: &str| {
            Mutex::new(
                file[k]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect::<VecDeque<_>>(),
            )
        };
        let note = file["note"].as_array().and_then(|n| {
            Some((
                n.first()?.as_str()?.to_string(),
                n.get(1)?.as_str()?.to_string(),
            ))
        });
        let used: BTreeSet<usize> = file["spans"]["used"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_u64())
                    .map(|v| v as usize)
                    .collect()
            })
            .unwrap_or_default();
        Some(Arc::new(Script {
            supervise: queue("supervise"),
            ports: queue("ports"),
            systems: queue("systems"),
            contexts: queue("contexts"),
            state: Mutex::new(ScriptState {
                spans_used: used,
                ..ScriptState::default()
            }),
            released: AtomicBool::new(false),
            asked: Arc::new(AtomicU64::new(file["questions_from"].as_u64().unwrap_or(0))),
            released_now: tokio::sync::Notify::new(),
            by_role: Mutex::new(
                file["by_role"]
                    .as_object()
                    .map(|m| {
                        m.iter()
                            .map(|(k, v)| {
                                (k.clone(), v.as_array().cloned().unwrap_or_default().into())
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            holds_released: Mutex::new(BTreeSet::new()),
            note: Mutex::new(note),
            db: OnceLock::new(),
            ids: OnceLock::new(),
            socket: OnceLock::new(),
            root: s(&file["root"]),
            resolved: s(&file["root_resolved"]),
            translation: Translation::load(&file),
            file,
        }))
    }

    /// Journal 265: each recorded id paired so far with the one this run made in its place (the store's pairs, then
    /// what the model was shown; a recorded id paired two ways is a problem, and what the model was shown wins).
    fn id_map(&self) -> std::collections::BTreeMap<String, String> {
        let Some(t) = &self.translation else {
            return Default::default();
        };
        let mut map: std::collections::BTreeMap<String, String> = Default::default();
        let mut conflicts = Vec::new();
        let shown: Vec<(String, String)> = lock(&t.shown)
            .iter()
            .map(|(a, b)| (a.clone(), b.clone()))
            .collect();
        for (want, got) in t.store_pairs(|x| self.mask(x)).into_iter().chain(shown) {
            match map.get(&want) {
                Some(had) if *had != got => conflicts.push(format!(
                    "the recorded id {want} paired with both {had} and {got}"
                )),
                _ => {}
            }
            map.insert(want, got);
        }
        for c in conflicts {
            if lock(&t.reported).insert(c.clone()) {
                self.problem(c);
            }
        }
        map
    }

    /// Journal 265: recorded content with each recorded id written as the one this run made in its place.
    pub fn to_run(&self, v: &Value) -> Value {
        if self.translation.is_none() {
            return v.clone();
        }
        let map = self.id_map();
        fn walk(v: &Value, map: &std::collections::BTreeMap<String, String>) -> Value {
            match v {
                Value::String(t) => json!(with_ids(t, map)),
                Value::Array(a) => Value::Array(a.iter().map(|x| walk(x, map)).collect()),
                Value::Object(m) => Value::Object(
                    m.iter()
                        .map(|(k, x)| (with_ids(k, map), walk(x, map)))
                        .collect(),
                ),
                other => other.clone(),
            }
        }
        walk(v, &map)
    }

    /// Journal 265: what the scripted model was shown (`actual`, `/FX` written) against what the recorded model was
    /// (`recorded`): their ids paired where the two are the same but for them.
    fn learn(&self, recorded: &str, actual: &str) {
        let Some(t) = &self.translation else {
            return;
        };
        let pairs = paired_ids(recorded, actual);
        let mut conflicts = Vec::new();
        {
            let mut shown = lock(&t.shown);
            for (want, got) in pairs {
                match shown.get(&want) {
                    Some(had) if *had != got => conflicts.push(format!(
                        "the recorded id {want} shown as both {had} and {got}"
                    )),
                    _ => {
                        shown.insert(want, got);
                    }
                }
            }
        }
        for c in conflicts {
            if lock(&t.reported).insert(c.clone()) {
                self.problem(c);
            }
        }
    }

    /// The run's folder written `/FX`, its resolved spelling first.
    pub fn mask(&self, text: &str) -> String {
        if self.root.is_empty() {
            return text.to_string();
        }
        text.replace(&self.resolved, "/FX")
            .replace(&self.root, "/FX")
    }

    fn problem(&self, what: String) {
        lock(&self.state).problems.push(what);
    }

    fn calls(&self) -> &[Value] {
        self.file["calls"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn first(&self) -> usize {
        self.file["first"].as_u64().unwrap_or(0) as usize
    }

    /// `by_role`: the next call of this role (None when the script has no queues).
    fn role_call(&self, role: &str) -> Option<Value> {
        let mut q = lock(&self.by_role);
        if q.is_empty() {
            return None;
        }
        let next = q.get_mut(role).and_then(|calls| match calls.front() {
            Some(c) if c["repeat"] == json!(true) => Some(c.clone()),
            _ => calls.pop_front(),
        });
        if next.is_none() {
            drop(q);
            self.problem(format!("a {role} call the script has none left for"));
        }
        Some(next.unwrap_or(Value::Null))
    }

    fn has_roles(&self) -> bool {
        self.file["by_role"].is_object()
    }

    /// The newest task tree's id (`@TREE@`).
    fn newest_tree(&self) -> String {
        let Some(db) = self.db.get() else {
            return String::new();
        };
        let db = lock(db);
        db.conn()
            .query_row(
                "SELECT id FROM tasks WHERE parent_id IS NULL ORDER BY rowid DESC LIMIT 1",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap_or_default()
    }

    /// A value with `@TREE@` written as the newest tree's id.
    fn with_tree(&self, v: &Value) -> Value {
        let text = v.to_string();
        if !text.contains("@TREE@") {
            return v.clone();
        }
        let tree = self.newest_tree();
        serde_json::from_str(&text.replace("@TREE@", &tree)).unwrap_or_else(|_| v.clone())
    }

    /// The control socket's `release`: a held call of this name answers now.
    pub fn release(&self, name: &str) {
        lock(&self.holds_released).insert(name.to_string());
        self.released_now.notify_waiters();
    }

    fn hold_released(&self, name: &str) -> bool {
        lock(&self.holds_released).contains(name)
    }

    fn gated(&self, n: usize) -> bool {
        self.file["gated"]
            .as_array()
            .is_some_and(|g| g.iter().any(|v| v.as_u64() == Some(n as u64)))
    }

    /// The owner's file, written when its moment comes (the first checker's answer, or a `note` step).
    pub fn apply_note(&self) {
        let note = lock(&self.note).take();
        if let Some((path, text)) = note {
            let _ = std::fs::write(path, text);
        }
    }

    /// The recorded desktop-observer row of this kind for this tree, written now under the next id.
    fn write_span(&self, kind: &str, tree_id: &str) {
        let cols: Vec<String> = self.file["spans"]["columns"]
            .as_array()
            .map(|a| a.iter().map(s).collect())
            .unwrap_or_default();
        let at = |name: &str| cols.iter().position(|c| c == name);
        let (Some(kc), Some(tc), Some(ic)) = (at("kind"), at("task_id"), at("id")) else {
            return;
        };
        // journal 265: the recorded rows name the recorded tree (this run's in the front door replay)
        let rows = self
            .to_run(&self.file["spans"]["rows"])
            .as_array()
            .cloned()
            .unwrap_or_default();
        let found = {
            let mut st = lock(&self.state);
            let found = rows.iter().enumerate().find(|(i, r)| {
                !st.spans_used.contains(i) && r[kc] == json!(kind) && r[tc] == json!(tree_id)
            });
            if let Some((i, _)) = found {
                st.spans_used.insert(i);
            }
            found.map(|(_, r)| r.clone())
        };
        let Some(mut row) = found else {
            self.problem(format!("no recorded {kind} span for {tree_id}"));
            return;
        };
        let (Some(ids), Some(db)) = (self.ids.get(), self.db.get()) else {
            return;
        };
        row[ic] = json!(ids.new_id());
        let to_sql = |v: &Value| match v {
            Value::Null => rusqlite::types::Value::Null,
            Value::String(t) => rusqlite::types::Value::Text(t.replace("/FX", &self.root)),
            Value::Number(n) if n.is_f64() => {
                rusqlite::types::Value::Real(n.as_f64().unwrap_or(0.0))
            }
            Value::Number(n) => rusqlite::types::Value::Integer(n.as_i64().unwrap_or(0)),
            other => rusqlite::types::Value::Text(other.to_string()),
        };
        let params: Vec<rusqlite::types::Value> = row
            .as_array()
            .map(|r| r.iter().map(to_sql).collect())
            .unwrap_or_default();
        let db = lock(db);
        if let Err(e) = db.conn().execute(
            &format!(
                "INSERT INTO spans ({}) VALUES ({})",
                cols.join(", "),
                vec!["?"; cols.len()].join(", ")
            ),
            rusqlite::params_from_iter(params),
        ) {
            drop(db);
            self.problem(format!("the recorded {kind} span: {e}"));
        }
    }

    /// The state as the control socket returns it.
    pub fn report(&self) -> Value {
        let st = lock(&self.state);
        json!({"made": st.made, "started": st.started, "answered": st.answered, "tool_diffs": st.tool_diffs,
               "tool_calls": st.tool_calls, "supervise_used": st.supervise_used,
               "systems_used": st.systems_used, "contexts_used": st.contexts_used,
               "spans_used": st.spans_used, "problems": st.problems,
               "questions": self.asked.load(Ordering::SeqCst), "front_results": st.front_results})
    }
}

/// The recorder's reads and the scripted model, as the supervisor's [`Reads`].
pub struct ScriptReads(pub Arc<Script>);

impl ScriptReads {
    fn pop(&self, q: &Mutex<VecDeque<Value>>, what: &str) -> Value {
        let next = lock(q).pop_front();
        match next {
            // journal 265: a recorded read names this run's ids (in the front door replay)
            Some(v) => self.0.to_run(&v),
            None => {
                self.0.problem(format!(
                    "the supervisor read {what} more often than the recording did"
                ));
                Value::Null
            }
        }
    }
}

impl ScriptReads {
    /// The next recorded answer of this port (a port the recording didn't call is a problem).
    fn port(&self, name: &str, got: Value) -> Value {
        let rec = {
            let mut q = lock(&self.0.ports);
            let at = q.iter().position(|r| r["port"] == name);
            at.and_then(|i| q.remove(i))
        };
        let Some(rec) = rec else {
            self.0
                .problem(format!("port {name}: not called in the recording"));
            return Value::Null;
        };
        let rec = self.0.to_run(&rec);
        let want = json!({"args": rec["args"], "kwargs": rec["kwargs"]});
        if want != got {
            self.0.problem(format!(
                "port {name} arguments\n    python: {want}\n    rust:   {got}"
            ));
        }
        rec["answer"].clone()
    }
}

impl Reads for ScriptReads {
    // decision: ORCH-654 (the Python recordings ran their checker on every report)
    fn checks_unacted_not_done(&self) -> bool {
        true
    }
    // decision: ORCH-694 (the Python recordings' checker was rerun and checked every report)
    fn lean_check(&self) -> bool {
        false
    }
    // decision: GATE-609 (the Python recordings' card replies reached the task with no planner call)
    fn reads_card_replies(&self) -> bool {
        false
    }
    // decision: ORCH-725 (the Python recordings' planner read the whole chat, its ages line and the latest result)
    fn bounded_planner(&self) -> bool {
        false
    }
    // decision: ORCH-731 (the Python recordings closed every program before the answer was used)
    fn answer_before_close(&self) -> bool {
        false
    }
    // decision: ORCH-760 — on for the stand-ins (no recording reports a conversation's size, so no replay changes);
    // a test's script turns it off
    fn fresh_run(&self) -> bool {
        self.0.file["fresh_run"].as_bool().unwrap_or(true)
    }
    // decision: ORCH-735 (the Python recordings' planner answered with its form): only a test's script turns the front
    // door on (`front_door`, tests/front_door_replay.rs)
    fn front_door(&self) -> bool {
        self.0.file["front_door"].as_bool().unwrap_or(false)
    }
    fn memory_blocks<'a>(
        &'a self,
        message: &'a str,
        focus: &'a str,
    ) -> BoxFuture<'a, Option<Vec<ferb_supervisor::history::Sourced>>> {
        // the memory port's candidates, as recorded: ranked blocks of recorded candidates aren't rebuilt here
        let answer = self.port(
            "memory_candidates",
            json!({"args": [message, focus], "kwargs": {}}),
        );
        if answer.as_array().is_some_and(|a| !a.is_empty()) {
            self.0
                .problem("port memory_candidates: recorded candidates aren't replayed".into());
        }
        Box::pin(async { Some(Vec::new()) })
    }
    fn browsing(&self, attended: bool, request_id: &str) -> Option<Map<String, Value>> {
        Some(
            self.port(
                "browsing",
                json!({"args": [attended, request_id], "kwargs": {}}),
            )
            .as_object()
            .cloned()
            .unwrap_or_default(),
        )
    }
    fn captures_at_send(&self) -> Option<bool> {
        Some(
            self.0.file["captures"]
                .as_array()
                .is_some_and(|a| !a.is_empty()),
        )
    }
    /// The front window the recorder's daemon captured as this message arrived (its picture: invented bytes).
    fn front_capture<'a>(
        &'a self,
        request_id: &'a str,
    ) -> Option<BoxFuture<'a, Result<Option<ferb_supervisor::flow::FrontCapture>, String>>> {
        // journal 265: the recorded captures name this run's requests (in the front door replay)
        let captures = self.0.to_run(&self.0.file["captures"]);
        let stalled = captures
            .as_array()
            .and_then(|a| a.iter().find(|c| c["request_id"] == request_id))
            .is_some_and(|c| c["stalled"] == json!(true));
        if stalled {
            // the recorder's capture hung (make_more.py `mm_capture_timeout`): the planner's limit ends the wait
            return Some(Box::pin(std::future::pending()));
        }
        let got = captures
            .as_array()
            .and_then(|a| a.iter().find(|c| c["request_id"] == request_id))
            .map(|c| {
                let image_ref = s(&c["image_ref"]);
                ferb_supervisor::flow::FrontCapture {
                    window: ferb_supervisor::planner::FrontWindow {
                        facts: c["facts"].as_object().cloned().unwrap_or_default(),
                        image_ref: image_ref.clone(),
                        withheld: String::new(),
                        masking: String::new(),
                        origin: s(&c["origin"]),
                    },
                    image: Some(ferb_core::types::ModelImage::new(
                        s(&c["image"]).into_bytes(),
                        &image_ref,
                    )),
                }
            });
        Some(Box::pin(async move { Ok(got) }))
    }
    fn environment<'a>(&'a self, _words: &'a str) -> BoxFuture<'a, Environment> {
        let next = lock(&self.0.supervise).front().map(|v| self.0.to_run(v));
        Box::pin(async move {
            let next = next.unwrap_or(Value::Null);
            Environment {
                stable: s(&next["environment"]["stable"]),
                live: s(&next["environment"]["live"]),
            }
        })
    }
    fn live_facts(&self, _last_runs: bool, _owner_state: bool) -> String {
        let rec = self.pop(&self.0.supervise, "the planner's live facts");
        lock(&self.0.state).supervise_used += 1;
        s(&rec["live_facts"])
    }
    fn system_inputs(&self, _live: bool, _fresh: bool) -> SystemInputs {
        let rec = self.pop(&self.0.systems, "a role's instructions");
        lock(&self.0.state).systems_used += 1;
        let i = &rec["inputs"];
        SystemInputs {
            facts: s(&i["facts"]),
            live_facts: s(&i["live_facts"]),
            capabilities: s(&i["capabilities"]),
            identity_context: s(&i["identity_context"]),
            standing_rules: None,
            skills_line: s(&i["skills_line"]),
            abilities_line: s(&i["abilities_line"]),
        }
    }
    fn availability(&self) -> String {
        s(&self.0.file["availability"])
    }
    fn relevant_context(&self, _words: &str) -> Activity {
        let next = lock(&self.0.contexts).front().map(|v| self.0.to_run(v));
        let Some(next) = next else {
            self.0
                .problem("a context read the recording didn't make".into());
            return Activity::Text(None);
        };
        // the activity search's rows held this run's paths (written `/FX`, as the owner's activity spells them)
        let text = serde_json::to_string(&next["activity"])
            .unwrap_or_default()
            .replace("/FX", &self.0.root);
        Activity::from_json(&serde_json::from_str(&text).unwrap_or(Value::Null))
    }
    fn disturbance_context(&self, _tree_id: &str) -> String {
        let rec = self.pop(&self.0.contexts, "a context");
        lock(&self.0.state).contexts_used += 1;
        s(&rec["disturbance"])
    }
    fn on_task_start<'a>(&'a self, tree_id: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            lock(&self.0.state).watching.insert(tree_id.to_string());
            self.0.write_span("disturbance.baseline", tree_id);
        })
    }
    fn on_task_finish<'a>(&'a self, tree_id: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            lock(&self.0.state).watching.remove(tree_id);
            self.0.write_span("disturbance.summary", tree_id);
        })
    }
    fn running_apps(&self) -> Vec<(String, String, i64)> {
        vec![
            ("com.apple.finder".into(), "Finder".into(), 4101),
            ("com.apple.TextEdit".into(), "TextEdit".into(), 4102),
        ]
    }
    fn memory_push<'a>(
        &'a self,
        _query: &'a str,
        _targets: &'a str,
    ) -> BoxFuture<'a, Vec<ferb_context::assemble::ContextBlock>> {
        // what memory gave this context's worker, as recorded (make_more.py keeps it)
        let front = lock(&self.0.contexts).front().map(|v| self.0.to_run(v));
        let blocks: Vec<ferb_context::assemble::ContextBlock> = front
            .and_then(|r| r["memory"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .map(|v| ferb_context::assemble::ContextBlock {
                text: s(&v["text"]),
                reference: s(&v["reference"]),
                protected: v["protected"].as_bool().unwrap_or(false),
                cache: v["cache"].as_bool().unwrap_or(false),
                priority: v["priority"].as_i64().unwrap_or(0),
                origin: s(&v["origin"]),
                source_refs: v["source_refs"]
                    .as_array()
                    .map(|a| a.iter().map(s).collect())
                    .unwrap_or_default(),
                payload: v["payload"].as_str().map(str::to_string),
                history: v["history"].as_bool().unwrap_or(false),
                fold: s(&v["fold"]),
                values: (!v["values"].is_null()).then(|| v["values"].clone()),
            })
            .collect();
        Box::pin(async move { blocks })
    }
    fn worker_limits(&self, _worker_type: &str) -> Option<ModelLimits> {
        // the scripted runtime's (a runtime made only to read its limits is never started, never recorded)
        ScriptRuntime::fake_limits().ok()
    }
    fn worker_types(&self) -> Vec<String> {
        // the recorder's daemon had Codex and Claude available (`_runtime_available`)
        vec!["default".into(), "codex".into(), "claude".into()]
    }
    fn shutdown(&self) {
        let open: Vec<String> = std::mem::take(&mut lock(&self.0.state).watching)
            .into_iter()
            .collect();
        for tree in open {
            self.0.write_span("disturbance.summary", &tree);
        }
        // what this process used, for the process a restart starts next (the control socket is gone by then)
        if let Some(out) = self.0.file["state_out"].as_str() {
            let _ = std::fs::write(out, self.0.report().to_string());
        }
    }
    fn make_runtime(
        &self,
        role: &str,
        mode: &str,
        tool_names: &[String],
        schema: &Value,
        effort: Option<&str>,
    ) -> Result<Arc<dyn ModelRuntime>, ferb_worker::Error> {
        let n = {
            let mut st = lock(&self.0.state);
            st.made.push(
                json!({"runtime_role": role, "mode": mode, "tools": tool_names,
                                "schema": schema, "effort": effort}),
            );
            self.0.first() + st.made.len() - 1
        };
        let call = match self.0.role_call(role) {
            Some(c) => c,
            None => self.0.calls().get(n).cloned().unwrap_or(Value::Null),
        };
        Ok(Arc::new(ScriptRuntime {
            script: self.0.clone(),
            n,
            hold: call["hold"].as_str().map(str::to_string),
            by_role: self.0.has_roles(),
            cancel_hold: call["cancel_hold"].as_str().map(str::to_string),
            events: call["events"]
                .as_array()
                .cloned()
                .unwrap_or_else(|| vec![json!({"kind": "(a call the recording didn't make)"})]),
            bridge: call["tool_bridge_lines"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
            shown: json!({"system": call["system"], "parts": call["parts"]}),
            context_tokens: call["context_tokens"].as_i64(),
            tools_background: call["tools_background"] == json!(true),
        }))
    }
}

struct ScriptRuntime {
    script: Arc<Script>,
    n: usize,
    hold: Option<String>,
    by_role: bool,
    cancel_hold: Option<String>,
    events: Vec<Value>,
    bridge: Vec<Value>,
    /// What the recorded call was shown (its instructions and input), for journal 265's pairs.
    shown: Value,
    /// ORCH-760: a test's script: the conversation's size the run reports once its tool calls are made (no
    /// recording has one).
    context_tokens: Option<i64>,
    tools_background: bool,
}

impl ModelRuntime for ScriptRuntime {
    fn name(&self) -> String {
        "fake".into()
    }
    fn limits(&self) -> Result<ModelLimits, ferb_core::errors::FerbError> {
        ScriptRuntime::fake_limits()
    }
    fn output_schema(&self) -> Option<Value> {
        None
    }
    fn start<'a>(
        &'a self,
        task: &'a Task,
        request: &'a ModelRequest,
        token: &'a str,
    ) -> BoxFuture<'a, Result<StartedRun, ferb_worker::Error>> {
        self.started(task, request, token)
    }
}

impl ScriptRuntime {
    fn fake_limits() -> Result<ModelLimits, ferb_core::errors::FerbError> {
        Ok(ModelLimits {
            model: "fake-model".into(),
            context_window: 1_000_000,
            max_output: 100_000,
            source: "test double".into(),
            window_includes_output: true,
            max_input_chars: None,
        })
    }
    fn started<'a>(
        &'a self,
        task: &'a Task,
        request: &'a ModelRequest,
        token: &'a str,
    ) -> BoxFuture<'a, Result<StartedRun, ferb_worker::Error>> {
        Box::pin(async move {
            let parts: Vec<String> = request.parts.iter().map(|p| self.script.mask(p)).collect();
            // journal 265: the ids this call is shown against the recorded call's (the front door replay only)
            if self.script.translation.is_some() {
                let recorded = self.shown["parts"]
                    .as_array()
                    .map(|a| a.iter().map(s).collect::<Vec<_>>());
                if let Some(recorded) = recorded {
                    self.script.learn(&recorded.join("\n"), &parts.join("\n"));
                }
                if let Some(system) = self.shown["system"].as_str() {
                    self.script
                        .learn(system, &self.script.mask(&request.system));
                }
            }
            lock(&self.script.state)
                .started
                .push(json!({"n": self.n, "task_id": task.id,
                "system": request.system, "parts": parts, "cacheable": request.cacheable,
                "images": request.images.len()}));
            let cancelled = Arc::new(AtomicBool::new(false));
            let woken = Arc::new(tokio::sync::Notify::new());
            let context = Arc::new(AtomicI64::new(-1));
            Ok(StartedRun {
                handle: RunHandle {
                    id: format!("run-{}", task.id),
                    runtime: "fake".into(),
                    task_id: task.id.clone(),
                    state: Map::new(),
                },
                events: Box::new(ScriptEvents {
                    script: self.script.clone(),
                    events: self.events.clone(),
                    hold: self.hold.clone(),
                    ends_at_cancel: self.by_role,
                    cancel_hold: self.cancel_hold.clone(),
                    at: 0,
                    cancelled: cancelled.clone(),
                    woken: woken.clone(),
                    n: self.n,
                    token: token.to_string(),
                    bridge: Some(self.bridge.clone()),
                    front_pending: None,
                    context: context.clone(),
                    context_tokens: self.context_tokens,
                    tools_background: self.tools_background,
                }),
                control: Arc::new(ScriptControl {
                    cancelled,
                    woken,
                    context,
                }),
            })
        })
    }
}

struct ScriptControl {
    cancelled: Arc<AtomicBool>,
    woken: Arc<tokio::sync::Notify>,
    /// ORCH-760: the size the run reported (-1: none yet).
    context: Arc<AtomicI64>,
}

impl RunControl for ScriptControl {
    fn steer<'a>(&'a self, _message: &'a str) -> BoxFuture<'a, Result<(), ContinueError>> {
        Box::pin(async { Ok(()) })
    }
    fn cancel(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.cancelled.store(true, Ordering::SeqCst);
            self.woken.notify_waiters();
        })
    }
    fn context_tokens(&self) -> Option<i64> {
        Some(self.context.load(Ordering::SeqCst)).filter(|n| *n >= 0)
    }
}

struct ScriptEvents {
    script: Arc<Script>,
    events: Vec<Value>,
    /// `by_role`'s `hold`: answers only after the control socket's `release` of this name.
    hold: Option<String>,
    /// `by_role`: a cancel ends the call (nothing more comes).
    ends_at_cancel: bool,
    /// `by_role`'s `cancel_hold`: a cancel ends the call only after the control socket's `release` of this name.
    cancel_hold: Option<String>,
    at: usize,
    cancelled: Arc<AtomicBool>,
    woken: Arc<tokio::sync::Notify>,
    n: usize,
    token: String,
    /// The tool calls still to make (before the first event: the scripted model calls its tools first).
    bridge: Option<Vec<Value>>,
    /// ORCH-736: a test's `front_call` event reported as started, made through the socket when the next event is read.
    front_pending: Option<Value>,
    /// ORCH-760: where the size is reported, and the script's size, reported once the tool calls are made.
    context: Arc<AtomicI64>,
    context_tokens: Option<i64>,
    /// ORCH-760's test: the tool calls are made beside the events, not before them.
    tools_background: bool,
}

impl ScriptEvents {
    /// The recorded tool calls, as the tool bridge makes them: `auth.hello` as `mcp_shim` with this run's token, then
    /// each `tools.call`; each reply compared with the recorded line.
    async fn call_tools(&self, lines: &[Value]) {
        call_tools(
            self.script.clone(),
            self.n,
            self.token.clone(),
            lines.to_vec(),
        )
        .await
    }
}

/// [`ScriptEvents::call_tools`] for call `n` of the script, with its run's token.
async fn call_tools(script: Arc<Script>, n: usize, token: String, lines: Vec<Value>) {
    Bridge { script, n, token }.call(&lines).await
}

/// The tool bridge of call `n` of the script, with its run's token.
struct Bridge {
    script: Arc<Script>,
    n: usize,
    token: String,
}

impl Bridge {
    async fn call(&self, lines: &[Value]) {
        let sent: Vec<Value> = lines
            .iter()
            .filter_map(|l| l.get("send").and_then(Value::as_str))
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        if sent.is_empty() {
            return;
        }
        let recv: Vec<Value> = lines
            .iter()
            .filter_map(|l| l.get("recv").and_then(Value::as_str))
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        let closed = lines.iter().any(|l| l.get("closed").is_some());
        let Some(socket) = self.script.socket.get() else {
            return;
        };
        let Ok(conn) = tokio::net::UnixStream::connect(socket).await else {
            self.script
                .problem(format!("call {}: no socket", self.n + 1));
            return;
        };
        let (r, mut w) = conn.into_split();
        let mut r = BufReader::new(r).lines();
        for msg in sent {
            // journal 265: the recorded call names this run's ids (in the front door replay)
            let mut msg = self.script.to_run(&msg);
            if msg["method"] == "auth.hello" {
                msg["params"]["token"] = json!(self.token);
            }
            let line = format!("{}\n", ferb_py::json::dumps(&msg));
            if w.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            // the reply with this id (a stream line in between is skipped)
            let got = loop {
                match r.next_line().await {
                    Ok(Some(l)) => {
                        let v: Value = serde_json::from_str(&l).unwrap_or(Value::Null);
                        if v["id"] == msg["id"] {
                            break Some(v);
                        }
                    }
                    _ => break None,
                }
            };
            if msg["method"] == "auth.hello" {
                continue;
            }
            lock(&self.script.state).tool_calls += 1;
            let want = recv
                .iter()
                .find(|v| v["id"] == msg["id"])
                .map(|v| v["result"].clone());
            let want = match want {
                Some(w) => w,
                None if closed => json!({"error": "Cancelled"}),
                None => Value::Null,
            };
            let got = match got {
                Some(v) if v.get("result").is_some() => v["result"].clone(),
                Some(v) => json!({"error": v["error"]["message"]}),
                None => json!({"error": "Cancelled"}),
            };
            let got: Value = serde_json::from_str(&self.script.mask(&ferb_py::json::dumps(&got)))
                .unwrap_or(Value::Null);
            // journal 265: the reply's ids against the recorded reply's, then the recorded reply as this run's ids
            let want = if self.script.translation.is_some() {
                self.script
                    .learn(&ferb_py::json::dumps(&want), &ferb_py::json::dumps(&got));
                self.script.to_run(&want)
            } else {
                want
            };
            if got != want {
                lock(&self.script.state).tool_diffs.push(format!(
                    "call {} {}\n    python: {}\n    rust:   {}",
                    self.n + 1,
                    msg["params"]["name"],
                    ferb_py::json::dumps(&want),
                    ferb_py::json::dumps(&got)
                ));
            }
        }
    }
}

impl ScriptEvents {
    /// ORCH-736: one call of a front door tool as the tool bridge makes it (`auth.hello` as `mcp_shim` with this run's
    /// token, then `tools.call`): the reply's result, or `{"error": ...}` when it has none.
    async fn front_call(&self, name: &str, arguments: &Value) -> Value {
        let Some(socket) = self.script.socket.get() else {
            return json!({"error": "no socket"});
        };
        let Ok(conn) = tokio::net::UnixStream::connect(socket).await else {
            return json!({"error": "no socket"});
        };
        let (r, mut w) = conn.into_split();
        let mut r = BufReader::new(r).lines();
        let lines = [
            json!({"jsonrpc": "2.0", "id": 1, "method": "auth.hello",
                   "params": {"role": "mcp_shim", "token": self.token}}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools.call",
                   "params": {"name": name, "arguments": arguments}}),
        ];
        let mut got = Value::Null;
        for msg in lines {
            let line = format!("{}\n", ferb_py::json::dumps(&msg));
            if w.write_all(line.as_bytes()).await.is_err() {
                return json!({"error": "the socket closed"});
            }
            got = loop {
                match r.next_line().await {
                    Ok(Some(l)) => {
                        let v: Value = serde_json::from_str(&l).unwrap_or(Value::Null);
                        if v["id"] == msg["id"] {
                            break v;
                        }
                    }
                    _ => return json!({"error": "the socket closed"}),
                }
            };
        }
        match got.get("result") {
            Some(v) => v.clone(),
            None => json!({"error": got["error"]["message"]}),
        }
    }

    /// The end of a cancelled call: at once, or once its `cancel_hold` is released.
    async fn cancel_end(&self) {
        let Some(name) = &self.cancel_hold else {
            return;
        };
        loop {
            let released = self.script.released_now.notified();
            tokio::pin!(released);
            released.as_mut().enable();
            if self.script.hold_released(name) {
                return;
            }
            released.await;
        }
    }
}

impl EventSource for ScriptEvents {
    fn next(&mut self) -> BoxFuture<'_, Option<Result<RuntimeEvent, ferb_worker::Error>>> {
        Box::pin(async move {
            // as tests/conftest.py's FakeRuntime: a cancel ends only a call that never answers (STALL); a scripted
            // answer already under way still comes
            if self.at == 0 && (self.script.gated(self.n) || self.hold.is_some()) {
                loop {
                    let opened = self.script.released_now.notified();
                    let woken = self.woken.notified();
                    tokio::pin!(opened, woken);
                    opened.as_mut().enable();
                    woken.as_mut().enable();
                    let released = match &self.hold {
                        Some(name) => self.script.hold_released(name),
                        None => self.script.released.load(Ordering::SeqCst),
                    };
                    if released {
                        break;
                    }
                    if self.cancelled.load(Ordering::SeqCst) {
                        self.cancel_end().await;
                        return None;
                    }
                    tokio::select! {
                        _ = opened => {}
                        _ = woken => {}
                    }
                }
            }
            if self.ends_at_cancel && self.cancelled.load(Ordering::SeqCst) {
                self.cancel_end().await;
                return None;
            }
            if let Some(lines) = self.bridge.take() {
                let lines: Vec<Value> = lines.iter().map(|l| self.script.with_tree(l)).collect();
                if self.tools_background {
                    // ORCH-760's test: the calls go on beside the run's events (a call that waits on the owner
                    // while the run reports more), as a runtime's parallel calls do
                    tokio::spawn(call_tools(
                        self.script.clone(),
                        self.n,
                        self.token.clone(),
                        lines,
                    ));
                } else {
                    self.call_tools(&lines).await;
                }
                if let Some(n) = self.context_tokens {
                    self.context.store(n, Ordering::SeqCst);
                }
                if self.ends_at_cancel && self.cancelled.load(Ordering::SeqCst) {
                    self.cancel_end().await;
                    return None;
                }
            }
            if let Some(ev) = self.front_pending.take() {
                // a test's own script: the front door call reported started, made now as the tool bridge makes it,
                // then reported completed with its error flag (as a real runtime reports a tool's end, RUN-480)
                let name = s(&ev["name"]);
                let got = self.front_call(&name, &ev["arguments"]).await;
                let error = got["is_error"] != json!(false);
                lock(&self.script.state).front_results.push(json!([
                    self.n,
                    name,
                    ev["arguments"],
                    got
                ]));
                let data = json!({"tool": name.replace('.', "_"), "name": name, "phase": "completed",
                                  "arguments": ev["arguments"], "error": error, "result": got["content"]});
                return Some(Ok(RuntimeEvent {
                    kind: RuntimeEventKind::ToolCall,
                    data: data.as_object().cloned().unwrap_or_default(),
                }));
            }
            if self.at + 1 == self.events.len() {
                lock(&self.script.state).answered.push(self.n);
                let first_check = self
                    .script
                    .calls()
                    .iter()
                    .position(|m| m["key"] == "worker.verify");
                if first_check == Some(self.n) {
                    self.script.apply_note();
                }
            }
            // ORCH-760's test: an event held until the control socket's `release` of its name
            if let Some(name) = self
                .events
                .get(self.at)
                .and_then(|e| e["hold"].as_str())
                .map(str::to_string)
            {
                loop {
                    let opened = self.script.released_now.notified();
                    let woken = self.woken.notified();
                    tokio::pin!(opened, woken);
                    opened.as_mut().enable();
                    woken.as_mut().enable();
                    if self.script.hold_released(&name) {
                        break;
                    }
                    if self.cancelled.load(Ordering::SeqCst) {
                        self.cancel_end().await;
                        return None;
                    }
                    tokio::select! {
                        _ = opened => {}
                        _ = woken => {}
                    }
                }
            }
            // journal 265: a recorded answer or front call names this run's ids (in the front door replay)
            let ev = self
                .script
                .to_run(&self.script.with_tree(self.events.get(self.at)?));
            self.at += 1;
            let kind = s(&ev["kind"]);
            if kind == "front_call" {
                // a test's own script (never a recording): a call of the front door's tools, started now
                let name = s(&ev["name"]);
                let data = json!({"tool": name.replace('.', "_"), "name": name, "phase": "started",
                                  "arguments": ev["arguments"]});
                self.front_pending = Some(ev);
                return Some(Ok(RuntimeEvent {
                    kind: RuntimeEventKind::ToolCall,
                    data: data.as_object().cloned().unwrap_or_default(),
                }));
            }
            let Some(kind) = RuntimeEventKind::ALL
                .iter()
                .copied()
                .find(|k| k.as_str() == kind)
            else {
                // "(no answer: FERB cancelled or left the call)": nothing until FERB cancels it
                loop {
                    let woken = self.woken.notified();
                    tokio::pin!(woken);
                    woken.as_mut().enable();
                    if self.cancelled.load(Ordering::SeqCst) {
                        self.cancel_end().await;
                        return None;
                    }
                    woken.await;
                }
            };
            Some(Ok(RuntimeEvent {
                kind,
                data: ev["data"].as_object().cloned().unwrap_or_default(),
            }))
        })
    }
}

// ---------------------------------------------------------------------------------------------------------------
// the recorder's file facts
// ---------------------------------------------------------------------------------------------------------------

/// A file tool whose results carry the recorder's invented identity facts (make.py `files.identity`):
/// `modified_at` 2026-01-01T09:00:00+00:00, `owner_uid` 501, `owner_gid` 20, `device` 1, `inode` the first 6 hex of
/// the SHA-256 of the `/FX` path, `mtime_ns` 1767258000000000000. Everything else is the real tool.
pub struct RecordedFacts {
    pub script: Arc<Script>,
}

impl RecordedFacts {
    fn rewrite(&self, v: &mut Value) {
        match v {
            Value::Object(m) => {
                if m.contains_key("inode") && m.contains_key("mtime_ns") {
                    let path = m.get("path").map(s).unwrap_or_default();
                    let fx = self.script.mask(&path);
                    use sha2::Digest;
                    let digest = hex::encode(sha2::Sha256::digest(fx.as_bytes()));
                    let inode = i64::from_str_radix(&digest[..6], 16).unwrap_or(0);
                    for (k, val) in [
                        ("modified_at", json!("2026-01-01T09:00:00+00:00")),
                        ("owner_uid", json!(501)),
                        ("owner_gid", json!(20)),
                        ("device", json!(1)),
                        ("inode", json!(inode)),
                        ("mtime_ns", json!(1_767_258_000_000_000_000i64)),
                    ] {
                        if m.contains_key(k) {
                            m.insert(k.into(), val);
                        }
                    }
                }
                for x in m.values_mut() {
                    self.rewrite(x);
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| self.rewrite(x)),
            _ => {}
        }
    }
}

impl crate::compose::tools::wrap::Around for RecordedFacts {
    fn run<'a>(
        &'a self,
        inner: &'a Arc<dyn Tool>,
        args: &'a Args,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            // `inner` is the registry's tool: a file tool already does its work in a thread (compose/tools/wrap.rs)
            let mut out = inner.run(args, ctx).await?;
            let mut content = Value::Object(std::mem::take(&mut out.result.content));
            self.rewrite(&mut content);
            if let Value::Object(m) = content {
                out.result.content = m;
            }
            Ok(out)
        })
    }
}

/// The questions' ids as the recorder's counter gives them (`ap-a00000000001`…): one counter per scenario, so a
/// restarted process continues from `asked` (the control socket's `script` report gives it).
pub fn question_counter(asked: Arc<AtomicU64>) -> crate::compose::gateway::QuestionIds {
    Arc::new(move || format!("ap-a{:011}", asked.fetch_add(1, Ordering::SeqCst) + 1))
}

/// The control socket's commands for a chat scenario.
pub fn control(
    d: &Arc<crate::compose::Daemon>,
    script: Option<&Arc<Script>>,
    req: &Value,
    what: &str,
) -> Option<Result<Value, String>> {
    Some(match what {
        "script" => match script {
            Some(s) => Ok(s.report()),
            None => Err("no script".into()),
        },
        // journal 265: recorded content (the owner's next call) with this run's ids
        "translate" => match script {
            Some(s) => Ok(s.to_run(&req["value"])),
            None => Err("no script".into()),
        },
        "release" => match script {
            Some(s) => {
                s.release(req["name"].as_str().unwrap_or_default());
                Ok(Value::Null)
            }
            None => Err("no script".into()),
        },
        "release_model" => match script {
            Some(s) => {
                s.released.store(true, Ordering::SeqCst);
                s.released_now.notify_waiters();
                Ok(Value::Null)
            }
            None => Err("no script".into()),
        },
        "note" => {
            if let Some(s) = script {
                s.apply_note();
            }
            Ok(Value::Null)
        }
        "pending" => {
            let n = ferb_supervisor::flow::Daemon::approvals(d.supervisor.daemon.as_ref()).len();
            Ok(json!(n))
        }
        "sleeping" => {
            let n = d.supervisor.supervisor.inner().live().sleeping.len();
            Ok(json!(n))
        }
        "root_status" => {
            let db = lock(&d.db);
            Ok(db
                .conn()
                .query_row(
                    "SELECT status FROM tasks WHERE parent_id IS NULL ORDER BY rowid DESC LIMIT 1",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .map(Value::String)
                .unwrap_or(Value::Null))
        }
        // make_more.py `loop_status`: a loop's latest status says it watches
        "loop_watching" => Ok(json!(d
            .supervisor
            .supervisor
            .loop_statuses()
            .iter()
            .any(|v| v["state"] == "watching"))),
        // a plan step waits for an event (`_plan_runs`): its wait step has taken its place in the plan's events (its
        // waiting state comes after it read the event cursor), so an event published now is one it sees
        "plan_waits" => {
            let inner = d.supervisor.supervisor.inner();
            let trees: Vec<_> = {
                let live = inner.live();
                live.plan_runs
                    .iter()
                    .filter_map(|id| live.tree_of.get(id).cloned())
                    .collect()
            };
            Ok(json!(trees.iter().any(|t| t
                .lock()
                .step_states
                .values()
                .any(
                    |s| s["status"] == "waiting" && s.get("event").is_some()
                ))))
        }
        // make_more.py: the owner is back (`mention_paused`, a task of its own on the daemon's loop)
        "mention_paused" => {
            let inner = d.supervisor.supervisor.inner().clone();
            tokio::spawn(async move {
                let _ = inner.mention_paused().await;
            });
            Ok(Value::Null)
        }
        // make_more.py: an event reaches the supervisor (`notify_plan_event`)
        "plan_event" => {
            let topic = req["topic"].as_str().unwrap_or_default().to_string();
            d.supervisor
                .supervisor
                .inner()
                .notify_plan_event(&topic, &req["data"]);
            Ok(Value::Null)
        }
        // make_more.py: the app's one tap on a runtime offer (`runtime_offer`, `answer_runtime_switch`)
        "runtime_switch" => {
            let inner = d.supervisor.supervisor.inner();
            let task = req["task_id"].as_str().unwrap_or_default();
            let runtime = req["runtime"].as_str().unwrap_or_default();
            let offer = inner.runtime_offer(task).unwrap_or(Value::Null);
            let answered = inner
                .answer_runtime_switch(task, runtime)
                .unwrap_or_else(|e| json!({"error": e}));
            Ok(json!({"offer": offer, "answered": answered}))
        }
        // stage 2: everything the chat log queued, applied and written now (the order check reads the store after it)
        "chat_flush" => {
            let inner = d.supervisor.supervisor.inner();
            inner.chat_log.flush(inner);
            Ok(Value::Null)
        }
        "quiet_mark" => {
            let db = lock(&d.db);
            let q = |sql: &str| {
                db.conn()
                    .query_row(sql, [], |r| r.get::<_, i64>(0))
                    .unwrap_or(0)
            };
            Ok(json!([
                q("SELECT COALESCE(MAX(seq), 0) FROM journal"),
                q("SELECT COUNT(*) FROM spans")
            ]))
        }
        _ => {
            let _ = req;
            return None;
        }
    })
}
