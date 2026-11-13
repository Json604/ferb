//! ferb/orchestrator/continuity.py: durable task context and the conversations tasks belong to. Context is data;
//! approval grants, run tokens and login codes never live here.
//!
//! decision: ORCH-584, ORCH-593, ORCH-619, STORE-403

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, OnceLock};

use ferb_sanitize::origin;
use ferb_store::Db;
use rusqlite::types::Value as Sql;
use serde_json::{json, Map, Value};

use crate::models::TaskSpec;
use crate::ports::Redact;
use crate::sql::{self, text, Error, Result};

pub const SCHEDULE_SESSION: &str = "schedule:";
pub const PREVIEW_SESSION: &str = "preview:";
pub const RESERVED_SESSIONS: &[&str] = &[SCHEDULE_SESSION, PREVIEW_SESSION];
pub const LEGACY_UPGRADE: &str = "continuity_legacy";

/// continuity.py:148 `KINDS`.
pub const KINDS: &[&str] = &[
    "chat.message",
    "chat.reply",
    "chat.clarify",
    "task.answer",
    "chat.notice",
];
/// continuity.py:149 `SELF_POSTS`.
pub const SELF_POSTS: &[&str] = &["schedule.posted", "timer.delivered", "chat.part"];

fn reserved(conversation: &str) -> bool {
    RESERVED_SESSIONS
        .iter()
        .any(|p| conversation.starts_with(p))
}

/// continuity.py:22 `in_conversation`.
pub fn in_conversation(kind: &str, data: &Map<String, Value>, conversation: &str) -> bool {
    if kind == "chat.notice" && !data.contains_key("session_id") {
        return !reserved(conversation);
    }
    match data.get("session_id") {
        None => conversation == "owner",
        Some(v) => v.as_str() == Some(conversation),
    }
}

/// `re.fullmatch(r"[\w:.-]{1,128}", conversation)`.
pub fn valid_conversation(conversation: &str) -> bool {
    let n = conversation.chars().count();
    (1..=128).contains(&n)
        && conversation
            .chars()
            .all(|c| crate::py::is_alnum(c) || matches!(c, '_' | ':' | '.' | '-'))
}

/// What `load` reads back: the spec, the request (its id, words, payload) and the task's state line.
#[derive(Debug, Clone, PartialEq)]
pub struct Context {
    pub spec: TaskSpec,
    pub request_id: String,
    pub words: String,
    pub payload: String,
    pub state: String,
}

/// journal.py:30 `journal_origin(kind, data)` (the memory part's).
pub type JournalOrigin = Arc<dyn Fn(&str, &Value) -> String + Send + Sync>;

/// continuity.py:32 `Continuity`.
pub struct Continuity<'a> {
    pub db: &'a Db,
    pub redact: Redact,
    /// SpanStore.task_origins: what a task's models were shown (S3).
    pub task_origins: crate::ports::Lookup<BTreeSet<String>>,
    pub journal_origin: JournalOrigin,
    /// `"chat-" + secrets.token_hex(6)`'s twelve hex characters (injected).
    pub new_chat: Arc<dyn Fn() -> String + Send + Sync>,
    lent: Mutex<HashMap<String, String>>,
}

impl<'a> Continuity<'a> {
    pub fn new(
        db: &'a Db,
        redact: Redact,
        task_origins: crate::ports::Lookup<BTreeSet<String>>,
        journal_origin: JournalOrigin,
        new_chat: Arc<dyn Fn() -> String + Send + Sync>,
    ) -> Continuity<'a> {
        Continuity {
            db,
            redact,
            task_origins,
            journal_origin,
            new_chat,
            lent: Mutex::new(HashMap::new()),
        }
    }

    fn lent(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.lent.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// continuity.py:38 `ensure`.
    pub fn ensure(&self, conversation: &str) -> Result<()> {
        if !valid_conversation(conversation) {
            return Err(Error::Value("invalid conversation id".into()));
        }
        sql::exec(
            self.db.conn(),
            "INSERT OR IGNORE INTO conversations(id) VALUES (?)",
            &[text(conversation)],
        )?;
        Ok(())
    }

    /// continuity.py:43 `attach`.
    pub fn attach(&self, new: bool, resume: Option<&str>) -> Result<String> {
        let resume = resume.filter(|r| !r.is_empty());
        if new && resume.is_some() {
            return Err(Error::Value("choose --new or --resume".into()));
        }
        let conversation = if let Some(r) = resume {
            if sql::row(
                self.db.conn(),
                "SELECT 1 FROM conversations WHERE id = ?",
                &[text(r)],
            )?
            .is_none()
            {
                return Err(Error::Value("that conversation does not exist".into()));
            }
            r.to_string()
        } else if new {
            format!("chat-{}", (self.new_chat)())
        } else {
            self.current()?
        };
        self.ensure(&conversation)?;
        sql::exec(
            self.db.conn(),
            "INSERT OR REPLACE INTO current_conversation VALUES (1, ?)",
            &[text(&conversation)],
        )?;
        Ok(conversation)
    }

    /// continuity.py:58 `current`.
    pub fn current(&self) -> Result<String> {
        Ok(sql::row(
            self.db.conn(),
            "SELECT conversation_id FROM current_conversation WHERE singleton = 1",
            &[],
        )?
        .map(|r| sql::col(&r, "conversation_id"))
        .unwrap_or_else(|| "owner".into()))
    }

    /// continuity.py:63 `pending`: (words, asked).
    pub fn pending(&self, conversation: &str) -> Result<(Value, Value)> {
        self.ensure(conversation)?;
        let row = sql::row(
            self.db.conn(),
            "SELECT pending, asked FROM conversations WHERE id = ?",
            &[text(conversation)],
        )?
        .unwrap_or_default();
        Ok((
            row.get("pending").cloned().unwrap_or(Value::Null),
            row.get("asked").cloned().unwrap_or(Value::Null),
        ))
    }

    /// continuity.py:68 `pending_payload`.
    pub fn pending_payload(&self, conversation: &str) -> Result<Value> {
        Ok(sql::row(
            self.db.conn(),
            "SELECT pending_payload FROM conversations WHERE id = ?",
            &[text(conversation)],
        )?
        .map(|r| r.get("pending_payload").cloned().unwrap_or(Value::Null))
        .unwrap_or(Value::String(String::new())))
    }

    /// continuity.py:72 `save_pending`.
    pub fn save_pending(
        &self,
        conversation: &str,
        words: &str,
        asked: &str,
        payload: &str,
    ) -> Result<()> {
        let clean = (self.redact)(&json!({"words": words, "asked": asked, "payload": payload}));
        sql::exec(
            self.db.conn(),
            "UPDATE conversations SET pending = ?, asked = ?, pending_payload = ? WHERE id = ?",
            &[
                sql::to_sql(&clean["words"]),
                sql::to_sql(&clean["asked"]),
                sql::to_sql(&clean["payload"]),
                text(conversation),
            ],
        )?;
        Ok(())
    }

    /// continuity.py:77 `save`.
    pub fn save(
        &self,
        task_id: &str,
        conversation: &str,
        spec: &TaskSpec,
        words: &str,
        prior_task: Option<&str>,
        payload: &str,
    ) -> Result<()> {
        self.ensure(conversation)?;
        let dumped = crate::validate::dump_task_spec(spec).to_value();
        let clean = (self.redact)(&json!({"spec": dumped, "words": words, "payload": payload}));
        sql::exec(
            self.db.conn(),
            "INSERT OR REPLACE INTO task_context (task_id,conversation_id,spec_json,owner_words,prior_task,request_payload) \
             VALUES (?, ?, ?, ?, ?, ?)",
            &[
                text(task_id),
                text(conversation),
                text(&ferb_py::json::dumps(&clean["spec"])),
                sql::to_sql(&clean["words"]),
                sql::opt(prior_task),
                sql::to_sql(&clean["payload"]),
            ],
        )?;
        Ok(())
    }

    /// continuity.py:85 `load`.
    pub fn load(&self, task_id: &str, conversation: &str) -> Result<Option<Context>> {
        let Some(row) = sql::row(
            self.db.conn(),
            "SELECT c.*, t.request_id, t.status, t.outcome_json FROM task_context c JOIN tasks t ON t.id = c.task_id \
             WHERE c.task_id = ? AND c.conversation_id = ?",
            &[text(task_id), text(conversation)],
        )?
        else {
            return Ok(None);
        };
        let spec = crate::validate::task_spec_validate_json(&sql::col(&row, "spec_json"))
            .map_err(|e| Error::Value(e.to_string()))?;
        let result = sql::loads(&sql::col(&row, "outcome_json"))?;
        let summary = result
            .get("summary")
            .map(crate::py::str_of)
            .unwrap_or_default();
        let state = format!("{}: {summary}", sql::col(&row, "status"));
        let owner_words = sql::col(&row, "owner_words");
        let words = ferb_sanitize::untrusted::without_ferb_question(&owner_words);
        let payload = sql::col(&row, "request_payload");
        Ok(Some(Context {
            spec,
            request_id: sql::col(&row, "request_id"),
            words,
            payload: if payload.is_empty() {
                owner_words
            } else {
                payload
            },
            state,
        }))
    }

    /// continuity.py:98 `prior_of`.
    pub fn prior_of(&self, task_id: &str) -> Result<Option<String>> {
        Ok(sql::row(
            self.db.conn(),
            "SELECT prior_task FROM task_context WHERE task_id = ?",
            &[text(task_id)],
        )?
        .and_then(|r| {
            r.get("prior_task")
                .and_then(|v| v.as_str())
                .map(String::from)
        }))
    }

    /// continuity.py:103 `candidates`.
    pub fn candidates(&self, conversation: &str) -> Result<Vec<Map<String, Value>>> {
        sql::rows(
            self.db.conn(),
            "SELECT c.task_id, c.prior_task, t.status, t.goal, t.updated_at, t.outcome_json FROM task_context c \
             JOIN tasks t ON t.id = c.task_id WHERE c.conversation_id = ? ORDER BY t.rowid DESC",
            &[text(conversation)],
        )
    }

    /// continuity.py:110 `conversation_of`.
    pub fn conversation_of(&self, task_id: &str) -> Result<Option<String>> {
        if let Some(c) = self.lent().get(task_id) {
            return Ok(Some(c.clone()));
        }
        Ok(sql::row(
            self.db.conn(),
            "SELECT conversation_id FROM task_context WHERE task_id = ?",
            &[text(task_id)],
        )?
        .map(|r| sql::col(&r, "conversation_id")))
    }

    pub fn lend(&self, task_id: &str, conversation: &str) {
        self.lent().insert(task_id.into(), conversation.into());
    }

    pub fn unlend(&self, task_id: &str) {
        self.lent().remove(task_id);
    }

    /// continuity.py:126 `task_ids`.
    pub fn task_ids(&self, conversation: &str) -> Result<BTreeSet<String>> {
        Ok(sql::rows(
            self.db.conn(),
            "SELECT task_id FROM task_context WHERE conversation_id = ?",
            &[text(conversation)],
        )?
        .iter()
        .map(|r| sql::col(r, "task_id"))
        .collect())
    }

    /// continuity.py:130 `reaches_schedules` (ORCH-593).
    pub fn reaches_schedules(&self, task_id: &str) -> bool {
        self.lent().get(task_id).is_some_and(|c| !reserved(c))
    }

    /// continuity.py:138 `readable_tasks`.
    pub fn readable_tasks(&self, task_id: &str) -> Result<BTreeSet<String>> {
        let Some(conversation) = self.conversation_of(task_id)? else {
            return Ok(BTreeSet::new());
        };
        let mut out = self.task_ids(&conversation)?;
        if self.reaches_schedules(task_id) {
            out.extend(self.schedule_tasks()?);
        }
        Ok(out)
    }

    /// continuity.py:151 `_where`.
    fn where_(conversation: &str, schedules: bool) -> (String, Vec<Sql>) {
        let owner_chat = !reserved(conversation);
        let mut kinds: Vec<&str> = KINDS.to_vec();
        if owner_chat {
            kinds.extend(SELF_POSTS);
        }
        let marks = vec!["?"; kinds.len()].join(",");
        let ran = if schedules {
            " OR conversation_id LIKE 'schedule:%'"
        } else {
            ""
        };
        let tasks = format!("(SELECT task_id FROM task_context WHERE conversation_id = ?{ran})");
        let session = format!(
            "json_extract(data_json, '$.session_id') = ?{}",
            if schedules {
                " OR json_extract(data_json, '$.session_id') LIKE 'schedule:%'"
            } else {
                ""
            }
        );
        let posts = if owner_chat {
            format!(
                " OR (kind = 'chat.notice' AND json_extract(data_json, '$.session_id') IS NULL) OR kind IN \
                 ('schedule.posted', 'timer.delivered') OR (kind = 'chat.part' AND json_extract(data_json, '$.task_id') IN {tasks})"
            )
        } else {
            String::new()
        };
        let mut args: Vec<Sql> = kinds.iter().map(|k| text(k)).collect();
        args.push(text(conversation));
        if owner_chat {
            args.push(text(conversation));
        }
        args.push(text(conversation));
        args.push(text(conversation));
        let own = "'task.answer', 'schedule.posted', 'timer.delivered', 'chat.part'";
        (
            format!(
                "kind IN ({marks}) AND ((kind = 'task.answer' AND json_extract(data_json, '$.task_id') IN {tasks}){posts} \
                 OR (kind NOT IN ({own}) AND json_extract(data_json, '$.session_id') IS NOT NULL AND ({session})) \
                 OR (kind NOT IN ({own}, 'chat.notice') AND json_extract(data_json, '$.session_id') IS NULL AND ? = 'owner'))"
            ),
            args,
        )
    }

    fn record(&self, row: &Map<String, Value>) -> Result<Value> {
        let data = sql::loads(&sql::col(row, "data_json"))?;
        let kind = sql::col(row, "kind");
        let made = match row.get("origin") {
            Some(Value::String(o)) if !o.is_empty() => o.clone(),
            _ => (self.journal_origin)(&kind, &data),
        };
        Ok(json!({"id": row["id"], "at": row["at"], "kind": kind, "data": data, "origin": made}))
    }

    /// continuity.py:179 `entries`: one page, oldest first, and the total.
    pub fn entries(
        &self,
        task_id: &str,
        since: Option<&str>,
        until: Option<&str>,
        after: i64,
        limit: i64,
    ) -> Result<(Vec<Value>, i64)> {
        let Some(conversation) = self.conversation_of(task_id)? else {
            return Ok((Vec::new(), 0));
        };
        let (mut where_, mut args) = Self::where_(&conversation, self.reaches_schedules(task_id));
        if let Some(s) = since.filter(|s| !s.is_empty()) {
            where_.push_str(" AND at >= ?");
            args.push(text(s));
        }
        if let Some(u) = until.filter(|u| !u.is_empty()) {
            where_.push_str(" AND at <= ?");
            args.push(text(u));
        }
        let total = sql::row(
            self.db.conn(),
            &format!("SELECT COUNT(*) AS n FROM journal WHERE {where_}"),
            &args,
        )?
        .and_then(|r| r.get("n").and_then(|v| v.as_i64()))
        .unwrap_or(0);
        args.push(Sql::Integer(limit));
        args.push(Sql::Integer(after));
        let rows = sql::rows(
            self.db.conn(),
            &format!("SELECT * FROM journal WHERE {where_} ORDER BY seq LIMIT ? OFFSET ?"),
            &args,
        )?;
        Ok((
            rows.iter().map(|r| self.record(r)).collect::<Result<_>>()?,
            total,
        ))
    }

    /// continuity.py:197 `entry`.
    pub fn entry(&self, task_id: &str, entry_id: &str) -> Result<Option<Value>> {
        let Some(conversation) = self.conversation_of(task_id)? else {
            return Ok(None);
        };
        let (where_, args) = Self::where_(&conversation, self.reaches_schedules(task_id));
        let mut all = vec![text(entry_id)];
        all.extend(args);
        match sql::row(
            self.db.conn(),
            &format!("SELECT * FROM journal WHERE id = ? AND {where_}"),
            &all,
        )? {
            Some(r) => Ok(Some(self.record(&r)?)),
            None => Ok(None),
        }
    }

    /// continuity.py:206 `result` (ORCH-619).
    pub fn result(&self, conversation: &str, entry_id: &str, kinds: &[&str]) -> Result<bool> {
        let (where_, args) = Self::where_(conversation, !reserved(conversation));
        let marks = vec!["?"; kinds.len()].join(",");
        let mut all = vec![text(entry_id)];
        all.extend(kinds.iter().map(|k| text(k)));
        all.extend(args);
        Ok(sql::row(
            self.db.conn(),
            &format!("SELECT 1 FROM journal WHERE id = ? AND kind IN ({marks}) AND {where_}"),
            &all,
        )?
        .is_some())
    }

    /// continuity.py:215 `part_order`.
    pub fn part_order(&self, task_id: &str) -> Result<Vec<String>> {
        match sql::row(
            self.db.conn(),
            "SELECT spec_json FROM task_context WHERE task_id = ?",
            &[text(task_id)],
        )? {
            Some(r) => Ok(
                crate::validate::task_spec_validate_json(&sql::col(&r, "spec_json"))
                    .map_err(|e| Error::Value(e.to_string()))?
                    .parts
                    .into_iter()
                    .map(|p| p.id)
                    .collect(),
            ),
            None => Ok(Vec::new()),
        }
    }

    /// continuity.py:220 `belongs`.
    pub fn belongs(
        &self,
        conversation: &str,
        kind: &str,
        session: &str,
        request: &str,
        doc: &str,
        schedules: bool,
    ) -> Result<bool> {
        if kind == "task.answer" {
            let task = if doc.is_empty() {
                None
            } else {
                let id = doc.strip_prefix("journal:").unwrap_or(doc);
                sql::row(
                    self.db.conn(),
                    "SELECT json_extract(data_json, '$.task_id') AS t FROM journal WHERE id = ?",
                    &[text(id)],
                )?
                .and_then(|r| r.get("t").and_then(|v| v.as_str()).map(String::from))
            };
            let mut held = self.task_ids(conversation)?;
            if schedules {
                held.extend(self.schedule_tasks()?);
            }
            return Ok(held.contains(request) || task.is_some_and(|t| held.contains(&t)));
        }
        if kind == "chat.notice" && session.is_empty() {
            return Ok(!reserved(conversation));
        }
        Ok(session == conversation || (schedules && session.starts_with(SCHEDULE_SESSION)))
    }

    fn schedule_tasks(&self) -> Result<BTreeSet<String>> {
        Ok(sql::rows(
            self.db.conn(),
            "SELECT task_id FROM task_context WHERE conversation_id LIKE ?",
            &[text(&format!("{SCHEDULE_SESSION}%"))],
        )?
        .iter()
        .map(|r| sql::col(r, "task_id"))
        .collect())
    }

    /// continuity.py:238 `tasks`.
    pub fn tasks(&self, task_id: &str) -> Result<Vec<Value>> {
        let Some(conversation) = self.conversation_of(task_id)? else {
            return Ok(Vec::new());
        };
        let mut rows = self.candidates(&conversation)?;
        if self.reaches_schedules(task_id) {
            rows.extend(sql::rows(
                self.db.conn(),
                "SELECT c.task_id, c.prior_task, t.status, t.goal, t.updated_at, t.outcome_json FROM task_context c \
                 JOIN tasks t ON t.id = c.task_id WHERE c.conversation_id LIKE ? ORDER BY t.rowid DESC",
                &[text(&format!("{SCHEDULE_SESSION}%"))],
            )?);
        }
        Ok(rows
            .into_iter()
            .map(|row| {
                let id = sql::col(&row, "task_id");
                let shown = (self.task_origins)(&id);
                let mut made: BTreeSet<String> = origin::kinds(shown.iter().map(String::as_str));
                made.insert(origin::INFERRED.to_string());
                let joined = origin::join(made.iter().map(String::as_str));
                json!({"id": id, "kind": "task", "data": Value::Object(row), "origin": joined})
            })
            .collect())
    }
}

/// continuity.py:306 `_OLD_YES`, verbatim: a full match of the old plain-yes words.
fn old_yes(line: &str) -> bool {
    static RX: OnceLock<regex::Regex> = OnceLock::new();
    let rx = RX.get_or_init(|| {
        regex::Regex::new(r"^[\t\n\x0B\x0C\r\x1C-\x1F \x{85}\x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}]*(?:yes|y|yeah|yep|yup|sure|ok|okay|go ahead|do it|please do|correct|right)[.! ]*[\t\n\x0B\x0C\r\x1C-\x1F \x{85}\x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}]*$")
            .expect("valid pattern")
    });
    rx.is_match(line)
}

/// continuity.py:309 `_strip_endorsed`.
pub fn strip_endorsed(words: &str, needle: &str) -> String {
    let mut words = words.to_string();
    let mut at = words.find(needle);
    while let Some(i) = at {
        let start = words[..i].rfind('\n').map(|j| j + 1).unwrap_or(0);
        let line = &words[start..i];
        let end = i + needle.len();
        if old_yes(&crate::py::lower(line))
            && (end == words.len() || words[end..].starts_with('\n'))
        {
            words = format!("{}{}", &words[..i], &words[end..]);
            at = words[i..].find(needle).map(|j| j + i);
        } else {
            let from = i + words[i..].chars().next().map(char::len_utf8).unwrap_or(1);
            at = words
                .get(from..)
                .and_then(|w| w.find(needle))
                .map(|j| j + from);
        }
    }
    words
}

/// `re.sub(r'<untrusted-data id="([^"]+)">.*?</untrusted-data id="\1">', " ", text, flags=re.S)`.
fn without_wrapped(text: &str) -> String {
    static RX: OnceLock<fancy_regex::Regex> = OnceLock::new();
    let rx = RX.get_or_init(|| {
        fancy_regex::Regex::new(r#"(?s)<untrusted-data id="([^"]+)">.*?</untrusted-data id="\1">"#)
            .expect("valid pattern")
    });
    rx.replace_all(text, " ").into_owned()
}

/// `re.sub(r"\byour\b", "my", question, flags=re.I)`.
fn your_to_my(question: &str) -> String {
    static RX: OnceLock<regex::Regex> = OnceLock::new();
    let rx = RX.get_or_init(|| regex::Regex::new(r"(?i)\byour\b").expect("valid pattern"));
    rx.replace_all(question, "my").into_owned()
}

/// continuity.py:260 `upgrade_legacy`: the one-time upgrade of continuity written before 2026-10-03.
pub fn upgrade_legacy(continuity: &Continuity<'_>) -> Result<Value> {
    let db = continuity.db;
    let mut questions: Vec<String> = Vec::new();
    for r in sql::rows(
        db.conn(),
        "SELECT data_json FROM journal WHERE kind = 'chat.clarify'",
        &[],
    )? {
        let data = sql::loads(&sql::col(&r, "data_json"))?;
        let q = match data.get("text") {
            Some(v) if ferb_py::value::truthy(v) => crate::py::str_of(v),
            _ => String::new(),
        };
        if !questions.contains(&q) {
            questions.push(q);
        }
    }
    let mut ordered: Vec<String> = questions.into_iter().filter(|q| !q.is_empty()).collect();
    // sorted(filter(None, questions), key=len, reverse=True): stable over the set's order, which is unordered in
    // Python; ties between equal lengths are sorted by text here so the result never depends on hash order
    ordered.sort_by(|a, b| {
        b.chars()
            .count()
            .cmp(&a.chars().count())
            .then_with(|| a.cmp(b))
    });
    let mut rewritten = 0;
    for row in sql::rows(
        db.conn(),
        "SELECT task_id, owner_words FROM task_context WHERE COALESCE(request_payload, '') = ''",
        &[],
    )? {
        let original = match row.get("owner_words") {
            Some(Value::String(s)) => s.clone(),
            _ => String::new(),
        };
        let mut words = original.clone();
        for q in &ordered {
            words = strip_endorsed(&words, &format!("\n{}", your_to_my(q)));
        }
        sql::exec(
            db.conn(),
            "UPDATE task_context SET owner_words = ?, request_payload = ? WHERE task_id = ?",
            &[
                text(&words),
                text(&original),
                text(&sql::col(&row, "task_id")),
            ],
        )?;
        rewritten += 1;
    }
    continuity.ensure("owner")?;
    let (mut imported, mut skipped) = (0, 0);
    let rows = sql::rows(
        db.conn(),
        "SELECT t.* FROM tasks t LEFT JOIN task_context c ON c.task_id=t.id WHERE t.parent_id IS NULL AND c.task_id IS NULL \
         ORDER BY t.rowid DESC",
        &[],
    )?;
    for row in rows {
        let messages = sql::rows(
            db.conn(),
            "SELECT data_json FROM journal WHERE kind='chat.message' AND json_extract(data_json, '$.request_id')=?",
            &[sql::to_sql(&row["request_id"])],
        )?;
        let spans = sql::rows(
            db.conn(),
            "SELECT input_json FROM spans WHERE kind='task' AND task_id=?",
            &[sql::to_sql(&row["id"])],
        )?;
        if messages.is_empty() || spans.is_empty() {
            continue;
        }
        let distinct = |rows: &[Map<String, Value>], key: &str| {
            rows.iter()
                .map(|r| sql::col(r, key))
                .collect::<BTreeSet<_>>()
                .len()
        };
        if distinct(&messages, "data_json") != 1 || distinct(&spans, "input_json") != 1 {
            skipped += 1;
            continue;
        }
        let data = sql::loads(&sql::col(&spans[0], "input_json"))?;
        let identity = data.get("identity").and_then(|v| v.as_str()).unwrap_or("");
        if identity != "owner" && identity != "ferb" {
            continue;
        }
        let goal = match data.get("goal") {
            Some(Value::String(g)) => g.clone(),
            _ => sql::col(&row, "goal"),
        };
        let targets = data.get("targets").cloned().unwrap_or(json!({}));
        let spec = crate::validate::task_spec_validate(&ferb_context::Py::from_value(
            &json!({"goal": goal, "identity": identity, "targets": targets, "success_conditions": []}),
        ))
        .map_err(|e| Error::Value(e.to_string()))?;
        let message = sql::loads(&sql::col(&messages[0], "data_json"))?;
        let text_ = message
            .get("text")
            .map(crate::py::str_of)
            .unwrap_or_default();
        let own = without_wrapped(&text_);
        continuity.save(
            &sql::col(&row, "id"),
            "owner",
            &spec,
            &ferb_sanitize::untrusted::mark(&own).own_words,
            None,
            &text_,
        )?;
        imported += 1;
    }
    Ok(json!({"rewritten": rewritten, "imported": imported, "conflicting": skipped}))
}
