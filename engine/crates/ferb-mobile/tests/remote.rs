//! The phone's calls beyond the chat (T-1221) against the fake Mac: the app's lock and Stop everything, what FERB has
//! waiting with each question's needs, the read mark at the newest entry, a message with files, files both ways with
//! progress, pause and resume, the live preview's frames in memory only, settings signed, the Mac's tasks and name, and
//! keys registered after pairing. The public relay refuses files and pictures before anything is asked (PHN-444).

mod common;

use std::sync::{Arc, Mutex};

use common::mac::FakeMac;
use common::phone::{online, Phone};
use ferb_mobile::remote::FRESH_UNLOCK_REQUIRED;
use ferb_mobile::store::Attachment;
use ferb_mobile::transport::TransferSink;
use ferb_mobile::{Answer, Change, ErrorCode, Mobile, Signer};
use serde_json::{json, Value};

/// A signer that records what it signed.
#[derive(Default)]
struct Recorded(Mutex<Vec<String>>);
impl Signer for Recorded {
    fn sign(&self, payload: &str) -> Option<String> {
        self.0.lock().unwrap().push(payload.to_string());
        Some("SIG".into())
    }
}

#[derive(Default)]
struct Moves(Mutex<Vec<(u64, u64)>>, Mutex<Vec<(String, u64)>>);
impl TransferSink for Moves {
    fn progress(&self, done: u64, of: u64) {
        self.0.lock().unwrap().push((done, of));
    }
    fn accepted(&self, upload_id: &str, offset: u64) {
        self.1.lock().unwrap().push((upload_id.to_string(), offset));
    }
}

fn connected(phone: &Phone, mac: &FakeMac, tier: &str) -> Mobile {
    let m = phone.open();
    m.connect(Arc::new(mac.clone())).unwrap();
    m.link_event(&online(tier));
    phone.jobs.run();
    m
}

fn entry(id: &str, pos: i64, text: &str) -> ferb_mobile::entry::ChatEntry {
    ferb_mobile::entry::ChatEntry::read(&json!({
        "id": id, "conversation_id": "C1", "pos": pos, "seq": 0, "rev": 1, "kind": "message", "state": "done",
        "open": false, "request_id": format!("r-{id}"), "created_at": "2026-01-02T10:00:00+00:00",
        "updated_at": "2026-01-02T10:00:00+00:00", "day": "2026-01-02",
        "body": {"text": text, "attachments": [], "hidden": []}, "deleted": false}))
    .unwrap()
}

/// The app locks: the Mac is told at once and the watch stops, so nothing of the conversation arrives until the
/// unlock; a sync meanwhile reads only what waits (the status, allowed locked) and says to unlock. Stop everything
/// works locked.
// decision: PHN-15
// decision: PHN-125
// decision: PHN-510
#[test]
fn locking_tells_the_mac_and_stops_the_conversation_until_the_unlock() {
    let mac = FakeMac::with("C1", &[entry("a", 10, "hello")]);
    let phone = Phone::new();
    let m = connected(&phone, &mac, "direct");
    assert!(m.link_state().watching);
    assert!(m.lock().unwrap(), "the Mac was told");
    assert_eq!(mac.calls_of("link.lock").len(), 1);
    assert!(!m.link_state().watching && m.link_state().locked);
    assert!(mac.lock().sink.is_none(), "the watch was closed");
    mac.commit(entry("b", 11, "while locked"));
    let pages = mac.calls_of("chat.entries.page").len();
    let e = m.sync().unwrap_err();
    assert_eq!(e.code, ErrorCode::NeedsUnlock);
    assert_eq!(
        mac.calls_of("chat.entries.page").len(),
        pages,
        "no page while locked"
    );
    assert_eq!(mac.calls_of("chat.status").len(), 1, "what waits, only");
    let t = m.transcript("C1").unwrap();
    assert_eq!(t.items.len(), 1, "nothing new arrived locked");
    m.stop_everything().unwrap();
    assert_eq!(mac.calls_of("stop.engage").len(), 1);
    // the unlock: read again
    m.unlocked();
    m.sync().unwrap();
    assert!(m.link_state().watching && !m.link_state().locked);
    assert_eq!(m.transcript("C1").unwrap().items.len(), 2);
    // offline, the lock is the phone's alone
    m.link_event(common::phone::OFFLINE);
    assert!(!m.lock().unwrap());
}

/// What waits, with each question's needs: pushed by the Mac (a `status` link event) or read; an answer that needs the
/// phone's proof signs the question's own challenge from the status (the only one the Mac's check takes), not a fresh
/// link challenge.
// decision: PHN-417
// decision: PHN-470
// decision: PHN-126
// decision: PHN-511
#[test]
fn the_status_says_each_question_s_needs_and_its_challenge_signs_the_answer() {
    let mac = FakeMac::new("C1");
    let phone = Phone::new();
    let m = connected(&phone, &mac, "direct");
    assert!(
        m.last_status().unwrap().is_none()
            || m.last_status().unwrap().unwrap().questions.is_empty()
    );
    let pushed = json!({"type": "status", "status": {
        "approvals": [
            {"approval_id": "q1", "kind": "approve", "reason": "Open the page?", "options": ["Yes", "No"],
             "phone": {"can_answer": true, "needs_unlock": false}},
            {"approval_id": "q2", "kind": "approve", "reason": "Pay $12.99?", "options": ["Yes", "No"],
             "phone": {"can_answer": true, "needs_unlock": true, "hidden": false, "challenge": "Q2-CH", "fingerprint": true}}],
        "running": [], "results": [], "stopped": false}});
    phone.heard.0.lock().unwrap().clear();
    m.link_event(&pushed.to_string());
    assert!(phone.heard.0.lock().unwrap().contains(&Change::Status));
    let st = m.last_status().unwrap().unwrap();
    assert!(st.questions[0].needs.routine());
    assert!(!st.questions[1].needs.routine() && st.questions[1].needs.fingerprint);
    // the same status again changes nothing
    phone.heard.0.lock().unwrap().clear();
    m.link_event(&pushed.to_string());
    assert!(phone.heard.0.lock().unwrap().is_empty());
    let signer = Arc::new(Recorded::default());
    m.answer(
        "q2",
        Answer::Approve {
            granted: true,
            choice: Some(1),
        },
        Some(signer.clone()),
    )
    .unwrap();
    let signed: Value = serde_json::from_str(&signer.0.lock().unwrap()[0]).unwrap();
    assert_eq!(signed["challenge"], "Q2-CH");
    assert_eq!(mac.calls_of("link.challenge").len(), 0);
    // read on demand, locked or not
    mac.lock().status["locked"] = json!(true);
    mac.lock().status["words"] = json!(false);
    let read = m.status().unwrap();
    assert!(read.locked && !read.words);
}

/// The read mark without a position: the newest entry the phone holds.
#[test]
fn the_read_mark_goes_to_the_newest_entry_held() {
    let mac = FakeMac::with("C1", &[entry("a", 10, "one"), entry("b", 12, "two")]);
    let phone = Phone::new();
    let m = connected(&phone, &mac, "direct");
    m.mark_read_latest("C1").unwrap();
    let marks: Vec<Value> = mac
        .calls_of("chat.entries.mark")
        .into_iter()
        .filter(|p| p.get("read_pos").is_some())
        .collect();
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0]["read_pos"], 12);
    // nothing moved: nothing said
    m.mark_read_latest("C1").unwrap();
    assert_eq!(
        mac.calls_of("chat.entries.mark")
            .iter()
            .filter(|p| p.get("read_pos").is_some())
            .count(),
        1
    );
}

/// A message with files rides the outbox like any message, kept across a restart, and goes with the uploads' ids.
// decision: PHN-127
// decision: PHN-401
// decision: PHN-513
#[test]
fn a_message_with_files_waits_in_the_outbox_and_goes_with_their_ids() {
    let mac = FakeMac::new("C1");
    let phone = Phone::new();
    {
        let m = phone.open();
        let files = [Attachment {
            upload_id: "U-1".into(),
            name: "a.jpg".into(),
        }];
        let item = m.send_with("look at this", &files).unwrap();
        assert_eq!(item.attachments, files);
    }
    let m = phone.open();
    assert_eq!(m.outbox().unwrap()[0].attachments[0].upload_id, "U-1");
    m.connect(Arc::new(mac.clone())).unwrap();
    m.link_event(&online("direct"));
    phone.jobs.run();
    let sent = mac.lock().sends.clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["attachments"], json!(["U-1"]));
    assert!(m.outbox().unwrap().is_empty());
}

/// Browsing: a folder's entries with their whole paths; a key place asks for its own proof, signed over the path
/// alone with the phone-facing method (PHN-121).
// decision: PHN-113
// decision: PHN-121
// decision: PHN-513
#[test]
fn a_folder_lists_and_a_key_place_takes_its_own_proof() {
    let mac = FakeMac::new("C1");
    mac.lock().folders.insert(
        "~".into(),
        json!({"path": "/Users/o", "parent": "/Users", "high_value": false, "total": 2, "cut": false,
               "entries": [{"name": "Desktop", "dir": true, "link": false, "size": 0, "modified": 1, "locked": false},
                           {"name": ".ssh", "dir": true, "link": false, "size": 0, "modified": 2, "locked": true}]}),
    );
    mac.lock().folders.insert(
        "/Users/o/.ssh".into(),
        json!({"path": "/Users/o/.ssh", "parent": "/Users/o", "high_value": true, "total": 0, "cut": false, "entries": []}),
    );
    mac.lock().key_places.push("/Users/o/.ssh".into());
    let phone = Phone::new();
    let m = connected(&phone, &mac, "direct");
    let home = m.files(None, None).unwrap();
    assert_eq!(home.path, "/Users/o");
    assert_eq!(home.entries[1].path, "/Users/o/.ssh");
    assert!(home.entries[1].locked);
    let e = m.files(Some("/Users/o/.ssh"), None).unwrap_err();
    assert_eq!(
        (e.code, e.link_code.as_deref()),
        (ErrorCode::NeedsUnlock, Some(FRESH_UNLOCK_REQUIRED))
    );
    let signer = Arc::new(Recorded::default());
    let ssh = m
        .files(Some("/Users/o/.ssh"), Some(signer.clone()))
        .unwrap();
    assert!(ssh.locked);
    let signed: Value = serde_json::from_str(&signer.0.lock().unwrap()[0]).unwrap();
    assert_eq!(
        signed,
        json!({"challenge": "C-1", "method": "link.files", "params": {"path": "/Users/o/.ssh"}})
    );
}

/// A file comes chunk by chunk into the phone's own file with progress; a pause keeps what arrived and the fetch goes
/// on from there (appended) to the whole file. A key place's file signs its path alone.
// decision: PHN-115
// decision: PHN-513
#[test]
fn a_fetch_saves_with_progress_pauses_and_goes_on() {
    let mac = FakeMac::new("C1");
    let body: Vec<u8> = (0..100u8).collect();
    mac.lock()
        .blobs
        .insert("/Users/o/Desktop/a.bin".into(), body.clone());
    mac.lock().pause_after = Some(40);
    let phone = Phone::new();
    let m = connected(&phone, &mac, "own_relay");
    let save = phone.dir.path().join("a.bin");
    let moves = Arc::new(Moves::default());
    let e = m
        .fetch(
            "f1",
            "/Users/o/Desktop/a.bin",
            save.to_str().unwrap(),
            0,
            None,
            moves.clone(),
        )
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Cancelled);
    assert_eq!(std::fs::read(&save).unwrap(), body[..40]);
    let done = m
        .fetch(
            "f2",
            "/Users/o/Desktop/a.bin",
            save.to_str().unwrap(),
            40,
            None,
            moves.clone(),
        )
        .unwrap();
    assert_eq!((done.received, done.total), (60, 100));
    assert_eq!(std::fs::read(&save).unwrap(), body);
    assert_eq!(moves.0.lock().unwrap().last(), Some(&(100, 100)));
    let gets = mac.lock().transfers.clone();
    assert_eq!(gets[1].1["offset"], 40);
    // a key place's file: its proof over the path alone
    mac.lock()
        .blobs
        .insert("/Users/o/.ssh/id".into(), b"k".to_vec());
    mac.lock().key_places.push("/Users/o/.ssh/id".into());
    let signer = Arc::new(Recorded::default());
    let key = phone.dir.path().join("id");
    m.fetch(
        "f3",
        "/Users/o/.ssh/id",
        key.to_str().unwrap(),
        0,
        Some(signer.clone()),
        moves,
    )
    .unwrap();
    let signed: Value = serde_json::from_str(&signer.0.lock().unwrap()[0]).unwrap();
    assert_eq!(
        signed,
        json!({"challenge": "C-1", "method": "link.get", "params": {"path": "/Users/o/.ssh/id"}})
    );
}

/// An upload from the phone's own storage: the Mac's go-ahead, progress, the upload's id; going on takes the id.
/// A folder is begun with its group.
// decision: PHN-127
// decision: PHN-513
#[test]
fn an_upload_reports_its_id_and_goes_on_by_it() {
    let mac = FakeMac::new("C1");
    let phone = Phone::new();
    let m = connected(&phone, &mac, "direct");
    let local = phone.dir.path().join("photo.jpg");
    std::fs::write(&local, b"jpegbytes").unwrap();
    let moves = Arc::new(Moves::default());
    let up = m
        .upload(
            "u1",
            local.to_str().unwrap(),
            "photo.jpg",
            None,
            None,
            None,
            moves.clone(),
        )
        .unwrap();
    assert_eq!(up.upload_id, "U-1");
    assert_eq!(moves.1.lock().unwrap()[0], ("U-1".to_string(), 0));
    assert_eq!(mac.lock().uploads[0].1, b"jpegbytes");
    let again = m
        .upload(
            "u2",
            local.to_str().unwrap(),
            "photo.jpg",
            Some("U-1"),
            None,
            None,
            moves,
        )
        .unwrap();
    assert_eq!(again.upload_id, "U-1");
    assert_eq!(mac.lock().uploads[1].2["resume"], "U-1");
    assert_eq!(m.put_group("Photos").unwrap(), "G-1");
    let tree = m.tree("/Users/o/Photos", None).unwrap();
    assert_eq!((tree.files.len(), tree.bytes, tree.skipped), (2, 7, 1));
}

/// The public relay: files, folders and the live preview are refused on the phone before anything is asked
/// (PHN-444, PHN-454); browsing a folder still goes (the Mac's listing fits a control frame).
// decision: PHN-444
// decision: PHN-454
// decision: PHN-512
// decision: PHN-513
#[test]
fn the_public_relay_refuses_files_and_the_preview_before_asking() {
    let mac = FakeMac::new("C1");
    let phone = Phone::new();
    let m = connected(&phone, &mac, "shared_relay");
    assert!(!m.link_state().bulk_allowed);
    let save = phone.dir.path().join("x");
    let codes = [
        m.fetch(
            "f",
            "/x",
            save.to_str().unwrap(),
            0,
            None,
            Arc::new(Moves::default()),
        )
        .unwrap_err()
        .code,
        m.upload(
            "u",
            "/nope",
            "x",
            None,
            None,
            None,
            Arc::new(Moves::default()),
        )
        .unwrap_err()
        .code,
        m.tree("/x", None).unwrap_err().code,
        m.put_group("x").unwrap_err().code,
        m.preview("t1", None).unwrap_err().code,
    ];
    assert!(
        codes.iter().all(|c| *c == ErrorCode::NeedsBetterPath),
        "{codes:?}"
    );
    assert!(mac.lock().transfers.is_empty(), "nothing was asked");
    assert!(mac.calls_of("link.tree").is_empty() && mac.calls_of("link.put_group").is_empty());
    // the link says the path carries bulk again
    m.link_event(r#"{"type":"bulk","allowed":true,"tier":"direct"}"#);
    mac.lock().frames.push_back((
        json!({"seq": 1, "state": "working", "fresh": true}),
        b"JPEG".to_vec(),
    ));
    assert!(m.preview("t1", None).unwrap().jpeg.is_some());
}

/// The live preview: the frame's bytes in memory (never stored, never in a listener's change), `have` sent, the
/// pace FERB.app keeps.
// decision: PHN-454
// decision: PHN-512
#[test]
fn preview_frames_come_in_memory_with_the_mac_s_state() {
    let mac = FakeMac::new("C1");
    mac.lock().frames.push_back((
        json!({"seq": 7, "state": "working", "fresh": true, "line": "Reading delivery options"}),
        b"\xff\xd8JPEG".to_vec(),
    ));
    mac.lock().frames.push_back((
        json!({"seq": 7, "state": "working", "busy": true}),
        Vec::new(),
    ));
    let phone = Phone::new();
    let m = connected(&phone, &mac, "direct");
    phone.heard.0.lock().unwrap().clear();
    let a = m.preview("t1", None).unwrap();
    assert_eq!(a.jpeg.as_deref(), Some(&b"\xff\xd8JPEG"[..]));
    assert_eq!((a.seq, a.next_after_ms()), (Some(7), 0));
    assert_eq!(a.line.as_deref(), Some("Reading delivery options"));
    let b = m.preview("t1", a.seq).unwrap();
    assert!(b.jpeg.is_none() && b.busy);
    assert_eq!(b.next_after_ms(), 200);
    let gets = mac.lock().transfers.clone();
    assert_eq!(gets[1].1["have"], 7);
    assert_eq!(gets[0].1["what"], "preview");
    assert!(
        phone.heard.0.lock().unwrap().is_empty(),
        "no change carries a frame"
    );
    // nothing of it in the store's file
    let db = std::fs::read(phone.path()).unwrap();
    assert!(!db.windows(4).any(|w| w == b"JPEG"));
}

/// Settings: read behind the unlock; a change is signed over `{challenge, method, params}` (the sensitive class).
// decision: PHN-404
// decision: PHN-112
// decision: PHN-513
#[test]
fn settings_are_read_and_a_change_is_signed() {
    let mac = FakeMac::new("C1");
    mac.lock()
        .settings
        .insert("sense.window_titles".into(), json!(true));
    let phone = Phone::new();
    let m = connected(&phone, &mac, "direct");
    let all = m.settings().unwrap();
    assert_eq!(all[0].key, "sense.window_titles");
    assert_eq!(all[0].value_json, "true");
    let signer = Arc::new(Recorded::default());
    let stored = m
        .set_setting("sense.window_titles", "false", signer.clone())
        .unwrap();
    assert_eq!(stored, "false");
    let signed: Value = serde_json::from_str(&signer.0.lock().unwrap()[0]).unwrap();
    assert_eq!(
        signed,
        json!({"challenge": "C-1", "method": "settings.set",
               "params": {"key": "sense.window_titles", "value": false}})
    );
    let call = mac.lock().calls.last().cloned().unwrap();
    let proof: Value = serde_json::from_str(call.2.as_deref().unwrap()).unwrap();
    assert_eq!(proof["sig"], "SIG");
    assert!(m.set_setting("x", "not json", signer).is_err());
}

/// The Mac's tasks, its name and this phone's pairing date (kept for offline), and keys registered after pairing.
// decision: PHN-133
// decision: PHN-513
#[test]
fn the_mac_s_tasks_name_and_late_keys() {
    let mac = FakeMac::new("C1");
    mac.lock().mac_name = Some("MacBook Pro".into());
    let phone = Phone::new();
    let m = connected(&phone, &mac, "direct");
    let page = m.tasks(20, None).unwrap();
    assert!(page.more);
    assert_eq!(page.tasks[0].steps_total, Some(3));
    assert_eq!(page.tasks[1].answer, "Copied.");
    assert!(m.cached_mac_info().unwrap().is_none());
    phone.heard.0.lock().unwrap().clear();
    let info = m.mac_info().unwrap();
    assert_eq!(info.name.as_deref(), Some("MacBook Pro"));
    assert_eq!(info.paired_at.as_deref(), Some("2026-11-10T09:00:00+00:00"));
    assert!(phone.heard.0.lock().unwrap().contains(&Change::Mac));
    drop(m);
    let again = phone.open();
    assert_eq!(again.cached_mac_info().unwrap(), Some(info));
    again.connect(Arc::new(mac.clone())).unwrap();
    again.link_event(&online("direct"));
    phone.jobs.run();
    let signer = Arc::new(Recorded::default());
    again
        .register_keys(None, Some("STRONG-PUB"), signer.clone())
        .unwrap();
    let signed: Value = serde_json::from_str(&signer.0.lock().unwrap()[0]).unwrap();
    assert_eq!(
        signed,
        json!({"challenge": "C-1", "method": "link.keys", "params": {"strong_pub": "STRONG-PUB"}})
    );
    assert!(again.register_keys(None, None, signer).is_err());
}
