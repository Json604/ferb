//! FERB's self-description (DMN-6, DMN-22: `about.get`, `about.guide`, the always-on summary, the `about.read`
//! tool's document), FERB.app's saved secrets (APP-2: `secrets.saved`, `secrets.delete`) and "What FERB saw" (step
//! 12e: `seen.list`, `seen.image`). Port of `ferb/daemon/areas/about.py` and `ferb/daemon/about.py`.
//!
//! Must not change a setting, show a secret's value, let anything but FERB.app delete one or see a screenshot, take a
//! capture anew, or show an image of an app or site on the owner's exclusion list.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine;
use ferb_browser::profiles::{named, shown, BrowserProfile};
use ferb_identity::owner_secrets::{OwnerSecrets, ANY_SITE};
use ferb_rpc::{HandlerError, HandlerResult, Server};
use ferb_runtimes::catalog::{self, ROLES};
use rusqlite::OptionalExtension;
use serde_json::{json, Map, Value};

use super::core::{ok, refusal, Host, P};
use super::notify::SharedDb;

/// The document's sections, in order (`about.SECTIONS`; ORCH-578: what lets FERB act without asking).
pub const SECTIONS: [&str; 11] = [
    "version",
    "models",
    "account",
    "permissions",
    "settings",
    "records",
    "health",
    "cost",
    "app",
    "permissions_in_force",
    "saved_logins",
];
/// The app guide FERB.app sends: at most this many entries of each list, each string at most this long.
pub const GUIDE_ITEMS: usize = 80;
pub const GUIDE_CHARS: usize = 400;
/// A shared FERB.app setting's value is one short line (APP-728).
pub const VALUE_CHARS: usize = 120;

/// The always-on lines (`about.summary`): no live numbers (prompt cache). `models`: role -> "runtime model".
/// PRM-363: the conversation is one, shown in FERB.app and in FERB's phone app (Python said FERB "always replies in
/// text in FERB.app").
pub fn summary(version: &str, models: &[(String, String)]) -> String {
    let shown: Vec<String> = models.iter().map(|(r, m)| format!("{r} {m}")).collect();
    format!(
        "- About FERB: version {version}; models: {}. The owner types or dictates messages to FERB in FERB.app \
         on this Mac or in FERB's app on a paired phone (dictation turns their speech into text); FERB replies in \
         text in that one conversation, which FERB.app and FERB's phone app both show, and never speaks aloud: \
         it has no voice. FERB's settings (AI models, permissions, browsers, notifications, privacy and how long \
         records are kept) are the owner's to change in FERB.app's Settings; FERB never changes them and tells the \
         owner where they are. A worker reads FERB's full self-description (what it records about the owner, where \
         and for how long, its permissions and settings, health, cost and how to do things in FERB.app) with the \
         about.read tool.",
        shown.join("; ")
    )
}

/// `" ".join(x.split())[:GUIDE_CHARS]` for a str, else "".
fn guide_text(x: Option<&Value>) -> String {
    match x {
        Some(Value::String(s)) => {
            ferb_py::value::prefix(&ferb_py::value::squeeze(s), GUIDE_CHARS).to_owned()
        }
        _ => String::new(),
    }
}

fn dicts(v: Option<&Value>) -> Vec<&Map<String, Value>> {
    match v {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_object).collect(),
        _ => vec![],
    }
}

/// `about.clean_guide`: the app guide as FERB.app sent it, kept only in its shape (DMN-545).
///
/// ENG-6: Python iterated `howtos`, `shortcuts` and `app_settings` whatever their type, so a string there was walked
/// character by character (and a number raised TypeError, an internal error); here anything but a list counts as
/// none (a string's characters are never dicts, so the kept guide is the same).
pub fn clean_guide(value: &Value) -> Map<String, Value> {
    let empty = Map::new();
    let data = value.as_object().unwrap_or(&empty);
    let mut howtos = vec![];
    for h in dicts(data.get("howtos")).into_iter().take(GUIDE_ITEMS) {
        let (t, s) = (guide_text(h.get("title")), guide_text(h.get("steps")));
        if !t.is_empty() && !s.is_empty() {
            howtos.push(json!({"title": t, "steps": s}));
        }
    }
    let mut shortcuts = vec![];
    for k in dicts(data.get("shortcuts")).into_iter().take(GUIDE_ITEMS) {
        let (a, keys) = (guide_text(k.get("action")), guide_text(k.get("keys")));
        if !a.is_empty() && !keys.is_empty() {
            shortcuts.push(json!({"action": a, "keys": keys}));
        }
    }
    let mut places = Map::new();
    if let Some(Value::Object(raw)) = data.get("places") {
        for (k, v) in raw.iter().take(GUIDE_ITEMS) {
            let (k, v) = (
                guide_text(Some(&Value::String(k.clone()))),
                guide_text(Some(v)),
            );
            places.insert(k, Value::String(v));
        }
    }
    places.retain(|k, v| !k.is_empty() && v.as_str().is_some_and(|s| !s.is_empty()));
    let mut settings = vec![];
    for row in dicts(data.get("app_settings"))
        .into_iter()
        .take(GUIDE_ITEMS)
    {
        let mut entry = Map::new();
        for f in ["key", "name", "where", "what"] {
            entry.insert(f.into(), json!(guide_text(row.get(f))));
        }
        let filled = |f: &str| entry[f].as_str().is_some_and(|s| !s.is_empty());
        if filled("key") && filled("name") && filled("where") {
            if let Some(v @ Value::String(_)) = row.get("value") {
                let v = guide_text(Some(v));
                entry.insert(
                    "value".into(),
                    json!(ferb_py::value::prefix(&v, VALUE_CHARS)),
                );
            }
            settings.push(Value::Object(entry));
        }
    }
    let mut out = Map::new();
    out.insert("howtos".into(), json!(howtos));
    out.insert("shortcuts".into(), json!(shortcuts));
    out.insert("places".into(), Value::Object(places));
    out.insert("app_settings".into(), json!(settings));
    out
}

/// The browser facts the account section reads (the browsers area's): profiles found now (none before Full Disk
/// Access) and whether FERB's own profile is paired as FERB.
pub trait Account: Send + Sync {
    fn profiles(&self) -> Vec<BrowserProfile>;
    fn identity_paired(&self) -> bool;
}

/// Where the stored screenshots are, and what the evidence store's rows reach (`EvidenceStore.stored_file`).
pub struct Evidence {
    pub root: PathBuf,
}

impl Evidence {
    /// A row's file, only when it resolves inside the store (symlinks followed); else None.
    pub fn stored_file(&self, rel: Option<&str>) -> Option<PathBuf> {
        let rel = rel.filter(|r| !r.is_empty())?;
        let path = std::fs::canonicalize(self.root.join(rel)).ok()?;
        let root = std::fs::canonicalize(&self.root).ok()?;
        path.starts_with(&root).then_some(path)
    }
}

/// `self.tasks.tree_of(task_id)`.
pub type TreeOf = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// A source of values the other parts own.
pub type Read = Arc<dyn Fn() -> Result<Value, String> + Send + Sync>;

pub struct Deps {
    /// `ferb.__version__`.
    pub version: String,
    /// FERB.app's guide as last sent (`_app_guide`, in memory).
    pub guide: Arc<Mutex<Option<Map<String, Value>>>>,
    /// `self.permission_check()`: each permission's status (TCC reads, run off the loop).
    pub grants: Arc<dyn Fn() -> Map<String, Value> + Send + Sync>,
    /// `_setup_containment()`.
    pub containment: Arc<dyn Fn() -> Map<String, Value> + Send + Sync>,
    pub account: Arc<dyn Account>,
    /// The daemon's store (health, cost, the evidence rows); None before it opens.
    pub store: Option<SharedDb>,
    /// `store_size(path)` and `cleanup.pending(path)` of that store.
    pub store_facts: Arc<dyn Fn() -> (u64, bool) + Send + Sync>,
    /// records.py `_permissions_in_force` and `_saved_logins` (group A's records area).
    pub permissions_in_force: Read,
    pub saved_logins: Read,
    /// The owner's saved secrets (`OwnerSecrets(home)`).
    pub owner_secrets: Arc<OwnerSecrets>,
    pub evidence: Option<Arc<Evidence>>,
    /// `self.tasks.tree_of(task_id)`.
    pub tree_of: TreeOf,
    /// APP-927: Location's state as FERB.app last reported it (`device.report`), "" before its first report.
    pub app_location: Arc<dyn Fn() -> String + Send + Sync>,
}

/// `_runtime_for(role)` for the three configured roles (runtimes.py:30).
fn runtime_for(h: &Host, role: &str) -> Result<String, HandlerError> {
    let s = |v: Value| ferb_py::value::py_str(&v);
    let worker = s(h.get_or("runtimes", "worker", json!("codex"))?);
    Ok(match role {
        "orchestrator" => s(h.get_or("runtimes", "orchestrator", json!("codex"))?),
        "checker" => match h.get("runtimes", "checker")? {
            Some(v) => s(v),
            None => worker,
        },
        _ => worker,
    })
}

/// The config as the runtime catalog reads it.
struct Cfg<'a>(&'a Host);

impl catalog::Config for Cfg<'_> {
    fn get(&self, section: &str, key: &str) -> Option<Value> {
        self.0.config.get(section, key).ok().flatten()
    }
}

/// `_models`: role -> "runtime model" (planner for the orchestrator).
pub fn models(h: &Host) -> Result<Vec<(String, String)>, HandlerError> {
    let mut out = vec![];
    for role in ROLES {
        let name = runtime_for(h, role)?;
        let text = format!(
            "{} {}",
            catalog::label(&name),
            catalog::role_model(&Cfg(h), &name, role)
        );
        let role = if role == "orchestrator" {
            "planner"
        } else {
            role
        };
        out.push((role.to_owned(), ferb_py::value::strip(&text).to_owned()));
    }
    Ok(out)
}

/// `_about_summary`.
pub fn about_summary(h: &Host, version: &str) -> Result<String, HandlerError> {
    Ok(summary(version, &models(h)?))
}

fn config_text(h: &Host, section: &str, key: &str) -> Result<String, HandlerError> {
    Ok(match h.get(section, key)? {
        Some(v) if ferb_py::value::truthy(&v) => ferb_py::value::py_str(&v),
        _ => String::new(),
    })
}

/// `_identity_profile`: FERB's own profile from config (`[identity] browser/profile`).
pub fn identity_profile(h: &Host) -> Result<Option<(String, String)>, HandlerError> {
    let folder = h.get("identity", "profile")?;
    if !folder.as_ref().is_some_and(ferb_py::value::truthy) {
        return Ok(None);
    }
    let browser = ferb_py::value::py_str(&h.get_or("identity", "browser", json!("chrome"))?);
    Ok(Some((
        browser,
        ferb_py::value::py_str(&folder.unwrap_or_default()),
    )))
}

/// `_owner_browsing_profile` (`browsing.owner_profile`, APP-1).
pub fn owner_browsing_profile(h: &Host) -> Result<Option<(String, String)>, HandlerError> {
    let raw = config_text(h, "browsing", "owner_profile")?;
    let (b, f) = raw.split_once('/').unwrap_or((&raw, ""));
    Ok((!b.is_empty() && !f.is_empty()).then(|| (b.to_owned(), f.to_owned())))
}

fn fault(e: impl std::fmt::Display) -> HandlerError {
    HandlerError::fault("DatabaseError", e.to_string())
}

/// One permission as FERB explains it when the owner asks why (APP-927): what FERB uses it for, what it doesn't do,
/// what stays on the Mac and what is shared, how to turn it off and what stops then. Plain words about FERB (APP-926):
/// a model paraphrases them, so each is a fact of the code as built.
pub struct PermissionReason {
    pub id: &'static str,
    pub name: &'static str,
    /// Whose grant it is: "daemon" (FERB's background part) or "app" (FERB.app); "per app" for Automation.
    pub held_by: &'static str,
    pub uses: &'static str,
    pub does_not: &'static str,
    pub shared: &'static str,
    pub turn_off: &'static str,
    pub without_it: &'static str,
}

/// Every permission of FERB's setup list (the daemon's five, FERB.app's three) and Automation, in setup's order.
pub const PERMISSION_REASONS: [PermissionReason; 9] = [
    PermissionReason {
        id: "accessibility",
        name: "Accessibility",
        held_by: "daemon",
        uses: "FERB clicks, types and reads what's in app windows for you, in the background.",
        does_not: "It doesn't move your cursor or take your keyboard unless you say Yes, and it asks first before typing into a terminal or other hard-to-undo steps.",
        shared: "What it reads in a window is used for the task you gave; only what that task needs goes to the service you connected in FERB's Settings, with passwords and codes hidden first.",
        turn_off: "System Settings → Privacy & Security → Accessibility: switch FERB off.",
        without_it: "FERB can't click, type or read inside apps.",
    },
    PermissionReason {
        id: "screen_recording",
        name: "Screen Recording",
        held_by: "daemon",
        uses: "FERB takes pictures of the app window a task works in to check its work, and listens to an app's sound when a task asks.",
        does_not: "It doesn't record your whole screen in the background: a picture is of the window a task uses, taken during that task.",
        shared: "Pictures stay on this Mac with text such as passwords painted over; only what a task needs goes to the service you connected in FERB's Settings.",
        turn_off: "System Settings → Privacy & Security → Screen & System Audio Recording: switch FERB off.",
        without_it: "FERB can't see app windows or check its work by looking, and can't listen to an app.",
    },
    PermissionReason {
        id: "input_monitoring",
        name: "Input Monitoring",
        held_by: "daemon",
        uses: "FERB notices when you're typing, so it waits instead of getting in your way. It also sees text you type in ordinary fields, which goes into its private record of your activity on this Mac.",
        does_not: "It never reads password fields, anything typed while macOS's Secure Input is on, or the apps you exclude in FERB's Settings → Privacy, and it never blocks or changes your typing.",
        shared: "That record stays on this Mac; a task gets a part of it only when the task needs it.",
        turn_off: "System Settings → Privacy & Security → Input Monitoring: switch FERB off.",
        without_it: "FERB can't tell when you're busy typing, and keeps no record of what you type.",
    },
    PermissionReason {
        id: "full_disk_access",
        name: "Full Disk Access",
        held_by: "daemon",
        uses: "FERB reads and tidies your files when a task asks, including folders macOS protects, such as Mail and browser data.",
        does_not: "It keeps a copy before it changes or deletes a file, and asks first before changing a protected folder.",
        shared: "Files stay on this Mac; only what a task needs goes to the service you connected in FERB's Settings.",
        turn_off: "System Settings → Privacy & Security → Full Disk Access: switch FERB off.",
        without_it: "FERB can't read many of your files, such as Mail and browser folders, and tasks on them stop.",
    },
    PermissionReason {
        id: "clipboard",
        name: "Paste from Other Apps",
        held_by: "daemon",
        uses: "FERB sees what you copy, so it can use it when you ask, and keeps it on this Mac for 7 days.",
        does_not: "It skips what apps mark as private or passing (such as copied passwords).",
        shared: "Kept only on this Mac; a task gets it only when the task needs it.",
        turn_off: "System Settings → Privacy & Security → Paste from Other Apps: set FERB to Deny.",
        without_it: "FERB can't see what you copied.",
    },
    PermissionReason {
        id: "notifications",
        name: "Notifications",
        held_by: "app",
        uses: "FERB tells you when it needs you or finishes, and gives your reminders, when the notch can't show them.",
        does_not: "It only notifies about your own tasks, questions, reminders and notices.",
        shared: "Nothing leaves this Mac for this.",
        turn_off: "System Settings → Notifications → FERB.",
        without_it: "You see FERB's news only in the notch.",
    },
    PermissionReason {
        id: "microphone",
        name: "Microphone",
        held_by: "app",
        uses: "FERB turns your voice into text while you dictate, and the sound of an app it listens to during a task (Microphone and Speech Recognition).",
        does_not: "It listens only while you hold the dictation keys or a task listens to an app, never in the background.",
        shared: "Speech is turned into text on this Mac; the sound is never sent anywhere.",
        turn_off: "System Settings → Privacy & Security → Microphone (and Speech Recognition): switch FERB off.",
        without_it: "You can't dictate to FERB, and it can't listen to an app.",
    },
    PermissionReason {
        id: "location",
        name: "Location",
        held_by: "app",
        uses: "FERB knows which city this Mac is in, so local times and places come out right. It uses your exact location only in a task where you say Yes.",
        does_not: "It doesn't keep a history of where you've been: it holds only the city it last saw, until FERB restarts.",
        shared: "To name the city, macOS asks Apple's location service, at most once per kilometer moved. Your city goes with each request to the service you connected in FERB's Settings; your exact location goes only in a task where you said Yes, and only for that task.",
        turn_off: "System Settings → Privacy & Security → Location Services: switch FERB off.",
        without_it: "FERB knows only your time zone, not your city, and can't use your exact location.",
    },
    PermissionReason {
        id: "automation",
        name: "Automation",
        held_by: "per app",
        uses: "FERB controls an app through its own commands (such as Mail or Music) when a task needs it; macOS asks you once per app, the first time.",
        does_not: "It only uses the apps macOS asked you about and you allowed.",
        shared: "Nothing extra leaves this Mac for this; what a task does with the app follows the task.",
        turn_off: "System Settings → Privacy & Security → Automation: switch off FERB's apps one by one.",
        without_it: "FERB can't control that app by its commands; it may click instead, with Accessibility.",
    },
];

/// What FERB's phone app is (PHN-650), the first line of the document's `app.phone_app`.
pub const PHONE_APP_WHAT: &str = "FERB's app for Android phones and iPhones lets the owner use FERB on this Mac from \
their phone. FERB itself runs only on this Mac, so the Mac must be on with FERB running; the app looks and works the \
same on both kinds of phone.";

/// How to do things in FERB's phone app (PHN-650): (title, steps), written from what the apps do.
pub const PHONE_APP_HOWTOS: &[(&str, &str)] = &[
    (
        "Pair a phone",
        "On the Mac: FERB.app → Settings → Phone → Pair a phone. On the phone: open FERB and scan the code the Mac \
         shows, or tap Type code and type the text shown under the code. Both screens then show the same six digits: \
         say yes on the Mac only if they match. Up to 5 phones can be paired.",
    ),
    (
        "Open the app (the lock)",
        "FERB on the phone opens only with the phone's fingerprint, face or PIN (a phone with no screen lock can't \
         use it). It locks again after more than 5 minutes in the background, and the unlock prompt appears by itself.",
    ),
    (
        "Find your way around the app",
        "Four tabs at the top: Chat, Tasks, Files and Mac. The profile button at the top opens This phone: the \
         phone's own switches and Forget this Mac.",
    ),
    (
        "See earlier messages on the phone",
        "Chat is the same conversation as FERB.app on the Mac: a message sent from either shows in both, and FERB \
         answers in both. Chat opens on the conversation, with a line for each day; the home page shows only while \
         today's chat is empty. Scroll up to reach earlier messages and earlier days: they load as you reach the top \
         (or tap Earlier messages). Through the free shared relay, earlier messages load only once the phone reaches \
         the Mac directly or through your own relay.",
    ),
    (
        "Send a message while the Mac can't be reached",
        "What you send waits on the phone (Waiting for your Mac) and is sent when the Mac is back; Cancel takes it \
         back before then.",
    ),
    (
        "Speak instead of typing",
        "Hold the microphone in the message box while you speak and let go when you're done; the words appear in the \
         box so you can check them before sending. The phone turns speech into words itself when it can; when it \
         can't, FERB says so first and then uses the phone's own speech service (Google's on most Android phones, \
         Apple's on iPhone), which may get the sound of your voice. Your voice never goes to the Mac.",
    ),
    (
        "Answer FERB's questions on the phone",
        "A question shows in Chat (Tap to answer) and, when notifications reach the phone, in the notification \
         itself. Routine questions, using the Mac's screen included, are answered straight away. A Yes to a \
         payment, deleting, account security or typing in a terminal needs your fingerprint. A private answer (a \
         code or a password) is typed into its own hidden field and goes only to the Mac. A few questions can be \
         answered only in FERB on the Mac, and the phone says so.",
    ),
    (
        "Let FERB use the Mac's screen until you're back",
        "When FERB asks to use the Mac's screen, the phone also offers Yes, until I'm back at my Mac: FERB then uses \
         the Mac's screen without asking again until you use the Mac's keyboard or pointer, send a message from the \
         Mac, or the Mac locks or sleeps. Payments, deleting, new recipients and the like still ask. Ask me again \
         on the Mac tab ends it.",
    ),
    (
        "Follow and stop tasks from the phone",
        "The Tasks tab lists what waits for you, what is running (with how far it got) and what finished earlier. \
         Stop on a running task's card in Chat stops that task.",
    ),
    (
        "Watch FERB work from the phone",
        "Watch live on a running task's card shows what FERB is doing on the Mac, full screen. It needs the app \
         unlocked and a direct connection or your own relay, never the free shared relay. Nothing is recorded, and \
         the picture is kept out of screenshots.",
    ),
    (
        "Get files from the Mac on the phone",
        "The Files tab shows the Mac's Desktop, Documents and Downloads, Recent files (made by FERB, and files moved \
         between the phone and the Mac) and a search of the Mac by name; Load more shows the rest of a long list, \
         and Get this whole folder fetches a folder's files. Tap a file to get it, then open, share or save it. \
         Files kept in FERB stay on the phone until FERB locks; a file from a place FERB guards closely asks for your \
         fingerprint each time and is removed as soon as you come back to FERB.",
    ),
    (
        "Send files from the phone to the Mac",
        "Send files to your Mac (Files tab), or attach files to a message in Chat; photos and whole folders can go \
         too. They are saved in Downloads › FERB from phone on the Mac, unless another folder is chosen on the Mac in \
         Settings → Phone, and the phone says where each one landed. A transfer can be paused and goes on where it \
         stopped; an interrupted one is kept 7 days. Getting and sending files needs a direct connection or your own \
         relay.",
    ),
    (
        "Get FERB's notifications on the phone",
        "On Android, Stay connected (This phone, behind the profile button) keeps the link up in the background, so \
         FERB's questions, replies and finished tasks arrive as notifications. On iPhone there is no Stay connected: \
         notifications come while FERB is open and for a short while after leaving it; once it's closed, questions \
         show when you open FERB. The words of payment, deleting and account-security questions are kept off the \
         lock screen (Hide checks on the lock screen).",
    ),
    (
        "Stop everything from the phone",
        "Mac tab → Stop everything: FERB stops what it is doing on the Mac and starts nothing new until the stop is \
         lifted in FERB on the Mac.",
    ),
    (
        "Change FERB's settings from the phone",
        "Mac tab → FERB's settings: how long FERB keeps its records and whether it records window titles; each \
         change asks for your fingerprint. The lists of apps and sites FERB ignores are changed on the Mac.",
    ),
    (
        "This phone's own switches",
        "This phone (the profile button): Stay connected (Android), Block screenshots, Hide checks on the lock \
         screen, and Share this phone's city with FERB (the city only; the exact location is asked for each time a \
         task needs it).",
    ),
    (
        "Remove a phone or forget the Mac",
        "On the Mac: Settings → Phone → Paired phones → Rename or Remove…. On the phone: This phone → Forget this \
         Mac deletes the chat kept on the phone and tells the Mac to remove the phone; the Mac keeps everything, and \
         the phone can be paired again. A phone unused for 30 days stays paired, and FERB says so once.",
    ),
    (
        "How the phone reaches the Mac",
        "Directly when it can, otherwise through a relay. Everything is encrypted end to end, so no relay can read \
         it. Without a relay of your own the phone uses the free shared relay, which carries only messages, FERB's \
         replies, questions and answers, and Stop: files, Watch live and earlier messages wait for a direct \
         connection or your own relay (set on the Mac in Settings → Phone).",
    ),
];

/// What the phone app can't do yet (PHN-650).
pub const PHONE_APP_NOT_YET: &[&str] = &[
    "FERB can't control or use the phone itself: it works on this Mac only.",
    "On iPhone, nothing reaches the phone while FERB is closed: notifications for a closed app come later.",
    "There is no sign-in on the phone: the phone talks to FERB on this Mac, so nothing is done while the Mac is off.",
    "A paired phone can't remove another (lost) phone yet: remove it on the Mac.",
];

/// The document's `app.phone_app` (PHN-650, a named addition to Python's document, after FERB.app's own guide): what
/// FERB's phone app is, how to do things in it and what it can't do yet. Read with the rest of `app` by about.read.
pub fn phone_app() -> Value {
    json!({
        "what": PHONE_APP_WHAT,
        "howtos": PHONE_APP_HOWTOS.iter().map(|(t, s)| json!({"title": t, "steps": s})).collect::<Vec<_>>(),
        "not_yet": PHONE_APP_NOT_YET,
    })
}

/// Adds `phone_app` at the end of the document's `app` section (the document builder calls it; the recorded
/// documents' tests call it on Python's).
// decision: PHN-650
pub fn add_phone_app(doc: &mut Map<String, Value>) {
    if let Some(Value::Object(app)) = doc.get_mut("app") {
        app.insert("phone_app".into(), phone_app());
    }
}

/// The document's `permission_reasons` (APP-927, a named addition to Python's document): each permission's reasons
/// and its status now: the daemon's own grants (`grants`, by id: the four TCC rows), Location as FERB.app last reported
/// it (`location`, "" before its first report); Notifications, Microphone and the clipboard as FERB.app's Settings shows
/// them (this daemon doesn't read them here).
pub fn permission_reasons(grants: &Map<String, Value>, location: &str) -> Value {
    Value::Array(
        PERMISSION_REASONS
            .iter()
            .map(|r| {
                let status = match r.id {
                    "location" if !location.is_empty() => json!(location),
                    "location" | "notifications" | "microphone" => {
                        json!("set in FERB.app (Settings → Permissions shows it)")
                    }
                    "automation" => json!("asked per app (Settings → Permissions lists them)"),
                    id => grants
                        .get(id)
                        .cloned()
                        .unwrap_or(json!("see FERB.app's Settings → Permissions")),
                };
                json!({"id": r.id, "name": r.name, "status": status, "held_by": r.held_by, "uses": r.uses,
                       "does_not": r.does_not, "shared": r.shared, "turn_off": r.turn_off,
                       "without_it": r.without_it})
            })
            .collect(),
    )
}

/// APP-927, a named addition to Python's document: `permission_reasons` right after `permissions`, its statuses from
/// that section's rows. The document builder calls it; the recorded documents' tests call it on Python's.
pub fn add_reasons(doc: &mut Map<String, Value>, location: &str) {
    let grants: Map<String, Value> = doc
        .get("permissions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|r| (ferb_py::value::py_str(&r["id"]), r["status"].clone()))
        .collect();
    let mut out = Map::new();
    for (k, v) in std::mem::take(doc) {
        if k == "permission_reasons" {
            continue;
        }
        let after = k == "permissions";
        out.insert(k, v);
        if after {
            out.insert(
                "permission_reasons".into(),
                permission_reasons(&grants, location),
            );
        }
    }
    *doc = out;
}

/// `_about_document`: the whole self-description, redacted (FERB.app's About page and `about.read` read this one).
pub fn document(h: &Host, d: &Deps) -> HandlerResult {
    let guide = d
        .guide
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| clean_guide(&json!({})));
    let places = guide["places"].as_object().cloned().unwrap_or_default();
    let rows = ferb_mac_hands::permissions::rows(&(d.grants)(), &(d.containment)(), &[]);
    let values = crate::areas_a::settings::read(&h.config).map_err(|e| refusal(e.to_string()))?;
    let address = config_text(h, "mailbox", "address")?;
    let (prof, owner) = (identity_profile(h)?, owner_browsing_profile(h)?);
    // HAND-424: profiles by name with the account each is signed in to, read fresh
    let found = if prof.is_some() || owner.is_some() {
        d.account.profiles()
    } else {
        vec![]
    };
    let mine = prof.as_ref().map(|(b, f)| named(&found, b, f));
    let theirs = owner.as_ref().map(|(b, f)| named(&found, b, f));
    let account = if !address.is_empty() {
        json!({"email": address, "browser": prof.as_ref().map(|p| p.0.clone()).unwrap_or_default(),
               "profile": prof.as_ref().map(|p| p.1.clone()).unwrap_or_default(),
               "profile_name": mine.as_ref().map(|m| m.display_name.clone()).unwrap_or_default(),
               "paired": d.account.identity_paired()})
    } else {
        let mut note = "FERB has no Google account of its own; features that need its own inbox (one-time codes \
                        for its own sign-ups, sending from its address) aren't set up"
            .to_owned();
        if let Some(t) = &theirs {
            note.push_str(&format!("; it browses as the owner in {}", shown(t, None)));
        }
        let mut a = json!({"email": "", "note": note});
        if let Some(t) = &theirs {
            a["owner_profile"] = json!({"browser": t.browser, "folder": t.folder, "name": t.display_name,
                                         "email": t.email});
        }
        a
    };
    let settings: Vec<Value> = values
        .iter()
        .map(|(k, v)| {
            let place = places
                .get(k)
                .cloned()
                .unwrap_or(json!("FERB.app → Settings"));
            json!({"key": k, "value": v, "where": place})
        })
        .collect();
    let get_days = |section: &str, key: &str, default: i64| -> i64 {
        match h.config.get(section, key).ok().flatten() {
            Some(Value::Number(n)) => n
                .as_i64()
                .unwrap_or_else(|| n.as_f64().unwrap_or(0.0) as i64),
            Some(Value::Bool(b)) => i64::from(b),
            _ => default,
        }
    };
    let records = ferb_store::retention::describe(&get_days);
    let app_settings = guide.get("app_settings").cloned().unwrap_or(json!([]));
    let app = json!({"howtos": guide["howtos"], "shortcuts": guide["shortcuts"],
                     "where_settings_live": if places.is_empty() {
                         json!("FERB.app hasn't sent its guide since FERB started") } else { Value::Object(places) },
                     "settings": if ferb_py::value::truthy(&app_settings) { app_settings } else { json!([]) }});
    let in_force = match &d.store {
        Some(_) => (d.permissions_in_force)().map_err(fault)?,
        None => json!({}),
    };
    let doc = json!({
        "version": {"ferb": d.version},
        "models": models(h)?.into_iter().map(|(r, m)| (r, Value::String(m))).collect::<Map<String, Value>>(),
        "account": account,
        "permissions": rows.iter().map(|r| json!({"id": r["id"], "status": r["status"]})).collect::<Vec<_>>(),
        "settings": settings,
        "records": records,
        "health": health(h, d)?,
        "cost": cost(h, d)?,
        "app": app,
        "permissions_in_force": in_force,
        "saved_logins": (d.saved_logins)().map_err(fault)?,
    });
    let mut doc = doc.as_object().cloned().unwrap_or_default();
    add_reasons(&mut doc, &(d.app_location)());
    add_phone_app(&mut doc);
    ok(h.redact(&Value::Object(doc)))
}

/// `_about_health`: the database's size and clean-up state; FERB's CPU and memory over the last day (L30).
fn health(h: &Host, d: &Deps) -> Result<Value, HandlerError> {
    let mut out = Map::new();
    let Some(db) = &d.store else {
        return Ok(Value::Object(out));
    };
    let (size, pending) = (d.store_facts)();
    out.insert("database_bytes".into(), json!(size));
    out.insert("cleanup_waiting_for_ok".into(), json!(pending));
    let since = ferb_core::when::isoformat(&(h.now() - chrono::Duration::days(1)));
    let db = db.lock().unwrap();
    let conn = db.conn();
    let row = conn
        .query_row(
            "SELECT SUM(window_s), SUM(cpu_s), MAX(peak_footprint) FROM resource_samples WHERE at >= ?",
            [&since],
            |r| Ok((r.get::<_, Option<f64>>(0)?, r.get::<_, Option<f64>>(1)?, r.get::<_, Option<i64>>(2)?)),
        )
        .ok();
    let last = conn
        .query_row(
            "SELECT footprint FROM resource_samples ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get::<_, Option<i64>>(0),
        )
        .optional()
        .ok()
        .flatten();
    if let Some((Some(window), cpu, peak)) = row {
        if window != 0.0 {
            out.insert(
                "cpu_share_last_day".into(),
                json!(ferb_py_round(cpu.unwrap_or(0.0) / window, 4)),
            );
            out.insert("memory_peak_last_day_bytes".into(), json!(peak));
        }
    }
    if let Some(Some(now)) = last {
        out.insert("memory_now_bytes".into(), json!(now));
    }
    Ok(Value::Object(out))
}

/// Python's `round(x, n)` for a float (correctly rounded, half to even on the exact decimal value).
pub fn ferb_py_round(x: f64, n: i32) -> f64 {
    let text = format!("{:.*}", n as usize, x);
    text.parse().unwrap_or(x)
}

/// `_about_cost`: the priced model calls in traces over the last 7 and 30 days.
fn cost(h: &Host, d: &Deps) -> Result<Value, HandlerError> {
    let Some(db) = &d.store else {
        return Ok(json!({}));
    };
    let mut out = json!({"basis": "API-equivalent price of the model calls FERB traced"});
    let db = db.lock().unwrap();
    for days in [7, 30] {
        let since = ferb_core::when::isoformat(&(h.now() - chrono::Duration::days(days)));
        let sum: Option<f64> = db
            .conn()
            .query_row(
                "SELECT SUM(cost_usd) FROM spans WHERE cost_usd IS NOT NULL AND started_at >= ?",
                [&since],
                |r| r.get(0),
            )
            .map_err(fault)?;
        out[format!("last_{days}_days_usd")] = json!(ferb_py_round(sum.unwrap_or(0.0), 2));
    }
    Ok(out)
}

pub fn register(server: &Server, host: &Host, deps: &Arc<Deps>) {
    let (h, d) = (host.clone(), deps.clone());
    server.register_unary("about.get", move |_s, _p: Map<String, Value>| {
        let (h, d) = (h.clone(), d.clone());
        async move {
            tokio::task::spawn_blocking(move || document(&h, &d))
                .await
                .map_err(join)?
        }
    });
    let d = deps.clone();
    server.register_unary("about.guide", move |_s, p: Map<String, Value>| {
        let d = d.clone();
        async move {
            let g = clean_guide(&Value::Object(p));
            let n = |k: &str| match &g[k] {
                Value::Array(a) => a.len(),
                Value::Object(o) => o.len(),
                _ => 0,
            };
            let answer = json!({"howtos": n("howtos"), "shortcuts": n("shortcuts"), "places": n("places"),
                                "app_settings": n("app_settings")});
            *d.guide.lock().unwrap() = Some(g);
            ok(answer)
        }
    });
    let d = deps.clone();
    server.register_unary("secrets.saved", move |_s, _p: Map<String, Value>| {
        let d = d.clone();
        async move {
            let entries = tokio::task::spawn_blocking(move || d.owner_secrets.inventory())
                .await
                .map_err(join)?
                .map_err(identity_error)?;
            ok(json!({"entries": entries}))
        }
    });
    let (h, d) = (host.clone(), deps.clone());
    server.register_unary("secrets.delete", move |_s, p: Map<String, Value>| {
        let (h, d) = (h.clone(), d.clone());
        async move {
            let p = P(&p);
            let (place, name) = (p.text_or_empty("place"), p.text_or_empty("name"));
            if place.is_empty() || name.is_empty() {
                return Err(refusal("say which saved secret: its place and its name"));
            }
            let (pl, nm, r) = (place.clone(), name.clone(), h.redactor.clone());
            let out = tokio::task::spawn_blocking(move || {
                let identifier = pl == ANY_SITE;
                if identifier {
                    r.show_identifiers([]); // GATE-602: masked until the grants left are read again
                }
                let out = d.owner_secrets.delete(&pl, &nm)?;
                if identifier {
                    shown_again(&d.owner_secrets, &r)?;
                }
                Ok(out)
            })
            .await
            .map_err(join)?
            .map_err(identity_error)?;
            h.journal(
                "secret.deleted",
                json!({"place": place, "name": name}),
                Some("owner"),
            )?;
            ok(out)
        }
    });
    // GATE-602 (owner GATE-601): the owner's "don't mask this" for one saved identifier, and its end. FERB.app's
    // own calls (app_only); no tool or model reaches them.
    for (method, shown) in [("secrets.unmask", true), ("secrets.mask", false)] {
        let (h, d) = (host.clone(), deps.clone());
        server.register_unary(method, move |_s, p: Map<String, Value>| {
            let (h, d) = (h.clone(), d.clone());
            async move {
                let name = P(&p).text_or_empty("name");
                if name.is_empty() {
                    return Err(refusal("say which saved identifier: its name"));
                }
                let (nm, r) = (name.clone(), h.redactor.clone());
                let out = tokio::task::spawn_blocking(move || {
                    if !shown {
                        r.show_identifiers([]); // masked at once, whatever the Keychain answers next
                    }
                    let out = d.owner_secrets.set_shown(&nm, shown)?;
                    shown_again(&d.owner_secrets, &r)?;
                    Ok(out)
                })
                .await
                .map_err(join)?
                .map_err(identity_error)?;
                h.journal(
                    if shown {
                        "secret.unmasked"
                    } else {
                        "secret.masked"
                    },
                    json!({"name": name}),
                    Some("owner"),
                )?;
                ok(out)
            }
        });
    }
    let (h, d) = (host.clone(), deps.clone());
    server.register_unary("seen.list", move |_s, p: Map<String, Value>| {
        let (h, d) = (h.clone(), d.clone());
        async move { seen_list(&h, &d, &p) }
    });
    let (h, d) = (host.clone(), deps.clone());
    server.register_unary("seen.image", move |_s, p: Map<String, Value>| {
        let (h, d) = (h.clone(), d.clone());
        async move { seen_image(&h, &d, &p) }
    });
}

/// GATE-602: the redactor holds exactly the saved identifiers now marked shown (their values read from the Keychain).
fn shown_again(
    store: &OwnerSecrets,
    redactor: &ferb_sanitize::Redactor,
) -> ferb_identity::Result<()> {
    let values = store.shown_values()?;
    redactor.show_identifiers(values.iter().map(|v| v.expose()));
    Ok(())
}

fn join(e: tokio::task::JoinError) -> HandlerError {
    HandlerError::fault("RuntimeError", e.to_string())
}

/// `Refused` → FERB's own words; anything else is an internal error, as Python's other exceptions are.
fn identity_error(e: ferb_identity::Error) -> HandlerError {
    match e {
        ferb_identity::Error::Refused(text) => refusal(text),
        ferb_identity::Error::HelperFailed { message, .. } => refusal(message),
        other => HandlerError::fault("OSError", other.to_string()),
    }
}

/// `_seen_item`: one stored screenshot as "What FERB saw" lists it.
fn seen_item(row: &BTreeMap<String, Value>) -> Map<String, Value> {
    let data: Value = row
        .get("data_json")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .and_then(|s| ferb_py::json::loads(s).ok())
        .unwrap_or(json!({}));
    let data = if data.is_object() { data } else { json!({}) };
    let field = |k: &str| match data.get(k) {
        Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
        _ => String::new(),
    };
    let url = field("url");
    let site = if url.starts_with("http://") || url.starts_with("https://") {
        ferb_policy::pyurl::urlsplit(&url)
            .ok()
            .and_then(|s| s.hostname())
            .unwrap_or_default()
    } else {
        String::new()
    };
    let title = ["tab_title", "title", "window"]
        .iter()
        .find_map(|k| data.get(*k).filter(|v| ferb_py::value::truthy(v)).cloned());
    let title = match title {
        Some(Value::String(s)) => s,
        _ => String::new(),
    };
    let col = |k: &str| row.get(k).cloned().unwrap_or(Value::Null);
    let mut out = Map::new();
    out.insert("id".into(), col("id"));
    out.insert("at".into(), col("captured_at"));
    out.insert("kind".into(), col("kind"));
    out.insert("task_id".into(), col("task_id"));
    out.insert("app".into(), json!(field("app")));
    out.insert("bundle_id".into(), json!(field("bundle_id")));
    out.insert("site".into(), json!(site));
    out.insert("url".into(), json!(url));
    out.insert("title".into(), json!(title));
    out.insert("tool".into(), json!(field("tool")));
    out.insert("origin".into(), col("origin"));
    out
}

/// `_seen_excluded`: an image of an app or site the owner excludes now.
fn seen_excluded(h: &Host, item: &Map<String, Value>) -> Result<bool, HandlerError> {
    let ex = super::sensing::exclusions(h)?;
    let s = |k: &str| {
        item.get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    };
    let url = s("url").unwrap_or("");
    Ok(ex.app(s("bundle_id"), s("app"))
        || ex.url(Some(url).filter(|u| !u.is_empty()))
        || url == ferb_sanitize::redact::EXCLUDED
        || item
            .get("origin")
            .map(ferb_py::value::py_str)
            .is_some_and(|o| o.contains("excluded")))
}

fn sql_value(v: rusqlite::types::ValueRef<'_>) -> Value {
    use rusqlite::types::ValueRef;
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => json!(i),
        ValueRef::Real(f) => json!(f),
        ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
        ValueRef::Blob(b) => json!(hex::encode(b)),
    }
}

/// `_seen_rows`: stored images of the evidence window, newest first, that the exclusion list allows today.
fn seen_rows(
    h: &Host,
    db: &SharedDb,
    where_: &str,
    args: &[String],
    limit: usize,
) -> Result<(Vec<Map<String, Value>>, usize), HandlerError> {
    let days =
        ferb_store::retention::days(match h.get_or("retention", "evidence_days", json!(7))? {
            Value::Number(n) => n.as_i64().unwrap_or(7),
            _ => 7,
        });
    let cutoff = ferb_core::when::isoformat(&(h.now() - chrono::Duration::days(days)));
    let rows: Vec<BTreeMap<String, Value>> = {
        let db = db.lock().unwrap();
        let sql = format!(
            "SELECT * FROM evidence WHERE path IS NOT NULL AND mime LIKE 'image/%' AND captured_at >= ? \
             AND ({where_}) ORDER BY captured_at DESC, rowid DESC"
        );
        let mut stmt = db.conn().prepare(&sql).map_err(fault)?;
        let names: Vec<String> = stmt.column_names().map(|s| s.to_string()).collect();
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&cutoff];
        for a in args {
            params.push(a);
        }
        let mapped = stmt
            .query_map(params.as_slice(), |r| {
                let mut m = BTreeMap::new();
                for (i, n) in names.iter().enumerate() {
                    m.insert(n.clone(), sql_value(r.get_ref(i)?));
                }
                Ok(m)
            })
            .map_err(fault)?;
        mapped.collect::<Result<_, _>>().map_err(fault)?
    };
    let mut shown = vec![];
    for row in &rows {
        let item = seen_item(row);
        if !seen_excluded(h, &item)? {
            shown.push(item);
        }
    }
    let total = shown.len();
    shown.truncate(limit);
    Ok((shown, total))
}

/// `_seen_list`. ENG-6: a `limit` that isn't a whole number was Python's ValueError (an internal error); refused.
fn seen_list(h: &Host, d: &Deps, params: &Map<String, Value>) -> HandlerResult {
    let Some(db) = &d.store else {
        return Err(refusal("daemon not started"));
    };
    let p = P(params);
    let limit = match params.get("limit") {
        Some(v) if ferb_py::value::truthy(v) => match v {
            Value::Number(n) if n.as_i64().is_some() => n.as_i64().unwrap_or(300),
            Value::Number(n) => n.as_f64().unwrap_or(300.0) as i64,
            Value::Bool(_) => 1,
            Value::String(s) if ferb_py::value::strip(s).parse::<i64>().is_ok() => {
                ferb_py::value::strip(s).parse().unwrap_or(300)
            }
            _ => return Err(refusal("limit must be a number of screenshots")),
        },
        _ => 300,
    };
    let limit = limit.clamp(1, 2000) as usize;
    let (request, task) = (p.text_or_empty("request_id"), p.text_or_empty("task_id"));
    let (where_, args): (&str, Vec<String>) = if !request.is_empty() {
        (
            "task_id = ? OR task_id IN (SELECT id FROM tasks WHERE tree_id IN \
             (SELECT tree_id FROM tasks WHERE request_id = ?) OR request_id = ?)",
            vec![request.clone(), request.clone(), request],
        )
    } else if !task.is_empty() {
        let tree =
            (d.tree_of)(&task).unwrap_or_else(|| task.split('.').next().unwrap_or("").to_owned());
        (
            "task_id = ? OR (task_id >= ? || '.' AND task_id < ? || '/') OR \
             task_id IN (SELECT id FROM tasks WHERE tree_id = ?)",
            vec![tree.clone(), tree.clone(), tree.clone(), tree],
        )
    } else {
        ("1", vec![])
    };
    let (items, total) = seen_rows(h, db, where_, &args, limit)?;
    let days = match h.get("retention", "evidence_days")? {
        Some(v) if ferb_py::value::truthy(&v) => v.as_i64().unwrap_or(7),
        _ => 7,
    };
    ok(json!({"captures": items, "total": total, "days": days}))
}

/// `_seen_image`: one stored redacted screenshot's bytes (base64), as the model got it.
fn seen_image(h: &Host, d: &Deps, params: &Map<String, Value>) -> HandlerResult {
    let (Some(db), Some(evidence)) = (&d.store, &d.evidence) else {
        return Err(refusal("daemon not started"));
    };
    let eid = P(params).text_or_empty("id");
    let (items, _) = seen_rows(h, db, "id = ?", std::slice::from_ref(&eid), 1)?;
    let Some(item) = items.into_iter().next() else {
        return Err(refusal(
            "that screenshot isn't kept any more, or it shows something you exclude",
        ));
    };
    let row: Option<(Option<String>, Option<String>)> = db
        .lock()
        .unwrap()
        .conn()
        .query_row(
            "SELECT path, mime FROM evidence WHERE id = ?",
            [&eid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(fault)?;
    let path = row
        .as_ref()
        .and_then(|(p, _)| evidence.stored_file(p.as_deref()));
    let image = path.and_then(|p: PathBuf| std::fs::read(Path::new(&p)).ok());
    let Some(image) = image else {
        return Err(refusal("that screenshot isn't kept any more"));
    };
    let mime = row
        .and_then(|(_, m)| m)
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "image/jpeg".into());
    let mut out = item;
    out.insert("mime".into(), json!(mime));
    out.insert(
        "image".into(),
        json!(base64::engine::general_purpose::STANDARD.encode(image)),
    );
    Ok(out)
}

#[cfg(test)]
mod phone_app_tests {
    use super::*;

    fn apps(rel: &str) -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../apps")
                .join(rel),
        )
        .unwrap_or_else(|e| panic!("{rel}: {e}"))
    }

    /// PHN-650: the phone app's section names only what the apps show (each control by its own words in the Android
    /// app, the same words on iPhone) and the numbers FERB enforces; plain words about FERB, none about "AI". It sits at
    /// the end of `app`, so about.read's frozen `app` section carries it.
    // decision: PHN-650
    // decision: PHN-653
    #[test]
    fn the_phone_guide_names_what_the_apps_show() {
        let strings = apps("android/app/src/main/res/values/strings.xml").replace("\\'", "'");
        let design = apps("android/design/src/main/kotlin/com/ferb/design/components/TopBar.kt")
            + &apps("android/design/src/main/kotlin/com/ferb/design/screens/SheetScreens.kt")
            + &apps("android/app/src/main/kotlin/com/ferb/app/ui/Files.kt");
        for words in [
            ">Type code<",
            ">Earlier messages<",
            ">Waiting for your Mac<",
            ">Yes, until I'm back at my Mac<",
            ">Ask me again<",
            ">Watch live<",
            ">Stop everything<",
            ">Get this whole folder<",
            ">Load more<",
            ">Downloads › FERB from phone<",
            ">FERB's settings<",
            ">Block screenshots<",
            ">Hide checks on the lock screen<",
            ">Share this phone's city with FERB<",
            ">Forget this Mac<",
            ">Tap to answer<",
        ] {
            assert!(strings.contains(words), "strings.xml has {words}");
        }
        for words in [
            "\"Chat\"",
            "\"Tasks\"",
            "\"Files\"",
            "\"Mac\"",
            "\"Stay connected\"",
            "\"Send files to your Mac\"",
        ] {
            assert!(design.contains(words), "the Android app shows {words}");
        }
        let limits = crate::phone::Limits::default();
        assert_eq!(limits.devices_max, 5);
        assert_eq!(limits.unlock_idle_s, 300.0);
        assert_eq!(limits.device_unused_days, 30);
        assert_eq!(crate::phone::files::UPLOAD_PARTIAL_KEEP_S, 7.0 * 86_400.0);
        let all = phone_app().to_string();
        for said in [
            "Up to 5 phones",
            "more than 5 minutes",
            "kept 7 days",
            "unused for 30 days",
        ] {
            assert!(all.contains(said), "{said}");
        }
        let words: Vec<&str> = all.split(|c: char| !c.is_alphanumeric()).collect();
        assert!(!words.contains(&"AI"), "{all}");
        let mut doc = json!({"version": {}, "app": {"howtos": [], "settings": []}})
            .as_object()
            .unwrap()
            .clone();
        add_phone_app(&mut doc);
        let keys: Vec<&String> = doc["app"].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["howtos", "settings", "phone_app"]);
        assert!(doc["app"]["phone_app"]["howtos"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h["title"] == "See earlier messages on the phone"
                && h["steps"].as_str().unwrap().contains("Scroll up")));
    }
}

#[cfg(test)]
mod permission_reason_tests {
    use super::*;

    /// APP-927: one explanation for every permission FERB.app's setup lists (its Allow all order) plus Automation, the
    /// same ids; APP-926: plain words about FERB, none about "AI" or servers.
    // decision: APP-927
    // decision: APP-926
    #[test]
    fn every_setup_permission_is_explained_in_plain_words() {
        let swift = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../apps/mac/FERBNotch/Sources/FERBNotch/SetupModel.swift"),
        )
        .unwrap();
        let line = swift
            .split("static let allowAllSequence = [")
            .nth(1)
            .and_then(|t| t.split(']').next())
            .unwrap();
        let mut setup: Vec<String> = line
            .split(',')
            .map(|s| s.trim().trim_matches('"').to_string())
            .filter(|s| !s.is_empty())
            .collect();
        setup.push("automation".into());
        setup.sort();
        let mut ours: Vec<String> = PERMISSION_REASONS
            .iter()
            .map(|r| r.id.to_string())
            .collect();
        ours.sort();
        assert_eq!(ours, setup);
        for r in &PERMISSION_REASONS {
            for text in [r.uses, r.does_not, r.shared, r.turn_off, r.without_it] {
                assert!(!text.is_empty(), "{}", r.id);
                let words: Vec<&str> = text.split(|c: char| !c.is_alphanumeric()).collect();
                assert!(
                    !words.contains(&"AI") && !text.to_lowercase().contains("server"),
                    "{}: {text}",
                    r.id
                );
            }
        }
    }

    // decision: APP-927
    #[test]
    fn the_reasons_follow_permissions_with_each_status() {
        let mut doc = serde_json::json!({"version": {}, "permissions": [
            {"id": "accessibility", "status": "granted"}, {"id": "screen_recording", "status": "denied"}],
            "settings": []})
        .as_object()
        .unwrap()
        .clone();
        add_reasons(&mut doc, "granted");
        let keys: Vec<&String> = doc.keys().collect();
        assert_eq!(
            keys,
            ["version", "permissions", "permission_reasons", "settings"]
        );
        let rows = doc["permission_reasons"].as_array().unwrap();
        let status = |id: &str| rows.iter().find(|r| r["id"] == id).unwrap()["status"].clone();
        assert_eq!(status("accessibility"), "granted");
        assert_eq!(status("screen_recording"), "denied");
        assert_eq!(status("location"), "granted");
        assert!(status("microphone").as_str().unwrap().contains("FERB.app"));
    }
}
