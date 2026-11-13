//! The ownership and waiting rules the Python code embodies, on fakes (tests/test_audio_listen.py,
//! tests/test_recording.py, tests/test_mac_hands.py's cases): a completion outlives a waiter that gave up; one
//! shared Automation check; ordered resource locks; foreground input yields to the owner; a held button is released
//! however a gesture ends; FERB quits only an app it opened. No real time where a paused clock does.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use ferb_mac_hands::apps::RunningApp;
use ferb_mac_hands::error::{HandError, Result};
use ferb_mac_hands::input::{RealInput, Step};
use ferb_mac_hands::launched::{AppControl, LaunchedApps, KEPT_UNSAVED};
use ferb_mac_hands::listen::{Capture, Hearing, Running, Transcribe};
use ferb_mac_hands::locks::{GuiLock, ResourceLocks};
use ferb_mac_hands::permissions::{AutomationChecks, Status};
use serde_json::{json, Map, Value};

/// A gate a fake native call waits on.
#[derive(Clone, Default)]
struct Gate(Arc<(Mutex<bool>, Condvar)>);
impl Gate {
    fn open(&self) {
        *self.0 .0.lock().unwrap() = true;
        self.0 .1.notify_all();
    }
    fn wait(&self) {
        let g = self.0 .0.lock().unwrap();
        let _g = self.0 .1.wait_while(g, |open| !*open).unwrap();
    }
}

// ---------- Hearing ----------

struct FakeRunning {
    stopped: Arc<AtomicUsize>,
}
impl Running for FakeRunning {
    fn started(&self) -> Option<f64> {
        Some(0.0)
    }
    fn stop(&self) -> Map<String, Value> {
        self.stopped.fetch_add(1, Ordering::SeqCst);
        json!({"frames": 16000, "sample_rate": 16000, "audio_s": 1.0, "peak": 0.5})
            .as_object()
            .unwrap()
            .clone()
    }
}

struct FakeCapture {
    gate: Gate,
    stopped: Arc<AtomicUsize>,
}
impl Capture for FakeCapture {
    fn start(&self, _b: &str, _pid: i32, path: &Path) -> Result<Arc<dyn Running>> {
        std::fs::write(path, b"RIFF").unwrap();
        self.gate.wait();
        Ok(Arc::new(FakeRunning {
            stopped: self.stopped.clone(),
        }))
    }
}

struct FakeTranscribe;
impl Transcribe for FakeTranscribe {
    fn transcribe<'a>(
        &'a self,
        _p: &'a Path,
        language: Option<&'a str>,
        _a: f64,
        offset: f64,
    ) -> ferb_mac_hands::listen::BoxFut<'a, Result<Map<String, Value>>> {
        Box::pin(async move {
            Ok(json!({"transcript": "hello there", "segments": [{"start": offset, "end": offset + 1.0, "text": "hello there"}],
                      "language": language.unwrap_or("en-US")})
            .as_object()
            .unwrap()
            .clone())
        })
    }
}

fn hearing(dir: &Path, gate: Gate, stopped: Arc<AtomicUsize>) -> Arc<Hearing> {
    let n = Arc::new(AtomicUsize::new(0));
    Hearing::new(
        dir.join("audio"),
        Arc::new(FakeCapture { gate, stopped }),
        Arc::new(FakeTranscribe),
        Arc::new(|s: &str| s.to_string()),
        Some(Arc::new(|name: &str| {
            if name == "Music" {
                Ok(("com.apple.Music".to_string(), 42))
            } else {
                Err(HandError::Refused(format!(
                    "{name} isn't running (open it first)"
                )))
            }
        })),
        Arc::new(|| 0.0),
        Arc::new(move || format!("listen-{:012}", n.fetch_add(1, Ordering::SeqCst))),
    )
}

// decision: HAND-401
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_listen_hears_transcribes_and_deletes_its_sound() {
    let dir = tempfile::tempdir().unwrap();
    let (gate, stopped) = (Gate::default(), Arc::new(AtomicUsize::new(0)));
    gate.open();
    let h = hearing(dir.path(), gate, stopped.clone());
    let out = h
        .listen("t1.worker", "com.apple.Music", 42, 0.01, Some("en-GB"))
        .await
        .unwrap();
    assert_eq!(out["transcript"], json!("hello there"));
    assert_eq!(out["on_device"], json!(true));
    assert_eq!(out["language"], json!("en-GB"));
    assert_eq!(out["audio_s"], json!(1.0));
    assert_eq!(stopped.load(Ordering::SeqCst), 1);
    assert!(
        std::fs::read_dir(dir.path().join("audio"))
            .unwrap()
            .next()
            .is_none(),
        "the sound file goes once transcribed"
    );
    assert!(h.listening.lock().unwrap().is_empty());
}

// listen.py:466 (a stop during ScreenCaptureKit's start still stops the stream once it exists)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_listen_given_up_mid_start_is_stopped_once_it_starts_and_its_sound_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let (gate, stopped) = (Gate::default(), Arc::new(AtomicUsize::new(0)));
    let h = hearing(dir.path(), gate.clone(), stopped.clone());
    let waited = tokio::time::timeout(
        std::time::Duration::from_millis(50),
        h.listen("t1.worker", "com.apple.Music", 42, 5.0, None),
    )
    .await;
    assert!(
        waited.is_err(),
        "the waiter gave up while the stream was starting"
    );
    assert_eq!(stopped.load(Ordering::SeqCst), 0);
    gate.open();
    for _ in 0..200 {
        if stopped.load(Ordering::SeqCst) == 1 && h.listening.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(
        stopped.load(Ordering::SeqCst),
        1,
        "the started stream was stopped"
    );
    assert!(h.listening.lock().unwrap().is_empty());
    h.shutdown().await;
    assert!(
        std::fs::read_dir(dir.path().join("audio"))
            .unwrap()
            .next()
            .is_none(),
        "its sound was deleted"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_task_ending_stops_its_listen_untranscribed() {
    let dir = tempfile::tempdir().unwrap();
    let (gate, stopped) = (Gate::default(), Arc::new(AtomicUsize::new(0)));
    gate.open();
    let h = hearing(dir.path(), gate, stopped.clone());
    let h2 = h.clone();
    let job = tokio::spawn(async move {
        h2.listen("t1.worker", "com.apple.Music", 42, 30.0, None)
            .await
    });
    for _ in 0..200 {
        if !h.listening.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let ended = h.end_task("t1.worker").await;
    assert_eq!(
        ended,
        vec![json!({"resource": "listen", "app": "com.apple.Music", "released": true})]
    );
    let out = job.await.unwrap().unwrap();
    assert_eq!(out["error"], json!("listening stopped: the task ended"));
    assert!(out.get("transcript").is_none());
    assert_eq!(stopped.load(Ordering::SeqCst), 1);
}

// ---------- the Automation check ----------

// permissions.py:181-221: one native check per (target, mode); a waiter that gives up stops waiting, not the check
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_shared_automation_check_outlives_a_waiter_that_gave_up() {
    let gate = Gate::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let (g2, c2) = (gate.clone(), calls.clone());
    let checks = AutomationChecks::new(Arc::new(move |_b: &str, _ask: bool| {
        c2.fetch_add(1, Ordering::SeqCst);
        g2.wait();
        Status::Denied
    }));
    let gave_up = tokio::time::timeout(
        std::time::Duration::from_millis(30),
        checks.check("com.apple.Notes", false),
    )
    .await;
    assert!(gave_up.is_err());
    assert_eq!(checks.in_flight(), 1, "the native check goes on");
    let c3 = checks.clone();
    let later = tokio::spawn(async move { c3.check("com.apple.Notes", false).await });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    gate.open();
    assert_eq!(later.await.unwrap(), Status::Denied);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the later caller shared the one call"
    );
    assert_eq!(checks.in_flight(), 0);
}

// ---------- locks ----------

#[tokio::test]
async fn locks_are_taken_in_one_order_and_a_bystander_never_queues() {
    let locks = ResourceLocks::new();
    let keys = vec![
        "tab:1".to_string(),
        "app:b".to_string(),
        "account:a".to_string(),
    ];
    let held = locks.hold(&keys, "w1", false).await.unwrap();
    assert_eq!(locks.holder("app:b"), Some("w1".into()));
    assert!(
        locks
            .try_hold(&["app:b".to_string()], "preview", true)
            .unwrap()
            .is_none(),
        "a writer's key is busy"
    );
    drop(held);
    assert_eq!(locks.holder("app:b"), None);
    let reading = locks
        .hold(&["app:b".to_string()], "w1", true)
        .await
        .unwrap();
    let look = locks
        .try_hold(&["app:b".to_string()], "preview", true)
        .unwrap();
    assert!(look.is_some(), "a bystander looks alongside a read");
    drop(look);
    assert!(locks
        .try_hold(&["app:b".to_string()], "preview", false)
        .unwrap()
        .is_none());
    drop(reading);
    assert!(locks
        .hold(&["bad:x".to_string()], "w1", false)
        .await
        .is_err());
    let gui = GuiLock::new();
    let g = gui.hold("w1").await;
    assert_eq!(gui.holder(), Some("w1".into()));
    drop(g);
    assert_eq!(gui.holder(), None);
}

// ---------- input ----------

fn input(owner: Vec<bool>) -> RealInput {
    let owner = Arc::new(Mutex::new(owner));
    let mut ri = RealInput::new(
        Arc::new(move || {
            let mut o = owner.lock().unwrap();
            if o.is_empty() {
                false
            } else {
                o.remove(0)
            }
        }),
        GuiLock::new(),
    );
    ri.sleep = Arc::new(|_| Box::pin(async {}));
    ri.yield_after_s = 0.2;
    ri.poll_s = 0.1;
    ri
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreground_input_yields_when_the_owner_takes_over() {
    let posted = Arc::new(Mutex::new(vec![]));
    let steps: Vec<Step> = (0..3)
        .map(|i| {
            let p = posted.clone();
            Step::new(format!("s{i}"), move || {
                p.lock().unwrap().push(i);
                Ok(())
            })
        })
        .collect();
    let err = input(vec![false, true])
        .run(steps, "w1", "typing")
        .await
        .unwrap_err();
    assert_eq!(
        err,
        HandError::Yielded {
            done: 1,
            total: 3,
            what: "typing".into()
        }
    );
    assert!(err.outcome_unknown());
    assert_eq!(*posted.lock().unwrap(), vec![0]);
    // quiet: the owner busy longer than yield_after_s
    assert!(matches!(
        input(vec![true; 10]).quiet("w1", "click").await,
        Err(HandError::Yielded { .. })
    ));
    assert!(input(vec![true, true, false])
        .quiet("w1", "click")
        .await
        .is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_held_button_is_released_once_however_the_gesture_ends() {
    let released = Arc::new(AtomicUsize::new(0));
    let r2 = released.clone();
    let cleanup: ferb_mac_hands::input::Cleanup = Arc::new(move || {
        r2.fetch_add(1, Ordering::SeqCst);
    });
    let steps = vec![
        Step::new("down", || Ok(())),
        Step::new("drag", || Err(HandError::StaleRef("moved".into()))),
        Step::new("up", || Ok(())),
    ];
    assert!(input(vec![])
        .to_process(steps, "w1", Some(cleanup.clone()), None)
        .await
        .is_err());
    assert_eq!(released.load(Ordering::SeqCst), 1);
    let steps = vec![Step::new("down", || Ok(())), Step::new("up", || Ok(()))];
    input(vec![])
        .to_process(steps, "w1", Some(cleanup.clone()), None)
        .await
        .unwrap();
    assert_eq!(released.load(Ordering::SeqCst), 2);
    // dropped mid-way (a stop): still released
    let gate = Gate::default();
    let g2 = gate.clone();
    let steps = vec![
        Step::new("down", || Ok(())),
        Step::new("wait", move || {
            g2.wait();
            Ok(())
        }),
    ];
    let ri = input(vec![]);
    let gone = tokio::time::timeout(
        std::time::Duration::from_millis(30),
        ri.to_process(steps, "w1", Some(cleanup), None),
    )
    .await;
    assert!(gone.is_err());
    assert_eq!(released.load(Ordering::SeqCst), 3);
    gate.open();
}

// ---------- launched apps (HAND-200) ----------

struct Control {
    unsaved: Option<bool>,
    quit: Arc<Mutex<Vec<i32>>>,
    running: Vec<RunningApp>,
}
impl AppControl for Control {
    fn running(&self) -> Vec<RunningApp> {
        self.running.clone()
    }
    fn unsaved(&self, _pid: i32) -> Option<bool> {
        self.unsaved
    }
    fn quit(&self, pid: i32, _b: &str) -> bool {
        self.quit.lock().unwrap().push(pid);
        true
    }
    fn hide(&self, _pid: i32, _b: &str) -> bool {
        true
    }
}

// decision: HAND-200
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ferb_quits_only_an_app_it_opened_and_only_without_unsaved_work() {
    let pages = RunningApp::new("com.apple.iWork.Pages", "Pages", 40, false);
    let quit = Arc::new(Mutex::new(vec![]));
    let make = |unsaved| {
        LaunchedApps::new(
            Arc::new(Control {
                unsaved,
                quit: quit.clone(),
                running: vec![pages.clone()],
            }),
            Arc::new(|_: &str, _: &ferb_mac_hands::launched::Launched| false),
            Arc::new(|| "2026-01-01T00:00:00+00:00".to_string()),
        )
    };
    let apps = make(Some(false));
    assert!(
        matches!(apps.quit_now("t1.worker", &pages).await, Err(HandError::Refused(m)) if m.contains("isn't an app FERB opened"))
    );
    apps.launched("t1.worker", "com.apple.iWork.Pages", "Pages", 40);
    assert!(
        apps.quit_now("t1.step2", &pages)
            .await
            .unwrap()
            .starts_with("asked Pages to quit"),
        "the tree's app, from any of its tasks"
    );
    assert_eq!(*quit.lock().unwrap(), vec![40]);
    let apps = make(Some(true));
    apps.launched("t2.worker", "com.apple.iWork.Pages", "Pages", 40);
    assert!(
        matches!(apps.quit_now("t2.worker", &pages).await, Err(HandError::Refused(m)) if m.ends_with(KEPT_UNSAVED))
    );
    let released = apps.release("t2.worker").await;
    assert_eq!(
        released,
        vec![
            json!({"resource": "app", "app": "Pages", "bundle_id": "com.apple.iWork.Pages", "released": true, "closed": false, "left_running": KEPT_UNSAVED})
        ]
    );
    assert_eq!(ferb_mac_hands::launched::left_running(&released), vec!["Pages is still open: FERB opened it for this task and left it running because it has unsaved changes.".to_string()]);
}

// ---------- recording text ----------

#[test]
fn the_recording_timeline_and_encode_keep_slow_captures_on_their_times() {
    let t = ferb_mac_hands::recording::timeline(&[0.0, 0.5, 0.52], 1.5);
    assert_eq!(t, "ffconcat version 1.0\nfile '00000000.jpg'\nduration 0.500000\nfile '00000001.jpg'\nduration 0.040000\nfile '00000002.jpg'\nduration 0.980000\nfile '00000002.jpg'\n");
    let args = ferb_mac_hands::recording::encode_args(
        "/opt/homebrew/bin/ffmpeg",
        "/x/frames.txt",
        1.5,
        "/x/video.mp4",
    );
    assert!(
        args.contains(&"fps=25,tpad=stop_mode=clone:stop_duration=1.5".to_string())
            || args.iter().any(|a| a.ends_with("stop_duration=1.5"))
    );
    assert_eq!(
        args[args.len() - 4..],
        [
            "1.5".to_string(),
            "-movflags".into(),
            "+faststart".into(),
            "/x/video.mp4".into()
        ]
    );
    let dir = tempfile::tempdir().unwrap();
    assert!(
        matches!(ferb_mac_hands::recording::executable("ffmpeg-not-here", Some(dir.path().to_str().unwrap()), dir.path().to_str().unwrap()), Err(HandError::Refused(m)) if m.contains("needs ffmpeg and ffprobe"))
    );
}

// ---------- the evidence store (store.py) ----------

#[cfg(target_os = "macos")]
#[test]
fn evidence_is_painted_before_it_is_written_and_never_a_file_without_its_row() {
    use ferb_mac_hands::evidence::{EvidenceStore, UNMASKABLE};
    use ferb_mac_hands::ocr::Paint;
    struct Painter(bool);
    impl Paint for Painter {
        fn paint(
            &self,
            image: &[u8],
            boxes: &[ferb_mac_hands::mask::ImgBox],
        ) -> std::result::Result<Vec<u8>, String> {
            if self.0 {
                Ok([image, format!(" painted {}", boxes.len()).as_bytes()].concat())
            } else {
                Err("not an image Quartz can decode".into())
            }
        }
    }
    let db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../ferb/store/migrations/0020_evidence.sql"),
        )
        .unwrap(),
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let n = Arc::new(AtomicUsize::new(0));
    let make = |ok: bool| {
        let n = n.clone();
        EvidenceStore::new(
            &dir.path().join("evidence"),
            Arc::new(|| {
                (
                    "2026-01-02T03:04:05+00:00".to_string(),
                    "2026-01-02".to_string(),
                )
            }),
            Arc::new(move || format!("ev{}", n.fetch_add(1, Ordering::SeqCst))),
            Arc::new(|v: &Value| v.clone()),
            Arc::new(Painter(ok)),
        )
        .unwrap()
    };
    let store = make(true);
    let id = store
        .put(
            &db,
            "t1",
            None,
            "screenshot",
            Some(b"jpeg"),
            None,
            &[(1, 2, 3, 4)],
            "image/jpeg",
            "app",
        )
        .unwrap();
    let (meta, bytes) = store.read(&db, &id).unwrap().unwrap();
    assert_eq!(bytes.unwrap(), b"jpeg painted 1");
    assert_eq!(meta["has_image"], json!(true));
    let unpaintable = make(false);
    let id2 = unpaintable
        .put(
            &db,
            "t1",
            None,
            "screenshot",
            Some(b"jpeg"),
            Some(Map::new()),
            &[(1, 2, 3, 4)],
            "image/jpeg",
            "app",
        )
        .unwrap();
    let (meta, bytes) = unpaintable.read(&db, &id2).unwrap().unwrap();
    assert!(bytes.is_none());
    assert_eq!(meta["data"]["image_withheld"], json!(UNMASKABLE));
    assert_eq!(store.list(&db, "t1").unwrap().len(), 2);
    assert_eq!(store.purge_before(&db, "2027-01-01").unwrap(), 2);
    assert!(store.list(&db, "t1").unwrap().is_empty());
    assert!(
        std::fs::read_dir(dir.path().join("evidence"))
            .unwrap()
            .next()
            .is_none(),
        "empty day folders go"
    );
}
