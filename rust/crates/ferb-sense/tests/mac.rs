//! The macOS sources, tested passively: nothing here types, clicks, moves focus or a Space, opens a window, writes
//! the pasteboard or makes macOS ask for a permission. A missing permission is an injected fact. The `#[ignore]`d
//! tests only read the Mac's current state (or watch a temporary folder) and are run by hand:
//! `cargo test --manifest-path rust/crates/ferb-sense/Cargo.toml --test mac -- --ignored --nocapture`.

#![cfg(target_os = "macos")]

use std::sync::{Arc, Mutex};

use ferb_sense::mac::apps::{
    app_of, focused_app, focused_title, normal_windows, parent_pid, MacFieldReads,
};
use ferb_sense::mac::streams::{MacFsEvents, MacTap};
use ferb_sense::mac::system::{
    displays_asleep, input_monitoring_granted, session, GeneralPasteboard,
};
use ferb_sense::manager::{tap_state, INPUT_MONITORING_OFF};
use ferb_sense::watchers::base::FsEvents;
use ferb_sense::watchers::clipboard::{access, Pasteboard};
use ferb_sense::watchers::owner_input::{OwnerInput, Tap};
use ferb_sense::watchers::text_input::FieldReads;

// decision: SENSE-405, SENSE-406
#[test]
fn without_input_monitoring_no_tap_is_created_and_the_tap_reads_as_down() {
    // the grant is an injected fact: false, so CGEventTapCreate is never called and macOS never prompts
    let oi = OwnerInput::new(|| 0.0, || 0.0, || false, Box::new(MacTap::default()));
    assert!(!oi.start_tap());
    assert!(!oi.is_running());
    assert_eq!(tap_state(&Tap(oi.clone())), INPUT_MONITORING_OFF);
    assert!(!oi.active() && oi.last_owner_wall().is_none());
}

#[test]
fn process_facts_of_this_test_process() {
    let me = i64::from(std::process::id());
    assert_eq!(
        parent_pid(me),
        Some(i64::from(std::os::unix::process::parent_id()))
    );
    assert_eq!(parent_pid(-5), None);
    assert!(app_of(me).is_none(), "a test binary is not an app");
}

#[test]
#[ignore = "reads the Mac's current state; run by hand"]
fn reads_the_current_state_passively() {
    eprintln!("frontmost app: {:?}", focused_app());
    if let Some(f) = focused_app() {
        eprintln!(
            "its sole window title: {:?}",
            focused_title(f.pid).map(|t| t.chars().count())
        );
    }
    eprintln!("normal windows on screen: {}", normal_windows().len());
    eprintln!(
        "session: locked {:?}, on console {:?}",
        session().locked,
        session().on_console
    );
    eprintln!("displays asleep: {:?}", displays_asleep());
    eprintln!("secure input: {}", MacFieldReads.secure_input());
    eprintln!("input monitoring granted: {}", input_monitoring_granted());
    let board = GeneralPasteboard;
    // change count, types and access state never make macOS ask; the contents are not read here
    eprintln!(
        "pasteboard: count {}, access {}, {} types",
        board.change_count(),
        access(board.access_behavior()),
        board.types().len()
    );
}

#[test]
#[ignore = "waits on a real FSEvents stream over a temporary folder; run by hand"]
fn fsevents_report_a_file_made_in_a_temporary_folder() {
    let dir = tempfile::tempdir().unwrap();
    let real = std::fs::canonicalize(dir.path())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let seen: Arc<Mutex<Vec<(String, u32)>>> = Arc::default();
    let s = seen.clone();
    let stream = MacFsEvents::default()
        .start(
            std::slice::from_ref(&real),
            Arc::new(move |p: &str, f: u32| s.lock().unwrap().push((p.to_string(), f))),
        )
        .expect("the stream starts");
    std::thread::sleep(std::time::Duration::from_millis(500));
    std::fs::write(dir.path().join("made.txt"), "x").unwrap();
    for _ in 0..50 {
        if seen
            .lock()
            .unwrap()
            .iter()
            .any(|(p, _)| p.ends_with("made.txt"))
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    drop(stream);
    let got = seen.lock().unwrap().clone();
    assert!(
        got.iter()
            .any(|(p, f)| p == &format!("{real}/made.txt") && f & 0x100 != 0),
        "{got:?}"
    );
}

mod trust {
    use ferb_sense::mac::apps::{trusted_with, TRUST_FRESH_S};
    use std::time::{Duration, Instant};

    // decision: SENSE-409
    #[test]
    fn trust_is_read_again_only_after_it_goes_stale() {
        let cache = std::sync::Mutex::new(None);
        let t0 = Instant::now();
        let mut reads = 0;
        assert!(!trusted_with(&cache, t0, || {
            reads += 1;
            false
        }));
        // within the window the last reading serves, whatever the API would say now
        assert!(!trusted_with(
            &cache,
            t0 + Duration::from_millis(500),
            || {
                reads += 1;
                true
            }
        ));
        assert_eq!(reads, 1);
        // past it, read again: the owner's Allow is seen within TRUST_FRESH_S
        let later = t0 + Duration::from_secs_f64(TRUST_FRESH_S + 0.1);
        assert!(trusted_with(&cache, later, || {
            reads += 1;
            true
        }));
        assert_eq!(reads, 2);
    }
}
