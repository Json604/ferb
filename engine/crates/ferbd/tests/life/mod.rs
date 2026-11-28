//! T-1277's lifecycle harness: the real `ferbd daemon` program (support's `Run`: a temporary home, the test build's
//! stand-ins, the Mac's own parts off as TEST-512 has them, the fake clock) driven as the owner's clients drive it, with
//! a hand-written scripted model (src/standins_chat.rs `by_role`: one queue of answers per model role, a call held
//! until the test releases it). Each step is read back as the owner would see it: the chat's stored entries
//! (`chat.entries.page`), `chat.status`, `tasks.list`, the open questions, the journal.
//!
//! A process can be stopped by FERB's own stop (SIGTERM with the stop marker), by a `kill -9`, and started again on the
//! same home with the next process's script.
//!
//! A test file using it also includes ferb-supervisor's entry order rules as `entry_rules` (as chat_replay.rs does):
//! [`Life::order_rules`] checks them on what the program wrote, across its restarts.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::support::{Raw, Run};

/// The planner's answer: a task with one step (no plan) and its plan line.
pub fn plan_task(goal: &str, line: &str) -> Value {
    planner(
        json!({"kind": "task", "answers_question": false, "task": task_spec(goal, Value::Null),
                   "control": null, "side_entries": null, "text": line}),
    )
}

/// The planner's answer: a task with these steps (`[id, goal, depends_on]`).
pub fn plan_steps(goal: &str, line: &str, steps: &[(&str, &str, &[&str])]) -> Value {
    let steps: Vec<Value> = steps
        .iter()
        .map(|(id, g, deps)| {
            json!({"id": id, "kind": "do", "label": g, "goal": g, "depends_on": deps, "inputs": [],
                   "worker_type": "default", "conditions": [], "until": null, "event": null,
                   "event_match": [], "question": "", "options": [], "conversation": null,
                   "repeat": null, "earlier_results": []})
        })
        .collect();
    let plan = json!({"steps": steps, "input_paths": [], "big_job": false, "time_estimate": "",
                      "cost_estimate": "", "standing_sends": []});
    planner(json!({"kind": "task", "answers_question": false,
                   "task": task_spec(goal, plan),
                   "control": null, "side_entries": null, "text": line}))
}

fn task_spec(goal: &str, plan: Value) -> Value {
    json!({"goal": goal, "name": goal, "identity": "owner", "worker_type": "default",
           "targets": {"apps": [], "sites": [], "paths": [], "recipients": []},
           "success_conditions": [{"text": goal, "owner_text": "", "layer": "outcome", "check": null}],
           "plan": plan, "folders": [], "keep_tabs_open": false, "parts": [], "earlier_results": []})
}

/// The planner's plain reply.
pub fn plan_reply(text: &str) -> Value {
    planner(
        json!({"kind": "reply", "answers_question": false, "task": null, "control": null,
                   "side_entries": null, "text": text}),
    )
}

/// The planner's control of the newest task (`stop`, `pause`, `continue`, `retry`), with its reply text.
pub fn plan_control(action: &str, text: &str) -> Value {
    planner(
        json!({"kind": "reply", "answers_question": false, "task": null,
                   "control": {"task_id": "@TREE@", "action": action, "new_goal": false,
                               "standing_sends": "unchanged", "supersedes": []},
                   "side_entries": null, "text": text}),
    )
}

/// The planner's retry of the newest task with a new plan (ORCH-682's re-plan).
pub fn plan_retry_with_steps(goal: &str, text: &str, steps: &[(&str, &str, &[&str])]) -> Value {
    let mut v = plan_steps(goal, text, steps);
    let mut answer: Value =
        serde_json::from_str(v["events"][1]["data"]["text"].as_str().unwrap()).unwrap();
    answer["kind"] = json!("reply");
    answer["control"] = json!({"task_id": "@TREE@", "action": "retry", "new_goal": false,
                               "standing_sends": "unchanged", "supersedes": []});
    v["events"][1]["data"]["text"] = json!(answer.to_string());
    v
}

/// The planner's answer as given (side entries and all).
pub fn planner_answer(answer: Value) -> Value {
    call(answer)
}

fn planner(answer: Value) -> Value {
    planner_answer(answer)
}

/// A worker's report: done with this answer.
pub fn worker_done(answer: &str) -> Value {
    call(
        json!({"status": "done", "reason": "", "question": null, "answer": answer, "artifacts": [], "how": "",
                "gaps": []}),
    )
}

/// A worker's report: not done, for this reason.
pub fn worker_not_done(reason: &str) -> Value {
    call(
        json!({"status": "not_done", "reason": reason, "question": null, "answer": "", "artifacts": [],
                "how": "", "gaps": []}),
    )
}

/// The checker's verdict on one condition: met, accept.
pub fn check_accept() -> Value {
    call(
        json!({"results": [{"index": 0, "met": true, "applies": true,
                             "evidence": "Seen in the worker's report.", "how": null, "left": null}],
                "next": "accept", "findings": [], "missing": "", "instructions": "", "worker_type": null,
                "question": "", "reason": "", "gaps": [], "progress": null}),
    )
}

/// The checker's verdict: not met, the step fails.
pub fn check_fail(reason: &str) -> Value {
    call(
        json!({"results": [{"index": 0, "met": false, "applies": true,
                             "evidence": "Not in the worker's report.", "how": null, "left": null}],
                "next": "fail", "findings": [], "missing": reason, "instructions": "", "worker_type": null,
                "question": "", "reason": reason, "gaps": [], "progress": null}),
    )
}

/// One model call answering `answer` (JSON) as its final text.
pub fn call(answer: Value) -> Value {
    json!({"events": [
        {"kind": "usage", "data": {"input_tokens": 10, "output_tokens": 5, "cost_usd": 0.001}},
        {"kind": "final", "data": {"text": answer.to_string()}},
    ]})
}

/// A call that never answers: it ends only when FERB cancels it.
pub fn stall() -> Value {
    json!({"events": [{"kind": "(no answer: FERB cancelled or left the call)"}]})
}

/// `c` answering every later call of its role too (the last of its queue: a checker that accepts whatever it is
/// asked, however many checks the code runs).
pub fn always(mut c: Value) -> Value {
    c["repeat"] = json!(true);
    c
}

/// `c` ending, once FERB cancels it, only after `release(name)`: a slow cancel, as a real run can take seconds to end.
pub fn slow_cancel(mut c: Value, name: &str) -> Value {
    c["cancel_hold"] = json!(name);
    c
}

/// `c` answering only after `release(name)`.
pub fn held(mut c: Value, name: &str) -> Value {
    c["hold"] = json!(name);
    c
}

/// `c` making these tool calls first, as the tool bridge does (each `(name, arguments)`; its replies are not compared).
pub fn with_tools(mut c: Value, calls: &[(&str, Value)]) -> Value {
    let mut lines = vec![
        json!({"send": json!({"jsonrpc": "2.0", "id": 1, "method": "auth.hello",
                                               "params": {"role": "mcp_shim", "token": ""}}).to_string()}),
    ];
    for (i, (name, args)) in calls.iter().enumerate() {
        lines.push(json!({"send": json!({"jsonrpc": "2.0", "id": i + 2, "method": "tools.call",
                                         "params": {"name": name, "arguments": args}}).to_string()}));
    }
    c["tool_bridge_lines"] = json!(lines);
    c
}

/// One process's model: its calls per role.
#[derive(Default, Clone)]
pub struct Model {
    pub roles: BTreeMap<String, Vec<Value>>,
    /// More of the script file (`fresh_run`, ...).
    pub script: BTreeMap<String, Value>,
}

impl Model {
    pub fn planner(mut self, c: Value) -> Self {
        self.roles.entry("orchestrator".into()).or_default().push(c);
        self
    }
    pub fn worker(mut self, c: Value) -> Self {
        self.roles
            .entry("worker:default".into())
            .or_default()
            .push(c);
        self
    }
    pub fn checker(mut self, c: Value) -> Self {
        self.roles.entry("checker".into()).or_default().push(c);
        self
    }
    /// The script file's `key` (read by the stand-ins: `fresh_run`, ...).
    pub fn script(mut self, key: &str, value: Value) -> Self {
        self.script.insert(key.into(), value);
        self
    }
}

/// The program on one temporary home, the owner's view of it read back.
pub struct Life {
    pub run: Run,
    /// The conversation FERB.app attached to (`chat.attach`): its messages go there and its chat reads it.
    pub session: String,
    /// More of every process's stand-ins (`link_helper`, ...).
    extra: Value,
    script: PathBuf,
    nn: String,
    processes: usize,
}

/// A process's stand-ins: group A real (the owner's questions, tasks and notices as the program has them), the pinned
/// Codex installed and never run, sequential ids of this process's own prefix (the real program's are random: a
/// restarted process never reuses one).
fn config(script: &Path, ids: &str) -> Value {
    json!({"group_a": false, "ids": ids, "script": script.to_string_lossy(), "programs": "recorder",
           "chat_trace": script.with_extension("trace.jsonl").to_string_lossy()})
}

/// The process's script; its questions are counted from `questions_from` (the stand-ins' counter ids: a later process
/// never gives an earlier one's id again, as the real program's ids never repeat).
fn with(mut cfg: Value, extra: &Value) -> Value {
    if let (Some(c), Some(e)) = (cfg.as_object_mut(), extra.as_object()) {
        c.extend(e.clone());
    }
    cfg
}

fn write_model(path: &Path, model: &Model, questions_from: u64) {
    let mut file = json!({"by_role": model.roles, "calls": [], "questions_from": questions_from});
    for (k, v) in &model.script {
        file[k.as_str()] = v.clone();
    }
    std::fs::write(path, file.to_string()).unwrap();
}

impl Life {
    pub fn start(root: &Path, nn: &str, model: Model) -> Life {
        Life::start_with(root, nn, model, json!({}))
    }

    /// As `start`, with more of the stand-ins (kept for every later process).
    pub fn start_with(root: &Path, nn: &str, model: Model, extra: Value) -> Life {
        let script = root.join(format!("{nn}-script.json"));
        write_model(&script, &model, 0);
        let (home, _) = Run::paths(root, nn);
        std::fs::create_dir_all(home.join("workspace")).unwrap();
        let run = Run::start(root, nn, with(config(&script, "id"), &extra));
        let mut life = Life {
            run,
            session: String::new(),
            extra,
            script,
            nn: nn.to_string(),
            processes: 1,
        };
        life.attach();
        life
    }

    /// FERB.app's attach at each connection (DaemonConnection.swift `attachDaemon`): the conversation it shows.
    pub fn attach(&mut self) {
        let mut params = json!({"new": false});
        if !self.session.is_empty() {
            params["resume"] = json!(self.session);
        }
        let attached = self.call("chat.attach", params);
        self.session = attached["result"]["session_id"]
            .as_str()
            .unwrap_or_else(|| panic!("chat.attach: {attached}"))
            .to_string();
    }

    /// The next process on the same home, with this model (after a stop or a kill).
    pub fn start_again(&mut self, model: Model) {
        assert!(self.run.child.is_none(), "the earlier process still runs");
        self.processes += 1;
        write_model(&self.script, &model, 1000 * (self.processes as u64 - 1));
        let mut cfg = with(
            config(&self.script, &format!("p{}", self.processes)),
            &self.extra,
        );
        cfg["user_home"] = json!(self.run.user.to_string_lossy());
        std::fs::write(&self.run.stand, cfg.to_string()).unwrap();
        self.run.spawn();
        self.attach();
    }

    /// `kill -9`: no shutdown at all, as a crash or a forced quit leaves it.
    pub fn kill(&mut self) {
        let (status, _) = self.run.signal_stop(libc::SIGKILL);
        assert_eq!(
            std::os::unix::process::ExitStatusExt::signal(&status),
            Some(libc::SIGKILL)
        );
    }

    /// FERB's own stop (SIGTERM with the stop marker): exit 0.
    pub fn stop(&mut self) {
        let (status, err) = self.run.stop();
        assert_eq!(status.code(), Some(0), "{err}");
    }

    pub fn client(&self) -> Raw {
        let mut c = Raw::connect(&self.run.socket()).unwrap();
        c.hello("cli").unwrap();
        c
    }

    /// A unary call's whole answer.
    pub fn call(&self, method: &str, params: Value) -> Value {
        let mut c = self.client();
        c.send(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string());
        loop {
            let line = c.recv().unwrap_or_else(|| panic!("{method}: no answer"));
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == json!(1) {
                return v;
            }
        }
    }

    /// `chat.send` from the owner: the stream, open.
    pub fn send(&self, text: &str) -> Send {
        let mut c = self.client();
        c.send(
            &json!({"jsonrpc": "2.0", "id": 2, "method": "chat.send",
                    "params": {"session_id": self.session, "text": text}})
            .to_string(),
        );
        Send {
            conn: c,
            events: Vec::new(),
            ended: None,
        }
    }

    /// The model calls started so far, per role (`runtime_role` of each made call).
    pub fn made(&self) -> Vec<String> {
        self.run.control(json!({"do": "script"}))["made"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|m| m["runtime_role"].as_str().unwrap_or("").to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The model calls that gave their final answer, by index of `made`.
    pub fn answered(&self) -> usize {
        self.run.control(json!({"do": "script"}))["answered"]
            .as_array()
            .map_or(0, Vec::len)
    }

    pub fn started(&self) -> Vec<Value> {
        self.run.control(json!({"do": "script"}))["started"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    pub fn release(&self, name: &str) {
        self.run.control(json!({"do": "release", "name": name}));
    }

    /// How many calls of this role have started.
    pub fn calls_of(&self, role: &str) -> usize {
        self.made().iter().filter(|r| *r == role).count()
    }

    /// Waits until `f`, else fails naming `what` with the chat as it stands.
    pub fn wait(&self, what: &str, secs: u64, mut f: impl FnMut(&Life) -> bool) {
        let until = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < until {
            if f(self) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if !f(self) {
            panic!(
                "never: {what}\n  made: {:?}\n  tasks: {}\n  journal: {}\n  chat: {}",
                self.made(),
                self.root_status().unwrap_or_default(),
                self.journal_kinds().join(" "),
                serde_json::to_string_pretty(&self.chat()).unwrap()
            );
        }
    }

    /// Until FERB is quiet: no new journal row or span for a while.
    pub fn settle(&self) {
        let mut last = self.run.control(json!({"do": "quiet_mark"}));
        let mut quiet = 0;
        let until = Instant::now() + Duration::from_secs(20);
        while Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
            let now = self.run.control(json!({"do": "quiet_mark"}));
            if now == last {
                quiet += 1;
                if quiet >= 15 {
                    return;
                }
            } else {
                quiet = 0;
                last = now;
            }
        }
    }

    /// The newest task tree's status.
    pub fn root_status(&self) -> Option<String> {
        self.run
            .control(json!({"do": "root_status"}))
            .as_str()
            .map(str::to_string)
    }

    /// The chat as every client reads it: the stored entries, oldest first (written out first).
    pub fn chat(&self) -> Vec<Value> {
        self.run.control(json!({"do": "chat_flush"}));
        let page = self.call("chat.entries.page", json!({"limit": 500}));
        page["result"]["entries"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("chat.entries.page: {page}"))
    }

    /// The chat's entries of one kind.
    pub fn entries(&self, kind: &str) -> Vec<Value> {
        self.chat()
            .into_iter()
            .filter(|e| e["kind"] == kind && e["deleted"] != json!(true))
            .collect()
    }

    /// The questions waiting for the owner (`chat.status`'s approvals).
    pub fn open_questions(&self) -> Vec<Value> {
        self.call("chat.status", json!({}))["result"]["approvals"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// The journal's rows of one kind (their data).
    pub fn journal(&self, kind: &str) -> Vec<Value> {
        let db = rusqlite::Connection::open(self.run.home.join("ferb.db")).unwrap();
        db.busy_timeout(Duration::from_secs(5)).unwrap();
        let mut q = db
            .prepare("SELECT data_json FROM journal WHERE kind = ?1 ORDER BY seq")
            .unwrap();
        let rows: Vec<String> = q
            .query_map([kind], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        rows.iter()
            .map(|d| serde_json::from_str(d).unwrap_or(Value::Null))
            .collect()
    }

    /// Every journal row's kind, in order (what a failed wait shows).
    pub fn journal_kinds(&self) -> Vec<String> {
        let db = rusqlite::Connection::open(self.run.home.join("ferb.db")).unwrap();
        db.busy_timeout(Duration::from_secs(5)).unwrap();
        let mut q = db.prepare("SELECT kind FROM journal ORDER BY seq").unwrap();
        let rows = q
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        rows
    }

    /// A plan step's stored status (`plan_steps`: a one-step task's step is `worker`); None before it has a row.
    pub fn step_status(&self, task: &str, step: &str) -> Option<String> {
        let db = rusqlite::Connection::open(self.run.home.join("ferb.db")).unwrap();
        db.busy_timeout(Duration::from_secs(5)).unwrap();
        db.query_row(
            "SELECT status FROM plan_steps WHERE task_id = ?1 AND step_id = ?2",
            [task, step],
            |r| r.get::<_, String>(0),
        )
        .ok()
    }

    /// The task rows (`id`, `status`), oldest first.
    pub fn tasks(&self) -> Vec<(String, String)> {
        let db = rusqlite::Connection::open(self.run.home.join("ferb.db")).unwrap();
        db.busy_timeout(Duration::from_secs(5)).unwrap();
        let mut q = db
            .prepare("SELECT id, status FROM tasks ORDER BY rowid")
            .unwrap();
        let rows = q
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        rows
    }

    /// Every file of the home beside the database whose name says it was set aside (STORE-1's `.broken-`).
    pub fn set_aside(&self) -> Vec<String> {
        std::fs::read_dir(&self.run.home)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".broken-"))
            .collect()
    }
}

impl Life {
    /// Stage 2's order rules (ferb-supervisor tests/support_entries) on the chat log's trace of every process so far
    /// and the stored entries, and rule 7 on the store (a client catching up after any change reads exactly the newer
    /// entries): what breaks, one line each.
    pub fn order_rules(&self, name: &str) -> Vec<String> {
        use ferb_core::chat_entry::Entry;
        self.run.control(json!({"do": "chat_flush"}));
        let items: Vec<Value> = std::fs::read_to_string(self.script.with_extension("trace.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        let conn = rusqlite::Connection::open_with_flags(
            self.run.home.join("ferb.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        conn.busy_timeout(Duration::from_secs(5)).unwrap();
        let mut stored: Vec<Entry> = Vec::new();
        let mut out = Vec::new();
        for c in ferb_store::chat_entries::conversations(&conn).unwrap() {
            let page = ferb_store::chat_entries::page(&conn, &c, None, None, 100_000).unwrap();
            let mut by_seq = page.entries.clone();
            by_seq.sort_by_key(|e| e.seq);
            let mut cuts = vec![0];
            cuts.extend(by_seq.iter().map(|e| e.seq));
            for k in cuts {
                let got = ferb_store::chat_entries::changes(&conn, &c, k, 100_000)
                    .unwrap()
                    .entries;
                let want: Vec<Entry> = by_seq.iter().filter(|e| e.seq > k).cloned().collect();
                if got != want {
                    out.push(format!(
                        "{name} [{c}]: changes after {k} differ from the entries above it (rule 7)"
                    ));
                    break;
                }
            }
            stored.extend(page.entries);
        }
        out.extend(crate::entry_rules::check(
            name,
            &crate::entry_rules::read(&items),
            &stored,
        ));
        out
    }
}

/// An open `chat.send` stream.
pub struct Send {
    conn: Raw,
    pub events: Vec<Value>,
    /// The stream's last line (its result or error), once it came.
    pub ended: Option<Value>,
}

impl Send {
    /// Reads the stream to its end (the line answering the call), within the connection's read timeout.
    pub fn end(&mut self) -> &Value {
        while self.ended.is_none() {
            let Some(line) = self.conn.recv() else {
                self.ended = Some(json!({"closed": true}));
                break;
            };
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == json!(2) {
                self.ended = Some(v);
            } else {
                self.events.push(v["params"].clone());
            }
        }
        self.ended.as_ref().unwrap()
    }

    /// The stream's frames of one type.
    pub fn frames(&self, kind: &str) -> Vec<&Value> {
        self.events.iter().filter(|e| e["type"] == kind).collect()
    }
}

/// An answer entry's words.
pub fn said(e: &Value) -> String {
    e["body"]["text"].as_str().unwrap_or_default().to_string()
}
