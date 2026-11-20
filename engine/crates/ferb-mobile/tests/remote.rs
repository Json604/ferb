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
    let home = m.files(None, None, None).unwrap();
    assert_eq!(home.path, "/Users/o");
    assert_eq!(home.entries[1].path, "/Users/o/.ssh");
    assert!(home.entries[1].locked);
    let e = m.files(Some("/Users/o/.ssh"), None, None).unwrap_err();
    assert_eq!(
        (e.code, e.link_code.as_deref()),
        (ErrorCode::NeedsUnlock, Some(FRESH_UNLOCK_REQUIRED))
    );
    let signer = Arc::new(Recorded::default());
    let ssh = m
        .files(Some("/Users/o/.ssh"), Some(signer.clone()), None)
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

/// "Until I'm back at my Mac" (GATE-615): offered per question in the status; the answer is `chat.approve {granted,
/// until_back}` with no proof (a signer passed is not used), refused `locked` when the unlock window is closed (the
/// app unlocks and answers again) and refused for a question it wasn't offered on. FERB's use of the screen is read
/// and ended with no unlock, and kept in the last status.
// decision: GATE-615
#[test]
fn until_back_answers_the_screen_question_and_screen_use_is_read_and_ended() {
    let mac = FakeMac::new("C1");
    mac.lock().status = json!({"approvals": [
        {"approval_id": "s1", "kind": "approve", "reason": "Let FERB use your screen?", "options": ["Yes", "No"],
         "phone": {"can_answer": true, "needs_unlock": false, "until_back": true}},
        {"approval_id": "q2", "kind": "approve", "reason": "Open it?", "options": ["Yes", "No"],
         "phone": {"can_answer": true, "needs_unlock": false}}],
        "running": [], "results": [], "stopped": false,
        "screen_use": {"active": false, "can_offer": true}});
    let phone = Phone::new();
    let m = connected(&phone, &mac, "direct");
    let st = m.status().unwrap();
    assert!(st.questions[0].needs.until_back && !st.questions[1].needs.until_back);
    assert_eq!(
        st.screen_use.as_ref().map(|u| (u.active, u.can_offer)),
        Some((false, true))
    );
    // the window closed: refused as locked, nothing given
    mac.lock().window_closed = true;
    let e = m.answer("s1", Answer::UntilBack, None).unwrap_err();
    assert_eq!(
        (e.code, e.link_code.as_deref()),
        (ErrorCode::NeedsUnlock, Some("locked"))
    );
    // a question it wasn't offered on: the Mac's bad request
    mac.lock().window_closed = false;
    let e = m.answer("q2", Answer::UntilBack, None).unwrap_err();
    assert_eq!(
        (e.code, e.link_code.as_deref()),
        (ErrorCode::Refused, Some("bad_request"))
    );
    // answered: exactly these params, and no proof even with a signer
    let signer = Arc::new(Recorded::default());
    m.answer("s1", Answer::UntilBack, Some(signer.clone()))
        .unwrap();
    assert!(signer.0.lock().unwrap().is_empty());
    let call = mac.lock().calls.last().cloned().unwrap();
    assert_eq!(call.0, "chat.approve");
    assert_eq!(
        call.1,
        json!({"approval_id": "s1", "granted": true, "until_back": true})
    );
    assert!(call.2.is_none());
    assert_eq!(mac.calls_of("link.challenge").len(), 0);
    // what is in force, read and ended with the phone locked (no unlock needed)
    assert!(m.lock().unwrap());
    phone.heard.0.lock().unwrap().clear();
    let now = m.screen_use().unwrap();
    assert!(now.active);
    assert_eq!(now.phone_name.as_deref(), Some("Pixel"));
    assert_eq!(now.since.as_deref(), Some("2026-11-17T10:00:00+05:30"));
    assert!(phone.heard.0.lock().unwrap().contains(&Change::Status));
    assert_eq!(m.last_status().unwrap().unwrap().screen_use, Some(now));
    let end = m.end_screen_use().unwrap();
    assert!(end.ended && !end.screen_use.active);
    assert_eq!(end.by, "EP-phone");
    assert!(!m.last_status().unwrap().unwrap().screen_use.unwrap().active);
    assert!(!m.end_screen_use().unwrap().ended, "nothing left to end");
    assert_eq!(mac.calls_of("screen_use.end").len(), 2);
}

/// "Load more" (PHN-589): a folder and a search come a page at a time; `next` goes back as `after` (beside a key
/// place's proof, which stays over the path alone) until the last page, and a marker of another folder is refused.
// decision: PHN-588
// decision: PHN-589
#[test]
fn listings_and_searches_load_more_by_their_marker() {
    let mac = FakeMac::new("C1");
    let entries: Vec<Value> = ["a", "b", "c"]
        .iter()
        .map(|n| json!({"name": n, "dir": false, "link": false, "size": 1, "modified": 1, "locked": false}))
        .collect();
    mac.lock().folders.insert(
        "/Users/o/.ssh".into(),
        json!({"path": "/Users/o/.ssh", "parent": "/Users/o", "high_value": true, "entries": entries}),
    );
    mac.lock().folders.insert(
        "~".into(),
        json!({"path": "/Users/o", "parent": "/Users", "high_value": false, "entries": []}),
    );
    mac.lock().key_places.push("/Users/o/.ssh".into());
    mac.lock().page_size = Some(2);
    let phone = Phone::new();
    let m = connected(&phone, &mac, "direct");
    let signer = Arc::new(Recorded::default());
    let first = m
        .files(Some("/Users/o/.ssh"), Some(signer.clone()), None)
        .unwrap();
    assert_eq!(
        (first.entries.len(), first.total, first.from, first.cut),
        (2, 3, 0, true)
    );
    let marker = first.next.clone().unwrap();
    let last = m
        .files(Some("/Users/o/.ssh"), Some(signer.clone()), Some(&marker))
        .unwrap();
    assert_eq!(last.entries[0].name, "c");
    assert_eq!(
        (last.from, last.cut, last.next.as_deref()),
        (2, false, None)
    );
    let sent = mac.calls_of("link.files");
    assert_eq!(sent[1], json!({"path": "/Users/o/.ssh", "after": marker}));
    for signed in signer.0.lock().unwrap().iter() {
        let signed: Value = serde_json::from_str(signed).unwrap();
        assert_eq!(
            signed["params"],
            json!({"path": "/Users/o/.ssh"}),
            "the proof is over the path alone"
        );
    }
    // another folder's marker: the Mac's bad request
    let e = m.files(None, None, Some(&marker)).unwrap_err();
    assert_eq!(
        (e.code, e.link_code.as_deref()),
        (ErrorCode::Refused, Some("bad_request"))
    );
    // a search, the same way
    mac.lock().found = (0..3)
        .map(|i| json!({"name": format!("inv{i}.pdf"), "path": format!("/Users/o/inv{i}.pdf"), "dir": false,
                        "size": 1, "modified": 1}))
        .collect();
    let one = m.search_files("inv", None, None).unwrap();
    assert_eq!((one.files.len(), one.from, one.cut), (2, 0, true));
    let two = m.search_files("inv", None, one.next.as_deref()).unwrap();
    assert_eq!(
        (two.files.len(), two.from, two.cut, two.next),
        (1, 2, false, None)
    );
    assert_eq!(
        mac.calls_of("phone.search")[1]["after"],
        json!(one.next.unwrap())
    );
    let e = m.search_files("other", None, Some(&marker)).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused);
}

/// Where files from the phone land (PHN-580, PHN-581): an upload and a folder say the folder plainly, and the Mac's
/// upload folder comes with what it says about itself (kept for offline).
// decision: PHN-580
// decision: PHN-581
#[test]
fn uploads_say_where_they_landed_and_the_mac_names_its_upload_folder() {
    let mac = FakeMac::new("C1");
    let phone = Phone::new();
    let m = connected(&phone, &mac, "direct");
    let local = phone.dir.path().join("photo.jpg");
    std::fs::write(&local, b"jpg").unwrap();
    let up = m
        .upload(
            "u1",
            local.to_str().unwrap(),
            "photo.jpg",
            None,
            None,
            None,
            Arc::new(Moves::default()),
        )
        .unwrap();
    assert_eq!(up.saved_in.as_deref(), Some("Downloads › FERB from phone"));
    let g = m.begin_group("Photos").unwrap();
    assert_eq!(g.group, "G-1");
    assert_eq!(
        g.saved_in.as_deref(),
        Some("Downloads › FERB from phone › Photos")
    );
    let info = m.mac_info().unwrap();
    let folder = info.upload_folder.clone().unwrap();
    assert_eq!(folder.shown, "Downloads › FERB from phone");
    assert!(folder.is_default && folder.path.ends_with("FERB from phone"));
    assert_eq!(m.cached_mac_info().unwrap(), Some(info));
}

/// Insomnia mode from the phone (APP-980): the start is the sensitive class, signed over exactly
/// `{challenge, method, params}` like a setting; a second start says it was on already; the end, Lock my Mac and
/// It's fine go with no proof and work while the phone is locked (APP-983). Every answer is kept in the last status
/// and told as a status change.
// decision: APP-980
// decision: APP-983
// decision: APP-984
// decision: PHN-112
#[test]
fn insomnia_starts_signed_and_ends_locks_and_dismisses_with_no_proof() {
    let mac = FakeMac::new("C1");
    let phone = Phone::new();
    let m = connected(&phone, &mac, "direct");
    m.status().unwrap();
    // an older Mac's status says nothing of it
    assert_eq!(m.last_status().unwrap().unwrap().insomnia, None);
    assert!(!m.insomnia_status().unwrap().on);
    // a way the Mac doesn't know is refused before anything is asked
    let signer = Arc::new(Recorded::default());
    let before = mac.lock().calls.len();
    assert_eq!(
        m.start_insomnia("asleep", true, signer.clone())
            .unwrap_err()
            .code,
        ErrorCode::Refused
    );
    assert_eq!(mac.lock().calls.len(), before);
    phone.heard.0.lock().unwrap().clear();
    let started = m.start_insomnia("shield", true, signer.clone()).unwrap();
    assert!(!started.already && started.insomnia.on && started.insomnia.screen_yes);
    assert_eq!(started.insomnia.variant.as_deref(), Some("shield"));
    let signed: Value = serde_json::from_str(&signer.0.lock().unwrap()[0]).unwrap();
    assert_eq!(
        signed,
        json!({"challenge": "C-1", "method": "insomnia.start",
               "params": {"variant": "shield", "screen_yes": true}})
    );
    let call = mac.lock().calls.last().cloned().unwrap();
    assert_eq!(call.0, "insomnia.start");
    let proof: Value = serde_json::from_str(call.2.as_deref().unwrap()).unwrap();
    assert_eq!(proof["sig"], "SIG");
    assert!(phone.heard.0.lock().unwrap().contains(&Change::Status));
    assert!(m.last_status().unwrap().unwrap().insomnia.unwrap().on);
    assert!(
        m.start_insomnia("locked", false, signer).unwrap().already,
        "on already: nothing changes"
    );
    // a person touched the Mac: the notice, answered with the phone locked
    mac.lock().insomnia["notice"] = json!({"id": "n-1", "kind": "touched", "at": "x", "app": null, "count": 1,
                                            "text": "Someone is using your Mac's keyboard or trackpad."});
    assert!(m.lock().unwrap());
    assert_eq!(m.insomnia_status().unwrap().notice.unwrap().id, "n-1");
    let e = m.dismiss_insomnia_notice("n-old").unwrap_err();
    assert_eq!(e.link_code.as_deref(), Some("not_found"));
    let after = m.dismiss_insomnia_notice("n-1").unwrap();
    assert!(after.notice.is_none() && after.on);
    assert!(m
        .last_status()
        .unwrap()
        .unwrap()
        .insomnia
        .unwrap()
        .notice
        .is_none());
    let locked = m.lock_mac().unwrap();
    assert!(locked.locked && locked.insomnia.unwrap().mac_locked);
    assert_eq!(locked.text, "Your Mac is locked.");
    let ended = m.end_insomnia().unwrap();
    assert!(ended.ended && !ended.insomnia.on);
    assert_eq!(ended.by.as_deref(), Some("EP-phone"));
    assert_eq!(ended.insomnia.last_end.unwrap().why, "turned_off");
    assert!(!m.end_insomnia().unwrap().ended, "nothing left to end");
    // only the start carried a proof
    for c in mac
        .lock()
        .calls
        .iter()
        .filter(|c| c.0.starts_with("insomnia.") && c.0 != "insomnia.start")
    {
        assert!(c.2.is_none(), "{} went with no proof", c.0);
    }
    assert_eq!(
        mac.calls_of("insomnia.dismiss")[1],
        json!({"notice_id": "n-1"})
    );
    assert_eq!(mac.calls_of("insomnia.end")[0], json!({}));
}

/// Show me (APP-983): the Mac's camera as `link.get what: camera`, frames in memory only (never stored, never in a
/// listener's change), `have` sent; on the public relay it is refused before anything is asked, like the preview.
// decision: APP-983
// decision: PHN-444
// decision: PHN-454
#[test]
fn the_mac_s_camera_comes_in_memory_and_never_over_the_public_relay() {
    let mac = FakeMac::new("C1");
    mac.lock().camera.push_back((
        json!({"seq": 3, "state": "live", "text": null}),
        b"\xff\xd8CAMJPEG".to_vec(),
    ));
    mac.lock()
        .camera
        .push_back((json!({"seq": 3, "state": "live"}), Vec::new()));
    let phone = Phone::new();
    let m = connected(&phone, &mac, "direct");
    phone.heard.0.lock().unwrap().clear();
    let a = m.mac_camera(None).unwrap();
    assert_eq!(a.jpeg.as_deref(), Some(&b"\xff\xd8CAMJPEG"[..]));
    assert_eq!((a.seq, a.state.as_str()), (Some(3), "live"));
    let b = m.mac_camera(a.seq).unwrap();
    assert!(b.jpeg.is_none());
    // nothing waiting: the Mac's starting words
    let c = m.mac_camera(b.seq).unwrap();
    assert_eq!(c.state, "starting");
    assert_eq!(c.text.as_deref(), Some("Starting your Mac's camera…"));
    let gets = mac.lock().transfers.clone();
    assert_eq!(gets[0].1, json!({"what": "camera"}));
    assert_eq!(gets[1].1, json!({"what": "camera", "have": 3}));
    assert!(
        phone.heard.0.lock().unwrap().is_empty(),
        "no change carries a frame"
    );
    let db = std::fs::read(phone.path()).unwrap();
    assert!(!db.windows(7).any(|w| w == b"CAMJPEG"));

    let mac = FakeMac::new("C1");
    let phone = Phone::new();
    let m = connected(&phone, &mac, "shared_relay");
    let e = m.mac_camera(None).unwrap_err();
    assert_eq!(e.code, ErrorCode::NeedsBetterPath);
    assert_eq!(
        e.message,
        "Your Mac's camera is available when connected directly or through your relay."
    );
    assert!(mac.lock().transfers.is_empty(), "nothing was asked");
}
