//! FERB.app's stored-data viewer and the owner's delete of a turn (owner STORE-422; agent STORE-423, STORE-424,
//! STORE-425), through the daemon's real RPC server on a socket in a temporary folder, after a recorded chat turn
//! (`plain_reply`: the owner's "hi there" and FERB's answer) ran through the real supervisor on a temporary store.
//!
//! What the planner is given is read with `data.context` (the same `_conversation_blocks` its next turn reads) before
//! and after: a deleted turn, or one before a fresh conversation, isn't in it.

mod common;
mod support_control;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;

use ferb_gateway::questions::{Approval, Approvals, ResolveError};
use ferb_orchestrator::ports::BoxFuture;
use ferb_rpc::auth::RunGrant;
use ferb_supervisor::flow::chat_rpc::{ChatDeps, ChatGateway, ChatRpc};
use serde_json::{json, Map, Value};

struct Fresh(std::sync::Mutex<Approvals>);

impl ChatGateway for Fresh {
    fn pending(&self) -> Vec<Approval> {
        Vec::new()
    }
    fn approval(&self, id: &str) -> Option<Approval> {
        self.0.lock().unwrap().get(id).cloned()
    }
    fn resolve(
        &self,
        id: &str,
        granted: bool,
        choice: Option<usize>,
        text: Option<&str>,
        _hidden: &[String],
    ) -> Result<Approval, ResolveError> {
        self.0
            .lock()
            .unwrap()
            .resolve(id, granted, choice, text)
            .cloned()
    }
    fn tools_for(&self, _grant: &RunGrant) -> Vec<Value> {
        Vec::new()
    }
    fn call<'a>(
        &'a self,
        _grant: &'a RunGrant,
        _name: &'a str,
        _arguments: &'a Map<String, Value>,
    ) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(async { Err("RuntimeError".to_string()) })
    }
    fn stopped(&self) -> bool {
        false
    }
}

struct RpcClock;

impl ferb_rpc::Clock for RpcClock {
    fn monotonic(&self) -> f64 {
        1000.0
    }
}

/// One connection as FERB.app's role (`cli`).
struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next: u64,
}

impl Client {
    fn new(socket: &Path) -> Client {
        let st = UnixStream::connect(socket).unwrap();
        let mut writer = st.try_clone().unwrap();
        let mut reader = BufReader::new(st);
        writer
            .write_all(b"{\"jsonrpc\": \"2.0\", \"id\": 0, \"method\": \"auth.hello\", \"params\": {\"role\": \"cli\"}}\n")
            .unwrap();
        let mut hello = String::new();
        reader.read_line(&mut hello).unwrap();
        Client {
            reader,
            writer,
            next: 1,
        }
    }

    /// The reply's `result`, or its `error`.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, Value> {
        let id = self.next;
        self.next += 1;
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.writer
            .write_all(format!("{line}\n").as_bytes())
            .unwrap();
        let mut got = String::new();
        self.reader.read_line(&mut got).unwrap();
        let reply: Value = serde_json::from_str(&got).unwrap();
        match reply.get("error") {
            Some(e) => Err(e.clone()),
            None => Ok(reply["result"].clone()),
        }
    }
}

fn texts(context: &Value) -> String {
    context["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["text"].as_str().unwrap().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn one(c: &rusqlite::Connection, sql: &str) -> i64 {
    c.query_row(sql, [], |r| r.get(0)).unwrap()
}

const REQUEST: &str = "id-000015";
const MESSAGE: &str = "hi there";
const REPLY: &str = "Hello! What can I do for you today?";

// decision: STORE-422
// decision: STORE-423
// decision: STORE-424
// decision: STORE-425
// decision: STORE-426
// decision: STORE-427
#[test]
fn a_turn_is_viewed_then_deleted_and_the_planner_no_longer_reads_it() {
    let mut w = support_control::world("plain_reply");
    support_control::run(&mut w, "plain_reply");
    w.sup.chat_log().flush(w.sup.inner());
    {
        // what else holds the turn: the search index's copy (the recall indexer's), FERB.app's own rows of it, and a
        // later model call that quoted FERB's answer
        let db = w.db.lock().unwrap();
        let c = db.conn();
        c.execute(
            "INSERT INTO chat_fts (doc, kind, session, at, request, text) \
             VALUES ('journal:id-000017', 'chat.message', 's1', '2026-01-01T09:00:00+00:00', ?, ?)",
            [REQUEST, MESSAGE],
        )
        .unwrap();
        let turn = json!({"id": "6B2F0C1E-0000-4000-8000-000000000001", "requestID": REQUEST,
                          "request": MESSAGE, "answer": REPLY, "createdAt": 0, "status": "done"});
        let reply = json!({"id": "6B2F0C1E-0000-4000-8000-000000000002",
                           "turnID": "6B2F0C1E-0000-4000-8000-000000000001", "createdAt": 1});
        for (id, kind, body) in [
            ("6B2F0C1E-0000-4000-8000-000000000001", "turn", &turn),
            ("6B2F0C1E-0000-4000-8000-000000000002", "reply", &reply),
        ] {
            c.execute(
                "INSERT INTO chat_history (id, kind, at, day, open, body_json, updated_at) \
                 VALUES (?, ?, '2026-01-01T09:00:00.000000+00:00', '2026-01-01', 0, ?, '2026-01-01T09:00:00+00:00')",
                [id, kind, &body.to_string()],
            )
            .unwrap();
        }
        let later = json!({"role": "supervisor", "request_id": "r-later",
                           "context": format!("Layer 3 — the conversation: FERB: {REPLY} …")});
        c.execute(
            "INSERT INTO spans (id, kind, started_at, input_json) VALUES ('later', 'model.turn', '2026-01-02T09:00:00+00:00', ?)",
            [ferb_py::json::dumps(&later)],
        )
        .unwrap();
    }
    let sup = w.sup.clone();
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("s.sock");
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let server = rt.block_on(async {
        let server = ferb_rpc::Server::new(
            socket.clone(),
            Arc::new(ferb_rpc::RunTokens::new(Arc::new(RpcClock))),
        );
        let rpc = ChatRpc::new(ChatDeps {
            supervisor: sup,
            gateway: Arc::new(Fresh(std::sync::Mutex::new(Approvals::new(|| {
                "ap-a00000000001".to_string()
            })))),
            persona: None,
            prepare_persona: Arc::new(|| {}),
            notices: Some(Arc::new(Vec::new)),
            conversations: Arc::new(Vec::new),
            sensing_paused: Arc::new(|| false),
            shutting_down: Arc::new(|| false),
            question_answered: Arc::new(|_ap| String::new()),
            ask_input_monitoring: Arc::new(|| {}),
            click_through: None,
        });
        ferb_supervisor::flow::chat_rpc::register(&server, rpc);
        server.set_is_app(|_| false);
        server.start().await.expect("the server");
        server
    });

    // a connection that isn't FERB.app's is refused every data call, the reads and the delete alike
    let mut other = Client::new(&socket);
    for method in ferb_supervisor::flow::data_rpc::METHODS {
        let e = other
            .call(method, json!({"table": "journal", "request_id": REQUEST}))
            .unwrap_err();
        assert_eq!(e["code"], ferb_rpc::wire::APP_ONLY_REFUSED, "{method}: {e}");
    }
    server.set_is_app(|_| true);
    let mut app = Client::new(&socket);

    // the viewer: every table, the journal newest first, the turn's row openable, the phrase found where it is
    let tables = app.call("data.tables", json!({})).unwrap();
    let journal = tables["tables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "journal")
        .unwrap();
    assert_eq!(journal["rows"], 17);
    let page = app
        .call("data.rows", json!({"table": "journal", "limit": 2}))
        .unwrap();
    assert_eq!(page["rows"].as_array().unwrap().len(), 2);
    assert_eq!(
        page["rows"][0]["action"],
        json!({"do": "turn", "id": REQUEST})
    );
    let rowid = page["rows"][1]["rowid"].as_i64().unwrap();
    let row = app
        .call("data.row", json!({"table": "journal", "rowid": rowid}))
        .unwrap();
    assert!(row["cells"][4].as_str().unwrap().contains(MESSAGE), "{row}");
    let found = app.call("data.search", json!({"text": MESSAGE})).unwrap();
    let tables_hit: Vec<&str> = found["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["table"].as_str().unwrap())
        .collect();
    for t in ["chat_history", "chat_entries", "journal", "spans"] {
        assert!(tables_hit.contains(&t), "{t} in {tables_hit:?}");
    }
    assert!(app
        .call("data.rows", json!({"table": "journal; DROP TABLE journal"}))
        .is_err());

    // what the planner reads now: the turn is in its conversation
    let before = app
        .call("data.context", json!({"conversation": "s1"}))
        .unwrap();
    assert!(texts(&before).contains(MESSAGE), "{before}");
    assert!(texts(&before).contains(REPLY));

    // STORE-425: a fresh conversation is a boundary: its planner reads none of the earlier turns
    let fresh = app.call("chat.attach", json!({"new": true})).unwrap();
    let fresh_id = fresh["session_id"].as_str().unwrap().to_string();
    assert_ne!(fresh_id, "s1");
    // STORE-427: the fresh conversation starts with its marker, an entry every client reads (never journaled, so the
    // planner's conversation of it stays empty)
    {
        let db = w.db.lock().unwrap();
        let (kind, body, seq): (String, String, i64) = db
            .conn()
            .query_row(
                "SELECT kind, body_json, seq FROM chat_entries WHERE conversation_id = ?",
                [&fresh_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((kind.as_str(), seq), ("notice", 1));
        assert!(
            body.contains(ferb_supervisor::flow::data_delete::FRESH_MARKER),
            "{body}"
        );
        assert_eq!(
            ferb_supervisor::flow::data_delete::FRESH_MARKER,
            "New conversation: FERB doesn't read the messages above."
        );
        assert_eq!(
            one(db.conn(), "SELECT COUNT(*) FROM journal WHERE instr(data_json, 'read the messages above') > 0"),
            0
        );
    }
    let next = app.call("data.context", json!({})).unwrap();
    assert_eq!(next["conversation"], fresh_id.as_str());
    assert!(
        !texts(&next).contains(MESSAGE) && !texts(&next).contains(REPLY),
        "{next}"
    );

    // the delete: unknown messages are refused, the preview changes nothing, the owner's Yes deletes
    let unknown = app
        .call("data.delete", json!({"request_id": "no-such-request"}))
        .unwrap_err();
    assert_eq!(
        unknown["message"],
        ferb_supervisor::flow::data_delete::NOTHING_STORED
    );
    let preview = app
        .call("data.delete", json!({"request_id": REQUEST}))
        .unwrap();
    assert_eq!(preview["confirmed"], false);
    assert_eq!(preview["will_delete"]["conversation_rows"], 2);
    assert_eq!(preview["will_delete"]["chat_entries"], 2);
    assert_eq!(preview["will_delete"]["chat_history"], 2);
    assert!(preview["will_delete"]["traces"].as_i64().unwrap() >= 3);
    let seq_before = {
        let db = w.db.lock().unwrap();
        one(
            db.conn(),
            "SELECT last_seq FROM chat_streams WHERE conversation_id = 's1'",
        )
    };
    let done = app
        .call(
            "data.delete",
            json!({"request_id": REQUEST, "confirm": true}),
        )
        .unwrap();
    assert_eq!(done["confirmed"], true, "{done}");
    assert_eq!(done["deleted"]["later_quotes_blanked"], 1);

    // the planner reads nothing of it any more
    let after = app
        .call("data.context", json!({"conversation": "s1"}))
        .unwrap();
    assert!(!texts(&after).contains(MESSAGE), "{after}");
    assert!(!texts(&after).contains(REPLY), "{after}");
    {
        let db = w.db.lock().unwrap();
        let c = db.conn();
        // the conversation rows are gone with their index copy; their ids are tombstoned, nothing of their text
        assert_eq!(
            one(
                c,
                "SELECT COUNT(*) FROM journal WHERE kind IN ('chat.message', 'chat.reply')"
            ),
            0
        );
        assert_eq!(one(c, "SELECT COUNT(*) FROM chat_fts"), 0);
        assert_eq!(
            one(c, "SELECT COUNT(*) FROM forgotten_refs WHERE ref IN ('journal:id-000017', 'journal:id-000021')"),
            2
        );
        // ENG-12: the entries are tombstones with emptied bodies and new seqs, so every device drops them
        assert_eq!(
            one(c, "SELECT COUNT(*) FROM chat_entries WHERE deleted = 1 AND body_json = '{}' AND open = 0"),
            2
        );
        assert_eq!(
            one(
                c,
                "SELECT COUNT(*) FROM chat_entries WHERE conversation_id = 's1'"
            ),
            2,
            "rows kept"
        );
        assert_eq!(
            one(
                c,
                "SELECT last_seq FROM chat_streams WHERE conversation_id = 's1'"
            ),
            seq_before + 2
        );
        assert_eq!(
            one(
                c,
                "SELECT MIN(seq) FROM chat_entries WHERE conversation_id = 's1'"
            ),
            seq_before + 1
        );
        assert_eq!(one(c, "SELECT COUNT(*) FROM chat_history"), 0);
        // its own traces go; a later call's quote of it is blanked
        assert_eq!(
            one(
                c,
                "SELECT COUNT(*) FROM spans WHERE instr(input_json, 'id-000015') > 0"
            ),
            0
        );
        let later: String = c
            .query_row("SELECT input_json FROM spans WHERE id = 'later'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(!later.contains(REPLY));
        assert!(later.contains("Removed: this recorded context"));
        // the delete itself is journaled as a fact: what and how many, never the words
        let fact: String = c
            .query_row(
                "SELECT data_json FROM journal WHERE kind = 'data.deleted'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            fact.contains(REQUEST) && !fact.contains(MESSAGE) && !fact.contains("Hello"),
            "{fact}"
        );
    }
    // nothing of it is found any more, except the audit row of its acknowledgement (kept: rule 6) and the fact
    let found = app.call("data.search", json!({"text": MESSAGE})).unwrap();
    assert!(found["hits"].as_array().unwrap().is_empty(), "{found}");
    // a second delete finds nothing left to delete but the kept audit row
    let again = app
        .call("data.delete", json!({"request_id": REQUEST}))
        .unwrap_err();
    assert_eq!(
        again["message"],
        ferb_supervisor::flow::data_delete::NOTHING_STORED
    );
    drop((other, app));
    rt.block_on(server.close());
}

/// The kinds a turn's delete removes are exactly the planner's conversation kinds, its self posts, the plan line and
/// the running summary: nothing the planner reads is left behind, and no other audit kind is touched.
// decision: STORE-424
#[test]
fn the_deleted_kinds_are_the_planner_s_conversation_kinds() {
    use ferb_supervisor::words::{CONVERSATION_KINDS, SELF_POSTS, SUMMARY_KIND};
    let mut want: Vec<&str> = CONVERSATION_KINDS.to_vec();
    want.extend(SELF_POSTS);
    want.push("chat.plan");
    want.push(SUMMARY_KIND);
    want.sort();
    let mut have = ferb_store::data_view::CONTENT_KINDS.to_vec();
    have.sort();
    assert_eq!(have, want);
}

fn exec(c: &rusqlite::Connection, sql: &str, args: &[&str]) {
    c.execute(sql, rusqlite::params_from_iter(args.iter()))
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

fn has(c: &rusqlite::Connection, table: &str, id: &str) -> bool {
    one(
        c,
        &format!("SELECT COUNT(*) FROM {table} WHERE id = '{id}'"),
    ) > 0
}

/// A turn with a task: its tasks, results, traces, the record of how it went, its screenshots (only files under the
/// evidence folder, never through a link) and what FERB learned only from it go; what FERB also learned elsewhere,
/// the audit row and the activity trail stay; the running summaries go from the first that folded the turn in.
// decision: STORE-424
// decision: STORE-426
// decision: STORE-428
#[test]
fn a_turn_with_a_task_goes_with_its_screenshots_and_what_was_learned_only_from_it() {
    let mut w = support_control::world("plain_reply");
    support_control::run(&mut w, "plain_reply");
    w.sup.chat_log().flush(w.sup.inner());
    let evidence = w.dir.path().join("evidence");
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("not-ferbs.jpg");
    std::fs::write(&target, b"the owner's own file").unwrap();
    std::fs::create_dir_all(evidence.join("2026-01-01")).unwrap();
    std::fs::write(evidence.join("2026-01-01/a.jpg"), b"jpeg").unwrap();
    std::os::unix::fs::symlink(&target, evidence.join("2026-01-01/link.jpg")).unwrap();
    std::fs::write(w.dir.path().join("escape.jpg"), b"beside the folder").unwrap();
    {
        let db = w.db.lock().unwrap();
        let c = db.conn();
        let at = "2026-01-01T09:00:05+00:00";
        for (id, parent) in [("t1", None), ("t1.1", Some("t1"))] {
            c.execute(
                "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, \
                 created_at, updated_at) VALUES (?, 't1', ?, ?, 'say hello back', 'owner', 'worker', '[]', 'done', ?, ?)",
                rusqlite::params![id, parent, REQUEST, at, at],
            )
            .unwrap();
        }
        exec(c, "INSERT INTO task_results (task_id, at, status, goal, answer) VALUES ('t1', ?, 'done', 'g', 'a')", &[at]);
        exec(c, "INSERT INTO task_context (task_id, conversation_id, spec_json, owner_words) VALUES ('t1', 's1', '{}', 'hi there')", &[]);
        exec(c, "INSERT INTO spans (id, kind, task_id, started_at, input_json) VALUES ('sp-t1', 'worker.run', 't1', ?, '{}')", &[at]);
        exec(c, "INSERT INTO spans (id, parent_id, kind, task_id, started_at, input_json) VALUES ('sp-t1-c', 'sp-t1', 'tool.call', 't1.1', ?, '{}')", &[at]);
        exec(c, "INSERT INTO journal (id, at, kind, data_json) VALUES ('j-answer', ?, 'task.answer', \
                 '{\"session_id\": \"s1\", \"task_id\": \"t1\", \"text\": \"Hello back from the sample task, as asked.\"}')", &[at]);
        exec(c, "INSERT INTO journal (id, at, kind, data_json) VALUES ('j-audit', ?, 'task.started', '{\"task_id\": \"t1\"}')", &[at]);
        // a chain of running summaries: the first folded only older rows, the second the turn, the third later ones
        for (id, covered) in [
            ("sum-1", "entry:older"),
            ("sum-2", "entry:id-000017"),
            ("sum-3", "entry:later"),
        ] {
            exec(c, &format!("INSERT INTO journal (id, at, kind, data_json) VALUES (?, ?, 'chat.summary', \
                 '{{\"session_id\": \"s1\", \"covered\": [\"{covered}\"], \"text\": \"summary\"}}')"), &[id, at]);
        }
        exec(
            c,
            "INSERT INTO trail (id, at, day, device_id, actor, task_id, source, data_json) \
                 VALUES ('tr1', ?, '2026-01-01', 'mac', 'ferb', 't1', 'browser', '{}')",
            &[at],
        );
        for (id, content) in [
            ("m-only", "The owner greets FERB with hi there"),
            ("m-shared", "The owner likes short greetings"),
        ] {
            exec(c, "INSERT INTO memory_items (id, kind, subject, key, content, origin, status, confidence, observed_at, \
                     valid_from, created_at) VALUES (?, 'fact', 'owner', ?, ?, 'owner_stated', 'active', 1.0, ?, ?, ?)",
                 &[id, id, content, at, at, at]);
        }
        for (item, kind, source) in [
            ("m-only", "journal", "id-000017"),
            ("m-only", "trail", "tr1"),
            ("m-shared", "journal", "id-000017"),
            ("m-shared", "journal", "elsewhere"),
        ] {
            exec(
                c,
                "INSERT INTO memory_sources (item_id, kind, source_id) VALUES (?, ?, ?)",
                &[item, kind, source],
            );
        }
        for (id, title) in [
            ("n-only", "Greets back warmly"),
            ("n-shared", "Keeps greetings short"),
        ] {
            exec(c, &format!("INSERT INTO learning_nodes (id, kind, key, body_json, origin, made_by, recorded_at, valid_from) \
                     VALUES (?, 'episode', ?, '{{\"title\": \"{title}\"}}', 'ferb', 'test', ?, ?)"), &[id, id, at, at]);
        }
        for (node, source) in [
            ("n-only", "journal:id-000021"),
            ("n-only", "episode:t1"),
            ("n-shared", "episode:t1"),
            ("n-shared", "journal:elsewhere"),
        ] {
            exec(
                c,
                "INSERT INTO learning_sources (node_id, source_ref) VALUES (?, ?)",
                &[node, source],
            );
        }
        exec(c, "INSERT INTO episodes (task_id, request_id, request_text, goal, status, answer, conditions_json, steps_json, problems_json, tools_json, origins, external_read, usage_json, desktop_json, made_by, built_at) \
                 VALUES ('t1', ?, 'hi there', 'say hello back', 'done', 'hello', '[]', '[]', '[]', '{}', 'owner', 0, '{}', '{}', 'test', '2026-01-01')", &[REQUEST]);
        for (id, path) in [
            ("e1", Some("2026-01-01/a.jpg")),
            ("e2", Some("2026-01-01/link.jpg")),
            ("e3", Some("../escape.jpg")),
            ("e4", None),
        ] {
            c.execute(
                "INSERT INTO evidence (id, task_id, kind, path, origin, captured_at) VALUES (?, 't1', 'screenshot', ?, 'ferb', ?)",
                rusqlite::params![id, path, at],
            )
            .unwrap();
        }
    }
    let inner = w.sup.inner().clone();
    let preview = inner.delete_turn(REQUEST, false).unwrap();
    let will = &preview["will_delete"];
    assert_eq!(will["tasks"], 2, "{preview}");
    assert_eq!(will["screenshots"], 4);
    assert_eq!(will["episodes"], 1);
    assert_eq!(will["summaries"], 2);
    assert_eq!(
        will["memories"],
        json!([{"id": "m-only", "text": "The owner greets FERB with hi there"}])
    );
    assert_eq!(
        will["learned"],
        json!([{"id": "n-only", "text": "Greets back warmly"}])
    );
    assert_eq!(
        preview["keeps_memories"],
        json!([{"id": "m-shared", "text": "The owner likes short greetings"}])
    );
    assert_eq!(
        preview["keeps_learned"],
        json!([{"id": "n-shared", "text": "Keeps greetings short"}])
    );
    assert_eq!(preview["keeps"]["audit_log"], 2, "the ack and task.started");
    assert!(
        w.daemon.forgot_learned.lock().unwrap().is_empty(),
        "the preview forgets nothing"
    );

    let done = inner.delete_turn(REQUEST, true).unwrap();
    let gone = &done["deleted"];
    assert_eq!(gone["tasks"], 2, "{done}");
    assert_eq!(gone["screenshots"], 3);
    assert_eq!(gone["screenshots_kept"], 1);
    assert_eq!(gone["episodes"], 1);
    assert_eq!(
        (gone["memories"].clone(), gone["learned"].clone()),
        (json!(1), json!(1))
    );
    // the memory port's own forget was asked for exactly what was learned only from the turn
    assert_eq!(
        *w.daemon.forgot_learned.lock().unwrap(),
        vec![(vec!["m-only".to_string()], vec!["n-only".to_string()])]
    );
    let db = w.db.lock().unwrap();
    let c = db.conn();
    assert_eq!(one(c, "SELECT COUNT(*) FROM tasks"), 0);
    for table in ["task_results", "task_context", "episodes"] {
        assert_eq!(
            one(
                c,
                &format!("SELECT COUNT(*) FROM {table} WHERE task_id = 't1'")
            ),
            0,
            "{table}"
        );
    }
    assert!(!has(c, "spans", "sp-t1") && !has(c, "spans", "sp-t1-c"));
    assert!(!has(c, "journal", "j-answer"));
    assert!(has(c, "journal", "j-audit"), "the audit row stays");
    assert!(has(c, "trail", "tr1"), "the activity trail stays");
    assert!(
        has(c, "journal", "sum-1") && !has(c, "journal", "sum-2") && !has(c, "journal", "sum-3")
    );
    // screenshots: the file under the folder and the link itself go; the link's target and anything outside stay
    assert!(!evidence.join("2026-01-01/a.jpg").exists());
    assert!(std::fs::symlink_metadata(evidence.join("2026-01-01/link.jpg")).is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"the owner's own file");
    assert!(w.dir.path().join("escape.jpg").exists());
    assert_eq!(one(c, "SELECT COUNT(*) FROM evidence"), 1);
    assert!(has(c, "evidence", "e3"));
    // the fact journaled: counts, never names or words
    let fact: String = c
        .query_row(
            "SELECT data_json FROM journal WHERE kind = 'data.deleted'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    for word in ["a.jpg", "link.jpg", "greets", "hi there"] {
        assert!(!fact.contains(word), "{word} in {fact}");
    }
}

/// A turn FERB is still answering, or whose task still runs, is refused in plain words; a finished one isn't.
// decision: STORE-424
#[test]
fn a_turn_still_answered_or_running_is_refused() {
    use ferb_supervisor::flow::data_delete::{busy, Plan, STILL_ANSWERING, STILL_WORKING};
    let p = Plan {
        request_id: "r1".into(),
        trees: vec!["t1".into()],
        ..Plan::default()
    };
    let running: std::collections::BTreeSet<String> = ["t1".to_string()].into();
    assert_eq!(busy(&p, true, &Default::default()), Some(STILL_ANSWERING));
    assert_eq!(busy(&p, false, &running), Some(STILL_WORKING));
    assert_eq!(busy(&p, false, &["t2".to_string()].into()), None);
    // the recorded turn's stream ended, so the chat log no longer holds it open
    let mut w = support_control::world("plain_reply");
    support_control::run(&mut w, "plain_reply");
    w.sup.chat_log().flush(w.sup.inner());
    assert!(!w.sup.chat_log().is_open(REQUEST));
}
