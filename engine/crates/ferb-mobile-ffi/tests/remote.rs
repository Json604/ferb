//! T-1221's calls as an app calls them through the exported objects (the fake Mac behind an app-supplied
//! `Transport`): the lock and Stop everything, the status with each question's needs, a message with files, files both
//! ways with progress and a cancelled transfer as its own error, the live preview's stream with its pace and its end on
//! the public relay, settings signed, the Mac's tasks and name, keys registered after pairing; and every new call is
//! `closed` after `shutdown`.

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{FakeMac, ForeignMac, Heard, Signs, config, online, open};
use ferb_mobile_ffi::records::{Attachment, Change, PreviewFrame};
use ferb_mobile_ffi::{FerbError, Mobile, PreviewSink, TransferListener};
use serde_json::{Value, json};

fn connected(mac: &FakeMac, dir: &std::path::Path, tier: &str) -> (Arc<Mobile>, Arc<Heard>) {
    let heard = Arc::new(Heard::default());
    let m = open(dir, config("phone-1"), heard.clone()).unwrap();
    m.connect_transport(ForeignMac::new(mac.clone())).unwrap();
    m.link_event(online(tier)).unwrap();
    m.sync().unwrap();
    (m, heard)
}

#[derive(Default)]
struct Moves(Mutex<Vec<(u64, u64)>>, Mutex<Vec<String>>);
impl TransferListener for Moves {
    fn progress(&self, done: u64, of: u64) {
        self.0.lock().unwrap().push((done, of));
    }
    fn accepted(&self, upload_id: String, _offset: u64) {
        self.1.lock().unwrap().push(upload_id);
    }
}

#[derive(Default)]
struct Watched {
    frames: Mutex<Vec<PreviewFrame>>,
    ended: Mutex<Option<(Option<String>, Option<String>)>>,
}
impl PreviewSink for Watched {
    fn frame(&self, frame: PreviewFrame) {
        self.frames.lock().unwrap().push(frame);
    }
    fn ended(&self, code: Option<String>, detail: Option<String>) {
        *self.ended.lock().unwrap() = Some((code, detail));
    }
}

fn wait(seconds: u64, f: impl Fn() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    f()
}

/// The lock, Stop everything and the status as the app reads them; a pushed status is a `Change.Status`.
// decision: PHN-15
// decision: PHN-125
// decision: PHN-417
// decision: PHN-510
// decision: PHN-511
#[test]
fn lock_stop_and_status_through_the_bindings() {
    let mac = FakeMac::new("C1");
    mac.lock().status = json!({"approvals": [
        {"approval_id": "q1", "kind": "approve", "reason": "Open it?", "options": ["Yes", "No"],
         "phone": {"can_answer": true, "needs_unlock": false}},
        {"approval_id": "q2", "kind": "input", "reason": "FERB needs a private answer", "options": [], "held": true,
         "phone": {"can_answer": false, "hidden": true, "error": {"message": "Answer this on the Mac."}}}],
        "running": [{"task_id": "t1", "name": "Desks", "goal": "Compare desks"}], "results": [], "stopped": false});
    let dir = tempfile::tempdir().unwrap();
    let (m, heard) = connected(&mac, dir.path(), "direct");
    let st = m.status().unwrap();
    assert!(st.questions[0].needs.can_answer && !st.questions[0].needs.needs_unlock);
    assert!(!st.questions[1].needs.can_answer && st.questions[1].held);
    assert_eq!(
        st.questions[1].needs.problem.as_deref(),
        Some("Answer this on the Mac.")
    );
    assert_eq!(m.last_status().unwrap(), Some(st));
    heard.0.lock().unwrap().clear();
    m.link_event(
        json!({"type": "status", "status": {"approvals": [], "stopped": true}}).to_string(),
    )
    .unwrap();
    assert!(heard.0.lock().unwrap().contains(&Change::Status));
    assert!(m.last_status().unwrap().unwrap().stopped);
    assert!(m.lock().unwrap());
    assert!(m.link_state().unwrap().locked);
    assert!(matches!(m.sync(), Err(FerbError::NeedsUnlock { .. })));
    m.stop_everything().unwrap();
    assert_eq!(mac.calls_of("stop.engage").len(), 1);
    m.unlocked().unwrap();
    m.sync().unwrap();
    assert!(!m.link_state().unwrap().locked && m.link_state().unwrap().watching);
}

/// Files both ways as the app drives them: a listing, a fetch with progress, a pause that is its own error and the
/// fetch going on, an upload's id riding with a message.
// decision: PHN-113
// decision: PHN-115
// decision: PHN-127
// decision: PHN-513
#[test]
fn files_both_ways_through_the_bindings() {
    let mac = FakeMac::new("C1");
    mac.lock().folders.insert(
        "~/Documents".into(),
        json!({"path": "/Users/o/Documents", "parent": "/Users/o", "high_value": false, "total": 1, "cut": false,
               "entries": [{"name": "Lease.pdf", "dir": false, "link": false, "size": 6, "modified": 1, "locked": false}]}),
    );
    mac.lock()
        .blobs
        .insert("/Users/o/Documents/Lease.pdf".into(), b"%PDF-1".to_vec());
    mac.lock().pause_after = Some(2);
    let dir = tempfile::tempdir().unwrap();
    let (m, _) = connected(&mac, dir.path(), "direct");
    let folder = m.files(Some("~/Documents".into()), None).unwrap();
    assert_eq!(folder.entries[0].path, "/Users/o/Documents/Lease.pdf");
    let save = dir.path().join("Lease.pdf").to_string_lossy().into_owned();
    let moves = Arc::new(Moves::default());
    let paused = m
        .fetch(
            "f1".into(),
            folder.entries[0].path.clone(),
            save.clone(),
            0,
            None,
            moves.clone(),
        )
        .unwrap_err();
    assert_eq!(paused.code(), "cancelled");
    assert!(matches!(paused, FerbError::Cancelled { .. }));
    let got = m
        .fetch(
            "f2".into(),
            folder.entries[0].path.clone(),
            save.clone(),
            2,
            None,
            moves.clone(),
        )
        .unwrap();
    assert_eq!((got.received, got.total), (4, 6));
    assert_eq!(std::fs::read(&save).unwrap(), b"%PDF-1");
    assert_eq!(moves.0.lock().unwrap().last(), Some(&(6, 6)));
    let local = dir.path().join("photo.jpg");
    std::fs::write(&local, b"jpg").unwrap();
    let up = m
        .upload(
            "u1".into(),
            local.to_string_lossy().into_owned(),
            "photo.jpg".into(),
            None,
            None,
            None,
            moves.clone(),
        )
        .unwrap();
    assert_eq!(
        *moves.1.lock().unwrap(),
        std::slice::from_ref(&up.upload_id)
    );
    let item = m
        .send_with(
            "here".into(),
            vec![Attachment {
                upload_id: up.upload_id.clone(),
                name: "photo.jpg".into(),
            }],
        )
        .unwrap();
    assert_eq!(item.attachments[0].upload_id, up.upload_id);
    assert!(wait(5, || !mac.lock().sends.is_empty()));
    assert_eq!(mac.lock().sends[0]["attachments"], json!([up.upload_id]));
}

/// The live preview as a stream: frames at the link's pace (never two requests within its 200 ms), `have` naming
/// the frame shown, `stop` ending it; on the public relay it ends by itself with `needs_better_path`, asking nothing.
// decision: PHN-454
// decision: PHN-444
// decision: PHN-512
#[test]
fn the_live_preview_streams_at_the_link_s_pace_and_never_on_the_public_relay() {
    let mac = FakeMac::new("C1");
    mac.lock().frames.push_back((
        json!({"seq": 1, "state": "working", "fresh": true, "line": "Reading"}),
        b"\xff\xd8one".to_vec(),
    ));
    mac.lock().frames.push_back((
        json!({"seq": 2, "state": "working", "fresh": true}),
        b"\xff\xd8two".to_vec(),
    ));
    let dir = tempfile::tempdir().unwrap();
    let (m, _) = connected(&mac, dir.path(), "direct");
    let sink = Arc::new(Watched::default());
    let started = Instant::now();
    let live = m.watch_live("t1".into(), sink.clone()).unwrap();
    assert!(wait(5, || sink.frames.lock().unwrap().len() >= 3));
    live.stop();
    assert!(wait(5, || sink.ended.lock().unwrap().is_some()));
    assert_eq!(*sink.ended.lock().unwrap(), Some((None, None)));
    let frames = sink.frames.lock().unwrap().clone();
    assert_eq!(frames[0].jpeg.as_deref(), Some(&b"\xff\xd8one"[..]));
    assert_eq!(frames[0].line.as_deref(), Some("Reading"));
    assert_eq!(frames[1].seq, Some(2));
    let asked = mac.lock().transfers.clone();
    assert_eq!(asked[1].1["have"], 1);
    assert_eq!(asked[2].1["have"], 2);
    assert!(
        started.elapsed() >= Duration::from_millis(400),
        "three requests take at least two of the link's 200 ms gaps"
    );
    // the public relay: ends at once, asks nothing
    let dir2 = tempfile::tempdir().unwrap();
    let mac2 = FakeMac::new("C1");
    let (m2, _) = connected(&mac2, dir2.path(), "shared_relay");
    let sink2 = Arc::new(Watched::default());
    let _live2 = m2.watch_live("t1".into(), sink2.clone()).unwrap();
    assert!(wait(5, || sink2.ended.lock().unwrap().is_some()));
    assert_eq!(
        sink2.ended.lock().unwrap().as_ref().unwrap().0.as_deref(),
        Some("needs_better_path")
    );
    assert!(mac2.lock().transfers.is_empty());
    assert!(sink2.frames.lock().unwrap().is_empty());
}

/// Settings signed, the Mac's tasks, its name kept, keys registered after pairing.
// decision: PHN-404
// decision: PHN-133
#[test]
fn settings_tasks_the_mac_and_late_keys_through_the_bindings() {
    let mac = FakeMac::new("C1");
    mac.lock()
        .settings
        .insert("retention.trace_days".into(), json!(30));
    mac.lock().mac_name = Some("Studio".into());
    let dir = tempfile::tempdir().unwrap();
    let (m, _) = connected(&mac, dir.path(), "direct");
    assert_eq!(m.settings().unwrap()[0].value_json, "30");
    let signer = Arc::new(Signs::default());
    assert_eq!(
        m.set_setting("retention.trace_days".into(), "14".into(), signer.clone())
            .unwrap(),
        "14"
    );
    let signed: Value = serde_json::from_str(&signer.0.lock().unwrap()[0]).unwrap();
    assert_eq!(signed["method"], "settings.set");
    assert_eq!(m.tasks(20, None).unwrap().tasks.len(), 2);
    assert_eq!(m.mac_info().unwrap().name.as_deref(), Some("Studio"));
    assert_eq!(
        m.cached_mac_info().unwrap().unwrap().name.as_deref(),
        Some("Studio")
    );
    m.register_keys(Some("DEV".into()), None, signer.clone())
        .unwrap();
    assert_eq!(mac.calls_of("link.keys")[0], json!({"device_pub": "DEV"}));
    // after shutdown every new call is closed
    m.shutdown();
    let closed = |r: Result<(), FerbError>| matches!(r, Err(FerbError::Closed { .. }));
    assert!(closed(m.lock().map(|_| ())));
    assert!(closed(m.stop_everything()));
    assert!(closed(m.status().map(|_| ())));
    assert!(closed(m.last_status().map(|_| ())));
    assert!(closed(m.files(None, None).map(|_| ())));
    assert!(closed(m.settings().map(|_| ())));
    assert!(closed(m.tasks(1, None).map(|_| ())));
    assert!(closed(m.mac_info().map(|_| ())));
    assert!(closed(m.cancel_transfer("x".into())));
    assert!(closed(m.mark_read_latest("C1".into())));
    assert!(closed(
        m.watch_live("t".into(), Arc::new(Watched::default()))
            .map(|_| ())
    ));
}
