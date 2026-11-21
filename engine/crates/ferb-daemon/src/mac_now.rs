//! What FERB knows of this Mac's sleep and power, and its own "Keep my Mac working" mode (insomnia mode), from facts it
//! reads itself (the owner, 2026-11-22: "ferb should knwo and tell user that its mac and not its service - can feb find
//! it out itself??"). The facts come from the insomnia area's power watch and reads (`ferb_core::power`, the Mac's
//! side in `ferb-platform-macos`); this module only words them, so every route says the same:
//!
//! - [`context_line`]: the one line the planner and workers get, and only while the Mac is asleep in a brief
//!   background wake (a Power Nap or maintenance "dark wake"): what FERB can and can't do then.
//! - [`asleep_line`]: the same truth for the owner, in the phone's status.
//! - [`power_facts`] and [`history`]: what `device.facts` answers when asked (the owner, 2026-11-22: facts behind a
//!   read tool the planner can call, not more always-sent context).
//!
//! The record: the insomnia area journals each change as [`JOURNAL_KIND`] (an operational kind, kept as long as the
//! other operational records, STORE-401): the Mac going to sleep, its full wake (with how many brief background wakes
//! it had while asleep), the lid and the power source. FERB's mode is journaled as `insomnia.state`.

use serde_json::{json, Map, Value};

use ferb_core::power::PowerReading;
use ferb_core::when::{self, DateTime};

/// The journal kind of the Mac's sleep, wake, lid and power-source changes.
pub const JOURNAL_KIND: &str = "mac.power";
/// FERB's mode as the owner sees it in FERB.app and on the phone.
pub const MODE_NAME: &str = "Keep my Mac working";
/// The most history entries one answer gives (a technical bound: the newest are kept, the rest counted).
pub const HISTORY_MAX: usize = 300;

/// The sleep FERB is following now: since when (as far as FERB saw), how it went to sleep, and how many times macOS
/// woke it briefly in the background since.
#[derive(Clone, Debug, PartialEq)]
pub struct Asleep {
    pub since: DateTime,
    pub brief_wakes: u32,
    pub lid_closed: Option<bool>,
    pub on_battery: Option<bool>,
    pub power_nap: Option<bool>,
    /// FERB first saw it in a brief wake (it started, or missed the sleep), so `since` is when FERB saw it.
    pub found: bool,
}

impl Asleep {
    /// The record of its start (`mac.power` event `asleep`).
    pub fn started_row(&self) -> Value {
        let mut row = json!({"event": "asleep", "lid_closed": self.lid_closed, "on_battery": self.on_battery,
                             "power_nap": self.power_nap});
        if self.found {
            row["found"] = json!(true);
        }
        row
    }

    /// The record of its end (`mac.power` event `awake`).
    pub fn ended_row(&self) -> Value {
        json!({"event": "awake", "asleep_since": when::isoformat(&self.since), "brief_wakes": self.brief_wakes})
    }
}

/// FERB's mode and what it does now, as the insomnia area knows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mode {
    pub on: bool,
    pub since: Option<DateTime>,
    /// Outside the mode: FERB holds the Mac from idle sleep now because a task runs and the lid is open.
    pub task_awake: bool,
    /// FERB's helper that keeps a closed Mac working on battery answers (None: not known yet).
    pub helper_ready: Option<bool>,
}

/// Everything the words below are made of.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MacState {
    pub reading: PowerReading,
    pub asleep: Option<Asleep>,
    pub mode: Mode,
}

/// The Mac is asleep and only briefly awake now (a dark wake), as the reading shows.
pub fn asleep_now(r: &PowerReading) -> bool {
    r.dark_wake == Some(true)
}

fn hm(t: &DateTime) -> String {
    t.format("%H:%M").to_string()
}

/// The one always-sent line, only while the Mac is asleep in a brief background wake ("" otherwise): FERB runs only
/// during such wakes, so replies are slow and the screen, keyboard and pointer can't be used.
pub fn context_line(s: &MacState) -> String {
    let r = &s.reading;
    if !asleep_now(r) {
        return String::new();
    }
    let lid = r.lid_closed == Some(true);
    let mut how = String::from("- This Mac is asleep");
    if lid {
        how.push_str(" with its lid closed");
    }
    match r.on_battery {
        Some(false) => how.push_str(", on power"),
        Some(true) => how.push_str(", on battery"),
        None => {}
    }
    if let Some(a) = &s.asleep {
        let seen = if a.found { " (as far as FERB saw)" } else { "" };
        how.push_str(&format!(" since {}{seen}", hm(&a.since)));
    }
    let wakes = if r.power_nap() == Some(true) {
        "macOS's Power Nap wakes it briefly now and then"
    } else {
        "macOS woke it briefly in the background"
    };
    let until = if lid {
        "until the Mac is opened"
    } else {
        "until the Mac is woken"
    };
    format!(
        "{how}: {wakes}, and FERB runs only during those wakes, so replies can take minutes and anything that needs \
         the screen, keyboard or pointer waits {until}; FERB's {MODE_NAME} mode is {}; device.facts has the details \
         and the sleep record",
        if s.mode.on { "on" } else { "off" }
    )
}

/// The phone's words while the Mac is asleep in a brief background wake (none otherwise): plain, about the Mac.
pub fn asleep_line(r: &PowerReading) -> Option<&'static str> {
    if !asleep_now(r) {
        return None;
    }
    Some(if r.lid_closed == Some(true) {
        "Your Mac is asleep with its lid closed. It wakes for a moment now and then, so replies can take a few \
         minutes, and anything that needs its screen waits until you open it."
    } else {
        "Your Mac is asleep. It wakes for a moment now and then, so replies can take a few minutes, and anything that \
         needs its screen waits until it's awake."
    })
}

/// What Power Nap means for FERB (in device.facts).
pub const POWER_NAP_ABOUT: &str = "macOS's Power Nap setting: while the Mac sleeps, macOS wakes it briefly now and \
then (to fetch mail, messages and updates); FERB's background service runs during those brief wakes, so FERB can \
answer while the Mac sleeps, slowly, but can't use its screen, keyboard or pointer";
/// What FERB's mode is (in device.facts).
pub const MODE_ABOUT: &str = "FERB's mode that keeps this Mac working while the owner is away (insomnia mode; the \
owner turns it on and off in FERB.app or on the phone). Only this mode keeps the Mac awake with its lid closed or \
while the owner is away";
/// What FERB does outside the mode (DMN-603: idle sleep only while a task tree runs and the lid isn't closed).
pub const OUTSIDE_MODE: &str = "when the mode is off, FERB keeps the Mac from idle sleep only while one of its tasks \
runs and the lid is open, and never keeps it awake once the lid is closed: a closed Mac on power then sleeps, and \
only macOS's own brief wakes (Power Nap) let FERB answer";

/// device.facts' `power` part: whether the Mac is awake or asleep in a brief wake, its lid, its power source, Power
/// Nap's setting; and `keep_my_mac_working`: FERB's mode now. A fact not read is left out.
pub fn power_facts(s: &MacState) -> Map<String, Value> {
    let r = &s.reading;
    let mut power = Map::new();
    match r.dark_wake {
        Some(true) => {
            power.insert(
                "awake".into(),
                json!("asleep: macOS woke it briefly in the background (Power Nap or maintenance); FERB runs only \
                       during such wakes, and the screen, keyboard and pointer can't be used until the Mac is \
                       opened or woken"),
            );
            if let Some(a) = &s.asleep {
                power.insert("asleep_since".into(), json!(when::isoformat(&a.since)));
                if a.found {
                    power.insert(
                        "asleep_since_note".into(),
                        json!("when FERB first saw it asleep (it may have slept earlier)"),
                    );
                }
                power.insert("brief_wakes_since".into(), json!(a.brief_wakes));
            }
        }
        Some(false) => {
            power.insert("awake".into(), json!("fully awake"));
        }
        None => {}
    }
    if let Some(c) = r.lid_closed {
        power.insert("lid".into(), json!(if c { "closed" } else { "open" }));
    }
    if let Some(b) = r.on_battery {
        power.insert(
            "power_source".into(),
            json!(if b { "battery" } else { "power adapter" }),
        );
    }
    if r.power_nap_ac.is_some() || r.power_nap_battery.is_some() {
        let mut nap = Map::new();
        if let Some(v) = r.power_nap_ac {
            nap.insert("plugged_in".into(), json!(if v { "on" } else { "off" }));
        }
        if let Some(v) = r.power_nap_battery {
            nap.insert("on_battery".into(), json!(if v { "on" } else { "off" }));
        }
        nap.insert("about".into(), json!(POWER_NAP_ABOUT));
        power.insert("power_nap".into(), Value::Object(nap));
    }
    let m = &s.mode;
    let mut mode = Map::new();
    mode.insert("on".into(), json!(m.on));
    if let Some(t) = m.since.filter(|_| m.on) {
        mode.insert("since".into(), json!(when::isoformat(&t)));
    }
    mode.insert("what_it_is".into(), json!(MODE_ABOUT));
    if m.on {
        if let Some(ready) = m.helper_ready {
            mode.insert(
                "closed_lid_on_battery".into(),
                json!(if ready {
                    "kept working: FERB's helper is allowed"
                } else {
                    "the Mac sleeps: FERB's helper isn't allowed yet (System Settings → General → Login Items)"
                }),
            );
        }
    } else {
        mode.insert("when_off".into(), json!(OUTSIDE_MODE));
        mode.insert("keeping_awake_for_a_task_now".into(), json!(m.task_awake));
    }
    let mut out = Map::new();
    if !power.is_empty() {
        out.insert("power".into(), Value::Object(power));
    }
    out.insert("keep_my_mac_working".into(), Value::Object(mode));
    out
}

/// One recorded row as device.facts lists it: when, and what happened in words, with its facts.
fn entry(
    at: &DateTime,
    kind: &str,
    data: &Value,
    local: &dyn Fn(&DateTime) -> String,
) -> Option<Value> {
    let flag = |k: &str| data.get(k).and_then(Value::as_bool);
    let mut e = Map::new();
    e.insert("at".into(), json!(local(at)));
    if kind == crate::insomnia::JOURNAL_KIND {
        let what = match data.get("state").and_then(Value::as_str)? {
            "started" => format!("{MODE_NAME} turned on"),
            "resumed" => format!("{MODE_NAME} on again after FERB restarted"),
            "ended" => format!("{MODE_NAME} turned off"),
            _ => return None,
        };
        e.insert("event".into(), json!(what));
        return Some(Value::Object(e));
    }
    match data.get("event").and_then(Value::as_str)? {
        "asleep" => {
            e.insert(
                "event".into(),
                json!(if flag("found") == Some(true) {
                    "found asleep (FERB started or missed the sleep)"
                } else {
                    "went to sleep"
                }),
            );
            if let Some(c) = flag("lid_closed") {
                e.insert("lid".into(), json!(if c { "closed" } else { "open" }));
            }
            if let Some(b) = flag("on_battery") {
                e.insert(
                    "power_source".into(),
                    json!(if b { "battery" } else { "power adapter" }),
                );
            }
            if let Some(n) = flag("power_nap") {
                e.insert("power_nap".into(), json!(if n { "on" } else { "off" }));
            }
        }
        "awake" => {
            e.insert("event".into(), json!("woke fully"));
            if let Some(t) = data
                .get("asleep_since")
                .and_then(Value::as_str)
                .and_then(when::parse)
            {
                e.insert("asleep_since".into(), json!(local(&t)));
            }
            if let Some(n) = data.get("brief_wakes").and_then(Value::as_u64) {
                e.insert("brief_wakes".into(), json!(n));
            }
        }
        "lid" => {
            e.insert(
                "event".into(),
                json!(if flag("closed")? {
                    "lid closed"
                } else {
                    "lid opened"
                }),
            );
        }
        "power" => {
            e.insert(
                "event".into(),
                json!(if flag("on_battery")? {
                    "unplugged: on battery"
                } else {
                    "plugged in"
                }),
            );
        }
        _ => return None,
    }
    Some(Value::Object(e))
}

/// What device.facts says of how brief wakes are counted.
pub const HISTORY_ABOUT: &str = "brief_wakes counts the times macOS woke the sleeping Mac in the background (Power \
Nap or maintenance) during that sleep: FERB's background service ran then and could answer, but the screen, keyboard \
and pointer couldn't be used";

/// device.facts' `history`: the recorded rows ([`JOURNAL_KIND`] and `insomnia.state`, each `(at, kind, data)`, any
/// order) from `since` up to `until` (not included), oldest first, each with its time written by `local`. Beyond
/// [`HISTORY_MAX`] the oldest are left out: answers the entries and how many were left out.
pub fn history(
    rows: &[(String, String, Value)],
    since: &DateTime,
    until: Option<&DateTime>,
    local: &dyn Fn(&DateTime) -> String,
) -> (Vec<Value>, usize) {
    let mut kept: Vec<(DateTime, &str, &Value)> = rows
        .iter()
        .filter_map(|(at, kind, data)| Some((when::parse(at)?, kind.as_str(), data)))
        .filter(|(t, ..)| t >= since && until.is_none_or(|u| t < u))
        .collect();
    kept.sort_by_key(|(t, ..)| *t);
    let mut out: Vec<Value> = kept
        .iter()
        .filter_map(|(t, k, d)| entry(t, k, d, local))
        .collect();
    let left = out.len().saturating_sub(HISTORY_MAX);
    out.drain(..left);
    (out, left)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime {
        when::parse(s).unwrap()
    }

    fn dark(lid: bool, battery: bool, nap: bool) -> PowerReading {
        PowerReading {
            on_battery: Some(battery),
            lid_closed: Some(lid),
            dark_wake: Some(true),
            power_nap_ac: Some(nap),
            power_nap_battery: Some(nap),
            ..Default::default()
        }
    }

    #[test]
    fn the_context_gains_a_line_only_while_the_mac_is_asleep() {
        let awake = MacState {
            reading: PowerReading {
                dark_wake: Some(false),
                lid_closed: Some(true),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(context_line(&awake), "");
        assert_eq!(context_line(&MacState::default()), "");
        assert_eq!(asleep_line(&awake.reading), None);
        let s = MacState {
            reading: dark(true, false, true),
            asleep: Some(Asleep {
                since: at("2026-11-21T22:24:00+05:30"),
                brief_wakes: 37,
                lid_closed: Some(true),
                on_battery: Some(false),
                power_nap: Some(true),
                found: false,
            }),
            mode: Mode::default(),
        };
        assert_eq!(
            context_line(&s),
            "- This Mac is asleep with its lid closed, on power since 22:24: macOS's Power Nap wakes it briefly now \
             and then, and FERB runs only during those wakes, so replies can take minutes and anything that needs \
             the screen, keyboard or pointer waits until the Mac is opened; FERB's Keep my Mac working mode is off; \
             device.facts has the details and the sleep record"
        );
        assert!(asleep_line(&s.reading).unwrap().contains("lid closed"));
        let open = dark(false, true, false);
        assert!(asleep_line(&open).unwrap().ends_with("until it's awake."));
    }
}
