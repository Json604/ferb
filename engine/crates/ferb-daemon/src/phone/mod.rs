//! The daemon's phone area (stage 3 S3, kb T-1202): what FERB's daemon does for a paired phone. It trusts nothing the
//! phone link says about a phone (PHN-124): the link's helper only passes a phone's calls and proofs on.
//!
//! - **Paired phones** are the daemon's own table (`ferb_store::phones`, migration 0065), at most [`Limits::devices_max`]
//!   (PHN-419). A pairing the helper has checked against the QR's one-time secret (PHN-102) waits for the owner's Yes on
//!   the Mac with the six digits both screens show (`link.pair_request`, `phone.pair_decide`, `link.pair_wait`).
//!   Renaming and removing are FERB.app's (PHN-106); a second phone removes a lost one with its fingerprint (PHN-419).
//!   A removed phone's sessions end at once, every later call under it is refused, and the helper is told
//!   (`link.events`) so it closes the phone's connections.
//! - **Sessions, challenges, the unlock window** ([`gate`]): in memory, as the lab keeps them. A challenge is one use,
//!   [`Limits::challenge_ttl_s`] old at most, and dies with its session (PHN-108); a restart ends every session. The
//!   unlock lasts [`Limits::unlock_idle_s`] of idle time, [`Limits::unlock_max_s`] at most (PHN-15, PHN-141).
//! - **Proofs** ([`proof`]): ECDSA P-256 over the exact JSON text the phone signed, which names the challenge, the
//!   method and the exact parameters (or, for a hidden answer, only the SHA-256 of the text: PHN-110).
//! - **The class table drives every call** (`ferb_rpc::classes`): `never` refused by the role check; `plain` and
//!   `private` need the unlock window, but for what a locked phone may do (status, Stop, self-proving answers:
//!   PHN-125); `sensitive` needs a proof for this exact call with a fresh challenge, by the fingerprint-only key when
//!   the phone registered one (PHN-112, PHN-133). Answers: a plain No needs nothing; a hidden input's needs its sealed
//!   proof by either key; a Yes to one of FERB's own checks needs the fingerprint unless it is a routine one, which the
//!   notification's buttons answer (PHN-417, PHN-137). A project publish is never routine (HAND-624), so its Yes takes the
//!   fingerprint (PHN-467).
//! - **The audit** ([`ferb_store::phones::audit`]): every private, sensitive or answering call, every refusal, the
//!   unlock and the keys, pairing and removal; method, phone, time, outcome and structural facts, never a value
//!   (rule 7); kept [`Limits::audit_keep_days`] (PHN-130).
//! - **A prompt on the Mac's screen** (PHN-120): with a [`MacPrompts`] in place, FERB asks the phone whether to click
//!   the prompt a call left open and clicks on the signed Yes, or says the system refused a click from software;
//!   without one (today: nothing in FERB clicks system prompts) the six on-screen methods are refused from a phone with
//!   `needs_the_mac`, before anything opens.
//!
//! - **The FERB account** ([`account`], PHN-828): a phone pairs only with a Mac signed in to the same FERB account. The
//!   pairing hello carries FERB's site's signed statement of the phone's account; `link.pair_request` checks it against
//!   the Mac's own account before the owner is asked, and the phone's row keeps the account it paired under. When the
//!   Mac's account changes, every phone bound to another account is removed ([`Phones::account_changed`]); a phone
//!   paired before accounts (none recorded) keeps working and binds itself when it signs in (`phone.bind_account`).
//!
//! A phone's turn is the owner's: the handlers FERB.app's calls reach answer it, with the phone named as the client
//! (`client_id` `phone:<endpoint>`), so the Mac shows which device sent it (APP-916).

pub mod account;
pub mod asking;
pub mod device;
pub mod files;
pub mod find;
pub mod gate;
pub mod link;
pub mod proof;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ferb_core::interfaces::Clock;
use ferb_rpc::phone_gate::{self, Refusal};
use ferb_rpc::server::{BoxFuture, EventStream, HandlerError, HandlerResult};
use ferb_rpc::{Params, Server, Session};
use ferb_store::phones::{self as table, AddRefused, AuditRow, NewPhone, Phone};
use ferb_store::Db;
use serde_json::{json, Map, Value};
use tokio::sync::{broadcast, Notify};

/// The phone area's numbers, each from a decision.
#[derive(Clone, Debug, PartialEq)]
pub struct Limits {
    /// At most this many paired phones (PHN-419).
    pub devices_max: usize,
    /// A phone unused this long is warned about on the Mac, never removed by itself (PHN-419).
    pub device_unused_days: i64,
    /// One unlock opens FERB for this much idle time (PHN-15: five minutes; PHN-112's `unlock_idle_s`).
    pub unlock_idle_s: f64,
    /// And at most this long however much it is used (PHN-141: 12 hours).
    pub unlock_max_s: f64,
    /// A challenge's life (CONTRACT §7 `challenge_ttl_s`, whose defaults stand: PHN-312 as kept by PHN-419).
    pub challenge_ttl_s: f64,
    /// How long the owner has to say Yes to a pairing on the Mac (CONTRACT §7 `pair_confirm_s`; PHN-312: 120 s).
    pub pair_confirm_s: f64,
    /// The audit is kept this long (PHN-130: 90 days).
    pub audit_keep_days: i64,
    /// An unlock statement the phone signed in its opening prompt is taken only this soon after it was made, by the
    /// phone's own steady clock (PHN-649: 120 s, the coordinator's number until the owner confirms it, T-1270).
    pub statement_age_s: f64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            devices_max: 5,
            device_unused_days: 30,
            unlock_idle_s: 300.0,
            unlock_max_s: 12.0 * 3600.0,
            challenge_ttl_s: 120.0,
            pair_confirm_s: 120.0,
            audit_keep_days: 90,
            statement_age_s: 120.0,
        }
    }
}

/// What the phone area needs to know of a question (the gateway's, through ferbd).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuestionFacts {
    /// Still waiting for an answer.
    pub pending: bool,
    /// A hidden input (`SECRET_KINDS`): answered only through the sealed path (PHN-110).
    pub hidden: bool,
    /// One of FERB's own checks (`OWN_CHECKS`).
    pub own_check: bool,
    /// One the owner's "don't ask for 30 minutes" covers (GATE-207's class; the gateway offered that answer):
    /// answered from the notification without the fingerprint (PHN-417).
    pub routine: bool,
}

/// GATE-615: how a "until I'm back at my Mac" ended, as [`Questions::end_until_back`] takes it (the gateway's
/// `screen_use::Ended`).
pub const UNTIL_BACK_PHONE_REMOVED: &str = "phone_removed";
pub const UNTIL_BACK_TURNED_OFF: &str = "turned_off";
pub const UNTIL_BACK_ANSWER_FAILED: &str = "answer_failed";

/// The questions FERB asks, as the phone area reads and asks them.
pub trait Questions: Send + Sync {
    fn facts(&self, approval_id: &str) -> Option<QuestionFacts>;
    /// Asks one of FERB's own checks (its Yes needs the phone's proof) and returns its id.
    fn ask_check(&self, reason: &str, exact: &str, task_id: &str) -> String;
    /// GATE-615: whether a phone may answer this question "until I'm back at my Mac" now (the screen-use question
    /// alone, while FERB can tell when the owner is back). None by default.
    fn until_back_offered(&self, _approval_id: &str) -> bool {
        false
    }
    /// Gives it with the phone's Yes (the gate checked the phone: paired, unlocked); `Err(why)` when it can't be.
    fn grant_until_back(
        &self,
        _approval_id: &str,
        _phone: &str,
        _phone_name: &str,
    ) -> Result<(), String> {
        Err("FERB can't tell now when you're back at your Mac, so this needs a plain Yes.".into())
    }
    /// Ends it with why (`UNTIL_BACK_*`); `phone`: only when that phone gave it.
    fn end_until_back(&self, _why: &str, _phone: Option<&str>) {}
    /// What a phone shows of it: `{active, since?, phone?, phone_name?, can_offer}`.
    fn screen_use_status(&self) -> Value {
        json!({"active": false, "can_offer": false})
    }
}

/// A prompt left open on the Mac's screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenPrompt {
    pub title: String,
    pub button: String,
}

/// Finding and clicking a prompt on the Mac's screen (PHN-120). Whether macOS takes a click from software is a fact
/// only the attempt gives.
pub trait MacPrompts: Send + Sync {
    /// The prompt this call just left open, if any.
    fn left_open(&self, method: &str, params: &Params) -> Option<ScreenPrompt>;
    /// Clicks its button: `Err(why)` when the system did not take the click.
    fn click(&self, prompt: &ScreenPrompt) -> Result<(), String>;
}

type Publish = dyn Fn(&str, Map<String, Value>) + Send + Sync;
type Note = dyn Fn(&str, Value) + Send + Sync;
type Redact = dyn Fn(&Value) -> Value + Send + Sync;
type Nonce = dyn Fn() -> [u8; 16] + Send + Sync;

/// What the phone area takes from the daemon.
pub struct Seams {
    pub db: Arc<Mutex<Db>>,
    pub clock: Arc<dyn Clock>,
    /// 16 random bytes for a challenge or a pairing's id (the system's random source; a test's own).
    pub nonce: Arc<Nonce>,
    /// The redactor, for the audit's facts.
    pub redact: Arc<Redact>,
    pub questions: Arc<dyn Questions>,
    /// `None`: nothing clicks prompts on the Mac's screen; the six on-screen methods say `needs_the_mac`.
    pub prompts: Option<Arc<dyn MacPrompts>>,
    /// A journal row (pairing asked and decided, removal, the unused warning).
    pub journal: Arc<Note>,
    /// A bus event FERB.app hears (`phone.pair_requested`, `phone.unused`, `phone.removed`).
    pub publish: Arc<Publish>,
    /// PHN-828: the trusted keys for the site's pairing statements and the Mac's own FERB account.
    pub account: account::AccountCheck,
}

impl Seams {
    /// The system's random bytes. No challenge is better than a guessable one: without the system's random source
    /// the daemon must not go on.
    pub fn system_nonce() -> Arc<Nonce> {
        Arc::new(|| {
            let mut b = [0u8; 16];
            getrandom::fill(&mut b).expect("the system's random source is unavailable");
            b
        })
    }
}

/// A pairing waiting for the owner's Yes.
#[derive(Clone, Debug)]
struct Pending {
    id: String,
    new: NewPhone,
    code: String,
    made: f64,
    decided: Option<bool>,
    /// PHN-849: a paired phone whose fingerprints changed asks to replace its fingerprint-only key (`new.strong_pub`)
    /// instead of being added.
    update: bool,
}

/// The daemon's phone area. Cheap to clone.
#[derive(Clone)]
pub struct Phones {
    inner: Arc<Inner>,
}

pub(crate) struct Inner {
    seams: Seams,
    limits: Limits,
    state: Mutex<gate::State>,
    pairing: Mutex<Option<Pending>>,
    decided: Notify,
    events: broadcast::Sender<Value>,
    /// The file area (stage 3 S3b, `files`), once `Phones::register_files` put it in place.
    files: std::sync::OnceLock<files::Area>,
    /// ferbd's control of the link's helper (S4, `link`), once `Phones::set_link` put it in place.
    link: Mutex<Option<Arc<dyn link::LinkControl>>>,
    /// The asks waiting for the helper's `link.answer` (S4).
    asks: Mutex<link::Asks>,
    /// The Mac's computer name, for a phone's `phone.list` (T-1221), once `Phones::set_mac_name` put it in place.
    mac_name: std::sync::OnceLock<MacName>,
    /// Each phone's own record and the open requests for a precise fix (T-1214, `device`; memory only).
    devices: Mutex<device::Devices>,
    /// Insomnia mode's status for `chat.status` (T-1274), once `Phones::set_insomnia` put it in place.
    insomnia: std::sync::OnceLock<InsomniaStatus>,
    /// The memory import's live card for `chat.status` (MEM-456), once `Phones::set_memory_import` put it in place.
    memory_import: std::sync::OnceLock<MemoryImportCard>,
}

/// The memory import's live card as last published (`areas_a::memory_import::refresh`), for the phone's `chat.status`
/// (MEM-456); None while nothing is under way.
pub type MemoryImportCard = Arc<dyn Fn() -> Option<Value> + Send + Sync>;

/// Insomnia mode's status now (`crate::insomnia::Insomnia::status`), for the phone's `chat.status` (T-1274).
pub type InsomniaStatus = Arc<dyn Fn() -> Map<String, Value> + Send + Sync>;

/// The Mac's computer name now (ferbd reads the system's; empty when unknown).
pub type MacName = Arc<dyn Fn() -> String + Send + Sync>;

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn refused(code: i64, message: impl Into<String>) -> Refusal {
    Refusal::new(code, message)
}

fn coded(r: Refusal) -> HandlerError {
    HandlerError::coded(r.code, r.message)
}

fn store_fault(e: impl std::fmt::Display) -> HandlerError {
    HandlerError::fault("OperationalError", e.to_string())
}

/// The lab's `clean_name`: control characters out, 40 characters, trimmed, "Phone" when nothing is left.
pub fn clean_name(name: &str) -> String {
    let s: String = name.chars().filter(|c| !c.is_control()).take(40).collect();
    let s = s.trim().to_string();
    if s.is_empty() {
        "Phone".into()
    } else {
        s
    }
}

fn text(params: &Params, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|t| !t.trim().is_empty())
}

/// A key a phone offers at pairing, kept only when it is one (the lab's `filter(verifying_key)`).
fn offered_key(params: &Params, name: &str) -> Option<String> {
    text(params, name).filter(|k| proof::key(k).is_some())
}

impl Phones {
    pub fn new(seams: Seams, limits: Limits) -> Self {
        let (events, _) = broadcast::channel(64);
        Phones {
            inner: Arc::new(Inner {
                seams,
                limits,
                state: Mutex::new(gate::State::default()),
                pairing: Mutex::new(None),
                decided: Notify::new(),
                events,
                files: std::sync::OnceLock::new(),
                link: Mutex::new(None),
                asks: Mutex::new(link::Asks::default()),
                mac_name: std::sync::OnceLock::new(),
                devices: Mutex::new(device::Devices::default()),
                insomnia: std::sync::OnceLock::new(),
                memory_import: std::sync::OnceLock::new(),
            }),
        }
    }

    pub fn limits(&self) -> &Limits {
        &self.inner.limits
    }

    /// PHN-454: whether this phone session may receive a live preview's frame as captured (the second reader of an
    /// `OwnerOnlyFrame`, APP-922): its phone is paired and not removed, its unlock window is open now (the gate admitted
    /// the call by its class: `tasks.preview` is private), and the link named a direct path or the owner's relay as the
    /// session's tier (never the public relay, nor a session whose tier is unknown). The answer is the only maker of
    /// the [`PairedPhoneLink`] the frame's reader takes.
    // decision: PHN-454
    pub fn preview_link(
        &self,
        phone: &ferb_rpc::phone_gate::PhoneSession,
    ) -> Result<ferb_mac_hands::point::PairedPhoneLink, Refusal> {
        let inner = &self.inner;
        let dev = inner.live(&phone.device)?;
        if !matches!(phone.tier.as_deref(), Some("direct" | "own_relay")) {
            return Err(refused(
                phone_gate::FILE_NOT_ALLOWED,
                "Watch live crosses only a direct connection or your own relay, not the shared relay.",
            ));
        }
        let mut st = lock(&inner.state);
        // decision: PHN-846 — a phone whose app lock the owner turned off watches its tasks without the window
        if !st.is_unlocked(phone, inner.mono(), &inner.limits)
            && !st.lock_off_open(&dev, phone, "tasks.preview")
        {
            return Err(refused(
                phone_gate::LOCKED,
                "FERB on your Mac opens for this phone only after its fingerprint, face or PIN.",
            ));
        }
        Ok(ferb_mac_hands::point::PairedPhoneLink::after_the_phone_gate_s_checks())
    }

    /// Where the Mac's name comes from (ferbd: the computer name), for a phone's `phone.list` (T-1221). Set once.
    pub fn set_mac_name(&self, name: MacName) {
        let _ = self.inner.mac_name.set(name);
    }

    /// Where insomnia mode's status comes from, for the phone's `chat.status` (T-1274). Set once.
    pub fn set_insomnia(&self, status: InsomniaStatus) {
        let _ = self.inner.insomnia.set(status);
    }

    /// Where the memory import's live card comes from, for the phone's `chat.status` (MEM-456). Set once.
    pub fn set_memory_import(&self, card: MemoryImportCard) {
        let _ = self.inner.memory_import.set(card);
    }

    /// The phone gate the server asks about phone sessions (`Server::set_phone_gate`).
    pub fn gate(&self) -> gate::Gate {
        gate::Gate(self.inner.clone())
    }

    /// Registers the area's methods and sets its gate on `server`.
    pub fn register(&self, server: &Server) {
        server.set_phone_gate(self.gate());
        let unary: [(&str, Handler); 18] = [
            ("phone.list", |p, s, _| p.list(&s)),
            ("phone.rename", |p, _, a| p.rename(&a)),
            ("phone.remove", |p, s, a| p.remove(&s, &a)),
            ("phone.forget_self", |p, s, _| p.forget_self(&s)),
            ("phone.pair_decide", |p, _, a| p.pair_decide(&a)),
            ("phone.audit", |p, _, a| p.audit_rows(&a)),
            ("phone.challenge", |p, s, _| {
                p.inner.on_session(&s, gate::challenge)
            }),
            ("phone.unlock", |p, s, _| {
                p.inner.on_session(&s, gate::unlock)
            }),
            ("phone.unlock_statement", |p, s, a| {
                let phone = s.phone.clone().unwrap_or_default();
                let dev = p.inner.live(&phone.device).map_err(coded)?;
                let mut st = lock(&p.inner.state);
                gate::unlock_statement(&p.inner, &mut st, &dev, &phone, &a).map_err(coded)
            }),
            ("phone.lock", |p, s, a| {
                let phone = s.phone.clone().unwrap_or_default();
                let dev = p.inner.live(&phone.device).map_err(coded)?;
                let mut st = lock(&p.inner.state);
                gate::lock_phone(&p.inner, &mut st, &dev, &phone, &a).map_err(coded)
            }),
            ("phone.present", |p, s, _| {
                p.inner.on_session(&s, gate::present)
            }),
            ("phone.session_end", |p, s, _| {
                p.inner.on_session(&s, gate::session_end)
            }),
            ("phone.keys", |p, s, a| p.keys(&s, &a)),
            ("phone.device_report", |p, s, a| p.device_report(&s, &a)),
            // decision: PHN-846 — the gate admitted lock_off by its signed proof (the sensitive class)
            ("phone.lock_off", |p, s, _| p.set_lock_off(&s, true)),
            ("phone.lock_on", |p, s, _| p.set_lock_off(&s, false)),
            // (phone.bind_account is async below: the Mac's account may be read from the Keychain)
            ("link.devices", |p, _, _| p.devices()),
            ("link.pair_cancel", |p, _, a| p.pair_cancel(&a)),
        ];
        for (name, f) in unary {
            let me = self.clone();
            server.register_unary(name, move |s: Session, a: Params| {
                let me = me.clone();
                async move { f(&me, s, a) }
            });
        }
        let me = self.clone();
        server.register_unary("phone.bind_account", move |s: Session, a: Params| {
            let me = me.clone();
            async move { me.bind_account(&s, &a).await }
        });
        let me = self.clone();
        server.register_unary("link.pair_request", move |_s: Session, a: Params| {
            let me = me.clone();
            async move { me.pair_request(&a).await }
        });
        let me = self.clone();
        server.register_unary("link.pair_wait", move |_s: Session, a: Params| {
            let me = me.clone();
            async move { me.pair_wait(&a).await }
        });
        let me = self.clone();
        server.register_stream("link.events", move |_s: Session, _a: Params| {
            Box::new(LinkEvents(me.inner.events.subscribe())) as Box<dyn EventStream>
        });
        // S4: FERB.app's control of the link through the daemon, and the helper's answers
        self.register_link(server);
    }

    /// Gives the 30-day warning to every live phone unused that long and not yet warned (PHN-419): a journal row and a
    /// bus event once, and `phone.list` says so until the phone is seen again. Never removes one.
    pub fn sweep_unused(&self) -> Vec<String> {
        let inner = &self.inner;
        let now = inner.seams.clock.now();
        let cutoff = now - chrono::Duration::days(inner.limits.device_unused_days);
        // marked under the database's lock, journaled and published after it: both take the same database (the
        // journal writes a row, a consumer of the bus reads plans), and a std lock taken twice waits on itself
        let due = {
            let db = lock(&inner.seams.db);
            let Ok(phones) = table::live(db.conn()) else {
                return Vec::new();
            };
            let mut due = Vec::new();
            for p in phones {
                let unused =
                    ferb_core::when::parse(&p.last_seen).is_some_and(|seen| seen <= cutoff);
                if unused && p.warned_at.is_none() {
                    let _ = table::mark_warned(db.conn(), p.id, &inner.now_iso());
                    due.push(p);
                }
            }
            due
        };
        let mut warned = Vec::new();
        for p in due {
            (inner.seams.journal)(
                "phone.unused_warning",
                json!({"phone": p.endpoint, "last_seen": p.last_seen}),
            );
            let mut ev = Map::new();
            ev.insert("phone".into(), json!(p.endpoint));
            ev.insert("name".into(), json!(p.name));
            ev.insert("last_seen".into(), json!(p.last_seen));
            ev.insert("days".into(), json!(inner.limits.device_unused_days));
            (inner.seams.publish)("phone.unused", ev);
            warned.push(p.endpoint);
        }
        warned
    }

    fn phone_json(&self, p: &Phone) -> Value {
        let inner = &self.inner;
        let sessions = lock(&inner.state).sessions_of(&p.endpoint);
        let unlocked = lock(&inner.state).device_unlocked(&p.endpoint, inner.mono(), &inner.limits);
        json!({
            "id": p.endpoint, "name": p.name, "app": p.app, "paired_at": p.paired_at, "last_seen": p.last_seen,
            "unlock": p.unlock_pub.is_some(), "device_key": p.device_pub.is_some(),
            "fingerprint_key": p.strong_pub.is_some(),
            "unused_warning": p.warned_at.is_some(), "warned_at": p.warned_at,
            "sessions": sessions, "unlocked": unlocked, "lock_off": p.lock_off,
        })
    }

    /// `phone.list`: the paired phones, the pairing waiting for the owner's Yes, the limit. Asked by a phone (a private
    /// call, behind its unlock), the calling phone's own row says `this: true` and the Mac's name comes too (`mac`),
    /// so the phone shows which Mac it is paired with and since when (T-1221).
    pub fn list(&self, session: &Session) -> HandlerResult {
        self.sweep_unused();
        let phones = {
            let db = lock(&self.inner.seams.db);
            table::live(db.conn()).map_err(store_fault)?
        };
        let pairing = self.inner.pending_json();
        let caller = session.phone.as_ref().map(|p| p.device.clone());
        let mut out = Params::new();
        out.insert(
            "phones".into(),
            Value::Array(
                phones
                    .iter()
                    .map(|p| {
                        let mut row = self.phone_json(p);
                        if caller.as_deref() == Some(p.endpoint.as_str()) {
                            row["this"] = json!(true);
                        }
                        row
                    })
                    .collect(),
            ),
        );
        if caller.is_some() {
            let name = self.inner.mac_name.get().map(|f| f()).unwrap_or_default();
            out.insert(
                "mac".into(),
                json!({"name": (!name.is_empty()).then_some(name)}),
            );
        }
        out.insert("pairing".into(), pairing);
        // decision: PHN-580 — where a phone's files land, said plainly (a phone reads it; only the Mac changes it)
        if let Some(a) = self.inner.files.get() {
            out.insert("upload_folder".into(), a.upload_folder_json());
        }
        out.insert("max".into(), json!(self.inner.limits.devices_max));
        out.insert(
            "unused_days".into(),
            json!(self.inner.limits.device_unused_days),
        );
        Ok(out)
    }

    /// `phone.lock_off {}` / `phone.lock_on {}` (PHN-846): the owner turned this phone's app lock off or on again,
    /// recorded for this live phone. The gate made each call's check before this: off is the sensitive class (signed
    /// over this exact call with a fresh challenge, by the fingerprint-only key where the phone registered one), on is
    /// allowed even locked (it only ever makes the phone stricter). While it is off the gate gives this phone's
    /// device-proved connections `classes::LOCK_OFF_OPEN` without the unlock window. `{lock_off}`.
    pub fn set_lock_off(&self, session: &Session, off: bool) -> HandlerResult {
        let inner = &self.inner;
        let Some(phone) = session.phone.as_ref() else {
            return Err(coded(refused(
                phone_gate::BAD_REQUEST,
                "the app lock is a paired phone's own switch",
            )));
        };
        let dev = inner.live(&phone.device).map_err(coded)?;
        {
            let db = lock(&inner.seams.db);
            table::set_lock_off(db.conn(), &dev.endpoint, off).map_err(store_fault)?;
        }
        if !off {
            // the sensitive "off" is audited with its outcome by the gate; "on", a plain call, here
            inner.audit(&phone.device, "phone.lock_on", "plain", "ok", json!({}));
        }
        let mut out = Params::new();
        out.insert("lock_off".into(), json!(off));
        Ok(out)
    }

    /// `phone.rename {id, name}` (FERB.app's).
    pub fn rename(&self, params: &Params) -> HandlerResult {
        let id = text(params, "id").ok_or_else(|| {
            coded(refused(
                phone_gate::BAD_REQUEST,
                "phone.rename needs the phone's id",
            ))
        })?;
        let name = clean_name(params.get("name").and_then(Value::as_str).unwrap_or(""));
        let done = {
            let db = lock(&self.inner.seams.db);
            table::rename(db.conn(), &id, &name).map_err(store_fault)?
        };
        if !done {
            return Err(coded(refused(phone_gate::NOT_FOUND, "No such phone.")));
        }
        let _ = self
            .inner
            .events
            .send(json!({"event": "device_renamed", "id": id, "name": name}));
        let mut out = Params::new();
        out.insert("id".into(), json!(id));
        out.insert("name".into(), json!(name));
        Ok(out)
    }

    /// `phone.remove {id}`: FERB.app's, or a second phone's with its fingerprint (the gate checked the proof). Its
    /// sessions end at once; the helper closes its connections (`link.events`); every later call under it is refused.
    /// `phone.forget_self` (the phone's `link.forget`, PHN-646): "Forget this Mac" on a phone removes that phone, and
    /// only it, from a session that proved its device key; journalled and audited as by that phone. Any other
    /// removal's checks and effects are `remove`'s.
    pub fn forget_self(&self, session: &Session) -> HandlerResult {
        let Some(phone) = session.phone.as_ref() else {
            return Err(coded(refused(
                phone_gate::BAD_REQUEST,
                "phone.forget_self is a paired phone's own call",
            )));
        };
        if !lock(&self.inner.state).proved_present(phone) {
            return Err(coded(refused(
                phone_gate::UNLOCK_INVALID,
                "This connection hasn't proved it is the paired phone itself.",
            )));
        }
        let mut params = Params::new();
        params.insert("id".into(), json!(phone.device));
        let out = self.remove(session, &params)?;
        self.inner
            .audit(&phone.device, "phone.forget_self", "link", "ok", json!({}));
        Ok(out)
    }

    pub fn remove(&self, session: &Session, params: &Params) -> HandlerResult {
        let inner = &self.inner;
        let id = text(params, "id").ok_or_else(|| {
            coded(refused(
                phone_gate::BAD_REQUEST,
                "phone.remove needs the phone's id",
            ))
        })?;
        let by = session
            .phone
            .as_ref()
            .map_or_else(|| "mac".to_owned(), |p| p.device.clone());
        if self.remove_phone(&id, &by)?.is_none() {
            return Err(coded(refused(phone_gate::NOT_FOUND, "No such phone.")));
        }
        if session.phone.is_none() {
            inner.audit("", "phone.remove", "owner", "ok", json!({"removed": id}));
        }
        let mut out = Params::new();
        out.insert("removed".into(), json!(id));
        Ok(out)
    }

    /// PHN-828: FERB on this Mac is signed in to `account_id` now. Every live phone bound to another account is removed
    /// as `phone.remove` removes one (`by` "account"), and a pairing waiting for the owner's Yes under another account
    /// ends unpaired. A phone paired before accounts (no account recorded) stays: it binds itself when it signs in
    /// (`phone.bind_account`). The phones removed, as `(id, name)`.
    // decision: PHN-828
    pub fn account_changed(&self, account_id: &str) -> Result<Vec<(String, String)>, HandlerError> {
        let inner = &self.inner;
        {
            let mut pairing = lock(&inner.pairing);
            if let Some(p) = pairing
                .as_mut()
                .filter(|p| p.decided.is_none() && p.new.account_id.as_deref() != Some(account_id))
            {
                p.decided = Some(false);
                let phone = p.new.endpoint.clone();
                drop(pairing);
                inner
                    .pair_ended(json!({"phone": phone, "paired": false, "why": "account changed"}));
                inner.decided.notify_waiters();
            }
        }
        let others: Vec<Phone> = {
            let db = lock(&inner.seams.db);
            table::live(db.conn()).map_err(store_fault)?
        }
        .into_iter()
        .filter(|p| p.account_id.as_deref().is_some_and(|a| a != account_id))
        .collect();
        let mut removed = Vec::new();
        for p in others {
            if let Some(gone) = self.remove_phone(&p.endpoint, "account")? {
                inner.audit(
                    "",
                    "account.save",
                    "owner",
                    "ok",
                    json!({"removed": gone.endpoint}),
                );
                removed.push((gone.endpoint, gone.name));
            }
        }
        Ok(removed)
    }

    /// `phone.bind_account {assertion}` (PHN-828): a paired phone's own statement of its FERB account (the site's signed
    /// assertion for this session's own endpoint), checked as a pairing's is. The Mac's account: the phone is bound to
    /// it (`changed` when it had none or another recorded). Another account: the phone is removed as `phone.remove`
    /// removes one (`by` "account") and refused `account_mismatch`. The Mac signed out, or a statement that can't be
    /// confirmed: refused, nothing changed. `{bound: true, changed}`.
    // decision: PHN-828
    pub async fn bind_account(&self, session: &Session, params: &Params) -> HandlerResult {
        let inner = &self.inner;
        let Some(phone) = session.phone.as_ref() else {
            return Err(coded(refused(
                phone_gate::BAD_REQUEST,
                "phone.bind_account is a paired phone's own call",
            )));
        };
        let dev = inner.live(&phone.device).map_err(coded)?;
        let check = inner.seams.account.clone();
        let mac = tokio::task::spawn_blocking(move || check.mac_account())
            .await
            .unwrap_or(Err(account::MacUnreadable));
        let now = inner.seams.clock.now().timestamp();
        let statement = params.get("assertion").and_then(Value::as_str);
        let refusal = match inner
            .seams
            .account
            .check_read(statement, &dev.endpoint, now, &mac)
        {
            Ok(c) => {
                let changed = dev.account_id.as_deref() != Some(c.account_id.as_str());
                if changed {
                    let db = lock(&inner.seams.db);
                    table::bind_account(db.conn(), &dev.endpoint, &c.account_id)
                        .map_err(store_fault)?;
                    drop(db);
                    (inner.seams.journal)(
                        "phone.account_bound",
                        json!({"phone": dev.endpoint, "had_account": dev.account_id.is_some()}),
                    );
                }
                inner.audit(
                    &dev.endpoint,
                    "phone.bind_account",
                    "plain",
                    "ok",
                    json!({"changed": changed}),
                );
                let mut out = Params::new();
                out.insert("bound".into(), json!(true));
                out.insert("changed".into(), json!(changed));
                return Ok(out);
            }
            Err(r) => r,
        };
        let detail = match &refusal {
            account::AccountRefused::Unconfirmed(why) => json!(why),
            _ => Value::Null,
        };
        (inner.seams.journal)(
            "phone.bind_refused",
            json!({"phone": dev.endpoint, "why": refusal.name(), "detail": detail}),
        );
        inner.audit(
            &dev.endpoint,
            "phone.bind_account",
            "plain",
            refusal.name(),
            json!({}),
        );
        if matches!(refusal, account::AccountRefused::Mismatch { .. }) {
            // another account than the Mac's: this phone goes, as any removal (its sessions end, the helper is told)
            self.remove_phone(&dev.endpoint, "account")?;
        }
        Err(coded(refused(refusal.code(), refusal.words())))
    }

    /// Removes one live phone: the row marked, its sessions and windows ended, its files' state and record dropped, a
    /// "until I'm back" it gave ended, the helper told, journaled and published. The phone as it was, or `None`.
    fn remove_phone(&self, id: &str, by: &str) -> Result<Option<Phone>, HandlerError> {
        let inner = &self.inner;
        let removed = {
            let db = lock(&inner.seams.db);
            table::remove(db.conn(), id, by, &inner.now_iso()).map_err(store_fault)?
        };
        let Some(removed) = removed else {
            return Ok(None);
        };
        let id = id.to_owned();
        lock(&inner.state).forget_device(&id);
        files::forget_device(inner, &id);
        inner.forget_device_record(&id);
        // decision: GATE-615 — the phone that gave "until I'm back at my Mac" is gone: it ends now
        inner
            .questions()
            .end_until_back(UNTIL_BACK_PHONE_REMOVED, Some(&id));
        let _ = inner
            .events
            .send(json!({"event": "device_removed", "id": id}));
        (inner.seams.journal)("phone.removed", json!({"phone": id, "by": by}));
        let mut ev = Map::new();
        ev.insert("phone".into(), json!(id));
        ev.insert("name".into(), json!(removed.name));
        (inner.seams.publish)("phone.removed", ev);
        Ok(Some(removed))
    }

    /// `phone.audit {limit?}`: what phones did, newest last (FERB.app's; no phone reads it).
    pub fn audit_rows(&self, params: &Params) -> HandlerResult {
        let limit = params
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(100)
            .clamp(1, 1000) as usize;
        let rows = {
            let db = lock(&self.inner.seams.db);
            table::audit_tail(db.conn(), limit).map_err(store_fault)?
        };
        let mut out = Params::new();
        out.insert(
            "rows".into(),
            Value::Array(
                rows.into_iter()
                    .map(|r| json!({"at": r.at, "phone": r.device, "method": r.method, "class": r.class, "outcome": r.outcome, "detail": r.detail}))
                    .collect(),
            ),
        );
        Ok(out)
    }

    /// `phone.keys {device_pub?, strong_pub?}` with a proof by the fingerprint key (where the phone has one) signed for
    /// exactly these keys as `link.keys`: a phone paired before these keys existed registers them, each once.
    pub fn keys(&self, session: &Session, params: &Params) -> HandlerResult {
        let inner = &self.inner;
        let phone = session.phone.clone().unwrap_or_default();
        let want: Vec<(&str, &Value)> = ["device_pub", "strong_pub"]
            .into_iter()
            .filter_map(|k| params.get(k).filter(|v| !v.is_null()).map(|v| (k, v)))
            .collect();
        if want.is_empty()
            || want
                .iter()
                .any(|(_, v)| v.as_str().and_then(proof::key).is_none())
        {
            inner.audit(
                &phone.device,
                "phone.keys",
                "link",
                "bad_request",
                json!({}),
            );
            return Err(coded(refused(
                phone_gate::BAD_REQUEST,
                "Those are not keys.",
            )));
        }
        let dev = inner.live(&phone.device).map_err(coded)?;
        let checked = {
            let mut st = lock(&inner.state);
            st.verify_call(
                inner,
                &dev,
                &phone,
                "link.keys",
                &Value::Object(params.clone()),
                true,
            )
        };
        if let Err(r) = checked {
            inner.audit(
                &phone.device,
                "phone.keys",
                "link",
                &gate::code_name(r.code),
                json!({}),
            );
            return Err(coded(r));
        }
        let get = |k: &str| params.get(k).and_then(Value::as_str);
        let now = {
            let db = lock(&inner.seams.db);
            table::add_keys(
                db.conn(),
                &phone.device,
                get("device_pub"),
                get("strong_pub"),
            )
            .map_err(store_fault)?
        };
        inner.audit(&phone.device, "phone.keys", "link", "ok", json!({}));
        let _ = inner
            .events
            .send(json!({"event": "device_keys", "id": phone.device}));
        let mut out = Params::new();
        out.insert(
            "device_pub".into(),
            json!(now.as_ref().and_then(|p| p.device_pub.clone())),
        );
        out.insert(
            "strong_pub".into(),
            json!(now.as_ref().and_then(|p| p.strong_pub.clone())),
        );
        Ok(out)
    }

    /// `link.devices`: the live phones as the helper needs them (which endpoints are paired, with what keys).
    pub fn devices(&self) -> HandlerResult {
        self.sweep_unused();
        let phones = {
            let db = lock(&self.inner.seams.db);
            table::live(db.conn()).map_err(store_fault)?
        };
        let mut out = Params::new();
        out.insert(
            "devices".into(),
            Value::Array(
                phones
                    .iter()
                    .map(|p| json!({"id": p.endpoint, "name": p.name, "app": p.app, "paired_at": p.paired_at,
                                    "last_seen": p.last_seen, "unlock": p.unlock_pub.is_some(),
                                    "device_key": p.device_pub.is_some(), "fingerprint_key": p.strong_pub.is_some()}))
                    .collect(),
            ),
        );
        out.insert("max".into(), json!(self.inner.limits.devices_max));
        Ok(out)
    }

    /// `link.pair_request {endpoint, name, app?, code, unlock_pub?, device_pub?, strong_pub?, account?}`: the helper
    /// checked the phone's proof of the QR's one-time secret (PHN-102); the phone's FERB account is checked against the
    /// Mac's (`account`, the site's signed statement: PHN-828), and only then is the owner asked on the Mac, naming the
    /// phone and the six digits both screens show. One pairing waits at a time: a new one replaces it (the old is a No).
    pub async fn pair_request(&self, params: &Params) -> HandlerResult {
        let (endpoint, code) = pair_request_shape(params)?;
        if params.get("update").and_then(Value::as_bool) == Some(true) {
            return self.update_request(params, endpoint, code);
        }
        // the Mac's account may be read from the Keychain the first time: off the async runtime
        let check = self.inner.seams.account.clone();
        let mac = tokio::task::spawn_blocking(move || check.mac_account())
            .await
            .unwrap_or(Err(account::MacUnreadable));
        let now = self.inner.seams.clock.now().timestamp();
        let statement = params.get("account").and_then(Value::as_str);
        // decision: PHN-828 — before the owner is asked: a phone of another account (or none confirmed) never is
        let account_id = match self
            .inner
            .seams
            .account
            .check_read(statement, &endpoint, now, &mac)
        {
            Ok(c) => c.account_id,
            Err(r) => {
                let detail = match &r {
                    account::AccountRefused::Unconfirmed(why) => json!(why),
                    _ => Value::Null,
                };
                (self.inner.seams.journal)(
                    "phone.pair_refused",
                    json!({"phone": endpoint, "why": r.name(), "detail": detail}),
                );
                self.inner
                    .audit(&endpoint, "link.pair_request", "link", r.name(), json!({}));
                return Err(coded(refused(r.code(), r.words())));
            }
        };
        self.pair_request_ask(params, endpoint, code, account_id)
    }

    fn pair_request_ask(
        &self,
        params: &Params,
        endpoint: String,
        code: String,
        account_id: String,
    ) -> HandlerResult {
        let inner = &self.inner;
        let bad = |why: &str| coded(refused(phone_gate::BAD_REQUEST, why));
        let live = {
            let db = lock(&inner.seams.db);
            table::live(db.conn()).map_err(store_fault)?
        };
        if live.len() >= inner.limits.devices_max {
            return Err(coded(refused(
                phone_gate::DEVICE_LIMIT,
                format!(
                    "FERB already has {} paired phones, the most it keeps. Remove one on the Mac first.",
                    inner.limits.devices_max
                ),
            )));
        }
        if live.iter().any(|p| p.endpoint == endpoint) {
            return Err(bad("this phone is paired already"));
        }
        let new = NewPhone {
            endpoint: endpoint.clone(),
            name: clean_name(params.get("name").and_then(Value::as_str).unwrap_or("")),
            app: params
                .get("app")
                .and_then(Value::as_str)
                .unwrap_or("")
                .chars()
                .filter(|c| !c.is_control())
                .take(40)
                .collect(),
            unlock_pub: offered_key(params, "unlock_pub"),
            device_pub: offered_key(params, "device_pub"),
            strong_pub: offered_key(params, "strong_pub"),
            account_id: Some(account_id),
        };
        let id = proof::b64u(&(inner.seams.nonce)());
        let pending = Pending {
            id: id.clone(),
            new: new.clone(),
            code: code.clone(),
            made: inner.mono(),
            decided: None,
            update: false,
        };
        inner.wait_for_answer(pending);
        let question = format!(
            "Pair the phone \"{}\"? Check that it shows the code {code}. It will be able to do everything FERB.app can: \
             send tasks, answer FERB's questions, see what FERB is doing, and, after its fingerprint or PIN, open and \
             send files on this Mac and change FERB's settings.",
            new.name
        );
        (inner.seams.journal)(
            "phone.pair_requested",
            json!({"phone": endpoint, "name": new.name, "app": new.app}),
        );
        let mut ev = Map::new();
        ev.insert("request_id".into(), json!(id));
        ev.insert("phone".into(), json!(endpoint));
        ev.insert("name".into(), json!(new.name));
        ev.insert("app".into(), json!(new.app));
        ev.insert("code".into(), json!(code));
        ev.insert("question".into(), json!(question));
        (inner.seams.publish)("phone.pair_requested", ev);
        let mut out = Params::new();
        out.insert("request_id".into(), json!(id));
        out.insert("seconds".into(), json!(inner.limits.pair_confirm_s));
        Ok(out)
    }

    /// `link.pair_request {endpoint, code, strong_pub, update: true}` (PHN-849): a paired phone whose fingerprints
    /// changed proved the one-time secret of a code the owner made for it ("Update fingerprint" on the Mac; the helper
    /// checked that the code was made for this phone) and offers a new fingerprint-only key. The owner is asked on the
    /// Mac with the six digits both screens show, as for a pairing, and his Yes replaces the key: the same proof a
    /// new pairing needs, for a phone that keeps its name, history and settings. Only a phone the Mac holds such a key
    /// for: one without registers its key with `phone.keys`.
    fn update_request(&self, params: &Params, endpoint: String, code: String) -> HandlerResult {
        let inner = &self.inner;
        let bad = |why: &str| coded(refused(phone_gate::BAD_REQUEST, why));
        let Some(strong_pub) = offered_key(params, "strong_pub") else {
            return Err(bad("an update needs the phone's new fingerprint key"));
        };
        let phone = {
            let db = lock(&inner.seams.db);
            table::by_endpoint(db.conn(), &endpoint).map_err(store_fault)?
        };
        let Some(phone) = phone else {
            return Err(coded(refused(
                phone_gate::NOT_FOUND,
                "This phone is not paired with this Mac.",
            )));
        };
        if phone.strong_pub.is_none() {
            return Err(bad("this phone has no fingerprint key to replace"));
        }
        let id = proof::b64u(&(inner.seams.nonce)());
        let new = NewPhone {
            endpoint: endpoint.clone(),
            name: phone.name.clone(),
            app: phone.app.clone(),
            unlock_pub: None,
            device_pub: None,
            strong_pub: Some(strong_pub),
            account_id: None,
        };
        inner.wait_for_answer(Pending {
            id: id.clone(),
            new,
            code: code.clone(),
            made: inner.mono(),
            decided: None,
            update: true,
        });
        (inner.seams.journal)(
            "phone.pair_requested",
            json!({"phone": endpoint, "name": phone.name, "app": phone.app, "update": true}),
        );
        let question = format!(
            "Update \u{201c}{}\u{201d}'s fingerprint? Its fingerprints changed. Say Yes only if you added the \
             fingerprint yourself and the phone shows this code: {code}",
            phone.name
        );
        let mut ev = Map::new();
        ev.insert("request_id".into(), json!(id));
        ev.insert("phone".into(), json!(endpoint));
        ev.insert("name".into(), json!(phone.name));
        ev.insert("app".into(), json!(phone.app));
        ev.insert("code".into(), json!(code));
        ev.insert("question".into(), json!(question));
        ev.insert("update".into(), json!(true));
        (inner.seams.publish)("phone.pair_requested", ev);
        let mut out = Params::new();
        out.insert("request_id".into(), json!(id));
        out.insert("seconds".into(), json!(inner.limits.pair_confirm_s));
        Ok(out)
    }

    /// `phone.pair_decide {yes, request_id?}`: the owner's answer on the Mac. A Yes adds the phone, the limit checked
    /// again in the same transaction.
    pub fn pair_decide(&self, params: &Params) -> HandlerResult {
        let inner = &self.inner;
        let yes = params.get("yes").and_then(Value::as_bool) == Some(true);
        let mut pairing = lock(&inner.pairing);
        let waiting = pairing.as_mut().filter(|p| {
            p.decided.is_none()
                && inner.mono() - p.made < inner.limits.pair_confirm_s
                && text(params, "request_id").is_none_or(|r| r == p.id)
        });
        let Some(p) = waiting else {
            return Err(coded(refused(
                phone_gate::NOT_FOUND,
                "No phone is waiting for your answer.",
            )));
        };
        let mut paired = false;
        let update = p.update;
        if yes && update {
            // decision: PHN-849 — the Yes replaces the fingerprint-only key of a phone that keeps its row
            let replaced = {
                let db = lock(&inner.seams.db);
                table::replace_strong(
                    db.conn(),
                    &p.new.endpoint,
                    p.new.strong_pub.as_deref().unwrap_or_default(),
                )
                .map_err(store_fault)?
            };
            paired = replaced.is_some();
            if paired {
                let _ = inner
                    .events
                    .send(json!({"event": "device_keys", "id": p.new.endpoint}));
            }
        } else if yes {
            let added = {
                let db = lock(&inner.seams.db);
                table::add(&db, &p.new, &inner.now_iso(), inner.limits.devices_max)
                    .map_err(store_fault)?
            };
            match added {
                Ok(phone) => {
                    paired = true;
                    let _ = inner.events.send(
                        json!({"event": "device_added", "id": phone.endpoint, "name": phone.name}),
                    );
                }
                Err(AddRefused::Full { max }) => {
                    p.decided = Some(false);
                    inner.decided.notify_waiters();
                    return Err(coded(refused(
                        phone_gate::DEVICE_LIMIT,
                        format!("FERB already has {max} paired phones, the most it keeps. Remove one first."),
                    )));
                }
                Err(AddRefused::AlreadyPaired) => paired = true,
            }
        }
        p.decided = Some(paired);
        let (endpoint, name) = (p.new.endpoint.clone(), p.new.name.clone());
        drop(pairing);
        if paired {
            // decision: PHN-647 — the owner's Yes here, after the matching code, is his proof: the new phone's FERB
            // opens as after a fingerprint (the same idle rule and 12 h limit), for the session that proves its key
            lock(&inner.state).open_device_window(&endpoint, inner.mono(), &inner.limits);
            inner.audit(
                &endpoint,
                "phone.unlock",
                "link",
                "ok",
                json!({"by": "pairing"}),
            );
        }
        inner.decided.notify_waiters();
        let mut ended = json!({"phone": endpoint, "paired": paired});
        if update {
            ended["update"] = json!(true);
        }
        inner.pair_ended(ended);
        // an update is the phone's own line in the audit ("Pixel updated its fingerprint"), a pairing the Mac's
        let (who, method) = if update {
            (endpoint.as_str(), "phone.fingerprint_update")
        } else {
            ("", "phone.pair_decide")
        };
        inner.audit(
            who,
            method,
            "owner",
            if paired { "ok" } else { "denied" },
            json!({"phone": endpoint}),
        );
        let mut out = Params::new();
        out.insert("phone".into(), json!(endpoint));
        out.insert("name".into(), json!(name));
        out.insert("paired".into(), json!(paired));
        if update {
            out.insert("update".into(), json!(true));
        }
        Ok(out)
    }

    /// `link.pair_wait {request_id}`: waits for the owner's answer, at most until the pairing's time is up; a No, no
    /// answer in time, or a pairing replaced is `paired: false`.
    pub async fn pair_wait(&self, params: &Params) -> HandlerResult {
        let inner = &self.inner;
        let id = text(params, "request_id").unwrap_or_default();
        loop {
            let notified = inner.decided.notified();
            let left = {
                let pairing = lock(&inner.pairing);
                match pairing.as_ref().filter(|p| p.id == id) {
                    None => return Ok(paired_json(false, None)),
                    Some(p) => match p.decided {
                        Some(paired) => {
                            let device = paired.then(|| p.new.endpoint.clone());
                            return Ok(paired_json(paired, device));
                        }
                        None => inner.limits.pair_confirm_s - (inner.mono() - p.made),
                    },
                }
            };
            if left <= 0.0 {
                let ended = {
                    let mut pairing = lock(&inner.pairing);
                    pairing
                        .as_mut()
                        .filter(|p| p.id == id && p.decided.is_none())
                        .map(|p| {
                            p.decided = Some(false);
                            ended_json(p, "no answer")
                        })
                };
                if let Some(ended) = ended {
                    inner.pair_ended(ended);
                }
                return Ok(paired_json(false, None));
            }
            // the injected clock decides the deadline; a real wait in slices sees it pass
            let _ = tokio::time::timeout(Duration::from_secs_f64(left.min(1.0)), notified).await;
        }
    }

    /// `link.pair_cancel {request_id}`: the phone left before the owner answered.
    pub fn pair_cancel(&self, params: &Params) -> HandlerResult {
        let inner = &self.inner;
        let id = text(params, "request_id").unwrap_or_default();
        let mut pairing = lock(&inner.pairing);
        let cancelled = pairing
            .as_ref()
            .is_some_and(|p| p.id == id && p.decided.is_none());
        let taken = if cancelled { pairing.take() } else { None };
        drop(pairing);
        if let Some(p) = taken {
            inner.pair_ended(ended_json(&p, "phone left"));
        }
        inner.decided.notify_waiters();
        let mut out = Params::new();
        out.insert("cancelled".into(), json!(cancelled));
        Ok(out)
    }
}

/// A pairing or update that ended without a Yes, as `phone.pair_ended` says it.
fn ended_json(p: &Pending, why: &str) -> Value {
    let mut v = json!({"phone": p.new.endpoint, "paired": false, "why": why});
    if p.update {
        v["update"] = json!(true);
    }
    v
}

/// `link.pair_request`'s endpoint and six digits, before anything else is looked at.
fn pair_request_shape(params: &Params) -> Result<(String, String), HandlerError> {
    let bad = |why: &str| coded(refused(phone_gate::BAD_REQUEST, why));
    let endpoint = text(params, "endpoint")
        .filter(|e| e.chars().count() <= 200)
        .ok_or_else(|| bad("link.pair_request needs the phone's endpoint id"))?;
    let code = text(params, "code").ok_or_else(|| bad("link.pair_request needs the six digits"))?;
    let digits: Vec<char> = code.chars().collect();
    let six = digits.len() == 7
        && digits[3] == ' '
        && digits
            .iter()
            .enumerate()
            .all(|(i, c)| i == 3 || c.is_ascii_digit());
    if !six {
        return Err(bad("the pairing code is six digits, as \"123 456\""));
    }
    Ok((endpoint, code))
}

fn paired_json(paired: bool, device: Option<String>) -> Params {
    let mut out = Params::new();
    out.insert("paired".into(), json!(paired));
    if let Some(d) = device {
        out.insert("device".into(), json!(d));
    }
    out
}

type Handler = fn(&Phones, Session, Params) -> HandlerResult;

impl Inner {
    pub(crate) fn mono(&self) -> f64 {
        self.seams.clock.monotonic()
    }

    pub(crate) fn now_iso(&self) -> String {
        ferb_core::when::isoformat(&self.seams.clock.now())
    }

    pub(crate) fn nonce(&self) -> String {
        proof::b64u(&(self.seams.nonce)())
    }

    /// The live phone of this endpoint, else the refusal a removed or unknown phone gets.
    pub(crate) fn live(&self, device: &str) -> Result<Phone, Refusal> {
        let db = lock(&self.seams.db);
        match table::by_endpoint(db.conn(), device) {
            Ok(Some(p)) => Ok(p),
            Ok(None) => Err(refused(
                phone_gate::UNKNOWN_PHONE,
                "This phone is not one of FERB's paired phones.",
            )),
            Err(e) => Err(refused(wire_failed(), e.to_string())),
        }
    }

    /// One audit row (rule 7: `detail` holds structural facts only, and is redacted); rows older than the kept days
    /// go at the same time.
    pub(crate) fn audit(
        &self,
        device: &str,
        method: &str,
        class: &str,
        outcome: &str,
        detail: Value,
    ) {
        let now = self.seams.clock.now();
        let before = ferb_core::when::isoformat(
            &(now - chrono::Duration::days(self.limits.audit_keep_days)),
        );
        let row = AuditRow {
            at: ferb_core::when::isoformat(&now),
            device: device.to_owned(),
            method: method.to_owned(),
            class: class.to_owned(),
            outcome: outcome.to_owned(),
            detail: (self.seams.redact)(&detail),
        };
        let db = lock(&self.seams.db);
        let _ = table::audit(db.conn(), &row);
        let _ = table::prune_audit(db.conn(), &before);
    }

    /// A phone session's own call (`PHONE_SESSION`): answered by the gate with the session's state.
    fn on_session(
        &self,
        session: &Session,
        f: fn(
            &Inner,
            &mut gate::State,
            &Phone,
            &ferb_rpc::phone_gate::PhoneSession,
        ) -> Result<Params, Refusal>,
    ) -> HandlerResult {
        let phone = session.phone.clone().unwrap_or_default();
        let dev = self.live(&phone.device).map_err(coded)?;
        let mut st = lock(&self.state);
        f(self, &mut st, &dev, &phone).map_err(coded)
    }

    /// One pairing or update waits for the owner's answer at a time: a new one replaces it (the old is a No),
    /// journaled and published once the pairing's lock is let go (both take the database).
    fn wait_for_answer(&self, pending: Pending) {
        let replaced = lock(&self.pairing).replace(pending);
        if let Some(old) = replaced {
            if old.decided.is_none() {
                let mut ended =
                    json!({"phone": old.new.endpoint, "paired": false, "why": "replaced"});
                if old.update {
                    ended["update"] = json!(true);
                }
                self.pair_ended(ended);
            }
        }
        self.decided.notify_waiters();
    }

    fn pending_json(&self) -> Value {
        let pairing = lock(&self.pairing);
        match pairing
            .as_ref()
            .filter(|p| p.decided.is_none() && self.mono() - p.made < self.limits.pair_confirm_s)
        {
            Some(p) => {
                let mut v = json!({"request_id": p.id, "phone": p.new.endpoint, "name": p.new.name, "app": p.new.app,
                               "code": p.code, "seconds_left": (self.limits.pair_confirm_s - (self.mono() - p.made)).ceil()});
                if p.update {
                    v["update"] = json!(true);
                }
                v
            }
            None => Value::Null,
        }
    }

    /// A pairing ended (paired, denied, replaced, timed out, the phone left): journaled, and on the bus so FERB.app stops
    /// its countdown (S4).
    /// Journals and publishes: never called under the pairing's (or any) lock, as both take the database.
    fn pair_ended(&self, data: Value) {
        (self.seams.journal)("phone.pair_ended", data.clone());
        if let Value::Object(m) = data {
            (self.seams.publish)("phone.pair_ended", m);
        }
    }

    pub(crate) fn prompts(&self) -> Option<&Arc<dyn MacPrompts>> {
        self.seams.prompts.as_ref()
    }

    pub(crate) fn questions(&self) -> &Arc<dyn Questions> {
        &self.seams.questions
    }
}

fn wire_failed() -> i64 {
    ferb_rpc::wire::FAILED
}

/// `link.events`: the device list's changes, for the helper (a removed phone's connections close at once).
struct LinkEvents(broadcast::Receiver<Value>);

impl EventStream for LinkEvents {
    fn next(&mut self) -> BoxFuture<'_, Option<HandlerResult>> {
        Box::pin(async move {
            loop {
                match self.0.recv().await {
                    Ok(Value::Object(m)) => return Some(Ok(m)),
                    Ok(_) => continue,
                    // the helper fell behind: it reads the list again
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let mut m = Map::new();
                        m.insert("event".into(), json!("resync"));
                        return Some(Ok(m));
                    }
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        })
    }
}
