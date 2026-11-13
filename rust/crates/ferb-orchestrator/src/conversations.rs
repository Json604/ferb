//! ferb/orchestrator/conversations.py: durable delegated conversations (F9, ORCH-301). Observations can wake work,
//! never create its authority. The scope's words (`approval_text`, `card`) are portable; the store and the runner
//! are Mac-side.
//!
//! decision: ORCH-301, GATE-566

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};

use crate::loops::Wake;
use crate::models::ConversationScope;
use crate::ports::{BoxFuture, Redact};
use crate::sql::{self, text, Error, Result};

/// confirm.py:53 `SEND_CLASS`.
pub const SEND_CLASS: &[&str] = &["send", "reply", "post", "mailbox_send", "submit", "call"];

/// conversations.py:32 `approval_text`.
pub fn approval_text(s: &ConversationScope) -> String {
    format!(
        "Continue this conversation as {}, account {}, on {} ({}), thread {}, with {}?\nTopic: {}\nHand over when: {}\n\
         This includes accepting this platform's automation terms and account risks for this task. You can pause, \
         correct the scope or take over at any time. Message sends still follow the separately displayed standing \
         permission and security checks.",
        s.identity,
        s.account,
        s.site,
        s.app,
        s.thread,
        s.recipients.join(", "),
        s.topic,
        s.handover_when
    )
}

/// conversations.py:40 `card` (GATE-566).
pub fn card(s: &ConversationScope) -> Value {
    let who = s.recipients.join(", ");
    let as_whom = if s.identity == "owner" {
        "as you"
    } else {
        "as FERB"
    };
    json!({
        "summary": format!("Let FERB chat {as_whom} with {who} on {}?", s.site),
        "exact": format!("{} · {} · thread {}", s.account, s.app, s.thread),
        "detail": format!(
            "About: {}. It hands back to you when {}. Automating {} may break its rules and risk the account. You can \
             pause or take over any time; each send still follows your send rules.",
            s.topic, s.handover_when, s.site
        ),
        "labels": ["Let FERB chat", "Don't"],
    })
}

/// A tool call as the conversation guard reads it (the gateway's grant, capability and spec).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GuardCall {
    pub observation: bool,
    pub task_id: String,
    pub identity: String,
    pub site: Option<String>,
    pub account: Option<String>,
    pub recipients: Vec<String>,
    pub contact: Option<String>,
    pub kind: String,
    /// `declared_kinds(spec)`.
    pub declared: Vec<String>,
}

/// conversations.py:54 `ConversationStore`.
pub struct ConversationStore<'a> {
    pub db: &'a ferb_store::Db,
    pub now: Arc<dyn Fn() -> String + Send + Sync>,
    pub redact: Redact,
    pub publish: crate::ports::Publish,
    wake: Mutex<HashMap<String, Arc<Wake>>>,
}

fn shift_take(row: &mut Map<String, Value>, key: &str) -> Value {
    row.shift_remove(key).unwrap_or(Value::Null)
}

impl<'a> ConversationStore<'a> {
    pub fn new(
        db: &'a ferb_store::Db,
        now: Arc<dyn Fn() -> String + Send + Sync>,
        redact: Redact,
        publish: crate::ports::Publish,
    ) -> ConversationStore<'a> {
        ConversationStore {
            db,
            now,
            redact,
            publish,
            wake: Mutex::new(HashMap::new()),
        }
    }

    fn waker(&self, ident: &str) -> Arc<Wake> {
        self.wake
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(ident.to_string())
            .or_default()
            .clone()
    }

    /// conversations.py:60 `get`.
    pub fn get(&self, ident: &str) -> Result<Map<String, Value>> {
        let Some(mut row) = sql::row(
            self.db.conn(),
            "SELECT * FROM delegated_conversations WHERE id=?",
            &[text(ident)],
        )?
        else {
            return Err(Error::Value("No such delegated conversation".into()));
        };
        let scope = shift_take(&mut row, "scope_json");
        row.insert(
            "scope".into(),
            sql::loads(scope.as_str().unwrap_or("null"))?,
        );
        let handover = shift_take(&mut row, "handover_json");
        let handover = match handover.as_str() {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => "null".into(),
        };
        row.insert("handover".into(), sql::loads(&handover)?);
        Ok(row)
    }

    /// conversations.py:69 `list`.
    pub fn list(&self, tree_id: Option<&str>) -> Result<Vec<Map<String, Value>>> {
        let rows = match tree_id {
            Some(t) => sql::rows(
                self.db.conn(),
                "SELECT id FROM delegated_conversations WHERE tree_id=? ORDER BY created_at",
                &[text(t)],
            )?,
            None => sql::rows(
                self.db.conn(),
                "SELECT id FROM delegated_conversations ORDER BY created_at",
                &[],
            )?,
        };
        rows.iter().map(|r| self.get(&sql::col(r, "id"))).collect()
    }

    /// conversations.py:76 `open`.
    pub fn open(
        &self,
        ident: &str,
        tree_id: &str,
        scope: &ConversationScope,
    ) -> Result<Map<String, Value>> {
        let dumped = crate::validate::dump_conversation_scope(scope).to_value();
        let encoded = ferb_py::json::dumps_with(
            &(self.redact)(&dumped),
            ferb_py::json::Style {
                ascii: true,
                sort_keys: true,
                indent: None,
            },
        );
        if let Some(existing) = sql::row(
            self.db.conn(),
            "SELECT tree_id,scope_json FROM delegated_conversations WHERE id=?",
            &[text(ident)],
        )? {
            if sql::col(&existing, "tree_id") != tree_id
                || sql::col(&existing, "scope_json") != encoded
            {
                return Err(Error::Value(
                    "An approved conversation scope cannot be silently replaced; start a newly approved scope".into(),
                ));
            }
            return self.get(ident);
        }
        if let Some(prior) = sql::row(
            self.db.conn(),
            "SELECT id,status FROM delegated_conversations WHERE tree_id=? AND scope_json=?",
            &[text(tree_id), text(&encoded)],
        )? {
            return Err(Error::Value(format!(
                "This task already has that conversation scope ({}); a new ID cannot repeat its approval or sends",
                sql::col(&prior, "id")
            )));
        }
        let at = (self.now)();
        sql::exec(
            self.db.conn(),
            "INSERT INTO delegated_conversations(id,tree_id,scope_json,status,created_at,updated_at) VALUES(?,?,?,'proposed',?,?)",
            &[text(ident), text(tree_id), text(&encoded), text(&at), text(&at)],
        )?;
        self.get(ident)
    }

    /// conversations.py:92 `approve`.
    pub fn approve(&self, ident: &str, approval_id: &str, answer: &str) -> Result<()> {
        if sql::col(&self.get(ident)?, "status") != "proposed" || approval_id.is_empty() {
            return Err(Error::Value(
                "Only a pending conversation can receive its one owner approval".into(),
            ));
        }
        sql::exec(
            self.db.conn(),
            "UPDATE delegated_conversations SET status=?,approval_id=?,updated_at=? WHERE id=?",
            &[
                text(if answer == "Yes" {
                    "active"
                } else {
                    "declined"
                }),
                text(approval_id),
                text(&(self.now)()),
                text(ident),
            ],
        )?;
        self.signal(ident);
        Ok(())
    }

    /// conversations.py:99 `bind`.
    pub fn bind(&self, ident: &str, task_id: &str) -> Result<()> {
        let row = self.get(ident)?;
        if task_id.split('.').next().unwrap_or("") != sql::col(&row, "tree_id") {
            return Err(Error::Value(
                "Conversation worker belongs to another task tree".into(),
            ));
        }
        sql::exec(
            self.db.conn(),
            "INSERT OR REPLACE INTO conversation_workers VALUES(?,?)",
            &[text(task_id), text(ident)],
        )?;
        Ok(())
    }

    /// conversations.py:105 `guard`: why this call may not go ahead in a delegated conversation, or None.
    pub fn guard(&self, call: &GuardCall) -> Result<Option<String>> {
        if call.observation {
            return Ok(None);
        }
        let bound = sql::row(
            self.db.conn(),
            "SELECT conversation_id FROM conversation_workers WHERE task_id=?",
            &[text(&call.task_id)],
        )?;
        let Some(bound) = bound else {
            let root = call.task_id.split('.').next().unwrap_or("").to_string();
            for row in self.list(Some(&root))? {
                let status = sql::col(&row, "status");
                if call.task_id == format!("{root}.worker")
                    && (status == "handed_over" || status == "closed")
                {
                    return Ok(Some(
                        "The final conversation summary is read-only after hand-over; the owner controls further actions.".into(),
                    ));
                }
                let scope = &row["scope"];
                let site = call.site.as_deref().map(Value::from).unwrap_or(Value::Null);
                if matches!(
                    status.as_str(),
                    "paused" | "handed_over" | "closed" | "declined"
                ) && (scope.get("site") == Some(&site) || scope.get("app") == Some(&site))
                {
                    return Ok(Some(
                        "This conversation belongs to the owner now; further input is paused."
                            .into(),
                    ));
                }
            }
            return Ok(None);
        };
        let row = self.get(&sql::col(&bound, "conversation_id"))?;
        if sql::col(&row, "status") != "active" {
            return Ok(Some(
                "This delegated conversation is not active; only the owner can resume it.".into(),
            ));
        }
        let scope = &row["scope"];
        let field = |k: &str| scope.get(k).map(crate::py::str_of).unwrap_or_default();
        if call.identity != field("identity") {
            return Ok(Some(
                "This conversation was approved under a different identity.".into(),
            ));
        }
        let site = crate::py::casefold(call.site.as_deref().unwrap_or(""));
        if site != crate::py::casefold(&field("site")) && site != crate::py::casefold(&field("app"))
        {
            return Ok(Some(
                "This conversation worker may act only on its approved app/site; use a separate research step.".into(),
            ));
        }
        if let Some(account) = &call.account {
            if *account != field("account") {
                return Ok(Some(
                    "The action names an account outside this conversation permission.".into(),
                ));
            }
        }
        let allowed: Vec<String> = scope
            .get("recipients")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().map(crate::py::str_of).collect())
            .unwrap_or_default();
        if !call.recipients.is_empty() {
            let mut given = call.recipients.clone();
            given.sort();
            let mut held = allowed.clone();
            held.sort();
            if given != held
                && (SEND_CLASS.contains(&call.kind.as_str())
                    || call
                        .declared
                        .iter()
                        .any(|k| SEND_CLASS.contains(&k.as_str())))
            {
                return Ok(Some(
                    "The action names a recipient outside this conversation permission.".into(),
                ));
            }
        }
        if let Some(contact) = &call.contact {
            if !allowed.contains(contact) {
                return Ok(Some(
                    "The action names a contact outside this conversation permission.".into(),
                ));
            }
        }
        Ok(None)
    }

    /// conversations.py:139 `events`.
    pub fn events(&self, ident: &str, after: i64) -> Result<Vec<Value>> {
        self.get(ident)?;
        sql::rows(
            self.db.conn(),
            "SELECT * FROM conversation_events WHERE conversation_id=? AND seq>? ORDER BY seq",
            &[text(ident), rusqlite::types::Value::Integer(after)],
        )?
        .iter()
        .map(|r| {
            Ok(
                json!({"seq": r["seq"], "source_id": r["source_id"], "origin": r["origin"],
                      "data": sql::loads(&sql::col(r, "data_json"))?, "at": r["at"]}),
            )
        })
        .collect()
    }

    /// conversations.py:145 `observe`: a trail event that may be a conversation's message or the owner's typing.
    pub fn observe(&self, event: &Value) -> Result<()> {
        let Some(id) = event.get("id").filter(|v| ferb_py::value::truthy(v)) else {
            return Ok(());
        };
        let id = crate::py::str_of(id);
        if sql::row(
            self.db.conn(),
            "SELECT 1 FROM forgotten_refs WHERE ref=?",
            &[text(&format!("trail:{id}"))],
        )?
        .is_some()
        {
            return Ok(());
        }
        let empty = json!({});
        let data = event.get("data").unwrap_or(&empty);
        for row in self.list(None)? {
            let status = sql::col(&row, "status");
            if !matches!(status.as_str(), "active" | "waiting" | "paused") {
                continue;
            }
            let scope = &row["scope"];
            let field = |k: &str| scope.get(k).map(crate::py::str_of).unwrap_or_default();
            let bundle = data
                .get("bundle_id")
                .map(crate::py::str_of)
                .unwrap_or_default();
            if crate::py::casefold(&bundle) != crate::py::casefold(&field("app")) {
                continue;
            }
            let source = event.get("source").and_then(|v| v.as_str());
            if source == Some("typed_text")
                && event.get("actor").and_then(|v| v.as_str()) == Some("owner")
            {
                let window = scope.get("window_id").filter(|v| !v.is_null());
                let same = match window {
                    None => true,
                    Some(w) => {
                        data.get("window_id")
                            .map(crate::py::str_of)
                            .unwrap_or_else(|| "None".into())
                            == crate::py::str_of(w)
                    }
                };
                if same {
                    let ident = sql::col(&row, "id");
                    self.set_status(&ident, "paused")?;
                    (self.publish)(
                        "conversation.paused",
                        json!({"id": ident, "tree_id": row["tree_id"], "reason": "owner typed in the conversation app/window",
                               "source_ref": format!("trail:{id}")}),
                    );
                }
                continue;
            }
            if !matches!(source, Some("notification") | Some("conversation_message")) {
                continue;
            }
            if data.get("conversation") != scope.get("thread") {
                continue;
            }
            if let Some(account) = data.get("account").filter(|v| !v.is_null()) {
                if Some(account) != scope.get("account") {
                    continue;
                }
            }
            let clean = (self.redact)(data);
            let ident = sql::col(&row, "id");
            sql::exec(
                self.db.conn(),
                "INSERT OR IGNORE INTO conversation_events(conversation_id,source_id,origin,data_json,at) \
                 VALUES(?,?,'external:conversation',?,?)",
                &[text(&ident), text(&format!("trail:{id}")), text(&ferb_py::json::dumps(&clean)), text(&(self.now)())],
            )?;
            self.signal(&ident);
        }
        Ok(())
    }

    /// conversations.py:175 `pending`.
    pub fn pending(&self, ident: &str) -> Result<bool> {
        let row = self.get(ident)?;
        Ok(sql::row(
            self.db.conn(),
            "SELECT 1 FROM conversation_events WHERE conversation_id=? AND seq>?",
            &[text(ident), sql::to_sql(&row["cursor"])],
        )?
        .is_some())
    }

    /// conversations.py:180 `begin_turn`.
    pub fn begin_turn(&self, ident: &str) -> Result<i64> {
        let row = self.get(ident)?;
        let status = sql::col(&row, "status");
        if !matches!(status.as_str(), "active" | "waiting")
            || !row.get("approval_id").is_some_and(ferb_py::value::truthy)
        {
            return Err(Error::Value(
                "The owner has not authorized an active conversation".into(),
            ));
        }
        let cursor = sql::row(
            self.db.conn(),
            "SELECT COALESCE(MAX(seq),0) AS m FROM conversation_events WHERE conversation_id=?",
            &[text(ident)],
        )?
        .and_then(|r| r.get("m").and_then(|v| v.as_i64()))
        .unwrap_or(0);
        let number = row.get("turn").and_then(|v| v.as_i64()).unwrap_or(0) + 1;
        let c = self.db.conn();
        c.execute_batch("BEGIN IMMEDIATE")?;
        let run = || -> Result<()> {
            sql::exec(
                c,
                "UPDATE delegated_conversations SET turn=?,turn_cursor=?,status='active',updated_at=? WHERE id=?",
                &[rusqlite::types::Value::Integer(number), rusqlite::types::Value::Integer(cursor), text(&(self.now)()), text(ident)],
            )?;
            sql::exec(
                c,
                "INSERT INTO conversation_turns VALUES(?,?,NULL,'running')",
                &[text(ident), rusqlite::types::Value::Integer(number)],
            )?;
            Ok(())
        };
        match run() {
            Ok(()) => c.execute_batch("COMMIT")?,
            Err(e) => {
                let _ = c.execute_batch("ROLLBACK");
                return Err(e);
            }
        }
        Ok(number)
    }

    /// conversations.py:198 `finish_turn`.
    pub fn finish_turn(&self, ident: &str, number: i64, output: &Value) -> Result<()> {
        let row = self.get(ident)?;
        let previous = sql::row(
            self.db.conn(),
            "SELECT status FROM conversation_turns WHERE conversation_id=? AND number=?",
            &[text(ident), rusqlite::types::Value::Integer(number)],
        )?;
        let mut output = output.clone();
        if previous.is_some_and(|p| sql::col(&p, "status") == "evidence_forgotten") {
            output = json!({"status": "blocked", "summary": "Source evidence was forgotten; inspect afresh after owner resume.",
                            "conversation_action": "blocked", "evidence": []});
        }
        let action = output
            .get("conversation_action")
            .and_then(|v| v.as_str())
            .map(String::from);
        if matches!(action.as_deref(), Some("handover") | Some("finished"))
            && !output.get("evidence").is_some_and(ferb_py::value::truthy)
        {
            return Err(Error::Value(
                "A hand-over or finished conversation needs observed evidence".into(),
            ));
        }
        let output = (self.redact)(&output);
        let current = sql::col(&row, "status");
        let mut status = if matches!(current.as_str(), "paused" | "handed_over" | "closed") {
            current.clone()
        } else {
            match action.as_deref() {
                Some("wait") => "waiting",
                Some("handover") => "handed_over",
                Some("finished") => "closed",
                _ => "blocked",
            }
            .to_string()
        };
        if output.get("status").and_then(|v| v.as_str()) != Some("done")
            && !matches!(status.as_str(), "paused" | "handed_over" | "closed")
        {
            status = "blocked".into();
        }
        let handover = if action.as_deref() == Some("handover") {
            Some(ferb_py::json::dumps(&output))
        } else {
            None
        };
        let turn_status = output
            .get("status")
            .cloned()
            .unwrap_or(Value::String("blocked".into()));
        let c = self.db.conn();
        c.execute_batch("BEGIN IMMEDIATE")?;
        let run = || -> Result<()> {
            sql::exec(
                c,
                "UPDATE conversation_turns SET output_json=?,status=? WHERE conversation_id=? AND number=?",
                &[text(&ferb_py::json::dumps(&output)), sql::to_sql(&turn_status), text(ident), rusqlite::types::Value::Integer(number)],
            )?;
            sql::exec(
                c,
                "UPDATE delegated_conversations SET status=?,cursor=turn_cursor,handover_json=COALESCE(?,handover_json),updated_at=? WHERE id=?",
                &[text(&status), sql::opt(handover.as_deref()), text(&(self.now)()), text(ident)],
            )?;
            Ok(())
        };
        match run() {
            Ok(()) => c.execute_batch("COMMIT")?,
            Err(e) => {
                let _ = c.execute_batch("ROLLBACK");
                return Err(e);
            }
        }
        self.signal(ident);
        Ok(())
    }

    /// conversations.py:227 `owner_change`.
    pub fn owner_change(&self, ident: &str, action: &str, owner_words: &str) -> Result<()> {
        let row = self.get(ident)?;
        let status = match action {
            "pause" => "paused",
            "resume" => "active",
            "takeover" => "handed_over",
            "close" => "closed",
            _ => "",
        };
        if status.is_empty() || crate::py::strip(owner_words).is_empty() {
            return Err(Error::Value(
                "Conversation control needs an owner instruction and supported action".into(),
            ));
        }
        if action == "resume"
            && (!row.get("approval_id").is_some_and(ferb_py::value::truthy)
                || sql::col(&row, "status") == "declined")
        {
            return Err(Error::Value(
                "A declined or unapproved scope needs a new conversation approval".into(),
            ));
        }
        let words = (self.redact)(&Value::String(owner_words.into()));
        sql::exec(
            self.db.conn(),
            "INSERT INTO conversation_owner_changes(conversation_id,action,owner_words,at) VALUES(?,?,?,?)",
            &[text(ident), text(action), sql::to_sql(&words), text(&(self.now)())],
        )?;
        self.set_status(ident, status)
    }

    /// conversations.py:237 `wait`: whether a turn may run now; false when it ended or was stopped.
    pub async fn wait(&self, ident: &str, stopped: &dyn Fn() -> bool) -> Result<bool> {
        let wake = self.waker(ident);
        while !stopped() {
            wake.clear();
            let row = self.get(ident)?;
            let status = sql::col(&row, "status");
            if matches!(
                status.as_str(),
                "closed" | "handed_over" | "declined" | "blocked"
            ) {
                return Ok(false);
            }
            if status == "active" || (status == "waiting" && self.pending(ident)?) {
                return Ok(true);
            }
            wake.wait().await;
        }
        Ok(false)
    }

    pub fn wake_all(&self) {
        for w in self.wake.lock().unwrap_or_else(|e| e.into_inner()).values() {
            w.set();
        }
    }

    fn set_status(&self, ident: &str, status: &str) -> Result<()> {
        sql::exec(
            self.db.conn(),
            "UPDATE delegated_conversations SET status=?,updated_at=? WHERE id=?",
            &[text(status), text(&(self.now)()), text(ident)],
        )?;
        self.signal(ident);
        Ok(())
    }

    fn signal(&self, ident: &str) {
        self.waker(ident).set();
    }

    /// conversations.py:261 `forget_sources`: an explicit owner forget only.
    pub fn forget_sources(&self, refs: &[String]) -> Result<Value> {
        let c = self.db.conn();
        let mut affected: Vec<String> = Vec::new();
        let mut events = 0usize;
        c.execute_batch("BEGIN IMMEDIATE")?;
        let run = |affected: &mut Vec<String>, events: &mut usize| -> Result<usize> {
            let now = (self.now)();
            for r in refs {
                sql::exec(
                    c,
                    "INSERT OR IGNORE INTO forgotten_refs VALUES(?, ?)",
                    &[text(r), text(&now)],
                )?;
                for row in sql::rows(
                    c,
                    "SELECT conversation_id FROM conversation_events WHERE source_id=?",
                    &[text(r)],
                )? {
                    let id = sql::col(&row, "conversation_id");
                    if !affected.contains(&id) {
                        affected.push(id);
                    }
                }
                *events += sql::exec(
                    c,
                    "DELETE FROM conversation_events WHERE source_id=?",
                    &[text(r)],
                )?;
            }
            let notices = crate::attention::expire_derived(
                self.db,
                refs,
                "Source evidence was forgotten.",
                &(self.now)(),
            )?;
            for ident in affected.iter() {
                sql::exec(c, "UPDATE conversation_turns SET output_json=NULL,status='evidence_forgotten' WHERE conversation_id=?", &[text(ident)])?;
                sql::exec(
                    c,
                    "UPDATE delegated_conversations SET handover_json=NULL,status='paused',updated_at=? WHERE id=?",
                    &[text(&(self.now)()), text(ident)],
                )?;
            }
            Ok(notices)
        };
        let notices = match run(&mut affected, &mut events) {
            Ok(n) => {
                c.execute_batch("COMMIT")?;
                n
            }
            Err(e) => {
                let _ = c.execute_batch("ROLLBACK");
                return Err(e);
            }
        };
        for ident in &affected {
            self.signal(ident);
        }
        Ok(
            json!({"conversation_events": events, "conversation_evidence": affected.len(), "attention_notices": notices}),
        )
    }

    /// conversations.py:287 `digest`.
    pub fn digest(&self, ident: &str) -> Result<Value> {
        let row = self.get(ident)?;
        let mut turns = Vec::new();
        for mut t in sql::rows(
            self.db.conn(),
            "SELECT number,status,output_json FROM conversation_turns WHERE conversation_id=? ORDER BY number",
            &[text(ident)],
        )? {
            let raw = shift_take(&mut t, "output_json");
            let text_ = match raw.as_str() {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => "null".into(),
            };
            t.insert("output".into(), sql::loads(&text_)?);
            turns.push(Value::Object(t));
        }
        let sends = sql::rows(
            self.db.conn(),
            "SELECT l.id,l.task_id,l.capability,l.target_json,l.state,l.evidence_json FROM action_ledger l JOIN \
             conversation_workers w ON w.task_id=l.task_id WHERE w.conversation_id=? ORDER BY l.rowid",
            &[text(ident)],
        )?;
        Ok(json!({"conversation": Value::Object(row), "turns": turns,
                  "actions": sends.into_iter().map(Value::Object).collect::<Vec<_>>(),
                  "warning": "Ledger done means tool returned; use turn evidence for delivery. Unknown actions must not be repeated automatically."}))
    }
}

/// What a conversation turn's runner needs from the supervisor.
pub trait ConversationTurns {
    /// Ask the owner to approve the scope: `{"approval_id", "answer"}` (no id: still waiting).
    fn approve<'a>(
        &'a self,
        text: &'a str,
        options: Vec<String>,
        card: Value,
    ) -> BoxFuture<'a, Value>;
    /// One checked turn: (conversation id, turn number, new events, digest) -> its output.
    fn turn<'a>(
        &'a self,
        ident: &'a str,
        number: i64,
        events: Vec<Value>,
        digest: Value,
    ) -> BoxFuture<'a, Value>;
    fn emit<'a>(&'a self, event: Value) -> BoxFuture<'a, ()>;
    fn stopped(&self) -> bool;
}

/// conversations.py:299 `ConversationRunner.run`: one checked turn at a time; sleeps without a model session.
pub async fn run_conversation(
    store: &ConversationStore<'_>,
    host: &dyn ConversationTurns,
    ident: &str,
    tree_id: &str,
    scope: &ConversationScope,
) -> Result<Value> {
    let row = store.open(ident, tree_id, scope)?;
    if sql::col(&row, "status") == "proposed" {
        let answer = host
            .approve(
                &approval_text(scope),
                vec!["Yes".into(), "No".into()],
                card(scope),
            )
            .await;
        let approval = answer
            .get("approval_id")
            .filter(|v| ferb_py::value::truthy(v))
            .map(crate::py::str_of);
        let Some(approval) = approval else {
            return Ok(
                json!({"status": "blocked", "summary": "Conversation scope is waiting for your answer.", "conversation_id": ident}),
            );
        };
        let said = answer
            .get("answer")
            .map(crate::py::str_of)
            .unwrap_or_default();
        store.approve(ident, &approval, &said)?;
    }
    let stopped = || host.stopped();
    while !host.stopped() {
        let row = store.get(ident)?;
        let status = sql::col(&row, "status");
        if status == "closed" || status == "handed_over" {
            let handover = row
                .get("handover")
                .filter(|v| ferb_py::value::truthy(v))
                .cloned()
                .unwrap_or(json!({}));
            return Ok(json!({
                "status": "done",
                "summary": handover.get("summary").cloned().unwrap_or(json!("Conversation handed back to you.")),
                "evidence": handover.get("evidence").cloned().unwrap_or(json!([])),
                "conversation_id": ident,
                "conversation_status": status,
                "digest": store.digest(ident)?,
            }));
        }
        if status == "declined" || status == "blocked" {
            return Ok(
                json!({"status": "blocked", "summary": "The conversation needs your attention.", "conversation_id": ident}),
            );
        }
        let quiet = &stopped as &dyn Fn() -> bool;
        if !store.wait(ident, quiet).await? {
            continue;
        }
        let number = store.begin_turn(ident)?;
        let events = store.events(
            ident,
            row.get("cursor").and_then(|v| v.as_i64()).unwrap_or(0),
        )?;
        let result = host.turn(ident, number, events, store.digest(ident)?).await;
        store.finish_turn(ident, number, &result)?;
        let current = sql::col(&store.get(ident)?, "status");
        if current == "handed_over" {
            let summary = result
                .get("summary")
                .map(crate::py::str_of)
                .unwrap_or_default();
            host.emit(json!({"type": "reply", "task_id": tree_id, "text": format!("Conversation handed over: {summary}"),
                             "evidence": result.get("evidence").cloned().unwrap_or(json!([]))}))
                .await;
        } else if current == "waiting" {
            host.emit(json!({"type": "progress", "task_id": tree_id, "text": format!("Waiting for a reply in {}", scope.thread),
                             "conversation_id": ident}))
                .await;
        }
    }
    Ok(json!({"status": "cancelled", "summary": "Stopped by you.", "conversation_id": ident}))
}
