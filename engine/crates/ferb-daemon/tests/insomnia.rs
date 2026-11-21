//! T-1274: insomnia mode in the daemon (APP-980..APP-984), with stand-ins for the power assertions, the power facts,
//! the lock, the work, the owner's unlock and the clocks. What the Mac itself does with the holds (macOS lets each go
//! when its lease runs out or the daemon dies) is IOKit's; here every hold is checked to be a lease.

mod insomnia_support;

use ferb_core::power::{Hold, PowerEvent, Thermal};
use ferb_daemon::insomnia::{
    Ended, Saved, Variant, CAMERA_KEEP_S, CAMERA_TOPIC, EPISODE_S, LEASE_S, REASON_MODE,
    REASON_TASK, SHIELD_LOST_S, STEP_TOPIC, TICK_S, TOPIC,
};
use insomnia_support::*;
use serde_json::{json, Map, Value};

fn report(ins: &ferb_daemon::insomnia::Insomnia, v: Value) {
    let p: Map<String, Value> = v.as_object().unwrap().clone();
    ins.report(&p).unwrap();
}

fn shield_up(ins: &ferb_daemon::insomnia::Insomnia) {
    report(
        ins,
        json!({"event": "shield", "state": "up", "camera": "allowed"}),
    );
}

// ---- holds and the lease -----------------------------------------------------------------------------------------

/// The mode holds idle sleep (and on AC any sleep, which covers a closed lid while plugged in), each a lease of
/// LEASE_S renewed at every look; the display only while FERB.app says its cover is up; the end lets everything go.
// decision: DMN-603
// decision: APP-980
// decision: APP-981
#[test]
fn the_mode_holds_on_a_renewed_lease_and_the_display_only_under_the_cover() {
    let (mac, ins) = insomnia();
    *mac.working.lock().unwrap() = 1;
    ins.start(Variant::Shield, true, "mac");
    assert_eq!(mac.held_now(), vec![Hold::IdleSleep, Hold::SystemSleep]);
    assert!(
        !mac.held_now().contains(&Hold::Display),
        "no display hold before the cover is up"
    );
    shield_up(&ins);
    assert_eq!(
        mac.held_now(),
        vec![Hold::IdleSleep, Hold::SystemSleep, Hold::Display]
    );
    let before = mac.holds.lock().unwrap().len();
    mac.advance(TICK_S as f64);
    ins.look();
    assert!(
        mac.holds.lock().unwrap().len() >= before + 3,
        "every hold renewed"
    );
    for (_, reason, lease) in mac.holds.lock().unwrap().iter() {
        assert_eq!(
            *lease, LEASE_S,
            "a hung or dead daemon holds nothing past the lease"
        );
        assert_eq!(
            reason, REASON_MODE,
            "generic words only, never a task's title"
        );
    }
    // on battery: system sleep stays held for the whole mode (macOS ignores it there; plugged in, it's already held)
    mac.set(|r| r.on_battery = Some(true));
    ins.look();
    assert_eq!(
        mac.held_now(),
        vec![Hold::IdleSleep, Hold::SystemSleep, Hold::Display]
    );
    // the cover stops saying it is up: the display hold goes, so an unlocked Mac sleeps and locks as usual
    mac.advance(SHIELD_LOST_S + 1.0);
    ins.look();
    assert_eq!(mac.held_now(), vec![Hold::IdleSleep, Hold::SystemSleep]);
    // the end: back to the task rule (a task runs, the lid is open: idle sleep only)
    assert!(ins.end(Ended::TurnedOff, "mac"));
    assert_eq!(mac.held_now(), vec![Hold::IdleSleep]);
    *mac.working.lock().unwrap() = 0;
    ins.look();
    assert_eq!(mac.held_now(), Vec::<Hold>::new());
}

/// Without the mode: while any task runs and the lid is open, idle sleep only; never the display or system sleep.
// decision: DMN-603
// decision: APP-980
#[test]
fn a_running_task_holds_idle_sleep_only_with_the_lid_open() {
    let (mac, ins) = insomnia();
    ins.look();
    assert!(mac.held_now().is_empty());
    *mac.working.lock().unwrap() = 2;
    ins.look();
    assert_eq!(mac.held_now(), vec![Hold::IdleSleep]);
    assert_eq!(ins.status()["task_awake"], true);
    assert!(mac
        .holds
        .lock()
        .unwrap()
        .iter()
        .all(|(h, r, l)| *h == Hold::IdleSleep && r == REASON_TASK && *l == LEASE_S));
    mac.set(|r| r.lid_closed = Some(true));
    ins.look();
    assert!(
        mac.held_now().is_empty(),
        "lid closed: macOS decides as usual"
    );
    mac.set(|r| r.lid_closed = Some(false));
    ins.look();
    *mac.working.lock().unwrap() = 0;
    ins.look();
    assert!(mac.held_now().is_empty(), "released when the tasks end");
    assert!(!mac
        .holds
        .lock()
        .unwrap()
        .iter()
        .any(|(h, _, _)| *h != Hold::IdleSleep));
}

// ---- warnings, never an end --------------------------------------------------------------------------------------

/// Low battery, a very low battery and heat are warned once each (the Mac's notice and the phone's status), and
/// nothing ends or changes the holds; a warning is gone when its cause is.
// decision: DMN-604
// decision: APP-981
#[test]
fn battery_and_heat_warn_and_nothing_ends() {
    let (mac, ins) = insomnia();
    *mac.working.lock().unwrap() = 1;
    ins.start(Variant::Locked, false, "mac");
    mac.set(|r| {
        r.on_battery = Some(true);
        r.battery = Some(18);
    });
    ins.look();
    ins.look();
    let st = ins.status();
    assert_eq!(st["on"], true);
    assert_eq!(st["warnings"].as_array().unwrap().len(), 1);
    assert_eq!(st["warnings"][0]["kind"], "battery_low");
    let told = mac.told_texts();
    assert_eq!(
        told.iter().filter(|t| t.contains("18%")).count(),
        1,
        "said once: {told:?}"
    );
    // very low: the urgent one replaces it; heat comes too
    mac.set(|r| {
        r.battery = Some(6);
        r.thermal = Some(Thermal::Critical);
    });
    ins.look();
    let kinds: Vec<Value> = ins.status()["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["kind"].clone())
        .collect();
    assert_eq!(kinds, vec![json!("battery_urgent"), json!("hot")]);
    assert_eq!(ins.status()["on"], true, "never ends on battery or heat");
    assert!(mac.held_now().contains(&Hold::IdleSleep));
    for t in mac.told_texts() {
        assert!(!t.contains("AI"), "{t}");
    }
    // plugged in and cool: the warnings are gone (the phone removes them)
    mac.set(|r| {
        r.on_battery = Some(false);
        r.thermal = Some(Thermal::Fair);
    });
    ins.look();
    assert!(ins.status()["warnings"].as_array().unwrap().is_empty());
    // a later drop warns again, with a new id
    mac.set(|r| {
        r.on_battery = Some(true);
        r.battery = Some(19);
    });
    ins.look();
    assert_eq!(ins.status()["warnings"][0]["kind"], "battery_low");
    // the owner's thresholds are his settings
    mac.settings.lock().unwrap().battery_warn = 15;
    ins.look();
    assert!(ins.status()["warnings"].as_array().unwrap().is_empty());
}

/// What macOS forced (the Mac slept anyway) is said plainly once, with the cause FERB last saw.
// decision: DMN-604
// decision: APP-981
#[test]
fn a_sleep_macos_forced_is_said_plainly() {
    let (mac, ins) = insomnia();
    *mac.working.lock().unwrap() = 1;
    ins.start(Variant::Locked, false, "mac");
    mac.set(|r| {
        r.on_battery = Some(true);
        r.lid_closed = Some(true);
    });
    ins.look();
    mac.sleep(3600.0);
    mac.advance(TICK_S as f64);
    ins.look();
    let told = mac.told_texts();
    let slept: Vec<&String> = told.iter().filter(|t| t.contains("slept")).collect();
    assert_eq!(slept.len(), 1, "{told:?}");
    assert!(
        slept[0].contains("lid was closed on battery"),
        "{}",
        slept[0]
    );
    assert!(mac.journal_kinds().contains(&"insomnia.slept".to_string()));
    assert_eq!(ins.status()["on"], true);
}

// ---- how it ends -------------------------------------------------------------------------------------------------

/// FERB's work done plus the grace ends it; new work in the grace keeps it on; the grace is the owner's setting.
// decision: DMN-605
// decision: APP-980
#[test]
fn the_grace_after_the_work_ends_it() {
    let (mac, ins) = insomnia();
    mac.settings.lock().unwrap().grace_minutes = 10;
    *mac.working.lock().unwrap() = 1;
    ins.start(Variant::Shield, true, "phone-ep");
    mac.advance(3600.0);
    ins.look();
    assert_eq!(ins.status()["on"], true, "work runs: no end");
    *mac.working.lock().unwrap() = 0;
    ins.look();
    let st = ins.status();
    assert!(st["ends_at"].is_string());
    assert!(
        st["line"].as_str().unwrap().contains("ends"),
        "{}",
        st["line"]
    );
    mac.advance(9.0 * 60.0);
    ins.look();
    assert_eq!(ins.status()["on"], true);
    // follow-up work in the grace
    *mac.working.lock().unwrap() = 1;
    ins.look();
    *mac.working.lock().unwrap() = 0;
    ins.look(); // the next look (every TICK_S) sees the work done
    mac.advance(5.0 * 60.0);
    ins.look();
    assert_eq!(
        ins.status()["on"],
        true,
        "the grace counts from the last task's end"
    );
    mac.advance(5.0 * 60.0 + 1.0);
    ins.look();
    let st = ins.status();
    assert_eq!(st["on"], false);
    assert_eq!(st["last_end"]["why"], "done");
    assert!(mac
        .journal_kinds()
        .contains(&"insomnia.state:ended".to_string()));
    assert!(mac.told_texts().contains(&Ended::Done.line().to_string()));
}

/// The owner's unlock after the start is his return; his end from a phone or the Mac turns it off; each said once.
// decision: DMN-605
// decision: APP-980
#[test]
fn the_owner_back_or_his_end_ends_it() {
    let (mac, ins) = insomnia();
    *mac.working.lock().unwrap() = 1;
    ins.start(Variant::Locked, false, "mac");
    *mac.unlocked_at.lock().unwrap() = Some(mac.now() - chrono::Duration::seconds(5));
    ins.look();
    assert_eq!(
        ins.status()["on"],
        true,
        "an unlock before the start is not a return"
    );
    mac.advance(60.0);
    *mac.unlocked_at.lock().unwrap() = Some(mac.now());
    ins.look();
    assert_eq!(ins.status()["last_end"]["why"], "back");
    assert!(!ins.end(Ended::TurnedOff, "mac"), "nothing left to end");
    let (mac, ins) = insomnia();
    ins.start(Variant::Shield, false, "mac");
    assert!(ins.end(Ended::TurnedOff, "phone-ep"));
    assert_eq!(ins.status()["last_end"]["why"], "turned_off");
    assert_eq!(mac.told_texts(), vec![Ended::TurnedOff.line().to_string()]);
}

/// A restart resumes the mode as it was started (variant, start, the one Yes), journaled and said once.
// decision: DMN-602
// decision: APP-980
#[test]
fn a_restart_resumes_it_and_says_so_once() {
    let (mac, ins) = insomnia();
    *mac.working.lock().unwrap() = 1;
    ins.start(Variant::Shield, true, "phone-ep");
    let saved = mac.saved.lock().unwrap().clone().expect("kept");
    assert_eq!(saved.variant, Variant::Shield);
    // the daemon starts again over the same journal
    let again = ferb_daemon::insomnia::Insomnia::new(seams(&mac));
    again.resume();
    let st = again.status();
    assert_eq!(st["on"], true);
    assert_eq!(st["variant"], "shield");
    assert_eq!(st["by"], "phone-ep");
    assert_eq!(st["screen_yes"], true);
    assert!(again.screen_yes_since().is_some());
    assert!(mac
        .journal_kinds()
        .contains(&"insomnia.state:resumed".to_string()));
    assert_eq!(
        mac.told_texts()
            .iter()
            .filter(|t| t.contains("restarted"))
            .count(),
        1
    );
    assert!(mac.held_now().contains(&Hold::IdleSleep));
    // nothing to resume: nothing happens
    let (mac2, fresh) = insomnia();
    fresh.resume();
    assert_eq!(fresh.status()["on"], false);
    assert!(mac2.journal.lock().unwrap().is_empty());
    let _ = Saved::from_json(&json!({})).is_none();
}

/// The one Yes covers screen use only with the cover, only while the mode is on.
// decision: APP-984
#[test]
fn the_one_yes_lasts_the_session_with_the_cover_only() {
    let (_mac, ins) = insomnia();
    assert_eq!(ins.screen_yes_since(), None);
    ins.start(Variant::Locked, true, "mac");
    assert_eq!(
        ins.screen_yes_since(),
        None,
        "no cover: nothing to bring forward under"
    );
    ins.end(Ended::TurnedOff, "mac");
    ins.start(Variant::Shield, false, "mac");
    assert_eq!(ins.screen_yes_since(), None, "not given");
    ins.end(Ended::TurnedOff, "mac");
    let st = ins.start(Variant::Shield, true, "mac");
    assert_eq!(
        ins.screen_yes_since(),
        st["since"].as_str().map(String::from)
    );
    assert_eq!(ins.start(Variant::Locked, false, "mac")["already"], true);
    ins.end(Ended::TurnedOff, "mac");
    assert_eq!(ins.screen_yes_since(), None);
}

// ---- someone at the Mac ------------------------------------------------------------------------------------------

/// A person's input at the cover is one notice per episode (journaled), answered by It's fine or Lock my Mac; an app
/// FERB didn't bring forward is one too. FERB never locks by itself; Lock my Mac is the owner's.
// decision: DMN-606
// decision: APP-983
#[test]
fn a_person_at_the_cover_is_a_notice_and_only_the_owner_locks() {
    let (mac, ins) = insomnia();
    *mac.working.lock().unwrap() = 1;
    ins.start(Variant::Shield, true, "mac");
    shield_up(&ins);
    report(&ins, json!({"event": "person_input", "input": "keyboard"}));
    let n = ins.status()["notice"].clone();
    assert_eq!(n["kind"], "touched");
    assert_eq!(n["count"], 1);
    mac.advance(10.0);
    report(
        &ins,
        json!({"event": "person_input", "input": "trackpad", "during_step": true}),
    );
    let n2 = ins.status()["notice"].clone();
    assert_eq!(
        (n2["id"].clone(), n2["count"].clone()),
        (n["id"].clone(), json!(2)),
        "one episode"
    );
    assert_eq!(
        mac.journal_kinds()
            .iter()
            .filter(|k| *k == "insomnia.touched")
            .count(),
        2
    );
    // It's fine
    assert!(!ins.dismiss("someone-else"));
    assert!(ins.dismiss(n["id"].as_str().unwrap()));
    assert!(ins.status()["notice"].is_null());
    // a later touch is a new notice
    mac.advance(EPISODE_S + 1.0);
    report(&ins, json!({"event": "person_input", "input": "pointer"}));
    let n3 = ins.status()["notice"].clone();
    assert_ne!(n3["id"], n["id"]);
    // nothing so far locked the Mac
    for _ in 0..5 {
        mac.advance(TICK_S as f64);
        ins.look();
    }
    assert_eq!(*mac.locks.lock().unwrap(), 0, "FERB never locks by itself");
    // Lock my Mac
    let (locked, text) = ins.lock_mac();
    assert!(locked && text.contains("locked"));
    assert_eq!(*mac.locks.lock().unwrap(), 1);
    assert!(ins.status()["notice"].is_null());
    assert_eq!(
        ins.status()["on"],
        true,
        "the mode goes on; his unlock later is his return"
    );
    assert_eq!(ins.status()["mac_locked"], true);
    // a Mac that doesn't ask for the password at once: said, not hidden
    *mac.lock_answer.lock().unwrap() = Some(Ok(false));
    let (locked, text) = ins.lock_mac();
    assert!(!locked && text.contains("Lock Screen"), "{text}");
    // an app FERB didn't open
    report(
        &ins,
        json!({"event": "activation", "app": "Notes", "bundle": "com.apple.Notes"}),
    );
    let n4 = ins.status()["notice"].clone();
    assert_eq!(n4["kind"], "activation");
    assert!(n4["text"].as_str().unwrap().starts_with("Notes "), "{n4}");
    assert!(mac
        .journal_kinds()
        .contains(&"insomnia.activation".to_string()));
}

/// The locked variant has no cover: input reports are nothing (macOS's own lock is in front).
// decision: APP-983
#[test]
fn without_the_cover_input_is_not_a_notice() {
    let (_mac, ins) = insomnia();
    report(&ins, json!({"event": "person_input", "input": "keyboard"}));
    ins.start(Variant::Locked, false, "mac");
    report(&ins, json!({"event": "person_input", "input": "keyboard"}));
    assert!(ins.status()["notice"].is_null());
    let p: Map<String, Value> = json!({"event": "nonsense"}).as_object().unwrap().clone();
    assert!(ins.report(&p).is_err());
}

// ---- the camera for Show me --------------------------------------------------------------------------------------

/// The camera runs only while the phone pulls: on at the first pull, a frame only when new, off CAMERA_KEEP_S after the
/// last pull, and no frame outlives the watching or the mode.
// decision: DMN-607
// decision: APP-983
#[test]
fn the_camera_runs_only_while_the_owner_watches() {
    let (mac, ins) = insomnia();
    assert!(ins.camera_pull(None).is_err(), "only in insomnia mode");
    ins.start(Variant::Shield, true, "mac");
    shield_up(&ins);
    ins.camera_frame(1, b"before".to_vec());
    assert!(!ins.camera_holds_frame(), "no frame is kept before he asks");
    let first = ins.camera_pull(None).unwrap();
    assert_eq!(first["state"], "starting");
    assert_eq!(mac.topics(CAMERA_TOPIC), vec![json!({"on": true})]);
    ins.camera_frame(7, b"\xff\xd8frame".to_vec());
    let live = ins.camera_pull(None).unwrap();
    assert_eq!(
        (live["state"].clone(), live["seq"].clone()),
        (json!("live"), json!(7))
    );
    assert!(live["image"].is_string());
    let same = ins.camera_pull(Some(7)).unwrap();
    assert!(
        same.get("image").is_none(),
        "the same frame isn't sent again"
    );
    mac.advance(CAMERA_KEEP_S - 1.0);
    ins.camera_check();
    assert!(ins.camera_holds_frame());
    mac.advance(2.0);
    ins.camera_check();
    assert!(
        !ins.camera_holds_frame(),
        "dropped when he stopped watching"
    );
    assert_eq!(
        mac.topics(CAMERA_TOPIC),
        vec![json!({"on": true}), json!({"on": false})]
    );
    // the end turns it off too
    ins.camera_pull(None).unwrap();
    ins.camera_frame(8, b"x".to_vec());
    ins.end(Ended::TurnedOff, "mac");
    assert!(!ins.camera_holds_frame());
    // nothing of a frame in the journal
    for (_, v) in mac.journal.lock().unwrap().iter() {
        assert!(!v.to_string().contains("frame"), "{v}");
    }
    // FERB.app said the camera isn't allowed: the owner is told how
    let (_mac2, ins2) = insomnia();
    ins2.start(Variant::Shield, true, "mac");
    report(
        &ins2,
        json!({"event": "shield", "state": "up", "camera": "denied"}),
    );
    let d = ins2.camera_pull(None).unwrap();
    assert_eq!(d["state"], "denied");
    assert!(d["text"].as_str().unwrap().contains("System Settings"));
}

// ---- steps under the cover ---------------------------------------------------------------------------------------

/// A step that brings an app forward under the cover waits for FERB.app to let input through; refused if it doesn't;
/// without the cover it goes at once.
// decision: DMN-608
// decision: APP-984
#[tokio::test(start_paused = true)]
async fn a_step_under_the_cover_waits_for_the_click_through() {
    let (mac, ins) = insomnia();
    assert_eq!(ins.step_begin(1, "com.apple.TextEdit").await, Ok(None));
    ins.start(Variant::Shield, true, "mac");
    assert_eq!(
        ins.step_begin(1, "com.apple.TextEdit").await,
        Ok(None),
        "no cover up yet"
    );
    shield_up(&ins);
    // FERB.app answers
    let i2 = ins.clone();
    let m2 = mac.clone();
    let answer = tokio::spawn(async move {
        loop {
            let begun = m2.topics(STEP_TOPIC);
            if let Some(b) = begun.first() {
                let step = b["step"].as_str().unwrap().to_owned();
                report(&i2, json!({"event": "step_ready", "step": step}));
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    });
    let step = ins
        .step_begin(42, "com.apple.TextEdit")
        .await
        .unwrap()
        .unwrap();
    answer.await.unwrap();
    let begun = &mac.topics(STEP_TOPIC)[0];
    assert_eq!(
        (begun["pid"].clone(), begun["phase"].clone()),
        (json!(42), json!("begin"))
    );
    ins.step_end(&step);
    assert_eq!(
        mac.topics(STEP_TOPIC)[1],
        json!({"step": step, "phase": "end"})
    );
    // FERB.app silent: refused, and the cover is told the step is over
    let refused = ins.step_begin(42, "com.apple.TextEdit").await;
    assert!(refused
        .unwrap_err()
        .contains("nothing was clicked or typed"));
    let steps = mac.topics(STEP_TOPIC);
    assert_eq!(steps.last().unwrap()["phase"], "end");
    // an expected app is announced only with the cover up
    ins.expect("com.apple.Notes");
    assert_eq!(mac.topics(ferb_daemon::insomnia::EXPECT_TOPIC).len(), 1);
}

/// Every change publishes the status FERB.app shows.
// decision: DMN-602
#[test]
fn changes_are_published() {
    let (mac, ins) = insomnia();
    ins.start(Variant::Shield, true, "mac");
    let last = mac.topics(TOPIC).last().cloned().unwrap();
    assert_eq!(last["on"], true);
    assert!(last["line"]
        .as_str()
        .unwrap()
        .starts_with("Keeping your Mac working"));
    ins.end(Ended::TurnedOff, "mac");
    assert_eq!(mac.topics(TOPIC).last().unwrap()["on"], false);
}

// ---- phase 2: the root helper (APP-986) --------------------------------------------------------------------------

/// With the helper approved, the mode holds its sleep lease (a closed Mac on battery keeps working), renewed at each
/// look and let go at the end; without it the status says so and nothing else changes. A closed lid on battery is a
/// warning, never an end, and the cover's display hold stays under a closed lid.
// decision: DMN-611
// decision: APP-986
// decision: APP-981
// decision: DMN-617
#[test]
fn the_helper_s_lease_keeps_a_closed_mac_working_on_battery() {
    let (mac, ins) = insomnia();
    *mac.working.lock().unwrap() = 1;
    ins.start(Variant::Shield, true, "mac");
    assert_eq!(ins.status()["closed_lid"], "absent");
    *mac.root.ready.lock().unwrap() = true;
    ins.look();
    assert_eq!(ins.status()["closed_lid"], "ready");
    assert!(*mac.root.leased.lock().unwrap());
    shield_up(&ins);
    mac.set(|r| {
        r.on_battery = Some(true);
        r.lid_closed = Some(true);
    });
    ins.look();
    let kinds: Vec<Value> = ins.status()["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["kind"].clone())
        .collect();
    assert_eq!(kinds, vec![json!("lid_closed_battery")]);
    assert_eq!(ins.status()["on"], true);
    // the cover's display hold stays under a closed lid: no idle lock, what plays goes on (DMN-617)
    assert!(mac.held_now().contains(&Hold::Display));
    let leases = mac
        .root
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|c| *c == &format!("lease {LEASE_S}"))
        .count();
    assert!(leases >= 3, "renewed at every look");
    ins.end(Ended::TurnedOff, "mac");
    assert!(!*mac.root.leased.lock().unwrap());
    assert_eq!(mac.root.calls.lock().unwrap().last().unwrap(), "release");
    // with the mode off a task never leases (the helper is the mode's only)
    mac.root.calls.lock().unwrap().clear();
    *mac.working.lock().unwrap() = 1;
    ins.look();
    assert!(mac.root.calls.lock().unwrap().is_empty());
}

/// FERB's wakes follow the owner's schedules: a minute before each next run, future only, at most 8 days ahead and
/// MAX_WAKES in number; a wake no schedule wants any more is cancelled.
// decision: DMN-611
// decision: APP-986
#[test]
fn wakes_follow_the_owner_s_schedules() {
    use ferb_core::power::helper::MAX_WAKES;
    use ferb_daemon::insomnia::{sync_wakes, WAKE_EARLY_S};
    let root = insomnia_support::Root::default();
    let now = 1_800_000_000;
    let runs = [
        now + 3600,
        now + 30,
        now + 9 * 24 * 3600,
        now + 7200,
        now + 3600,
    ];
    assert_eq!(sync_wakes(&root, &runs, now), Ok((2, 0)));
    assert_eq!(
        *root.wakes.lock().unwrap(),
        vec![now + 3600 - WAKE_EARLY_S, now + 7200 - WAKE_EARLY_S]
    );
    // the 2-hour schedule was cancelled by the owner
    assert_eq!(sync_wakes(&root, &[now + 3600], now), Ok((0, 1)));
    let many: Vec<i64> = (1..20).map(|h| now + h * 3600).collect();
    sync_wakes(&root, &many, now).unwrap();
    assert_eq!(root.wakes.lock().unwrap().len(), MAX_WAKES);
}

// ---- the owner's lid-closed test of 2026-11-20 ---------------------------------------------------------------------

/// System sleep is held from the mode's first look on battery too, so AC arriving under a closed lid finds it held;
/// a power-source or lid change is looked at at once (the watch), not at the next 20-second look.
// decision: DMN-613
#[test]
fn system_sleep_is_held_from_the_start_and_changes_are_seen_at_once() {
    let (mac, ins) = insomnia();
    mac.set(|r| r.on_battery = Some(true));
    ins.start(Variant::Shield, true, "phone-ep");
    assert!(
        mac.held_now().contains(&Hold::SystemSleep),
        "held on battery already"
    );
    let looks = mac.holds.lock().unwrap().len();
    // the lid closes: the watch's event looks now (no clock moved, no tick)
    mac.set(|r| r.lid_closed = Some(true));
    assert_eq!(ins.power_event(PowerEvent::Changed), 0);
    assert!(mac.holds.lock().unwrap().len() > looks, "renewed at once");
    let st = ins.status();
    assert_eq!(st["warnings"][0]["kind"], "lid_closed_battery");
    assert_eq!(st["closed_lid"], "absent");
}

/// The words follow the helper's real state: without it a closed lid on battery sleeps, and the status's lid_line
/// (on and off) and the warning say so; with it, they say FERB keeps working.
// decision: DMN-615
#[test]
fn the_lid_words_follow_the_helper() {
    let (mac, ins) = insomnia();
    ins.look();
    let off = ins.status();
    assert_eq!(off["closed_lid"], "absent");
    let line = off["lid_line"].as_str().unwrap();
    assert!(
        line.contains("sleeps on battery") && line.contains("locks and pauses"),
        "{line}"
    );
    *mac.root.ready.lock().unwrap() = true;
    ins.look();
    assert!(ins.status()["lid_line"]
        .as_str()
        .unwrap()
        .contains("running as if it were open"));
    *mac.root.ready.lock().unwrap() = false;
    mac.set(|r| {
        r.on_battery = Some(true);
        r.lid_closed = Some(true);
    });
    ins.start(Variant::Locked, false, "phone-ep");
    let w = ins.status()["warnings"][0].clone();
    assert!(
        w["text"].as_str().unwrap().contains("will go to sleep"),
        "{w}"
    );
    assert!(w["text"].as_str().unwrap().contains("Login Items"), "{w}");
    // approved while the warning lasts: same warning, the true words
    *mac.root.ready.lock().unwrap() = true;
    ins.look();
    let w2 = ins.status()["warnings"][0].clone();
    assert_eq!(w2["id"], w["id"]);
    assert!(
        w2["text"]
            .as_str()
            .unwrap()
            .contains("FERB keeps it working"),
        "{w2}"
    );
    for t in mac.told_texts() {
        assert!(
            !(t.contains("keeps it working") && t.contains("will go to sleep")),
            "{t}"
        );
    }
}

/// macOS about to sleep during the mode is published first and waits a moment for the phone; the wake says once when
/// and why it slept (the lid closed on battery, with the way to keep it working); the next look doesn't say it again.
/// Off, nothing waits and nothing is said.
// decision: DMN-614
#[test]
fn sleep_is_said_before_and_after() {
    let (mac, ins) = insomnia();
    assert_eq!(
        ins.power_event(PowerEvent::WillSleep),
        0,
        "off: macOS doesn't wait"
    );
    ins.power_event(PowerEvent::Woke);
    assert!(mac.told_texts().is_empty());
    *mac.working.lock().unwrap() = 1;
    ins.start(Variant::Shield, true, "phone-ep");
    mac.set(|r| {
        r.on_battery = Some(true);
        r.lid_closed = Some(true);
    });
    assert_eq!(
        ins.power_event(PowerEvent::WillSleep),
        ferb_daemon::insomnia::SLEEP_TELL_MS
    );
    let st = mac.topics(TOPIC).last().cloned().unwrap();
    assert_eq!(st["going_to_sleep"], true);
    assert!(st["line"].as_str().unwrap().contains("going to sleep"));
    mac.sleep(35.0);
    mac.set(|r| r.on_battery = Some(false));
    ins.power_event(PowerEvent::Woke);
    let slept: Vec<String> = mac
        .told_texts()
        .into_iter()
        .filter(|t| t.contains("slept"))
        .collect();
    assert_eq!(slept.len(), 1, "{slept:?}");
    assert!(slept[0].contains("lid was closed on battery") && slept[0].contains("Login Items"));
    assert_eq!(ins.status()["going_to_sleep"], false);
    mac.advance(TICK_S as f64);
    ins.look();
    assert_eq!(
        mac.told_texts()
            .iter()
            .filter(|t| t.contains("slept"))
            .count(),
        1,
        "said once"
    );
}

/// Without the watch, a short sleep (35 s, as the owner's) is still found by the wall clock's gap at the next look.
// decision: DMN-614
#[test]
fn a_short_sleep_is_found_by_the_next_look() {
    let (mac, ins) = insomnia();
    *mac.working.lock().unwrap() = 1;
    ins.start(Variant::Shield, true, "phone-ep");
    mac.set(|r| {
        r.on_battery = Some(true);
        r.lid_closed = Some(true);
    });
    ins.look();
    mac.sleep(35.0);
    mac.advance(5.0);
    ins.look();
    assert_eq!(
        mac.told_texts()
            .iter()
            .filter(|t| t.contains("slept"))
            .count(),
        1,
        "{:?}",
        mac.told_texts()
    );
}

/// FERB.app's word on setting up the helper is kept, journaled once per change and shown: a failure with its error, a
/// wait for the owner's OK; nothing once the helper answers.
// decision: DMN-616
#[test]
fn the_helper_s_setup_is_never_silent() {
    let (mac, ins) = insomnia();
    ins.look();
    let p = json!({"event": "helper", "state": "failed", "error": "Operation not permitted (SMAppServiceErrorDomain 1)"});
    report(&ins, p.clone());
    report(&ins, p);
    let st = ins.status();
    assert_eq!(st["helper"]["state"], "failed");
    assert!(st["helper"]["problem"]
        .as_str()
        .unwrap()
        .contains("Operation not permitted"));
    assert_eq!(
        mac.journal_kinds()
            .iter()
            .filter(|k| k.starts_with("insomnia.helper"))
            .count(),
        1,
        "once per change"
    );
    report(
        &ins,
        json!({"event": "helper", "state": "requires_approval"}),
    );
    assert!(ins.status()["helper"]["problem"]
        .as_str()
        .unwrap()
        .contains("Login Items"));
    *mac.root.ready.lock().unwrap() = true;
    ins.look();
    assert!(
        ins.status()["helper"]["problem"].is_null(),
        "it answers: no problem"
    );
}
