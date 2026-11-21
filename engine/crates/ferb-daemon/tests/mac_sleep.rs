//! The Mac's sleep as FERB follows it (the owner, 2026-11-22: FERB should know, from facts it reads itself, that the
//! Mac and not FERB answered with the lid closed): a night with the lid closed on power and Power Nap on, its brief
//! background wakes ("DarkWake") counted, the record journaled as `mac.power`, the phone's status saying it while it
//! is true, and the facts device.facts answers. Stand-ins for the power facts and clocks (insomnia_support).

mod insomnia_support;

use ferb_core::power::PowerEvent;
use ferb_daemon::mac_now;
use insomnia_support::*;
use serde_json::{json, Value};

fn rows(mac: &Mac) -> Vec<Value> {
    mac.journal
        .lock()
        .unwrap()
        .iter()
        .filter(|(k, _)| k == mac_now::JOURNAL_KIND)
        .map(|(_, d)| d.clone())
        .collect()
}

/// One brief background wake: macOS wakes the sleeping Mac without graphics, FERB's look runs, macOS sleeps again.
fn brief_wake(mac: &Mac, ins: &ferb_daemon::insomnia::Insomnia) {
    mac.sleep(120.0);
    mac.set(|r| r.dark_wake = Some(true));
    ins.power_event(PowerEvent::Woke);
    mac.advance(10.0);
    ins.power_event(PowerEvent::WillSleep);
}

// decisions: SENSE-426, SENSE-429, SENSE-430
#[test]
fn a_night_of_power_nap_wakes_is_followed_recorded_and_said() {
    let (mac, ins) = insomnia();
    mac.set(|r| {
        r.dark_wake = Some(false);
        r.power_nap_ac = Some(true);
        r.power_nap_battery = Some(false);
    });
    ins.look();
    assert_eq!(ins.status()["asleep"], false);
    assert_eq!(ins.status()["asleep_line"], Value::Null);
    // the owner closes the lid on power, with insomnia mode off: macOS sleeps
    mac.set(|r| r.lid_closed = Some(true));
    ins.power_event(PowerEvent::Changed);
    assert_eq!(
        ins.power_event(PowerEvent::WillSleep),
        0,
        "the mode is off: nothing holds macOS's sleep"
    );
    for _ in 0..3 {
        brief_wake(&mac, &ins);
    }
    // a brief wake now: the phone's status and the one always-sent line say it
    mac.sleep(90.0);
    mac.set(|r| r.dark_wake = Some(true));
    ins.power_event(PowerEvent::Woke);
    let st = ins.status();
    assert_eq!(st["on"], false);
    assert_eq!(st["asleep"], true);
    assert_eq!(
        st["asleep_line"],
        "Your Mac is asleep with its lid closed. It wakes for a moment now and then, so replies can take a few \
         minutes, and anything that needs its screen waits until you open it."
    );
    let state = ins.mac_state();
    assert_eq!(state.asleep.as_ref().unwrap().brief_wakes, 4);
    let line = mac_now::context_line(&state);
    assert!(
        line.starts_with("- This Mac is asleep with its lid closed, on power since 15:00:")
            && line.contains("Power Nap")
            && line.contains("Keep my Mac working mode is off"),
        "{line}"
    );
    let facts = Value::Object(mac_now::power_facts(&state));
    assert_eq!(facts["power"]["lid"], "closed");
    assert_eq!(facts["power"]["power_nap"]["plugged_in"], "on");
    assert_eq!(facts["power"]["power_nap"]["on_battery"], "off");
    assert_eq!(facts["power"]["brief_wakes_since"], 4);
    assert!(facts["power"]["awake"]
        .as_str()
        .unwrap()
        .starts_with("asleep"));
    assert_eq!(facts["keep_my_mac_working"]["on"], false);
    assert!(facts["keep_my_mac_working"]["when_off"]
        .as_str()
        .unwrap()
        .contains("never keeps it awake once the lid is closed"));
    // morning: the lid opens, the Mac wakes fully
    mac.advance(10.0);
    ins.power_event(PowerEvent::WillSleep);
    mac.sleep(3600.0);
    mac.set(|r| {
        r.dark_wake = Some(false);
        r.lid_closed = Some(false);
    });
    ins.power_event(PowerEvent::Woke);
    assert_eq!(ins.status()["asleep"], false);
    assert_eq!(mac_now::context_line(&ins.mac_state()), "");
    assert_eq!(
        rows(&mac),
        vec![
            json!({"event": "lid", "closed": true}),
            json!({"event": "asleep", "lid_closed": true, "on_battery": false, "power_nap": true}),
            json!({"event": "awake", "asleep_since": "2026-11-20T15:00:00+05:30", "brief_wakes": 4}),
            json!({"event": "lid", "closed": false}),
        ]
    );
}

/// A brief wake FERB sees first (it started during the night, or missed the sleep) starts the record as found; a wake
/// whose kind can't be read ends it (never counted as brief).
// decisions: SENSE-429
#[test]
fn a_sleep_found_in_a_brief_wake_is_recorded_as_found() {
    let (mac, ins) = insomnia();
    mac.set(|r| r.dark_wake = Some(true));
    ins.look();
    let a = ins.mac_state().asleep.unwrap();
    assert!(a.found);
    assert_eq!(a.brief_wakes, 0);
    mac.set(|r| r.dark_wake = None);
    ins.power_event(PowerEvent::Woke);
    let r = rows(&mac);
    assert_eq!(r[0]["event"], "asleep");
    assert_eq!(r[0]["found"], true);
    assert_eq!(r[1]["event"], "awake");
    assert!(ins.mac_state().asleep.is_none());
}

/// With the mode on and the Mac asleep anyway (closed on battery without the helper), each Power Nap wake neither holds
/// macOS's sleep again nor says a sleep line: the sleep is said once, at the full wake, from its start.
// decisions: DMN-614, SENSE-429
#[test]
fn brief_wakes_during_the_mode_say_the_sleep_once() {
    let (mac, ins) = insomnia();
    *mac.working.lock().unwrap() = 1;
    ins.start(ferb_daemon::insomnia::Variant::Shield, true, "phone-ep");
    mac.set(|r| {
        r.dark_wake = Some(false);
        r.on_battery = Some(true);
        r.lid_closed = Some(true);
        r.power_nap_battery = Some(true);
    });
    ins.look();
    assert_eq!(
        ins.power_event(PowerEvent::WillSleep),
        ferb_daemon::insomnia::SLEEP_TELL_MS
    );
    for _ in 0..2 {
        mac.sleep(600.0);
        mac.set(|r| r.dark_wake = Some(true));
        ins.power_event(PowerEvent::Woke);
        mac.advance(5.0);
        assert_eq!(
            ins.power_event(PowerEvent::WillSleep),
            0,
            "a brief wake's end"
        );
    }
    let slept = |mac: &Mac| {
        mac.told_texts()
            .into_iter()
            .filter(|t| t.contains("slept"))
            .collect::<Vec<_>>()
    };
    assert!(slept(&mac).is_empty(), "nothing said in the brief wakes");
    mac.sleep(600.0);
    mac.set(|r| {
        r.dark_wake = Some(false);
        r.lid_closed = Some(false);
    });
    ins.power_event(PowerEvent::Woke);
    let said = slept(&mac);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].contains("from 3:00 PM"), "{said:?}");
}
