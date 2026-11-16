//! A fake Mac for the core's tests: one conversation's entries as the engine's store keeps them (the newest copy of
//! each, a seq counter bumped at every commit), answering `chat.entries.page`, `changes`, `get`, `watch` and
//! `chat.send` by ENG-14's rules (ferb-supervisor flow/entries_rpc.rs), with the link's public-relay refusal (PHN-444)
//! and failures to inject. Built from the chat-entries fixtures.
#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use ferb_mobile::entry::ChatEntry;
use ferb_mobile::transport::{LinkFailure, Transport, WatchHandle, WatchSink};
use serde_json::{json, Value};

/// What the next `chat.send` does.
#[derive(Debug, Clone)]
pub enum SendOutcome {
    Accept,
    /// The Mac accepts the message, then the link drops before the ack reaches the phone (an unknown outcome).
    AcceptThenDrop,
    /// The Mac refuses it (its words).
    Refuse(String),
    /// Any other link failure, by code.
    Fail(String),
}

pub struct MacState {
    pub conversation: String,
    pub epoch: String,
    pub head: i64,
    pub entries: HashMap<String, ChatEntry>,
    pub online: bool,
    /// The public relay: older pages refused before the Mac is asked.
    pub public_relay: bool,
    /// Cut a body's words longer than this in pages, changes and watch frames (`body_cut`).
    pub cut_over: Option<usize>,
    pub calls: Vec<(String, Value, Option<String>)>,
    pub sends: Vec<Value>,
    pub accepted: HashMap<String, (String, String, i64)>,
    pub script: VecDeque<SendOutcome>,
    pub sink: Option<Arc<dyn WatchSink>>,
    pub watches: usize,
    pub next_request: u64,
    pub next_pos: i64,
}

#[derive(Clone)]
pub struct FakeMac(pub Arc<Mutex<MacState>>);

fn closed_failure(code: &str) -> LinkFailure {
    LinkFailure::new(code, format!("({code})"))
}

impl FakeMac {
    pub fn new(conversation: &str) -> FakeMac {
        FakeMac(Arc::new(Mutex::new(MacState {
            conversation: conversation.into(),
            epoch: "E1".into(),
            head: 0,
            entries: HashMap::new(),
            online: true,
            public_relay: false,
            cut_over: None,
            calls: Vec::new(),
            sends: Vec::new(),
            accepted: HashMap::new(),
            script: VecDeque::new(),
            sink: None,
            watches: 0,
            next_request: 1,
            next_pos: 1 << 33,
        })))
    }

    pub fn with(conversation: &str, entries: &[ChatEntry]) -> FakeMac {
        let mac = FakeMac::new(conversation);
        for e in entries {
            mac.commit(e.clone());
        }
        mac
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, MacState> {
        self.0.lock().unwrap()
    }

    /// One committed change: the entry at a new seq, sent to the watch when one runs.
    pub fn commit(&self, mut e: ChatEntry) {
        let sink = {
            let mut st = self.lock();
            st.head += 1;
            e.seq = st.head;
            e.conversation_id = st.conversation.clone();
            let frame = json!({"type": "change", "entry": st.wire(&e)});
            st.entries.insert(e.id.clone(), e);
            st.sink.clone().map(|s| (s, frame))
        };
        if let Some((s, frame)) = sink {
            s.frame(frame.to_string());
        }
    }

    /// A live frame on the watch.
    pub fn live(&self, request_id: &str, frame: Value) {
        let sink = self.lock().sink.clone();
        if let Some(s) = sink {
            s.frame(json!({"type": "live", "request_id": request_id, "frame": frame}).to_string());
        }
    }

    /// The watch falls behind: `behind`, then the end.
    pub fn behind(&self) {
        let sink = self.lock().sink.take();
        if let Some(s) = sink {
            let head = self.lock().head;
            s.frame(json!({"type": "behind", "head": head}).to_string());
            s.ended(None);
        }
    }

    /// The newest copy of every entry (what a phone's fold should equal).
    pub fn newest(&self) -> Vec<ChatEntry> {
        let mut v: Vec<ChatEntry> = self.lock().entries.values().cloned().collect();
        v.sort_by_key(|e| e.pos);
        v
    }

    pub fn calls_of(&self, method: &str) -> Vec<Value> {
        self.lock()
            .calls
            .iter()
            .filter(|c| c.0 == method)
            .map(|c| c.1.clone())
            .collect()
    }

    /// The messages the Mac holds that this submission made.
    pub fn messages_of(&self, submission: &str) -> usize {
        self.lock()
            .entries
            .values()
            .filter(|e| e.kind == "message" && e.submission_id.as_deref() == Some(submission))
            .count()
    }
}

impl MacState {
    fn wire(&self, e: &ChatEntry) -> Value {
        let mut v = e.to_json();
        if let (Some(n), Some(text)) = (self.cut_over, e.body.get("text").and_then(Value::as_str)) {
            let chars: Vec<char> = text.chars().collect();
            if chars.len() > n {
                v["body"]["text"] = json!(chars[..n].iter().collect::<String>());
                v["body_cut"] = json!({"chars": n, "full_chars": chars.len()});
            }
        }
        v
    }

    fn sorted_by_pos(&self) -> Vec<&ChatEntry> {
        let mut v: Vec<&ChatEntry> = self.entries.values().collect();
        v.sort_by_key(|e| e.pos);
        v
    }

    fn page(&self, p: &Value) -> Result<Value, LinkFailure> {
        let before = p["before_pos"].as_i64();
        if before.is_some() && self.public_relay {
            return Err(LinkFailure::new(
                "needs_direct_or_own_relay",
                "Older messages need a direct connection or your own relay.",
            ));
        }
        let limit = p["limit"].as_u64().unwrap_or(50) as usize;
        let all: Vec<&ChatEntry> = self
            .sorted_by_pos()
            .into_iter()
            .filter(|e| before.is_none_or(|b| e.pos < b))
            .collect();
        let from = all.len().saturating_sub(limit);
        let page: Vec<Value> = all[from..].iter().map(|e| self.wire(e)).collect();
        let more = from > 0;
        Ok(
            json!({"conversation": self.conversation, "epoch": self.epoch, "head": self.head, "entries": page,
                  "more": more, "before_pos": if more { json!(all[from].pos) } else { Value::Null }}),
        )
    }

    fn cursor_reply(&self, epoch: &str, after: i64) -> Option<Value> {
        if epoch != self.epoch {
            return Some(
                json!({"conversation": self.conversation, "reset_required": true, "epoch": self.epoch,
                               "head": self.head}),
            );
        }
        if after > self.head {
            return Some(
                json!({"conversation": self.conversation, "cursor_ahead": true, "epoch": self.epoch,
                               "head": self.head}),
            );
        }
        None
    }

    fn after(&self, after: i64) -> Vec<&ChatEntry> {
        let mut v: Vec<&ChatEntry> = self.entries.values().filter(|e| e.seq > after).collect();
        v.sort_by_key(|e| e.seq);
        v
    }

    fn changes(&self, p: &Value) -> Value {
        let after = p["after_seq"].as_i64().unwrap_or(0);
        if let Some(r) = self.cursor_reply(p["epoch"].as_str().unwrap_or_default(), after) {
            return r;
        }
        let limit = p["limit"].as_u64().unwrap_or(50) as usize;
        let all = self.after(after);
        let page: Vec<&ChatEntry> = all.iter().take(limit).copied().collect();
        let next = page.last().map(|e| e.seq).unwrap_or(after);
        json!({"conversation": self.conversation, "epoch": self.epoch, "head": self.head,
               "entries": page.iter().map(|e| self.wire(e)).collect::<Vec<_>>(),
               "more": all.len() > page.len(), "next_seq": next})
    }

    fn get(&self, p: &Value) -> Result<Value, LinkFailure> {
        let id = p["id"].as_str().unwrap_or_default();
        let e = self
            .entries
            .get(id)
            .ok_or_else(|| LinkFailure::new("ferb_error", "There's no chat entry with that id."))?;
        let mut v = e.to_json();
        let offset = p["offset"].as_u64().unwrap_or(0) as usize;
        let piece = p["max_chars"].as_u64().unwrap_or(16_000) as usize;
        if let Some(text) = e.body.get("text").and_then(Value::as_str) {
            let chars: Vec<char> = text.chars().collect();
            if offset > 0 || chars.len() > piece {
                let from = offset.min(chars.len());
                let to = (from + piece).min(chars.len());
                v["body"]["text"] = json!(chars[from..to].iter().collect::<String>());
                v["text_piece"] = json!({"offset": from, "chars": to - from, "full_chars": chars.len(),
                                         "more": to < chars.len()});
            }
        }
        Ok(json!({"entry": v}))
    }
}

struct FakeWatch(FakeMac);

impl WatchHandle for FakeWatch {
    fn close(&self) {
        self.0.lock().sink = None;
    }
}

impl Transport for FakeMac {
    fn call(
        &self,
        method: &str,
        params_json: &str,
        proof_json: Option<&str>,
    ) -> Result<String, LinkFailure> {
        let p: Value = serde_json::from_str(params_json).unwrap();
        let mut st = self.lock();
        st.calls.push((
            method.to_string(),
            p.clone(),
            proof_json.map(str::to_string),
        ));
        if !st.online {
            return Err(closed_failure("mac_unreachable"));
        }
        let r = match method {
            "chat.entries.page" => st.page(&p)?,
            "chat.entries.changes" => st.changes(&p),
            "chat.entries.get" => st.get(&p)?,
            "chat.entries.mark" => json!({"marks": {}}),
            "link.challenge" => json!({"challenge": "C-1"}),
            "task.stop" | "chat.approve" | "chat.answer" | "chat.correct" => json!({"ok": true}),
            other => {
                return Err(LinkFailure::new(
                    "method_not_allowed",
                    format!("{other} is not allowed"),
                ));
            }
        };
        Ok(r.to_string())
    }

    fn send(&self, params_json: &str) -> Result<String, LinkFailure> {
        let p: Value = serde_json::from_str(params_json).unwrap();
        let (outcome, made) = {
            let mut st = self.lock();
            st.sends.push(p.clone());
            if !st.online {
                return Err(closed_failure("mac_unreachable"));
            }
            let sub = p["submission_id"].as_str().unwrap_or_default().to_string();
            if let Some((request, entry, seq)) = st.accepted.get(&sub) {
                let ack = json!({"type": "ack", "request_id": request, "entry_id": entry, "seq": seq,
                                 "epoch": st.epoch, "submission_id": sub, "repeat": true});
                return Ok(ack.to_string());
            }
            let outcome = st.script.pop_front().unwrap_or(SendOutcome::Accept);
            let made = match &outcome {
                SendOutcome::Accept | SendOutcome::AcceptThenDrop => {
                    let n = st.next_request;
                    st.next_request += 1;
                    let pos = st.next_pos;
                    st.next_pos += 1;
                    let request = format!("r-phone-{n}");
                    let entry = format!("pm-{n}");
                    let e = ChatEntry::read(&json!({
                        "id": entry, "conversation_id": st.conversation, "pos": pos, "seq": 0, "rev": 1,
                        "kind": "message", "state": "working", "open": true, "request_id": request,
                        "client_id": p["client_id"], "submission_id": sub,
                        "created_at": "2026-01-02T10:00:00+00:00", "updated_at": "2026-01-02T10:00:00+00:00",
                        "day": "2026-01-02", "body": {"text": p["text"], "attachments": [], "hidden": []},
                        "deleted": false}))
                    .unwrap();
                    Some((sub, request, entry, e))
                }
                _ => None,
            };
            (outcome, made)
        };
        if let Some((sub, request, entry, e)) = made {
            self.commit(e);
            let seq = self.lock().head;
            self.lock()
                .accepted
                .insert(sub.clone(), (request.clone(), entry.clone(), seq));
            if matches!(outcome, SendOutcome::AcceptThenDrop) {
                return Err(closed_failure("mac_unreachable"));
            }
            let epoch = self.lock().epoch.clone();
            return Ok(
                json!({"type": "ack", "request_id": request, "entry_id": entry, "seq": seq,
                             "epoch": epoch, "submission_id": sub})
                .to_string(),
            );
        }
        match outcome {
            SendOutcome::Refuse(m) => Err(LinkFailure::new("ferb_error", m)),
            SendOutcome::Fail(code) => Err(closed_failure(&code)),
            _ => unreachable!(),
        }
    }

    fn watch(
        &self,
        params_json: &str,
        sink: Arc<dyn WatchSink>,
    ) -> Result<Box<dyn WatchHandle>, LinkFailure> {
        let p: Value = serde_json::from_str(params_json).unwrap();
        let frames = {
            let mut st = self.lock();
            st.calls
                .push(("chat.entries.watch".into(), p.clone(), None));
            if !st.online {
                return Err(closed_failure("mac_unreachable"));
            }
            st.watches += 1;
            let mut frames = Vec::new();
            let mut sent = st.head;
            if let (Some(epoch), Some(after)) = (p["epoch"].as_str(), p["after_seq"].as_i64()) {
                if let Some(mut r) = st.cursor_reply(epoch, after) {
                    r["type"] = json!(if r.get("reset_required").is_some() {
                        "reset_required"
                    } else {
                        "cursor_ahead"
                    });
                    drop(st);
                    sink.frame(r.to_string());
                    sink.ended(None);
                    return Ok(Box::new(FakeWatch(self.clone())));
                }
                for e in st.after(after) {
                    frames.push(json!({"type": "change", "catchup": true, "entry": st.wire(e)}));
                    sent = sent.max(e.seq);
                }
            }
            frames.push(
                json!({"type": "caught_up", "conversation": st.conversation, "epoch": st.epoch,
                               "head": sent}),
            );
            st.sink = Some(sink.clone());
            frames
        };
        for f in frames {
            sink.frame(f.to_string());
        }
        Ok(Box::new(FakeWatch(self.clone())))
    }
}
