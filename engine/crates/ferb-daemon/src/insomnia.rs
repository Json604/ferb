//! Insomnia mode (kb T-1274; the owner's APP-980..APP-985): FERB keeps the Mac working while the owner is away.
//!
//! One daemon state. Only the owner turns it on (FERB.app's dialog, or his paired phone with the fingerprint, after a
//! warning that advises leaving the lid open, APP-980). While it is on:
//!
//! - the daemon holds the Mac from idle sleep on a renewable lease ([`LEASE_S`], renewed every tick), and from any
//!   sleep on AC power (which also covers a closed lid while plugged in); with the cover ([`Variant::Shield`]) also
//!   the display's idle sleep, and only while FERB.app says its cover is up: a display that never sleeps without the
//!   cover would leave an unlocked Mac open to anyone. macOS lets every hold go when the lease runs out or this process
//!   dies;
//! - it never ends or slows itself for battery, heat or the lid (APP-981): it warns the owner on the Mac and the phone
//!   when the battery is low, the Mac is hot or the cover is not up, and says plainly afterwards when macOS put the Mac
//!   to sleep anyway;
//! - a person's input at the cover, or an app FERB didn't bring forward, is a notice for the owner's phone with Show me
//!   (the camera), Lock my Mac and It's fine (APP-983). FERB never locks by itself;
//! - with the one Yes given at the start ([`Mode::screen_yes`]), the gateway answers the screen-use question for the
//!   session (APP-984), and each step that brings an app forward asks FERB.app to let its input through the cover
//!   first ([`Insomnia::step_begin`]).
//!
//! It ends when the owner is back (the cover's Touch ID or password, or his unlock of the Mac), when he ends it from
//! the phone or the Mac, or when FERB's work has been done for the grace (the owner's setting
//! `insomnia.grace_minutes`). It is journaled ([`JOURNAL_KIND`]) and a daemon restart resumes it, said once.
//!
//! Separately, with the mode off, while any task runs and the lid is open FERB holds the Mac from idle sleep only
//! (the owner agreed: never the display outside the cover).
//!
//! Everything the Mac does comes in through [`Seams`]; tests use stand-ins.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ferb_core::power::{
    HelperState, Hold, KeepAwake, LockMac, PowerFacts, PowerReading, RootHelper, Thermal,
};
use ferb_core::when::{self, DateTime};
use ferb_rpc::server::HandlerError;
use ferb_rpc::wire::Params;
use ferb_rpc::Server;
use serde_json::{json, Map, Value};
use tokio::sync::oneshot;

/// The journal kind of a start, a resume and an end (`state`), read back at the next start.
pub const JOURNAL_KIND: &str = "insomnia.state";
/// The bus topic of the status (FERB.app's cover, menu bar and notch line).
pub const TOPIC: &str = "insomnia.changed";
/// A step that brings an app forward under the cover: `begin` (let input through), `end`.
pub const STEP_TOPIC: &str = "insomnia.step";
/// An app FERB is about to bring forward for background work: its activation is FERB's own.
pub const EXPECT_TOPIC: &str = "insomnia.expect";
/// Start or stop the Mac's camera for Show me.
pub const CAMERA_TOPIC: &str = "insomnia.camera";

/// How long one hold lasts unless renewed (seconds): a hung daemon keeps the Mac awake at most this long.
pub const LEASE_S: u64 = 120;
/// How often the daemon looks (renews, reads the power facts, counts the work, checks the grace).
pub const TICK_S: u64 = 20;
/// A tick this much later than its period means the Mac slept in between (the tick's timer stops while it sleeps).
pub const SLEPT_GAP_S: f64 = 60.0;
/// FERB.app says its cover is up every 30 s; none for this long and the cover counts as down.
pub const SHIELD_LOST_S: f64 = 90.0;
/// How long after the start the cover may take to come up before the owner is told it isn't up.
pub const SHIELD_FIRST_S: f64 = 30.0;
/// Each camera pull keeps the camera on this much longer; it goes off when the phone stops watching.
pub const CAMERA_KEEP_S: f64 = 6.0;
/// How long a step waits for FERB.app to let its input through the cover.
pub const STEP_WAIT_S: f64 = 3.0;
/// Person input within this long of a notice is the same episode (its count grows; no new notice).
pub const EPISODE_S: f64 = 120.0;
/// How long an end stays in the status, so each device can say it once.
pub const LAST_END_S: f64 = 600.0;
/// How long an app FERB announced may take to come forward.
pub const EXPECT_S: u64 = 5;

/// The owner's numbers (rule 18): settings with defaults he confirms (`insomnia.*`).
pub const GRACE_DEFAULT_MIN: i64 = 10;
pub const BATTERY_WARN_DEFAULT: i64 = 20;
pub const BATTERY_URGENT_DEFAULT: i64 = 10;

/// What the power log shows for FERB's holds: generic words, never a task's title (rule 7's spirit).
pub const REASON_MODE: &str = "FERB is keeping your Mac working while you're away";
pub const REASON_TASK: &str = "FERB is working on a task";

/// How the Mac is kept while the owner is away (APP-980).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// FERB.app's cover over an unlocked Mac (the default with the lid open): FERB works underneath.
    Shield,
    /// macOS locks the Mac as usual; FERB keeps it awake.
    Locked,
}

impl Variant {
    pub fn as_str(self) -> &'static str {
        match self {
            Variant::Shield => "shield",
            Variant::Locked => "locked",
        }
    }
    pub fn parse(s: &str) -> Option<Variant> {
        match s {
            "shield" => Some(Variant::Shield),
            "locked" => Some(Variant::Locked),
            _ => None,
        }
    }
}

/// Why it ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    /// The owner is back: the cover's Touch ID or password, or his unlock of the Mac.
    Back,
    /// He ended it from the phone or the Mac.
    TurnedOff,
    /// FERB's work was done for the grace.
    Done,
}

impl Ended {
    pub fn as_str(self) -> &'static str {
        match self {
            Ended::Back => "back",
            Ended::TurnedOff => "turned_off",
            Ended::Done => "done",
        }
    }

    /// Said once on the Mac and the phone.
    pub fn line(self) -> &'static str {
        match self {
            Ended::Back => "Welcome back. Your Mac can sleep as usual again.",
            Ended::TurnedOff => "Done. Your Mac can sleep as usual again.",
            Ended::Done => "FERB finished its work, so your Mac can sleep as usual again.",
        }
    }
}

/// The owner's settings for the mode, read at each look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub grace_minutes: i64,
    pub battery_warn: i64,
    pub battery_urgent: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            grace_minutes: GRACE_DEFAULT_MIN,
            battery_warn: BATTERY_WARN_DEFAULT,
            battery_urgent: BATTERY_URGENT_DEFAULT,
        }
    }
}

/// What is kept of a mode across a restart (the journal's `started` row).
#[derive(Clone, Debug, PartialEq)]
pub struct Saved {
    pub variant: Variant,
    pub since: DateTime,
    pub by: String,
    pub screen_yes: bool,
}

impl Saved {
    pub fn to_json(&self) -> Value {
        json!({"variant": self.variant.as_str(), "since": when::isoformat(&self.since), "by": self.by,
               "screen_yes": self.screen_yes})
    }
    pub fn from_json(v: &Value) -> Option<Saved> {
        Some(Saved {
            variant: Variant::parse(v.get("variant")?.as_str()?)?,
            since: when::parse(v.get("since")?.as_str()?)?,
            by: v
                .get("by")
                .and_then(Value::as_str)
                .unwrap_or("mac")
                .to_owned(),
            screen_yes: v
                .get("screen_yes")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }
}

/// Where the mode is kept across a restart, and the record of what happened in it (ferbd: the journal).
pub trait Persist: Send + Sync {
    /// The mode that was on when FERB last stopped (the last `started` or `resumed` row with no `ended` after it).
    fn load(&self) -> Option<Saved>;
    /// A journal row.
    fn note(&self, kind: &str, data: Value);
}

/// A bus event (topic, data).
pub type Publish = Arc<dyn Fn(&str, Value) + Send + Sync>;
/// One line for the owner on the Mac (ident, text).
pub type Tell = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// Everything the mode reaches outside itself.
#[derive(Clone)]
pub struct Seams {
    pub keep: Arc<dyn KeepAwake>,
    /// APP-986: FERB's root helper (a closed Mac kept working on battery; scheduled wakes), when the owner approved it.
    pub root: Arc<dyn RootHelper>,
    pub facts: Arc<dyn PowerFacts>,
    pub lock: Arc<dyn LockMac>,
    /// How many tasks run now.
    pub working: Arc<dyn Fn() -> usize + Send + Sync>,
    /// The owner unlocked the Mac after this time (the presence watcher's `mac.back_in_use` with signal `unlock`).
    pub unlocked_after: Arc<dyn Fn(&DateTime) -> bool + Send + Sync>,
    pub settings: Arc<dyn Fn() -> Settings + Send + Sync>,
    pub persist: Arc<dyn Persist>,
    /// A bus event (topic, data).
    pub publish: Publish,
    /// One line for the owner on the Mac (a notice in the chat), once per ident.
    pub tell: Tell,
    /// Wall time.
    pub now: Arc<dyn Fn() -> DateTime + Send + Sync>,
    /// Seconds that never jump (the daemon's monotonic clock).
    pub mono: Arc<dyn Fn() -> f64 + Send + Sync>,
    pub new_id: Arc<dyn Fn() -> String + Send + Sync>,
}

/// A warning in force (APP-981).
#[derive(Clone, Debug, PartialEq)]
pub struct Warning {
    pub id: String,
    pub kind: &'static str,
    pub text: String,
    pub at: DateTime,
}

/// A "someone's at your Mac" notice waiting for the owner (APP-983).
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub id: String,
    /// "touched" or "activation".
    pub kind: &'static str,
    pub at: DateTime,
    pub text: String,
    pub app: Option<String>,
    pub count: u64,
    mono: f64,
}

/// The mode while it is on.
#[derive(Clone, Debug)]
pub struct Mode {
    pub variant: Variant,
    pub since: DateTime,
    pub by: String,
    pub screen_yes: bool,
    since_mono: f64,
    /// When the work last became none (monotonic).
    idle_since: Option<f64>,
    /// FERB.app's last "my cover is up" (monotonic).
    shield_seen: Option<f64>,
    camera_access: String,
    warnings: Vec<Warning>,
    notice: Option<Notice>,
}

impl Mode {
    fn shield_up(&self, now: f64) -> bool {
        self.variant == Variant::Shield
            && self.shield_seen.is_some_and(|t| now - t <= SHIELD_LOST_S)
    }
}

#[derive(Default)]
struct Camera {
    on: bool,
    until: f64,
    /// The latest frame (in memory only, never stored or logged): its number and JPEG bytes.
    frame: Option<(i64, Vec<u8>)>,
    /// FERB.app's word on the camera: "on", "off", "denied", "failed".
    state: String,
}

#[derive(Default)]
struct Inner {
    mode: Option<Mode>,
    /// (id, why, at, monotonic)
    last_end: Option<(String, Ended, DateTime, f64)>,
    task_awake: bool,
    reading: PowerReading,
    /// The last look's wall time (seconds) and monotonic time: a wall gap much longer than the monotonic one is sleep.
    last_look: Option<(f64, f64, DateTime)>,
    held: HashSet<Hold>,
    /// APP-986: the helper's sleep lease is held; what the helper answered last.
    root_leased: bool,
    root_state: Option<HelperState>,
    camera: Camera,
    steps: HashMap<String, oneshot::Sender<Result<(), String>>>,
}

/// The insomnia area.
#[derive(Clone)]
pub struct Insomnia {
    seams: Seams,
    inner: Arc<Mutex<Inner>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn secs(t: &DateTime) -> f64 {
    t.timestamp_micros() as f64 / 1e6
}

/// "3:05 PM", in the clock's zone.
pub fn clock_time(t: &DateTime) -> String {
    t.format("%-I:%M %p").to_string()
}

/// The warning words (APP-981): what is true and what FERB does, never that it stops.
pub fn warning_text(kind: &str, battery: Option<i64>) -> String {
    let n = battery.unwrap_or(0);
    match kind {
        "battery_low" => format!("Your Mac's battery is at {n}%. FERB keeps working; plug it in if you can."),
        "battery_urgent" => format!(
            "Your Mac's battery is at {n}%. When it runs out, macOS puts your Mac to sleep and FERB stops until it's \
             charged."
        ),
        "hot" => "Your Mac is getting hot. FERB keeps working, but macOS may slow it down, or put it to sleep if it \
                  gets too hot."
            .to_owned(),
        "lid_closed_battery" => "Your Mac's lid is closed and it's running on battery. FERB keeps it working; keep it \
                                 somewhere it can stay cool, not in a bag."
            .to_owned(),
        "shield_down" => "The cover isn't showing on your Mac's screen, so anyone at your Mac could use it. FERB keeps \
                          working; you can lock your Mac from here."
            .to_owned(),
        _ => String::new(),
    }
}

/// What macOS forced while the mode was on (APP-981), said plainly once: the Mac slept from `from` to `to`.
pub fn slept_text(from: &DateTime, to: &DateTime, before: &PowerReading) -> String {
    let (a, b) = (clock_time(from), clock_time(to));
    if before.lid_closed == Some(true) && before.on_battery == Some(true) {
        format!(
            "Your Mac slept from {a} to {b} because its lid was closed on battery. Plugged in, FERB can keep it \
             working with the lid closed."
        )
    } else if before.on_battery == Some(true) && before.battery.is_some_and(|p| p <= 3) {
        format!("Your Mac slept from {a} to {b} because its battery ran out.")
    } else if before.thermal == Some(Thermal::Critical) {
        format!("Your Mac slept from {a} to {b} because it got too hot.")
    } else {
        format!("Your Mac slept from {a} to {b}. macOS put it to sleep, and FERB can't stop that.")
    }
}

pub const TOUCHED_TEXT: &str = "Someone is using your Mac's keyboard or trackpad.";
pub const RESUMED_TEXT: &str = "FERB restarted and is still keeping your Mac working.";
pub const LOCKED_TEXT: &str = "Your Mac is locked.";
pub const NOT_LOCKED_TEXT: &str =
    "Your Mac's screen is off, but your Mac doesn't ask for your password straight away. \
                                   You can change that in System Settings > Lock Screen.";
pub const LOCK_FAILED_TEXT: &str = "FERB couldn't lock your Mac.";
pub const CAMERA_OFF_TEXT: &str =
    "Your Mac's camera is only for while FERB keeps your Mac working.";
pub const CAMERA_STARTING_TEXT: &str = "Starting your Mac's camera…";
pub const CAMERA_DENIED_TEXT: &str =
    "FERB isn't allowed to use your Mac's camera. You can allow it in System Settings \
                                      > Privacy & Security > Camera.";
pub const CAMERA_NOT_ASKED_TEXT: &str =
    "FERB hasn't asked to use your Mac's camera yet. It asks the next time you turn this on at your Mac.";
pub const CAMERA_FAILED_TEXT: &str = "Your Mac's camera didn't start.";
pub const CAMERA_RELAY_TEXT: &str =
    "Your Mac's camera crosses only a direct connection or your own relay, not the shared relay.";
pub const STEP_REFUSED_TEXT: &str =
    "The cover on your Mac's screen didn't let FERB through, so nothing was clicked or typed. Try the step again.";

fn activation_text(app: &str) -> String {
    format!("{app} came to the front on your Mac, and FERB didn't open it.")
}

impl Insomnia {
    pub fn new(seams: Seams) -> Insomnia {
        Insomnia {
            seams,
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    fn mono(&self) -> f64 {
        (self.seams.mono)()
    }

    /// At the daemon's start: a mode that was on when FERB stopped goes on (journaled `resumed`, said once).
    pub fn resume(&self) {
        let Some(saved) = self.seams.persist.load() else {
            return;
        };
        let now = self.mono();
        {
            let mut i = lock(&self.inner);
            i.mode = Some(Mode {
                variant: saved.variant,
                since: saved.since,
                by: saved.by.clone(),
                screen_yes: saved.screen_yes,
                since_mono: now,
                idle_since: None,
                shield_seen: None,
                camera_access: "unknown".into(),
                warnings: Vec::new(),
                notice: None,
            });
        }
        let mut row = saved.to_json();
        row["state"] = json!("resumed");
        self.seams.persist.note(JOURNAL_KIND, row);
        (self.seams.tell)(
            &format!("insomnia-resumed-{}", when::isoformat(&(self.seams.now)())),
            RESUMED_TEXT,
        );
        self.look();
    }

    /// The owner turns it on (APP-980): from FERB.app's dialog or a phone's signed call. On already: nothing changes.
    pub fn start(&self, variant: Variant, screen_yes: bool, by: &str) -> Map<String, Value> {
        let now = (self.seams.now)();
        let saved = {
            let mut i = lock(&self.inner);
            if i.mode.is_some() {
                drop(i);
                let mut out = self.status();
                out.insert("already".into(), json!(true));
                return out;
            }
            let saved = Saved {
                variant,
                since: now,
                by: by.to_owned(),
                screen_yes: screen_yes && variant == Variant::Shield,
            };
            i.mode = Some(Mode {
                variant,
                since: saved.since,
                by: saved.by.clone(),
                screen_yes: saved.screen_yes,
                since_mono: self.mono(),
                idle_since: None,
                shield_seen: None,
                camera_access: "unknown".into(),
                warnings: Vec::new(),
                notice: None,
            });
            i.last_end = None;
            saved
        };
        let mut row = saved.to_json();
        row["state"] = json!("started");
        self.seams.persist.note(JOURNAL_KIND, row);
        self.look();
        self.status()
    }

    /// Ends it, if on: holds back to the task rule, journaled, published, said once on the Mac (the phone reads
    /// `last_end`).
    pub fn end(&self, why: Ended, by: &str) -> bool {
        let ended = {
            let mut i = lock(&self.inner);
            let Some(_) = i.mode.take() else {
                return false;
            };
            let id = (self.seams.new_id)();
            i.last_end = Some((id.clone(), why, (self.seams.now)(), self.mono()));
            for (_, tx) in i.steps.drain() {
                let _ = tx.send(Err(STEP_REFUSED_TEXT.into()));
            }
            id
        };
        self.camera_off();
        self.seams.persist.note(
            JOURNAL_KIND,
            json!({"state": "ended", "why": why.as_str(), "by": by}),
        );
        (self.seams.tell)(&format!("insomnia-ended-{ended}"), why.line());
        self.look();
        true
    }

    /// One look: the work, the power facts, the holds, the grace, the owner's return, the warnings, sleep that
    /// happened. Called every [`TICK_S`] and after every change.
    pub fn look(&self) {
        let reading = self.seams.facts.read();
        let working = (self.seams.working)();
        let settings = (self.seams.settings)();
        let now_dt = (self.seams.now)();
        let now_wall = secs(&now_dt);
        let now = self.mono();
        let mut tells: Vec<(String, String)> = Vec::new();
        let mut end: Option<Ended> = None;
        let back = {
            let since = lock(&self.inner).mode.as_ref().map(|m| m.since);
            since.is_some_and(|s| (self.seams.unlocked_after)(&s))
        };
        let wanted: HashSet<Hold>;
        {
            let mut i = lock(&self.inner);
            // what macOS forced: a wall-clock gap the monotonic clock didn't see is sleep
            if let Some((wall, mono, at)) = i.last_look {
                let gap = (now_wall - wall) - (now - mono);
                if i.mode.is_some() && gap > SLEPT_GAP_S {
                    let text = slept_text(&at, &now_dt, &i.reading);
                    self.seams.persist.note(
                        "insomnia.slept",
                        json!({"from": when::isoformat(&at), "to": when::isoformat(&now_dt)}),
                    );
                    tells.push((format!("insomnia-slept-{}", when::isoformat(&now_dt)), text));
                }
            }
            i.last_look = Some((now_wall, now, now_dt));
            i.reading = reading.clone();
            if let Some(m) = i.mode.as_mut() {
                if back {
                    end = Some(Ended::Back);
                } else if working > 0 {
                    m.idle_since = None;
                } else {
                    let idle = *m.idle_since.get_or_insert(now);
                    if now - idle >= settings.grace_minutes.max(0) as f64 * 60.0 {
                        end = Some(Ended::Done);
                    }
                }
                let raised = Self::warnings(m, &reading, &settings, now, &now_dt, &self.seams);
                for w in raised {
                    tells.push((format!("insomnia-warning-{}", w.id), w.text.clone()));
                }
            }
            if i.last_end.as_ref().is_some_and(|e| now - e.3 > LAST_END_S) {
                i.last_end = None;
            }
            wanted = match (&i.mode, end) {
                (Some(m), None) => {
                    let mut h = HashSet::from([Hold::IdleSleep]);
                    if reading.on_battery == Some(false) {
                        h.insert(Hold::SystemSleep);
                    }
                    // decision: APP-980 — the display only under the cover (and never under a closed lid): without it an
                    // unlocked Mac must lock as usual
                    if m.shield_up(now) && reading.lid_closed != Some(true) {
                        h.insert(Hold::Display);
                    }
                    i.task_awake = false;
                    h
                }
                _ => {
                    // the owner's agreement: with the mode off, idle sleep only while a task runs and the lid is open
                    i.task_awake = working > 0 && reading.lid_closed != Some(true);
                    if i.task_awake {
                        HashSet::from([Hold::IdleSleep])
                    } else {
                        HashSet::new()
                    }
                }
            };
        }
        let reason = if lock(&self.inner).mode.is_some() && end.is_none() {
            REASON_MODE
        } else {
            REASON_TASK
        };
        for h in Hold::ALL {
            if wanted.contains(&h) {
                if let Err(e) = self.seams.keep.hold(h, reason, LEASE_S) {
                    self.seams.persist.note(
                        "insomnia.hold_failed",
                        json!({"hold": h.as_str(), "error": e}),
                    );
                }
            } else if lock(&self.inner).held.contains(&h) {
                self.seams.keep.release(h);
            }
        }
        lock(&self.inner).held = wanted;
        // decision: APP-986 — the root helper's lease while the mode is on: a closed Mac on battery keeps working; it
        // ends by itself when not renewed, and is let go at the mode's end
        let on = lock(&self.inner).mode.is_some() && end.is_none();
        let leased = lock(&self.inner).root_leased;
        if on {
            let r = self.seams.root.lease(LEASE_S);
            let mut i = lock(&self.inner);
            i.root_leased = r.is_ok();
            i.root_state = Some(if r.is_ok() {
                HelperState::Ready
            } else {
                HelperState::Absent
            });
        } else if leased {
            let _ = self.seams.root.release();
            lock(&self.inner).root_leased = false;
        }
        for (ident, text) in tells {
            (self.seams.tell)(&ident, &text);
        }
        if let Some(why) = end {
            self.end(why, "ferb");
            return;
        }
        self.publish();
    }

    /// The warnings in force for this reading (APP-981): each raised once while its condition lasts, gone when it
    /// clears (so a phone removes its notification). Answers the ones raised now.
    fn warnings(
        m: &mut Mode,
        r: &PowerReading,
        s: &Settings,
        now: f64,
        at: &DateTime,
        seams: &Seams,
    ) -> Vec<Warning> {
        let battery = r.battery.filter(|_| r.on_battery == Some(true));
        let urgent = battery.is_some_and(|b| b <= s.battery_urgent);
        let low = !urgent && battery.is_some_and(|b| b <= s.battery_warn);
        let hot = r.thermal.is_some_and(|t| t >= Thermal::Serious);
        let shield_down = m.variant == Variant::Shield
            && now - m.since_mono > SHIELD_FIRST_S
            && !m.shield_up(now);
        // decision: APP-981 — a closed lid on battery is said, never refused (with the helper it keeps working)
        let lid_battery = r.lid_closed == Some(true) && r.on_battery == Some(true);
        let conditions = [
            ("battery_low", low),
            ("battery_urgent", urgent),
            ("hot", hot),
            ("shield_down", shield_down),
            ("lid_closed_battery", lid_battery),
        ];
        let mut raised = Vec::new();
        for (kind, on) in conditions {
            let present = m.warnings.iter().position(|w| w.kind == kind);
            match (on, present) {
                (true, None) => {
                    let w = Warning {
                        id: (seams.new_id)(),
                        kind,
                        text: warning_text(kind, r.battery),
                        at: *at,
                    };
                    seams.persist.note(
                        "insomnia.warning",
                        json!({"kind": kind, "battery": r.battery}),
                    );
                    m.warnings.push(w.clone());
                    raised.push(w);
                }
                (false, Some(p)) => {
                    m.warnings.remove(p);
                }
                _ => {}
            }
        }
        raised
    }

    fn publish(&self) {
        (self.seams.publish)(TOPIC, Value::Object(self.status()));
    }

    /// The status every client shows (the contract's object; FERB.app, `chat.status`'s `insomnia`, the bus).
    pub fn status(&self) -> Map<String, Value> {
        let settings = (self.seams.settings)();
        let now = self.mono();
        let i = lock(&self.inner);
        let r = &i.reading;
        let power = json!({"battery": r.battery, "on_battery": r.on_battery, "lid_closed": r.lid_closed,
                           "thermal": r.thermal.map(Thermal::as_str)});
        let last_end = i.last_end.as_ref().map(|(id, why, at, _)| {
            json!({"id": id, "why": why.as_str(), "text": why.line(), "at": when::isoformat(at)})
        });
        let mut out = Map::new();
        let Some(m) = &i.mode else {
            out.insert("on".into(), json!(false));
            out.insert("power".into(), power);
            out.insert("task_awake".into(), json!(i.task_awake));
            out.insert("last_end".into(), last_end.unwrap_or(Value::Null));
            out.insert("line".into(), json!(""));
            return out;
        };
        let working = (self.seams.working)();
        let ends_at = m.idle_since.filter(|_| working == 0).map(|idle| {
            let left = (idle + settings.grace_minutes.max(0) as f64 * 60.0 - now).max(0.0);
            (self.seams.now)() + chrono::Duration::milliseconds((left * 1000.0) as i64)
        });
        let shield_up = m.shield_up(now);
        let mut parts = vec!["Keeping your Mac working".to_owned()];
        if working > 0 {
            parts.push(if working == 1 {
                "1 task running".to_owned()
            } else {
                format!("{working} tasks running")
            });
        } else if let Some(t) = &ends_at {
            parts.push(format!("ends {} if nothing new comes", clock_time(t)));
        }
        if r.screen_locked == Some(true) {
            parts.push("Mac locked".into());
        } else if m.variant == Variant::Shield && !shield_up && now - m.since_mono > SHIELD_FIRST_S
        {
            parts.push("cover not showing".into());
        }
        if let (Some(b), Some(true)) = (r.battery, r.on_battery) {
            parts.push(format!("battery {b}%"));
        }
        out.insert("on".into(), json!(true));
        out.insert("variant".into(), json!(m.variant.as_str()));
        out.insert("since".into(), json!(when::isoformat(&m.since)));
        out.insert("by".into(), json!(m.by));
        out.insert("screen_yes".into(), json!(m.screen_yes));
        out.insert("grace_minutes".into(), json!(settings.grace_minutes));
        out.insert("working".into(), json!(working));
        out.insert(
            "ends_at".into(),
            ends_at.map_or(Value::Null, |t| json!(when::isoformat(&t))),
        );
        out.insert("shield_up".into(), json!(shield_up));
        // APP-986: whether FERB's helper keeps a closed Mac working on battery ("ready") or isn't approved ("absent")
        out.insert(
            "closed_lid".into(),
            json!(i.root_state.map_or("unknown", HelperState::as_str)),
        );
        out.insert("mac_locked".into(), json!(r.screen_locked == Some(true)));
        out.insert(
            "camera".into(),
            json!({"on": i.camera.on, "state": m.camera_access}),
        );
        out.insert("power".into(), power);
        out.insert(
            "warnings".into(),
            Value::Array(
                m.warnings
                    .iter()
                    .map(|w| json!({"id": w.id, "kind": w.kind, "text": w.text, "at": when::isoformat(&w.at)}))
                    .collect(),
            ),
        );
        out.insert(
            "notice".into(),
            m.notice.as_ref().map_or(Value::Null, |n| {
                json!({"id": n.id, "kind": n.kind, "at": when::isoformat(&n.at), "text": n.text, "app": n.app,
                       "count": n.count})
            }),
        );
        out.insert("line".into(), json!(parts.join(" · ")));
        out.insert("last_end".into(), Value::Null);
        out
    }

    /// The session's since when the one Yes covers screen use now (APP-984): the gateway's standing answer.
    pub fn screen_yes_since(&self) -> Option<String> {
        lock(&self.inner)
            .mode
            .as_ref()
            .filter(|m| m.screen_yes)
            .map(|m| when::isoformat(&m.since))
    }

    /// FERB.app's report from its cover.
    pub fn report(&self, params: &Params) -> Result<(), String> {
        let event = params.get("event").and_then(Value::as_str).unwrap_or("");
        let text = |k: &str| {
            params
                .get(k)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        };
        let now = self.mono();
        match event {
            "shield" => {
                let changed = {
                    let mut i = lock(&self.inner);
                    let Some(m) = i.mode.as_mut() else {
                        return Ok(());
                    };
                    let was = m.shield_up(now);
                    m.shield_seen = (text("state") == "up").then_some(now);
                    let camera = text("camera");
                    if !camera.is_empty() {
                        m.camera_access = camera;
                    }
                    was != m.shield_up(now)
                };
                if changed {
                    self.look();
                }
                Ok(())
            }
            "person_input" | "activation" => {
                let at = (self.seams.now)();
                {
                    let mut i = lock(&self.inner);
                    let Some(m) = i.mode.as_mut().filter(|m| m.variant == Variant::Shield) else {
                        return Ok(());
                    };
                    let kind: &'static str = if event == "activation" {
                        "activation"
                    } else {
                        "touched"
                    };
                    let app = (kind == "activation")
                        .then(|| text("app"))
                        .filter(|a| !a.is_empty());
                    match m.notice.as_mut() {
                        Some(n) if n.kind == kind && now - n.mono < EPISODE_S && n.app == app => {
                            n.count += 1;
                            n.at = at;
                            n.mono = now;
                        }
                        _ => {
                            let words = match &app {
                                Some(a) => activation_text(a),
                                None if kind == "activation" => activation_text("An app"),
                                None => TOUCHED_TEXT.to_owned(),
                            };
                            m.notice = Some(Notice {
                                id: (self.seams.new_id)(),
                                kind,
                                at,
                                text: words,
                                app: app.clone(),
                                count: 1,
                                mono: now,
                            });
                        }
                    }
                }
                let row = if event == "activation" {
                    json!({"app": text("app"), "bundle": text("bundle")})
                } else {
                    json!({"input": text("input"),
                           "during_step": params.get("during_step").and_then(Value::as_bool).unwrap_or(false)})
                };
                let kind = if event == "activation" {
                    "insomnia.activation"
                } else {
                    "insomnia.touched"
                };
                self.seams.persist.note(kind, row);
                self.publish();
                Ok(())
            }
            "step_ready" | "step_failed" => {
                let step = text("step");
                let tx = lock(&self.inner).steps.remove(&step);
                if let Some(tx) = tx {
                    let _ = tx.send(if event == "step_ready" {
                        Ok(())
                    } else {
                        Err(STEP_REFUSED_TEXT.into())
                    });
                }
                Ok(())
            }
            "camera" => {
                let state = text("state");
                lock(&self.inner).camera.state = state;
                Ok(())
            }
            other => Err(format!("insomnia.report doesn't know the event {other:?}")),
        }
    }

    /// The owner's It's fine: the notice is gone.
    pub fn dismiss(&self, notice_id: &str) -> bool {
        let gone = {
            let mut i = lock(&self.inner);
            match i.mode.as_mut() {
                Some(m) if m.notice.as_ref().is_some_and(|n| n.id == notice_id) => {
                    m.notice = None;
                    true
                }
                _ => false,
            }
        };
        if gone {
            self.seams
                .persist
                .note("insomnia.notice_answered", json!({"answer": "fine"}));
            self.publish();
        }
        gone
    }

    /// The owner's Lock my Mac (APP-983): the display sleeps and the lock is read back. The notice is answered; the
    /// mode goes on (his unlock later ends it: he's back).
    pub fn lock_mac(&self) -> (bool, &'static str) {
        let result = self.seams.lock.lock_now();
        let (locked, text) = match &result {
            Ok(true) => (true, LOCKED_TEXT),
            Ok(false) => (false, NOT_LOCKED_TEXT),
            Err(_) => (false, LOCK_FAILED_TEXT),
        };
        if let Some(m) = lock(&self.inner).mode.as_mut() {
            m.notice = None;
        }
        self.seams.persist.note(
            "insomnia.notice_answered",
            json!({"answer": "lock", "locked": locked, "error": result.err()}),
        );
        self.look();
        (locked, text)
    }

    /// One pull of Show me: keeps the camera on [`CAMERA_KEEP_S`] more and answers the latest frame when it is not the
    /// one the phone shows (`have`). Frames live here only.
    pub fn camera_pull(&self, have: Option<i64>) -> Result<Map<String, Value>, String> {
        let now = self.mono();
        let start = {
            let mut i = lock(&self.inner);
            let Some(access) = i.mode.as_ref().map(|m| m.camera_access.clone()) else {
                return Err(CAMERA_OFF_TEXT.into());
            };
            let c = &mut i.camera;
            c.until = now + CAMERA_KEEP_S;
            let start = !c.on;
            if start {
                c.on = true;
                c.frame = None;
                c.state = "starting".into();
            }
            let (state, text) = match (access.as_str(), c.state.as_str(), &c.frame) {
                ("denied", _, _) | (_, "denied", _) => ("denied", CAMERA_DENIED_TEXT),
                ("not_asked", _, None) => ("denied", CAMERA_NOT_ASKED_TEXT),
                (_, "failed", _) => ("failed", CAMERA_FAILED_TEXT),
                (_, _, Some(_)) => ("live", ""),
                _ => ("starting", CAMERA_STARTING_TEXT),
            };
            let mut out = Map::new();
            out.insert("state".into(), json!(state));
            out.insert("text".into(), json!(text));
            match &c.frame {
                Some((seq, bytes)) => {
                    out.insert("seq".into(), json!(seq));
                    if have != Some(*seq) {
                        use base64::Engine;
                        out.insert(
                            "image".into(),
                            json!(base64::engine::general_purpose::STANDARD.encode(bytes)),
                        );
                    }
                }
                None => {
                    out.insert("seq".into(), Value::Null);
                }
            }
            (start, out)
        };
        if start.0 {
            (self.seams.publish)(CAMERA_TOPIC, json!({"on": true}));
            self.seams
                .persist
                .note("insomnia.camera", json!({"on": true}));
        }
        Ok(start.1)
    }

    /// FERB.app's frame: kept (the latest only) while the camera is wanted, dropped otherwise.
    pub fn camera_frame(&self, seq: i64, jpeg: Vec<u8>) {
        let mut i = lock(&self.inner);
        if i.camera.on && i.mode.is_some() {
            i.camera.frame = Some((seq, jpeg));
        }
    }

    /// The camera's watchdog: off once nobody pulled for [`CAMERA_KEEP_S`].
    pub fn camera_check(&self) {
        let stale = {
            let i = lock(&self.inner);
            i.camera.on && self.mono() > i.camera.until
        };
        if stale {
            self.camera_off();
        }
    }

    fn camera_off(&self) {
        let was = {
            let mut i = lock(&self.inner);
            let was = i.camera.on;
            i.camera = Camera::default();
            was
        };
        if was {
            (self.seams.publish)(CAMERA_TOPIC, json!({"on": false}));
            self.seams
                .persist
                .note("insomnia.camera", json!({"on": false}));
        }
    }

    /// Whether a camera frame is held now (tests: nothing outlives the watching).
    pub fn camera_holds_frame(&self) -> bool {
        lock(&self.inner).camera.frame.is_some()
    }

    /// The holds taken now (tests and `ferb trace`'s readers).
    pub fn held(&self) -> HashSet<Hold> {
        lock(&self.inner).held.clone()
    }

    /// Before FERB brings `bundle` forward for real input (APP-984): with the cover up, FERB.app is asked to let input
    /// through it and to expect that app; the step goes on once it says so (`Some(step)`, end it with
    /// [`Insomnia::step_end`]), and is refused if it doesn't within [`STEP_WAIT_S`]. Without the cover: `None` at once.
    pub async fn step_begin(&self, pid: i32, bundle: &str) -> Result<Option<String>, String> {
        let (step, rx) = {
            let mut i = lock(&self.inner);
            let now = self.mono();
            if !i.mode.as_ref().is_some_and(|m| m.shield_up(now)) {
                return Ok(None);
            }
            let step = format!("s-{}", (self.seams.new_id)());
            let (tx, rx) = oneshot::channel();
            i.steps.insert(step.clone(), tx);
            (step, rx)
        };
        (self.seams.publish)(
            STEP_TOPIC,
            json!({"step": step, "phase": "begin", "pid": pid, "bundle": bundle}),
        );
        let answer = tokio::time::timeout(Duration::from_secs_f64(STEP_WAIT_S), rx).await;
        lock(&self.inner).steps.remove(&step);
        match answer {
            Ok(Ok(Ok(()))) => Ok(Some(step)),
            Ok(Ok(Err(e))) => {
                self.step_end(&step);
                Err(e)
            }
            _ => {
                self.step_end(&step);
                Err(STEP_REFUSED_TEXT.into())
            }
        }
    }

    /// The step is over: the cover catches input again and takes the front back.
    pub fn step_end(&self, step: &str) {
        (self.seams.publish)(STEP_TOPIC, json!({"step": step, "phase": "end"}));
    }

    /// FERB is about to bring `bundle` forward for background work: with the cover up, its activation is FERB's own.
    pub fn expect(&self, bundle: &str) {
        let up = {
            let i = lock(&self.inner);
            let now = self.mono();
            i.mode.as_ref().is_some_and(|m| m.shield_up(now))
        };
        if up && !bundle.is_empty() {
            (self.seams.publish)(EXPECT_TOPIC, json!({"bundle": bundle, "seconds": EXPECT_S}));
        }
    }

    /// Whether the mode is on now.
    pub fn on(&self) -> bool {
        lock(&self.inner).mode.is_some()
    }

    /// Registers `insomnia.*` on `server`.
    pub fn register(&self, server: &Server) {
        register(self.clone(), server);
    }
}

/// APP-986: FERB's own wakes match the owner's schedules: a wake [`WAKE_EARLY_S`] before each of the next active
/// schedules' runs (`next_runs`, POSIX seconds), only in the future and at most the helper's maximum ahead and in
/// number; a wake no longer wanted is cancelled. Answers (scheduled, cancelled).
pub fn sync_wakes(
    root: &dyn RootHelper,
    next_runs: &[i64],
    now: i64,
) -> Result<(usize, usize), String> {
    use ferb_core::power::helper::{MAX_AHEAD_S, MAX_WAKES};
    let mut wanted: Vec<i64> = next_runs
        .iter()
        .map(|t| t - WAKE_EARLY_S)
        .filter(|t| *t > now + 60 && *t <= now + MAX_AHEAD_S)
        .collect();
    wanted.sort_unstable();
    wanted.dedup();
    wanted.truncate(MAX_WAKES);
    let have = root.wakes()?;
    let mut cancelled = 0;
    for w in &have {
        if !wanted.contains(w) {
            root.unwake(*w)?;
            cancelled += 1;
        }
    }
    let mut scheduled = 0;
    for w in &wanted {
        if !have.contains(w) {
            root.wake(*w)?;
            scheduled += 1;
        }
    }
    Ok((scheduled, cancelled))
}

/// How long before a scheduled run FERB wakes the Mac.
pub const WAKE_EARLY_S: i64 = 60;

fn refusal(message: impl Into<String>) -> HandlerError {
    crate::shared::refusal(message)
}

fn by_of(s: &ferb_rpc::Session) -> String {
    s.phone
        .as_ref()
        .map_or_else(|| "mac".to_owned(), |p| p.device.clone())
}

fn register(ins: Insomnia, server: &Server) {
    let i = ins.clone();
    server.register_unary("insomnia.status", move |_s, _p: Params| {
        let out = i.status();
        async move { Ok(out) }
    });
    let i = ins.clone();
    server.register_unary("insomnia.start", move |s: ferb_rpc::Session, p: Params| {
        let i = i.clone();
        async move {
            let variant = match p.get("variant").and_then(Value::as_str) {
                None | Some("") => Variant::Shield,
                Some(v) => Variant::parse(v)
                    .ok_or_else(|| refusal("variant must be \"shield\" or \"locked\""))?,
            };
            let screen_yes = p
                .get("screen_yes")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let by = by_of(&s);
            tokio::task::spawn_blocking(move || i.start(variant, screen_yes, &by))
                .await
                .map_err(|e| refusal(e.to_string()))
        }
    });
    let i = ins.clone();
    server.register_unary("insomnia.end", move |s: ferb_rpc::Session, p: Params| {
        let i = i.clone();
        async move {
            // only FERB.app's cover says the owner is back (his Touch ID or password there); a phone only turns it off
            let why = if s.phone.is_none() && p.get("why").and_then(Value::as_str) == Some("back") {
                Ended::Back
            } else {
                Ended::TurnedOff
            };
            let by = by_of(&s);
            let i2 = i.clone();
            let ended = tokio::task::spawn_blocking(move || i2.end(why, &by))
                .await
                .map_err(|e| refusal(e.to_string()))?;
            let mut out = i.status();
            out.insert("ended".into(), json!(ended));
            out.insert("by".into(), json!(by_of(&s)));
            Ok(out)
        }
    });
    let i = ins.clone();
    server.register_unary("insomnia.lock", move |_s, _p: Params| {
        let i = i.clone();
        async move {
            let i2 = i.clone();
            let (locked, text) = tokio::task::spawn_blocking(move || i2.lock_mac())
                .await
                .map_err(|e| refusal(e.to_string()))?;
            let mut out = Map::new();
            out.insert("locked".into(), json!(locked));
            out.insert("text".into(), json!(text));
            out.insert("insomnia".into(), Value::Object(i.status()));
            Ok(out)
        }
    });
    let i = ins.clone();
    server.register_unary("insomnia.dismiss", move |_s, p: Params| {
        let i = i.clone();
        async move {
            let id = crate::shared::text(&p, "notice_id");
            let gone = i.dismiss(&id);
            let mut out = i.status();
            out.insert("dismissed".into(), json!(gone));
            Ok(out)
        }
    });
    let i = ins.clone();
    server.register_unary("insomnia.camera", move |s: ferb_rpc::Session, p: Params| {
        let i = i.clone();
        async move {
            // decision: APP-983 — Show me crosses only a direct connection or the owner's relay (PHN-454's live-view rule)
            if let Some(phone) = &s.phone {
                if !matches!(phone.tier.as_deref(), Some("direct" | "own_relay")) {
                    return Err(HandlerError::coded(
                        ferb_rpc::phone_gate::FILE_NOT_ALLOWED,
                        CAMERA_RELAY_TEXT,
                    ));
                }
            }
            let have = p.get("have").and_then(Value::as_i64);
            i.camera_pull(have).map_err(refusal)
        }
    });
    let i = ins.clone();
    server.register_unary("insomnia.camera_frame", move |_s, p: Params| {
        let i = i.clone();
        async move {
            use base64::Engine;
            let seq = p.get("seq").and_then(Value::as_i64).unwrap_or(0);
            let jpeg = p
                .get("jpeg")
                .and_then(Value::as_str)
                .and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok())
                .ok_or_else(|| refusal("jpeg must be the frame's JPEG in base64"))?;
            i.camera_frame(seq, jpeg);
            Ok(Map::new())
        }
    });
    let i = ins;
    server.register_unary("insomnia.report", move |_s, p: Params| {
        let i = i.clone();
        async move {
            let i2 = i.clone();
            tokio::task::spawn_blocking(move || i2.report(&p))
                .await
                .map_err(|e| refusal(e.to_string()))?
                .map_err(refusal)?;
            Ok(Map::new())
        }
    });
}
