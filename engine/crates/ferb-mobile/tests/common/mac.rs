//! A fake Mac for the core's tests: one conversation's entries as the engine's store keeps them (the newest copy of
//! each, a seq counter bumped at every commit), answering `chat.entries.page`, `changes`, `get`, `watch` and
//! `chat.send` by ENG-14's rules (ferb-supervisor flow/entries_rpc.rs), with the link's public-relay refusal (PHN-444)
//! and failures to inject. Built from the chat-entries fixtures.
#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use ferb_mobile::entry::ChatEntry;
use ferb_mobile::transport::{LinkFailure, TransferSink, Transport, WatchHandle, WatchSink};
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
    /// The conversation the running watch follows (the chat's, or the temporary chat's `temporary:<id>`).
    pub watched: String,
    /// Frames for the watch made under the lock, handed to it once the lock is let go.
    pub outgoing: Vec<String>,
    pub watches: usize,
    pub next_request: u64,
    pub next_pos: i64,
    /// `chat.status` as the gate shapes it.
    pub status: Value,
    /// Folders by path (`link.files` replies) and files by path (their bytes) for `link.get`.
    pub folders: HashMap<String, Value>,
    pub blobs: HashMap<String, Vec<u8>>,
    /// Paths that need their own fresh proof (key places): refused `fresh_unlock_required` without one.
    pub key_places: Vec<String>,
    /// A fetch stops after this many bytes (a pause) with `cancelled`.
    pub pause_after: Option<usize>,
    /// What arrived through `link.put` (name → bytes) and the next upload ids.
    pub uploads: Vec<(String, Vec<u8>, Value)>,
    /// The preview's answers, in order (`meta` and the frame's bytes).
    pub frames: VecDeque<(Value, Vec<u8>)>,
    pub settings: serde_json::Map<String, Value>,
    pub mac_name: Option<String>,
    /// `phone.device_report` refused as locked (an app locked on a connection that hasn't proved its device key).
    pub device_locked: bool,
    /// PHN-846: this phone's app lock is off on the Mac (`phone.lock_off`, signed; `phone.lock_on`).
    pub lock_off: bool,
    /// Transfers by kind, with their params (get / put).
    pub transfers: Vec<(String, Value)>,
    /// Folder listings and searches answer pages of this many (PHN-589), each with its `from`, `next` and `cut`.
    pub page_size: Option<usize>,
    /// `phone.search`'s matches when set (paged by `page_size`); else one fixed answer.
    pub found: Vec<Value>,
    /// The phone's unlock window is closed: what needs it (an "until I'm back" Yes) is refused `locked`.
    pub window_closed: bool,
    /// `screen_use.status` (GATE-615).
    pub screen_use: Value,
    /// Insomnia mode (APP-980..APP-985): what `insomnia.*` answer and change.
    pub insomnia: Value,
    /// The camera's answers in insomnia mode, in order (`meta` and the frame's bytes).
    pub camera: VecDeque<(Value, Vec<u8>)>,
    /// The temporary chat, held in the daemon's memory only (none while off): never in `entries`.
    pub temporary: Option<TempChat>,
    /// Each temporary chat's id is "T<n>".
    pub temp_count: u64,
    /// The temporary chat's rev: one more on every change (start, each entry, end), across chats.
    pub temp_rev: i64,
}

/// The fake daemon's temporary chat: its id, start and entries (ENG-8 JSON, in pos order).
#[derive(Debug, Clone)]
pub struct TempChat {
    pub id: String,
    pub started_at: String,
    pub entries: Vec<Value>,
}

impl MacState {
    /// `chat.temporary.get`'s answer.
    pub fn temporary_reply(&self) -> Value {
        match &self.temporary {
            None => json!({"on": false}),
            Some(t) => {
                json!({"on": true, "id": t.id, "conversation": format!("temporary:{}", t.id),
                              "started_at": t.started_at, "rev": self.temp_rev, "entries": t.entries})
            }
        }
    }

    pub fn start_temporary(&mut self) {
        if self.temporary.is_none() {
            self.temp_count += 1;
            self.temp_rev += 1;
            self.temporary = Some(TempChat {
                id: format!("T{}", self.temp_count),
                started_at: "2026-01-02T10:05:00+00:00".into(),
                entries: Vec::new(),
            });
        }
    }

    /// One entry of the temporary chat (a message or FERB's answer), as the daemon's memory keeps it.
    pub fn temporary_entry(&mut self, kind: &str, text: &str, client_id: &Value) -> Value {
        let t = self.temporary.as_mut().expect("on");
        let n = t.entries.len() as i64 + 1;
        let answers = if kind == "answer" {
            t.entries
                .last()
                .map(|m| m["id"].clone())
                .unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        let e = json!({
            "id": format!("{}-e{n}", t.id), "conversation_id": format!("temporary:{}", t.id), "pos": n, "seq": n,
            "rev": 1, "kind": kind, "state": "done", "open": false, "request_id": format!("{}-r1", t.id),
            "client_id": client_id, "answers": answers, "created_at": "2026-01-02T10:06:00+00:00",
            "updated_at": "2026-01-02T10:06:00+00:00", "day": "2026-01-02",
            "body": {"text": text, "attachments": [], "hidden": []}, "deleted": false});
        t.entries.push(e.clone());
        self.temp_rev += 1;
        if self.watched == format!("temporary:{}", t.id) && self.sink.is_some() {
            self.outgoing
                .push(json!({"type": "change", "entry": e}).to_string());
        }
        e
    }

    /// The temporary chat ends (its icon, on any client): its watch is told `ended`.
    pub fn end_temporary(&mut self) {
        if let Some(t) = self.temporary.take() {
            self.temp_rev += 1;
            let c = format!("temporary:{}", t.id);
            if self.watched == c && self.sink.is_some() {
                self.outgoing
                    .push(json!({"type": "ended", "conversation": c}).to_string());
            }
        }
    }
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
            watched: String::new(),
            outgoing: Vec::new(),
            watches: 0,
            next_request: 1,
            next_pos: 1 << 33,
            status: json!({"approvals": [], "running": [], "results": [], "stopped": false}),
            folders: HashMap::new(),
            blobs: HashMap::new(),
            key_places: Vec::new(),
            pause_after: None,
            uploads: Vec::new(),
            frames: VecDeque::new(),
            settings: serde_json::Map::new(),
            mac_name: None,
            device_locked: false,
            lock_off: false,
            transfers: Vec::new(),
            page_size: None,
            found: Vec::new(),
            window_closed: false,
            screen_use: json!({"active": false, "can_offer": true}),
            insomnia: json!({"on": false, "power": {"battery": 64, "on_battery": true}, "task_awake": false,
                             "last_end": null, "line": "", "closed_lid": "absent",
                             "lid_line": "On battery with the lid closed, your Mac sleeps until you open it or plug it in.",
                             "helper": {"state": "requires_approval", "error": null,
                                        "problem": "FERB's helper is waiting for your OK in System Settings."}}),
            camera: VecDeque::new(),
            temporary: None,
            temp_count: 0,
            temp_rev: 0,
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

    /// Hands the watch what was made for it under the lock (never with the lock held: the phone's sink takes its own).
    pub fn deliver(&self) {
        let (sink, frames, ended) = {
            let mut st = self.lock();
            let frames = std::mem::take(&mut st.outgoing);
            let ended = frames.iter().any(|f| f.contains("\"type\":\"ended\""));
            let sink = if ended {
                st.sink.take()
            } else {
                st.sink.clone()
            };
            (sink, frames, ended)
        };
        if let Some(s) = sink {
            for f in frames {
                s.frame(f);
            }
            if ended {
                s.ended(None);
            }
        }
    }

    /// One turn of the temporary chat made on the Mac (by another client, or FERB's answer): its watch gets it.
    pub fn temporary_said(&self, kind: &str, text: &str) -> Value {
        let e = self.lock().temporary_entry(kind, text, &Value::Null);
        self.deliver();
        e
    }

    /// His message in the temporary chat, still working (FERB hasn't answered): its watch gets it.
    pub fn temporary_working(&self, text: &str) -> Value {
        let e = {
            let mut st = self.lock();
            let mut e = st.temporary_entry("message", text, &Value::Null);
            e["state"] = json!("working");
            e["open"] = json!(true);
            if let Some(t) = st.temporary.as_mut() {
                *t.entries.last_mut().unwrap() = e.clone();
            }
            if let Some(last) = st.outgoing.last_mut() {
                *last = json!({"type": "change", "entry": e}).to_string();
            }
            e
        };
        self.deliver();
        e
    }

    /// Another client ends the temporary chat.
    pub fn temporary_ended(&self) {
        self.lock().end_temporary();
        self.deliver();
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
            let watching = st.watched == st.conversation;
            st.sink.clone().filter(|_| watching).map(|s| (s, frame))
        };
        if let Some((s, frame)) = sink {
            s.frame(frame.to_string());
        }
    }

    /// The conversation the watch follows now (empty: none).
    pub fn watched(&self) -> String {
        let st = self.lock();
        if st.sink.is_some() {
            st.watched.clone()
        } else {
            String::new()
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
    /// One page of `all` after the marker (`<what>|<key>|<n>`, as the Mac's own markers: valid only for its folder
    /// and query), with `from`, `next` and `cut` (PHN-589).
    fn paged(
        &self,
        what: &str,
        key: &str,
        all: &[Value],
        after: &Value,
    ) -> Result<(Vec<Value>, Value), LinkFailure> {
        let from = match after.as_str() {
            None => 0,
            Some(m) => {
                let mut parts = m.rsplitn(2, '|');
                let n = parts.next().and_then(|n| n.parse::<usize>().ok());
                match (n, parts.next()) {
                    (Some(n), Some(k)) if k == format!("{what}|{key}") => n,
                    _ => {
                        return Err(LinkFailure::new(
                            "bad_request",
                            "That page marker is not this folder's.",
                        ))
                    }
                }
            }
        };
        let size = self.page_size.unwrap_or(usize::MAX);
        let page: Vec<Value> = all.iter().skip(from).take(size).cloned().collect();
        let more = from + page.len() < all.len();
        let next = if more {
            json!(format!("{what}|{key}|{}", from + page.len()))
        } else {
            Value::Null
        };
        Ok((page, json!({"from": from, "cut": more, "next": next})))
    }

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

impl FakeMac {
    /// The bulk methods' public-relay refusal (PHN-444), as the link says it.
    fn refuse_bulk(st: &MacState) -> Result<(), LinkFailure> {
        if st.public_relay {
            return Err(LinkFailure::new(
                "needs_direct_or_own_relay",
                "This needs a direct connection or your own relay.",
            ));
        }
        Ok(())
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
            "chat.approve" if p.get("until_back").is_some() => {
                // GATE-615: a Yes "until I'm back", only where offered, only on the unlock window
                let offered = st.status["approvals"].as_array().is_some_and(|a| {
                    a.iter().any(|q| {
                        q["approval_id"] == p["approval_id"]
                            && q["phone"]["until_back"] == json!(true)
                    })
                });
                if p["granted"] != json!(true) || p["until_back"] != json!(true) || !offered {
                    return Err(LinkFailure::new(
                        "bad_request",
                        "That question can't be answered so.",
                    ));
                }
                if st.window_closed {
                    return Err(LinkFailure::new(
                        "locked",
                        "Unlock FERB on your phone first.",
                    ));
                }
                st.screen_use = json!({"active": true, "since": "2026-11-17T10:00:00+05:30", "phone": "EP-phone",
                                       "phone_name": "Pixel", "can_offer": true});
                json!({"ok": true})
            }
            "task.stop" | "chat.approve" | "chat.answer" | "chat.correct" => json!({"ok": true}),
            "screen_use.status" => st.screen_use.clone(),
            "screen_use.end" => {
                let ended = st.screen_use["active"] == json!(true);
                st.screen_use = json!({"active": false, "can_offer": true});
                let mut out = st.screen_use.clone();
                out["ended"] = json!(ended);
                out["by"] = json!("EP-phone");
                out
            }
            "insomnia.status" => st.insomnia.clone(),
            "insomnia.start" => {
                // the sensitive class: refused without the phone's proof (PHN-112)
                if proof_json.is_none() {
                    return Err(LinkFailure::new(
                        "fresh_unlock_required",
                        "This needs your fingerprint.",
                    ));
                }
                if st.insomnia["on"] == json!(true) {
                    let mut out = st.insomnia.clone();
                    out["already"] = json!(true);
                    out
                } else {
                    st.insomnia = json!({"on": true, "variant": p["variant"], "since": "2026-11-20T15:00:00+05:30",
                        "by": "EP-phone", "screen_yes": p["screen_yes"], "grace_minutes": 10, "working": 1,
                        "ends_at": null, "shield_up": p["variant"] == "shield", "mac_locked": p["variant"] == "locked",
                        "camera": {"on": false, "state": "allowed"},
                        "power": {"battery": 64, "on_battery": true, "lid_closed": false, "thermal": "nominal"},
                        "warnings": [], "notice": null,
                        "line": "Keeping your Mac working · 1 task running · battery 64%", "last_end": null});
                    st.insomnia.clone()
                }
            }
            "insomnia.end" => {
                let ended = st.insomnia["on"] == json!(true);
                if ended {
                    st.insomnia = json!({"on": false, "power": {"battery": 64, "on_battery": true}, "task_awake": false,
                        "line": "", "last_end": {"id": "e-1", "why": "turned_off",
                        "text": "FERB stopped keeping your Mac working.", "at": "2026-11-20T16:00:00+05:30"}});
                }
                let mut out = st.insomnia.clone();
                out["ended"] = json!(ended);
                out["by"] = json!("EP-phone");
                out
            }
            "insomnia.lock" => {
                st.insomnia["mac_locked"] = json!(true);
                st.insomnia["notice"] = Value::Null;
                json!({"locked": true, "text": "Your Mac is locked.", "insomnia": st.insomnia.clone()})
            }
            "insomnia.dismiss" => {
                if st.insomnia["notice"]["id"] != p["notice_id"] {
                    return Err(LinkFailure::new(
                        "not_found",
                        "That notice isn't waiting any more.",
                    ));
                }
                st.insomnia["notice"] = Value::Null;
                st.insomnia.clone()
            }
            "stop.engage" => {
                st.status["stopped"] = json!(true);
                json!({"stopped": true})
            }
            "link.lock" => json!({"unlocked": false}),
            "chat.status" => {
                let mut s = st.status.clone();
                if let Some(t) = &st.temporary {
                    s["temporary"] =
                        json!({"id": t.id, "rev": st.temp_rev, "started_at": t.started_at});
                }
                s
            }
            "chat.temporary.get" => st.temporary_reply(),
            "chat.temporary.set" => {
                // `on` is required and a bool: never a default
                match p["on"].as_bool() {
                    Some(true) => st.start_temporary(),
                    Some(false) => st.end_temporary(),
                    None => {
                        return Err(LinkFailure::new(
                            "ferb_error",
                            "chat.temporary.set needs on (true or false).",
                        ));
                    }
                }
                st.temporary_reply()
            }
            "link.files" => {
                let path = p["path"].as_str().unwrap_or("~").to_string();
                if st.key_places.contains(&path) && proof_json.is_none() {
                    return Err(LinkFailure::new(
                        "fresh_unlock_required",
                        "This place needs your fingerprint each time.",
                    ));
                }
                let mut folder =
                    st.folders.get(&path).cloned().ok_or_else(|| {
                        LinkFailure::new("not_found", "That isn't there any more.")
                    })?;
                if st.page_size.is_some() || p.get("after").is_some() {
                    let all = folder["entries"].as_array().cloned().unwrap_or_default();
                    let (page, more) = st.paged("files", &path, &all, &p["after"])?;
                    folder["total"] = json!(all.len());
                    folder["entries"] = json!(page);
                    for k in ["from", "cut", "next"] {
                        folder[k] = more[k].clone();
                    }
                }
                folder
            }
            "link.tree" => {
                json!({"path": p["path"], "name": "Photos", "files": [{"rel": "a.jpg", "size": 3}, {"rel": "b/c.jpg", "size": 4}],
                                   "bytes": 7, "skipped": 1, "cut": false})
            }
            "link.put_group" => {
                json!({"group": "G-1", "saved": "/Users/o/Downloads/FERB from phone/Photos",
                       "saved_in": "Downloads › FERB from phone › Photos"})
            }
            "link.keys" => json!({"enrolled": true}),
            "settings.get" => json!({"values": st.settings.clone()}),
            "settings.set" => {
                let key = p["key"].as_str().unwrap_or_default().to_string();
                st.settings.insert(key.clone(), p["value"].clone());
                json!({"key": key, "value": p["value"]})
            }
            "tasks.list" => json!({"tasks": [
                {"task_id": "t2", "name": "Desks", "goal": "Compare desks", "status": "running", "created_at": "2026-01-02T10:00:00+00:00",
                 "updated_at": null, "answer": "", "verdict": null, "steps": {"done": 1, "total": 3}},
                {"task_id": "t1", "name": "Backup", "goal": "Back up", "status": "done", "created_at": "2026-01-01T10:00:00+00:00",
                 "updated_at": "2026-01-01T11:00:00+00:00", "answer": "Copied.", "verdict": null}], "more": true}),
            "phone.device_report" => {
                if st.device_locked && p.get("request_id").is_none() {
                    return Err(LinkFailure::new("locked", "Unlock first."));
                }
                if p.get("request_id").is_some() {
                    json!({"delivered": true})
                } else {
                    json!({"ok": true})
                }
            }
            "phone.recent" => json!({"files": [
                {"name": "Invoice-Oct.pdf", "path": "/Users/o/Downloads/Invoice-Oct.pdf", "dir": false, "size": 4,
                 "modified": 1, "source": "made", "at": "2026-11-16T09:00:00+05:30", "task_id": "t1",
                 "task_name": "October invoice"},
                {"name": "photo.jpg", "path": "/Users/o/Downloads/FERB from phone/photo.jpg", "dir": false, "size": 9,
                 "modified": 2, "source": "sent", "at": "2026-11-16T08:00:00+05:30", "task_id": null, "task_name": null}],
                "skipped": 2}),
            "phone.search" if !st.found.is_empty() => {
                let key = format!(
                    "{}|{}",
                    p["path"].as_str().unwrap_or("~"),
                    p["query"].as_str().unwrap_or("")
                );
                let (page, more) = st.paged("search", &key, &st.found, &p["after"])?;
                json!({"path": "/Users/o", "query": p["query"], "files": page, "looked": 10, "skipped": 0,
                       "incomplete": false, "from": more["from"], "cut": more["cut"], "next": more["next"]})
            }
            "phone.search" => json!({"path": "/Users/o", "query": p["query"], "files": [
                {"name": "Invoices", "path": "/Users/o/Documents/Invoices", "dir": true, "size": 0, "modified": 3}],
                "looked": 1200, "skipped": 4, "cut": false, "incomplete": true}),
            // PHN-828: the fake Mac is signed in to the account "fpa1.same" names; "fpa1.other" is another account's
            "phone.bind_account" => match p["assertion"].as_str() {
                Some("fpa1.same") => json!({"bound": true, "changed": true}),
                Some("fpa1.other") => {
                    return Err(LinkFailure::new(
                        "account_mismatch",
                        "This phone is signed in as bob@example.com, but FERB on this Mac is signed in as \
                         ada@example.com. Sign in with the same account on both.",
                    ));
                }
                _ => {
                    return Err(LinkFailure::new(
                        "account_unconfirmed",
                        "FERB couldn't confirm which account this phone is signed in to. Check that the phone is \
                         online, then scan a new code on the Mac.",
                    ));
                }
            },
            // PHN-846: off only with a proof (the gate checks it on the real Mac); on with none
            "phone.lock_off" if proof_json.is_none() => {
                return Err(LinkFailure::new(
                    "unlock_required",
                    "Unlock the phone first.",
                ));
            }
            "phone.lock_off" => {
                st.lock_off = true;
                json!({"lock_off": true})
            }
            "phone.lock_on" => {
                st.lock_off = false;
                json!({"lock_off": false})
            }
            "phone.list" => json!({"phones": [
                {"id": "other", "name": "Old phone", "paired_at": "2026-01-01T00:00:00+00:00", "lock_off": false},
                {"id": "me", "name": "Pixel", "paired_at": "2026-11-10T09:00:00+00:00", "this": true,
                 "lock_off": st.lock_off}],
                "mac": {"name": st.mac_name}, "pairing": null, "max": 5,
                "upload_folder": {"path": "/Users/o/Downloads/FERB from phone", "shown": "Downloads › FERB from phone",
                                  "default": true}}),
            other => {
                return Err(LinkFailure::new(
                    "method_not_allowed",
                    format!("{other} is not allowed"),
                ));
            }
        };
        drop(st);
        self.deliver();
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
            if p["fresh"] == json!(true) {
                // the temporary chat: started when off; its turns only in memory, never `entries` (no seq, no epoch)
                if let Some(SendOutcome::Fail(code)) = st.script.front().cloned() {
                    st.script.pop_front();
                    return Err(closed_failure(&code));
                }
                st.start_temporary();
                st.temporary_entry(
                    "message",
                    p["text"].as_str().unwrap_or_default(),
                    &p["client_id"],
                );
                let text = format!(
                    "Fresh answer to: {}",
                    p["text"].as_str().unwrap_or_default()
                );
                st.temporary_entry("answer", &text, &Value::Null);
                let id = st
                    .temporary
                    .as_ref()
                    .map(|t| t.id.clone())
                    .unwrap_or_default();
                let ack = json!({"type": "ack", "request_id": format!("{id}-r1"), "fresh": true,
                                 "conversation": format!("temporary:{id}")})
                .to_string();
                drop(st);
                self.deliver();
                return Ok(ack);
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
                    let mut body = json!({"text": p["text"], "attachments": [], "hidden": []});
                    // the engine's quote (chat.rs `quoted`): an entry of this conversation, as it was shown; an id it
                    // can't find is dropped, never refused
                    if let Some(q) = p["reply_to"].as_str().and_then(|id| st.entries.get(id)) {
                        let who = if q.kind == "message" { "owner" } else { "ferb" };
                        body["reply_to"] = json!({"entry_id": q.id, "kind": q.kind, "who": who,
                                                  "excerpt": q.body.get("text").cloned().unwrap_or_default()});
                    }
                    let e = ChatEntry::read(&json!({
                        "id": entry, "conversation_id": st.conversation, "pos": pos, "seq": 0, "rev": 1,
                        "kind": "message", "state": "working", "open": true, "request_id": request,
                        "client_id": p["client_id"], "submission_id": sub,
                        "created_at": "2026-01-02T10:00:00+00:00", "updated_at": "2026-01-02T10:00:00+00:00",
                        "day": "2026-01-02", "body": body,
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

    fn get(
        &self,
        _id: &str,
        params_json: &str,
        save_to: Option<&str>,
        append: bool,
        sink: Arc<dyn TransferSink>,
    ) -> Result<String, LinkFailure> {
        let p: Value = serde_json::from_str(params_json).unwrap();
        let mut st = self.lock();
        st.transfers.push(("get".into(), p.clone()));
        if !st.online {
            return Err(closed_failure("mac_unreachable"));
        }
        Self::refuse_bulk(&st)?;
        if p["what"] == "camera" {
            let (meta, bytes) = st.camera.pop_front().unwrap_or((
                json!({"seq": null, "state": "starting", "text": "Starting your Mac's camera…"}),
                Vec::new(),
            ));
            use base64::Engine;
            return Ok(
                json!({"meta": meta, "size": bytes.len(), "received": bytes.len(),
                             "data": base64::engine::general_purpose::STANDARD.encode(&bytes)})
                .to_string(),
            );
        }
        if p["what"] == "preview" {
            let (meta, bytes) = st
                .frames
                .pop_front()
                .unwrap_or((json!({"state": "working", "busy": true}), Vec::new()));
            use base64::Engine;
            return Ok(
                json!({"meta": meta, "size": bytes.len(), "received": bytes.len(),
                             "data": base64::engine::general_purpose::STANDARD.encode(&bytes)})
                .to_string(),
            );
        }
        let path = if p["what"] == "file" {
            format!(
                "task:{}:{}",
                p["task_id"].as_str().unwrap_or_default(),
                p["index"]
            )
        } else {
            p["path"].as_str().unwrap_or_default().to_string()
        };
        if st.key_places.contains(&path) && p.get("proof").is_none() {
            return Err(LinkFailure::new(
                "fresh_unlock_required",
                "This file needs your fingerprint each time.",
            ));
        }
        let data = st
            .blobs
            .get(&path)
            .cloned()
            .ok_or_else(|| LinkFailure::new("not_found", "That file isn't there any more."))?;
        let offset = p["offset"].as_u64().unwrap_or(0) as usize;
        let rest = &data[offset.min(data.len())..];
        let (part, paused) = match st.pause_after {
            Some(n) if n < rest.len() => (&rest[..n], true),
            _ => (rest, false),
        };
        st.pause_after = None;
        let to = save_to.expect("a file is saved");
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(to)
            .unwrap();
        std::io::Write::write_all(&mut f, part).unwrap();
        for i in 1..=part.len() {
            sink.progress((offset + i) as u64, data.len() as u64);
        }
        if paused {
            return Err(LinkFailure::new("cancelled", "Stopped on the phone."));
        }
        Ok(json!({"meta": {"name": path.rsplit('/').next(), "offset": offset, "total": data.len()},
                  "received": part.len(), "saved_to": to})
        .to_string())
    }

    fn put(
        &self,
        _id: &str,
        path: &str,
        opts_json: &str,
        sink: Arc<dyn TransferSink>,
    ) -> Result<String, LinkFailure> {
        let opts: Value = serde_json::from_str(opts_json).unwrap();
        let mut st = self.lock();
        st.transfers.push(("put".into(), opts.clone()));
        Self::refuse_bulk(&st)?;
        let bytes =
            std::fs::read(path).map_err(|e| LinkFailure::new("not_found", e.to_string()))?;
        let id = opts["resume"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("U-{}", st.uploads.len() + 1));
        sink.accepted(&id, 0);
        sink.progress(bytes.len() as u64, bytes.len() as u64);
        let name = opts["name"].as_str().unwrap_or("file").to_string();
        st.uploads.push((name.clone(), bytes.clone(), opts));
        Ok(json!({"upload_id": id, "saved": format!("/Users/o/Downloads/FERB from phone/{id}-{name}"),
                  "saved_in": "Downloads › FERB from phone", "size": bytes.len(), "sha256": "x"})
        .to_string())
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
            // the temporary chat's watch (ORCH-716): from its memory, its id the epoch; `ended` when it isn't on
            if let Some(id) = p["conversation"]
                .as_str()
                .and_then(|c| c.strip_prefix("temporary:"))
            {
                let c = format!("temporary:{id}");
                let Some(t) = st.temporary.clone().filter(|t| t.id == id) else {
                    drop(st);
                    sink.frame(json!({"type": "ended", "conversation": c}).to_string());
                    sink.ended(None);
                    return Ok(Box::new(FakeWatch(self.clone())));
                };
                let after = p["after_seq"].as_i64().unwrap_or(0);
                for e in t
                    .entries
                    .iter()
                    .filter(|e| e["seq"].as_i64().unwrap_or(0) > after)
                {
                    frames.push(json!({"type": "change", "catchup": true, "entry": e}));
                }
                frames.push(json!({"type": "caught_up", "conversation": c, "epoch": id,
                                   "head": t.entries.len()}));
                st.sink = Some(sink.clone());
                st.watched = c;
                drop(st);
                for f in frames {
                    sink.frame(f.to_string());
                }
                return Ok(Box::new(FakeWatch(self.clone())));
            }
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
            st.watched = st.conversation.clone();
            frames
        };
        for f in frames {
            sink.frame(f.to_string());
        }
        Ok(Box::new(FakeWatch(self.clone())))
    }
}
