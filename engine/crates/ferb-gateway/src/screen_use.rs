//! "Until I'm back at my Mac" (owner, 2026-11-17, T-1215): a Yes to FERB using the Mac's screen, given from a paired,
//! unlocked phone, that also answers Yes to every later screen-use question until the owner is back at the Mac.
//!
//! It covers one question kind only: the foreground question for input FERB posts itself (`floor:foreground`, every
//! need of the question that rule, none of them a script). Money, account security, deletion, new recipients, terminal
//! typing, a script (`mac.applescript`: its System Events input carries no FERB mark) and every other question still
//! ask (rule 5); nothing rule 6 never lifts is touched. It ends at the first of ([`Ended`]):
//!
//! - the owner's own keyboard or pointer input on the Mac after it was given: an event without FERB's mark that is
//!   **clear**, i.e. more than [`UNCLEAR_WINDOW_S`] after FERB's own last posted event (GATE-618);
//! - an **unclear** event (inside that window: macOS might have made it from one of FERB's own posts) holds screen use
//!   for [`HOLD_S`] and asks "Are you back?" on the Mac; any input during the hold is the owner's and ends it; a hold
//!   that passes in silence was FERB's echo: screen use goes on and unclear events are not asked about again for
//!   [`QUIET_S`] (clear events still end it) (GATE-618, GATE-619);
//! - a message sent from FERB.app on the Mac;
//! - the Mac out of use: asleep, its screen locked or asleep, or its login session switched away;
//! - FERB unable to tell any more (the input tap or the presence watcher stopped);
//! - the owner turning it off (from the phone or the Mac), the phone that gave it being removed, or FERB restarting.
//!
//! The phone locking does not end it: the owner is away from the Mac with the phone in a pocket. It is offered only
//! while FERB can tell (Input Monitoring on and the tap running, the presence watcher running) and the Mac is in use.
//! In memory only: a restart ends it, and the daemon says so once (journal kind [`JOURNAL_KIND`]).

use ferb_core::when::DateTime;
use ferb_policy::confirm::{self, Need};
use serde_json::{json, Value};

/// The journal kind of a start, a hold, its end, an ignored unclear event and an end (`state`).
pub const JOURNAL_KIND: &str = "screen.until_back";
/// The bus topic of a start, a check and an end: FERB.app's Settings → Phone line, the quiet "Are you back?" line in
/// the notch, and the owner's notice when it ends.
pub const TOPIC: &str = "screen.until_back";
/// GATE-618: an unmarked event this soon after FERB's own last posted input event might be macOS's own echo of that
/// event (a mouse-moved or a modifier change it derives from a posted click or key), so it is unclear. macOS hands a
/// listen-only session tap its events asynchronously: derived ones follow the posted event in the same dispatch,
/// within milliseconds, but a busy WindowServer delivers a tap's callbacks tens to a few hundred milliseconds late (it
/// disables a tap whose callback stalls about a second). Half a second covers that lag; anything later is the owner,
/// whose own movement goes on well past it.
pub const UNCLEAR_WINDOW_S: f64 = 0.5;
/// GATE-618 (owner: "a 10second ttl question"): how long an unclear event holds screen use while FERB asks.
pub const HOLD_S: f64 = 10.0;
/// GATE-619 (owner: "maybe not ask for a some time of the 10s quesion isnt answered"): after a hold that passed in
/// silence, unclear events are not asked about for this long (10 minutes, the owner's APP-958).
// decision: APP-958
pub const QUIET_S: f64 = 600.0;
/// The quiet line FERB.app shows in the notch while it holds (no sound, nothing to the phone).
pub const CHECK_TITLE: &str = "Are you back?";
pub const CHECK_LINE: &str =
    "FERB stopped using your screen for a moment. If you're here, just move the pointer or press a key.";

/// What FERB can see of the owner at the Mac (ferbd: the owner-input tap and the Mac's presence; the times are POSIX
/// seconds on the daemon's clock).
pub trait AtTheMac: Send + Sync {
    /// FERB can tell the owner's keyboard and pointer from its own (Input Monitoring granted and its tap running) and
    /// watches whether the Mac is in use.
    fn can_tell(&self) -> bool;
    /// The last event without FERB's mark that came more than [`UNCLEAR_WINDOW_S`] after FERB's own last posted event.
    fn clear_input_at(&self) -> Option<f64>;
    /// The last event without FERB's mark that came within [`UNCLEAR_WINDOW_S`] of FERB's own last posted event.
    fn unclear_input_at(&self) -> Option<f64>;
    /// FERB's own last posted keyboard or pointer event.
    fn ferb_posted_at(&self) -> Option<f64>;
    /// The Mac is out of use now: asleep, its screen locked or asleep, or its session switched away.
    fn away(&self) -> bool;
}

/// One "until I'm back" in force.
#[derive(Clone, Debug, PartialEq)]
pub struct UntilBack {
    pub since: DateTime,
    /// The phone that gave it (its endpoint) and its name in FERB's list.
    pub phone: String,
    pub phone_name: String,
    /// The question whose answer gave it.
    pub approval_id: String,
    /// A hold under way: (since, until), POSIX seconds. FERB posts no screen input while it lasts.
    pub hold: Option<(f64, f64)>,
    /// After a hold that passed in silence: unclear events are let pass until then.
    pub quiet_until: Option<f64>,
    /// The latest unclear event already handled.
    pub seen_unclear: f64,
    /// How many holds there were (the first one's line is shown).
    pub holds: u32,
}

impl UntilBack {
    pub fn new(since: DateTime, phone: &str, phone_name: &str, approval_id: &str) -> UntilBack {
        UntilBack {
            since,
            phone: phone.into(),
            phone_name: phone_name.into(),
            approval_id: approval_id.into(),
            hold: None,
            quiet_until: None,
            seen_unclear: 0.0,
            holds: 0,
        }
    }

    /// What a client shows while it runs (a hold is not shown to the phone: the status stays as it is).
    pub fn status(&self) -> Value {
        json!({"active": true, "since": ferb_core::when::isoformat(&self.since), "phone": self.phone,
               "phone_name": self.phone_name})
    }

    fn since_s(&self) -> f64 {
        self.since.timestamp_micros() as f64 / 1e6
    }
}

/// Why it ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    /// The owner's own keyboard or pointer input on the Mac.
    OwnerInput,
    /// A message sent from FERB.app on the Mac.
    MacMessage,
    /// The Mac asleep, its screen locked or asleep, or its session switched away.
    MacAway,
    /// The input tap or the presence watcher stopped: FERB can't tell when the owner is back.
    CannotTell,
    /// The owner turned it off (from a phone or the Mac).
    TurnedOff,
    /// The phone that gave it was removed.
    PhoneRemoved,
    /// The phone's answer that gave it was not taken: nothing is said.
    AnswerFailed,
    /// FERB restarted while it ran (found at the next start).
    Restarted,
}

impl Ended {
    pub fn as_str(self) -> &'static str {
        match self {
            Ended::OwnerInput => "owner_input",
            Ended::MacMessage => "mac_message",
            Ended::MacAway => "mac_away",
            Ended::CannotTell => "cannot_tell",
            Ended::TurnedOff => "turned_off",
            Ended::PhoneRemoved => "phone_removed",
            Ended::AnswerFailed => "answer_failed",
            Ended::Restarted => "restarted",
        }
    }

    pub fn parse(s: &str) -> Option<Ended> {
        [
            Ended::OwnerInput,
            Ended::MacMessage,
            Ended::MacAway,
            Ended::CannotTell,
            Ended::TurnedOff,
            Ended::PhoneRemoved,
            Ended::AnswerFailed,
            Ended::Restarted,
        ]
        .into_iter()
        .find(|e| e.as_str() == s)
    }

    /// The owner's notice in chat, once, when it ended (none for an answer that was not taken).
    pub fn line(self) -> Option<&'static str> {
        Some(match self {
            Ended::OwnerInput => {
                "Welcome back. FERB will ask before it uses your screen again: a task that needs it waits for your Yes."
            }
            Ended::MacMessage => {
                "You sent a message from your Mac, so FERB will ask again before it uses your Mac's screen."
            }
            Ended::MacAway => {
                "Your Mac locked or went to sleep, so FERB will ask again before it uses your Mac's screen."
            }
            Ended::CannotTell => {
                "FERB can't tell any more when you're back at your Mac, so it will ask again before it uses your Mac's \
                 screen."
            }
            Ended::TurnedOff => "Done. FERB will ask again before it uses your Mac's screen.",
            Ended::PhoneRemoved => {
                "The phone that allowed it was removed, so FERB will ask again before it uses your Mac's screen."
            }
            Ended::Restarted => {
                "FERB restarted, so it will ask again before it uses your Mac's screen."
            }
            Ended::AnswerFailed => return None,
        })
    }
}

/// The question is the screen-use question alone, for input FERB posts itself: every one of its needs is the foreground
/// rule and none is a script (whose System Events input carries no FERB mark).
pub fn screen_only(needs: &[Need]) -> bool {
    !needs.is_empty()
        && needs
            .iter()
            .all(|n| n.rule == confirm::FOREGROUND && !n.values.has("script"))
}

/// Whether "until I'm back" may be given now: FERB can tell and the Mac is in use.
pub fn offered(at: Option<&dyn AtTheMac>) -> bool {
    at.is_some_and(|a| a.can_tell() && !a.away())
}

/// What one look at the owner did to a running "until I'm back".
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    /// Nothing changed (a hold may be under way).
    Keep,
    /// It ends, with why.
    Ended(Ended),
    /// An unclear event at `at` started a hold until `until`; `first`: the first hold of this grant.
    Held { at: f64, until: f64, first: bool },
    /// A hold passed with no input: screen use goes on; unclear events are let pass until `quiet_until`.
    Resumed { quiet_until: f64 },
    /// An unclear event inside the quiet time was let pass.
    Ignored { at: f64 },
}

/// GATE-618, GATE-619: one look at the owner at `now`, changing `g` as it says. The order: FERB can't tell; a clear
/// event (always ends, quiet time or not); during a hold any event after FERB's last own post (plus the window: the
/// call that was running finishes its posting) ends it, else the hold runs out; the Mac out of use; a new unclear event.
pub fn step(g: &mut UntilBack, now: f64, at: Option<&dyn AtTheMac>) -> Step {
    let Some(at) = at else {
        return Step::Ended(Ended::CannotTell);
    };
    if !at.can_tell() {
        return Step::Ended(Ended::CannotTell);
    }
    let since = g.since_s();
    let clear = at.clear_input_at();
    if clear.is_some_and(|t| t > since) {
        return Step::Ended(Ended::OwnerInput);
    }
    if let Some((held, until)) = g.hold {
        let from = at
            .ferb_posted_at()
            .map_or(held, |p| held.max(p + UNCLEAR_WINDOW_S));
        let any = clear
            .into_iter()
            .chain(at.unclear_input_at())
            .fold(f64::MIN, f64::max);
        if any > from {
            return Step::Ended(Ended::OwnerInput);
        }
        if now < until {
            return Step::Keep;
        }
        g.hold = None;
        let quiet_until = now + QUIET_S;
        g.quiet_until = Some(quiet_until);
        return Step::Resumed { quiet_until };
    }
    if at.away() {
        return Step::Ended(Ended::MacAway);
    }
    match at.unclear_input_at() {
        Some(t) if t > since && t > g.seen_unclear => {
            g.seen_unclear = t;
            if g.quiet_until.is_some_and(|q| now < q) {
                return Step::Ignored { at: t };
            }
            g.holds += 1;
            let until = now + HOLD_S;
            g.hold = Some((now, until));
            Step::Held {
                at: t,
                until,
                first: g.holds == 1,
            }
        }
        _ => Step::Keep,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferb_policy::confirm::Values;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Mac {
        blind: bool,
        away: bool,
        clear: Mutex<Option<f64>>,
        unclear: Mutex<Option<f64>>,
        posted: Mutex<Option<f64>>,
    }

    impl AtTheMac for Mac {
        fn can_tell(&self) -> bool {
            !self.blind
        }
        fn clear_input_at(&self) -> Option<f64> {
            *self.clear.lock().unwrap()
        }
        fn unclear_input_at(&self) -> Option<f64> {
            *self.unclear.lock().unwrap()
        }
        fn ferb_posted_at(&self) -> Option<f64> {
            *self.posted.lock().unwrap()
        }
        fn away(&self) -> bool {
            self.away
        }
    }

    fn grant() -> UntilBack {
        UntilBack::new(
            ferb_core::when::parse("2026-11-17T10:00:00+00:00").unwrap(),
            "ep1",
            "Pixel",
            "ap-1",
        )
    }

    fn t0() -> f64 {
        grant().since.timestamp() as f64
    }

    // decision: GATE-615
    // decision: GATE-618
    #[test]
    fn a_clear_event_ends_it_and_so_does_a_mac_out_of_use_or_blind() {
        let mac = Mac::default();
        *mac.clear.lock().unwrap() = Some(t0() - 5.0);
        let mut g = grant();
        assert_eq!(
            step(&mut g, t0() + 1.0, Some(&mac)),
            Step::Keep,
            "input before it was given"
        );
        *mac.clear.lock().unwrap() = Some(t0() + 0.5);
        assert_eq!(
            step(&mut g, t0() + 1.0, Some(&mac)),
            Step::Ended(Ended::OwnerInput)
        );
        let away = Mac {
            away: true,
            ..Mac::default()
        };
        assert_eq!(
            step(&mut grant(), t0() + 1.0, Some(&away)),
            Step::Ended(Ended::MacAway)
        );
        let blind = Mac {
            blind: true,
            ..Mac::default()
        };
        assert_eq!(
            step(&mut grant(), t0() + 1.0, Some(&blind)),
            Step::Ended(Ended::CannotTell)
        );
        assert_eq!(
            step(&mut grant(), t0() + 1.0, None),
            Step::Ended(Ended::CannotTell)
        );
        assert!(!offered(Some(&blind)) && !offered(Some(&away)) && !offered(None));
    }

    /// An unclear event holds for HOLD_S; any input after FERB's last own post (plus the window) is the owner's.
    // decision: GATE-618
    #[test]
    fn an_unclear_event_holds_and_any_input_during_the_hold_ends_it() {
        let (mac, mut g) = (Mac::default(), grant());
        *mac.posted.lock().unwrap() = Some(t0() + 10.0);
        *mac.unclear.lock().unwrap() = Some(t0() + 10.2);
        let now = t0() + 11.0;
        assert_eq!(
            step(&mut g, now, Some(&mac)),
            Step::Held {
                at: t0() + 10.2,
                until: now + HOLD_S,
                first: true
            }
        );
        assert_eq!(
            step(&mut g, now + 1.0, Some(&mac)),
            Step::Keep,
            "the same event is not input during the hold"
        );
        // the call that was running posts once more as it finishes: its echo inside the window still isn't the owner
        *mac.posted.lock().unwrap() = Some(now + 0.5);
        *mac.unclear.lock().unwrap() = Some(now + 0.6);
        assert_eq!(step(&mut g, now + 2.0, Some(&mac)), Step::Keep);
        // after that, any event at all is the owner's
        *mac.unclear.lock().unwrap() = Some(now + 3.0);
        assert_eq!(
            step(&mut g, now + 3.5, Some(&mac)),
            Step::Ended(Ended::OwnerInput)
        );
    }

    /// A hold that passes in silence resumes; unclear events are let pass for QUIET_S (10 minutes, APP-958); a clear
    /// one still ends it.
    // decision: GATE-618
    // decision: GATE-619
    #[test]
    fn a_silent_hold_resumes_then_unclear_events_pass_but_a_clear_one_ends() {
        let (mac, mut g) = (Mac::default(), grant());
        *mac.posted.lock().unwrap() = Some(t0() + 10.0);
        *mac.unclear.lock().unwrap() = Some(t0() + 10.1);
        let now = t0() + 10.5;
        assert!(matches!(step(&mut g, now, Some(&mac)), Step::Held { .. }));
        assert_eq!(step(&mut g, now + HOLD_S - 0.1, Some(&mac)), Step::Keep);
        assert_eq!(
            step(&mut g, now + HOLD_S, Some(&mac)),
            Step::Resumed {
                quiet_until: now + HOLD_S + QUIET_S
            }
        );
        assert_eq!(g.hold, None);
        let later = now + HOLD_S + 60.0;
        *mac.posted.lock().unwrap() = Some(later);
        *mac.unclear.lock().unwrap() = Some(later + 0.1);
        assert_eq!(
            step(&mut g, later + 1.0, Some(&mac)),
            Step::Ignored { at: later + 0.1 }
        );
        assert_eq!(
            step(&mut g, later + 2.0, Some(&mac)),
            Step::Keep,
            "handled once"
        );
        *mac.clear.lock().unwrap() = Some(later + 3.0);
        assert_eq!(
            step(&mut g, later + 4.0, Some(&mac)),
            Step::Ended(Ended::OwnerInput)
        );
        // past the quiet time an unclear event holds again (not the first one)
        let (mac2, mut g2) = (Mac::default(), grant());
        g2.quiet_until = Some(t0() + 100.0);
        g2.holds = 1;
        *mac2.unclear.lock().unwrap() = Some(t0() + 150.0);
        assert!(matches!(
            step(&mut g2, t0() + 151.0, Some(&mac2)),
            Step::Held { first: false, .. }
        ));
    }

    // decision: GATE-615
    #[test]
    fn only_the_screen_use_question_alone_is_covered() {
        let fg = Need::new(
            confirm::FOREGROUND,
            "k".into(),
            Values::of(&[("app", "Notes")]),
        );
        let pay = Need::new(confirm::ALWAYS_PAY, "p".into(), Values::of(&[]));
        let script = Need::new(
            confirm::FOREGROUND,
            "s".into(),
            Values::of(&[("app", "Notes"), ("script", "yes")]),
        );
        assert!(screen_only(std::slice::from_ref(&fg)));
        assert!(!screen_only(&[fg, pay]));
        assert!(
            !screen_only(&[script]),
            "a script's input carries no FERB mark"
        );
        assert!(!screen_only(&[]));
    }

    #[test]
    fn every_end_but_a_failed_answer_is_told_once_without_alarming_words() {
        for e in [
            Ended::OwnerInput,
            Ended::MacMessage,
            Ended::MacAway,
            Ended::CannotTell,
            Ended::TurnedOff,
            Ended::PhoneRemoved,
            Ended::Restarted,
        ] {
            let line = e.line().unwrap();
            assert!(!line.contains("AI"), "{line}");
            assert_eq!(Ended::parse(e.as_str()), Some(e));
        }
        assert_eq!(Ended::AnswerFailed.line(), None);
    }
}
