//! What a paired phone may ask the engine to do: every RPC method of [`crate::auth::METHODS`] in one of four classes
//! (PHN-111, PHN-112, PHN-117, PHN-134). The paired phone is the owner's full remote, so the class decides what the
//! Mac checks before the call goes on, never whether a feature exists:
//!
//! - `plain`: the phone was unlocked towards the Mac within the unlock window (`unlock_idle_s`), PHN-125.
//! - `private`: the same window; it shows the owner's data, and the audit names it so.
//! - `sensitive`: a proof signed over this exact call with a fresh one-time challenge, by the phone's
//!   fingerprint-only key when it registered one (PHN-112, PHN-133). It changes FERB's protection, settings, keys,
//!   memory, schedules or what FERB may do, lifts a stop, deletes, or installs an update.
//! - `never`: calling it from a phone would break something; each row says what (PHN-117, PHN-134).
//!
//! Before the Mac has verified the phone's unlock a connection reaches only what [`Locked`] allows (PHN-125).
//!
//! `Role::Phone` is given only by the phone link's authenticated session (stage 3, S3): never by the local socket,
//! whose hello names only [`crate::auth::Role::ALL`]. GATE-612 keeps the owner's words and answers (`chat.send`,
//! `chat.approve`, `chat.answer`, `chat.correct`) to FERB.app among local programs; the phone, the owner's other
//! surface, reaches them only through the link, where the class and the unlock decide.
//!
//! A method with no row here is `never` ([`class`]): one added to the table later is refused until it is classified,
//! and `tests/phone_classes.rs` fails until it is. `engine/fixtures/phone/classes.json` is this table as data, for the
//! link crate (ferb-link) and the phone apps to compare against. Structure only: method names, never a judgment of
//! content.

/// A method's class for the phone role (PHN-112).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Class {
    Plain,
    Private,
    Sensitive,
    Never,
}

impl Class {
    pub const ALL: [Class; 4] = [Class::Plain, Class::Private, Class::Sensitive, Class::Never];

    pub fn as_str(self) -> &'static str {
        match self {
            Class::Plain => "plain",
            Class::Private => "private",
            Class::Sensitive => "sensitive",
            Class::Never => "never",
        }
    }
}

/// What a connection whose unlock the Mac has not verified may do with a method (PHN-125).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Locked {
    /// Refused until the unlock: everything but the rows below.
    Refused,
    /// Allowed while locked: learn that something waits (`chat.status`, its words held back unless they are for the
    /// paired phone's own notifications), stop FERB, `ping`.
    Allowed,
    /// An answer that does not rest on the unlock window: a plain No, one that carries its own proof (a hidden input's
    /// sealed answer, a Yes to one of FERB's own checks signed for this question: PHN-110, PHN-417), or a routine
    /// question's answer from the notification by a connection that proved the phone's device key (PHN-415). The
    /// daemon decides per question (S3); any other answer needs the window.
    Answer,
}

impl Locked {
    pub fn as_str(self) -> &'static str {
        match self {
            Locked::Refused => "refused",
            Locked::Allowed => "allowed",
            Locked::Answer => "answer",
        }
    }
}

/// One method's row: its class, and what else the phone's side must know about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhoneMethod {
    pub name: &'static str,
    pub class: Class,
    pub locked: Locked,
    /// The daemon answers it as a stream (`Server::register_stream`).
    pub stream: bool,
    /// It opens a prompt or a window on the Mac's screen: FERB clicks it on the owner's signed Yes (PHN-117, PHN-120).
    pub on_mac_screen: bool,
    /// Parameters a phone may not send with it: a path on the Mac that did not come from the phone's own upload, the
    /// test-run tag.
    pub refused_params: &'static [&'static str],
    /// The class is the safer choice where no decision covers who may do it from a phone: the owner's word is asked.
    pub owner_to_confirm: bool,
    /// Why this class (for `never`, the reason the phone is told).
    pub why: &'static str,
}

const fn row(name: &'static str, class: Class, why: &'static str) -> PhoneMethod {
    PhoneMethod {
        name,
        class,
        locked: Locked::Refused,
        stream: false,
        on_mac_screen: false,
        refused_params: &[],
        owner_to_confirm: false,
        why,
    }
}
const fn plain(name: &'static str, why: &'static str) -> PhoneMethod {
    row(name, Class::Plain, why)
}
const fn private(name: &'static str, why: &'static str) -> PhoneMethod {
    row(name, Class::Private, why)
}
const fn sensitive(name: &'static str, why: &'static str) -> PhoneMethod {
    row(name, Class::Sensitive, why)
}
const fn never(name: &'static str, why: &'static str) -> PhoneMethod {
    row(name, Class::Never, why)
}

impl PhoneMethod {
    const fn locked(self, locked: Locked) -> Self {
        PhoneMethod { locked, ..self }
    }
    const fn stream(self) -> Self {
        PhoneMethod {
            stream: true,
            ..self
        }
    }
    const fn on_mac_screen(self) -> Self {
        PhoneMethod {
            on_mac_screen: true,
            ..self
        }
    }
    const fn refuse(self, refused_params: &'static [&'static str]) -> Self {
        PhoneMethod {
            refused_params,
            ..self
        }
    }
    /// Marks a row whose class waits for the owner's word (none since PHN-467).
    #[allow(dead_code)]
    const fn ask(self) -> Self {
        PhoneMethod {
            owner_to_confirm: true,
            ..self
        }
    }
}

const PHONE_SESSION_WHY: &str =
    "the phone session's own call: the daemon's phone gate answers it and checks the proof it carries (PHN-124)";
const LINK_CONTROL_WHY: &str =
    "is FERB.app's control of the phone link (the pairing code, the owner's relay, the switch), the Mac owner's side (PHN-106)";
const LINK_WHY: &str =
    "is the phone link helper's own call about pairing and the device list, the Mac owner's side (PHN-106)";

const LOCKED_REASON: &str =
    "a locked phone must be able to stop FERB and see that something waits (PHN-125)";

/// Every method of [`crate::auth::METHODS`], grouped; [`phone_method`] finds a row by name.
pub const PHONE_METHODS: &[PhoneMethod] = &[
    // ---- reachable while locked (PHN-125)
    plain("ping", LOCKED_REASON).locked(Locked::Allowed),
    plain("chat.status", LOCKED_REASON).locked(Locked::Allowed),
    plain("stop.engage", LOCKED_REASON).locked(Locked::Allowed),
    // decision: GATE-615 — "FERB may use your Mac's screen until you're back" and its end: ending takes something away,
    // like Stop, so a locked phone may do it
    plain(
        "screen_use.status",
        "whether FERB may use the Mac's screen until the owner is back, so the phone shows it with a way to end it",
    )
    .locked(Locked::Allowed),
    plain(
        "screen_use.end",
        "ends \"until I'm back at my Mac\": it only takes something away, like Stop (GATE-615)",
    )
    .locked(Locked::Allowed),
    plain("task.stop", LOCKED_REASON).locked(Locked::Allowed),
    // decision: APP-983 — Lock my Mac and It's fine answer a "someone's at your Mac" notice from the notification, and
    // ending insomnia mode lets the Mac sleep as usual: each only takes something away, like Stop
    plain(
        "insomnia.end",
        "ends insomnia mode: the Mac sleeps as usual again; it only takes something away, like Stop (APP-980)",
    )
    .locked(Locked::Allowed),
    plain(
        "insomnia.lock",
        "the owner's Lock my Mac after someone touched it: the Mac's own lock, only ever safer (APP-983)",
    )
    .locked(Locked::Allowed),
    plain(
        "insomnia.dismiss",
        "the owner's It's fine to a notice that someone touched the Mac; it changes nothing on the Mac (APP-983)",
    )
    .locked(Locked::Allowed),
    // ---- the owner's words and answers: the phone is his surface (PHN-111); which answer needs the fingerprint is
    //      the question's, decided by the daemon (PHN-415, PHN-417, PHN-403)
    plain("chat.send", "the owner's message, as from FERB.app")
        .stream()
        .refuse(&["input_paths", "test_run"]),
    plain("chat.attach", "follows a turn already running, or starts a fresh conversation (history kept)"),
    // ORCH-699: the temporary chat (held in the daemon's memory): reading it is the owner's conversation; the icon only
    // starts or ends it
    private("chat.temporary.get", "the owner's temporary conversation"),
    plain("chat.temporary.set", "the owner's temporary-chat icon: starts or ends it"),
    plain("chat.approve", "the owner's answer; a hard question's Yes carries its own fingerprint proof")
        .locked(Locked::Answer),
    plain("chat.answer", "the owner's answer; a hidden input only through the sealed path (PHN-110)")
        .locked(Locked::Answer),
    plain("chat.correct", "the owner's correction to a question").locked(Locked::Answer),
    // ---- the conversation, fetched like from a server (ENG-10, ENG-14; T-1201: reads private, mark plain)
    private("chat.entries.changes", "the owner's conversation"),
    private("chat.entries.days", "the owner's conversation"),
    private("chat.entries.get", "the owner's conversation"),
    plain("chat.entries.mark", "marks what this phone has seen; shows nothing"),
    private("chat.entries.page", "the owner's conversation"),
    private("chat.entries.watch", "the owner's conversation, live").stream(),
    private("chat.history.days", "FERB.app's saved chat, read"),
    private("chat.history.page", "FERB.app's saved chat, read"),
    never(
        "chat.history.put",
        "is FERB.app's own record of its chat window; a second writer would mix two chats",
    ),
    never(
        "chat.history.import",
        "is FERB.app's own record of its chat window; a second writer would mix two chats",
    ),
    // ---- tasks and what they show
    plain("tasks.list", "which tasks run; no content"),
    private("tasks.preview", "the Mac's screen, unmasked inside the link only (PHN-454, APP-922)"),
    private("trace.get", "what a task did, with its content"),
    private("timeline.get", "what FERB did, with its content"),
    private("questions.picture", "a question's picture, such as a payment page (PHN-403)"),
    plain("results.ack", "marks a result seen"),
    plain("attention.ack", "marks a notice seen"),
    plain("notices.list", "FERB's notices"),
    plain("conversations.list", "the conversations' names"),
    sensitive("conversations.change", "changes which conversation FERB keeps"),
    plain("ps.list", "running commands, no output"),
    private("ps.attach", "a running command's output").stream(),
    sensitive("ps.send", "types into a running command"),
    sensitive("ps.kill", "ends a running command"),
    plain("reminders.list", "the owner's reminders"),
    plain("reminders.cancel", "cancels one of the owner's reminders"),
    plain("timers.states", "loops' states"),
    plain("timers.repeat", "runs a loop again now"),
    plain("schedules.list", "the schedules"),
    sensitive("schedules.change", "changes what FERB does on its own"),
    // ---- memory
    private("memory.list", "what FERB remembers of the owner"),
    private("memory.show", "what FERB remembers of the owner"),
    private("memory.profile", "what FERB remembers of the owner"),
    private("memory.repeats", "what FERB learned from the owner's tasks"),
    plain("memory.learning", "whether learning is on"),
    sensitive("memory.add", "changes FERB's memory"),
    sensitive("memory.edit", "changes FERB's memory"),
    sensitive("memory.forget", "deletes from FERB's memory"),
    sensitive("memory.restore", "changes FERB's memory"),
    sensitive("memory.retract", "changes FERB's memory"),
    plain("memory.import.estimate", "the size of an import"),
    sensitive("memory.import.start", "fills FERB's memory from another assistant's export"),
    plain("memory.import.status", "an import's progress"),
    plain("memory.import.cancel", "stops an import"),
    sensitive("memory.import.scope", "changes where an imported memory applies"),
    private("memory.import.lines", "where imported memories apply and their offers"),
    sensitive("memory.import.offer", "answers an imported memory's offer"),
    // decision: MEM-459 — the owner's switches for bringing in what other assistants know: his fingerprint on a phone
    sensitive("memory.import.switch", "turns bringing in what other assistants know on or off"),
    // MEM-460: the review of what an import brought in, and the owner's answers to it item by item: his answer to
    // FERB's own question (chat.answer's class is plain), shown with his data, so private
    private("memory.review", "what FERB asks the owner to check after an import"),
    private("memory.review.answer", "the owner's answer to FERB's review of an import"),
    // ---- perception
    plain("sense.status", "whether FERB watches the Mac"),
    plain("sense.pause", "stops FERB watching: only ever less"),
    sensitive("sense.resume", "lets FERB watch the Mac again"),
    private("seen.list", "what FERB saw on the Mac"),
    private("seen.image", "a picture of what FERB saw on the Mac"),
    plain("jev.stats", "the decision engine's counts"),
    sensitive("jev.enable", "changes what FERB may do"),
    sensitive("jev.reload", "changes what FERB may do"),
    // ---- insomnia mode (T-1274)
    private(
        "insomnia.status",
        "whether FERB keeps the Mac working while the owner is away, with its battery, heat and notices",
    ),
    // decision: APP-980 — only the owner turns it on, from the Mac or his phone after the warning: signed like a setting
    sensitive(
        "insomnia.start",
        "keeps the Mac awake and working (and, with the cover, lets FERB bring apps forward under it: APP-984)",
    ),
    // decision: APP-983 — Show me: the Mac's camera, only for the paired, unlocked phone, never on the shared relay
    private(
        "insomnia.camera",
        "the Mac's camera, live, for the owner's own eyes (APP-983); never on the shared relay (PHN-454)",
    ),
    never(
        "insomnia.report",
        "is FERB.app's own report of what happens at its cover on the Mac's screen",
    ),
    never(
        "insomnia.camera_frame",
        "is FERB.app's own picture from the Mac's camera; a phone only watches it",
    ),
    // ---- settings, keys and model runtimes
    private("settings.get", "the owner's settings (PHN-404)"),
    sensitive("settings.set", "changes FERB's settings (PHN-404)"),
    sensitive("keys.store", "stores a provider key"),
    sensitive("keys.delete", "deletes a provider key"),
    plain("runtimes.status", "which runtimes are ready"),
    plain("runtimes.check", "checks a runtime"),
    plain("runtimes.models", "the models offered"),
    sensitive("runtimes.check_calls", "spends a provider's calls"),
    sensitive("runtimes.model", "changes FERB's model"),
    sensitive("runtimes.use", "changes FERB's runtime"),
    sensitive("runtimes.sign_in", "signs FERB in to a provider"),
    sensitive("runtimes.switch_task", "moves a task to another runtime"),
    // ---- saved secrets and identities: names and handles only, values never travel
    private("secrets.saved", "the saved-secrets list, names only (PHN-404)"),
    sensitive("secrets.delete", "deletes a saved secret"),
    sensitive("secrets.unmask", "lets the AI see a saved identifier (PHN-402)"),
    sensitive("secrets.mask", "changes what the redactor masks"),
    plain("identity.check", "whether FERB's own accounts are set"),
    private("identity.find", "the owner's accounts and profiles"),
    sensitive("identity.choose", "changes whose accounts FERB acts with"),
    sensitive("identity.pair", "pairs a browser profile"),
    // ---- browsers and setup
    plain("browsers.installed", "which browsers the Mac has"),
    plain("browsers.list", "the paired browsers"),
    sensitive("browsers.pair", "changes where FERB may act"),
    sensitive("browsers.unpair", "changes where FERB may act"),
    plain("setup.browsers", "the browsers' setup state"),
    sensitive("setup.browser_pair", "changes where FERB may act"),
    sensitive("setup.browsers_pair_all", "changes where FERB may act"),
    sensitive("setup.browsers_pairing", "changes where FERB may act").stream(),
    plain("setup.permissions", "which permissions FERB holds"),
    sensitive("setup.persona", "changes FERB's persona"),
    sensitive("setup.finish", "changes FERB's setup"),
    // PHN-467: the owner's no
    never(
        "setup.uninstall_browser_files",
        "is a step of FERB.app's Uninstall: from a phone it would cut FERB off from its browsers with FERB still installed",
    ),
    // ---- prompts and windows on the Mac's screen (PHN-117, PHN-120): FERB clicks them on the owner's signed Yes
    sensitive("automation.request", "asks macOS for a permission").on_mac_screen(),
    sensitive("clipboard.request", "reads or writes the Mac's clipboard").on_mac_screen(),
    sensitive("setup.permission_request", "asks macOS for a permission").on_mac_screen(),
    sensitive("setup.browser_open", "opens a browser on the Mac").on_mac_screen(),
    sensitive("ui.click_through", "lets clicks through FERB's window").on_mac_screen(),
    sensitive("browser.show_handoff", "shows a browser window for the owner").on_mac_screen(),
    // ---- stops, updates, reports and recovery
    sensitive("stop.lift", "lifts a stop"),
    sensitive("update.prepare", "installs an update"),
    sensitive("verdict.set", "records the owner's verdict on a task, which FERB learns from"),
    plain("about.get", "FERB's version"),
    plain("doctor.check", "FERB's health"),
    private("logs.subscribe", "FERB's logs").stream(),
    private("reports.build", "a report of the owner's tasks"),
    private("reports.diagnostic", "a report of the owner's tasks"),
    sensitive("reports.send", "sends a report off the Mac"),
    private("recovery.list", "the recovery copies of the owner's files"),
    sensitive("recovery.restore", "overwrites the owner's files"),
    // ---- the stored-data viewer and the owner's delete (STORE-422..425; PHN-402: viewing unlocked, deleting with
    //      the fingerprint)
    private("data.tables", "the stored data (PHN-402)"),
    private("data.rows", "the stored data (PHN-402)"),
    private("data.row", "the stored data (PHN-402)"),
    private("data.cell", "the stored data (PHN-402)"),
    private("data.search", "the stored data (PHN-402)"),
    private("data.context", "the stored data (PHN-402)"),
    sensitive("data.delete", "deletes a turn (PHN-402)"),
    // ---- the owner's projects (HAND-620, HAND-625); a publish question is answered through chat.approve
    private("projects.list", "the owner's project folders"),
    sensitive("projects.remove", "changes what FERB may write and publish"),
    // ---- FERB.app's own (PHN-134), and what FERB.app reports of this Mac
    plain("intro.suggest", "first-run suggestions"),
    sensitive("notifications.categories", "changes how the Mac shows FERB's notifications"),
    never(
        "notifications.subscribe",
        "while open it takes FERB's notifications on the Mac away from FERB.app",
    )
    .stream(),
    never(
        "intro.preview",
        "is FERB.app's own first-run and how-to data, written by the app",
    )
    .stream(),
    never(
        "about.guide",
        "is FERB.app's own first-run and how-to data, written by the app",
    ),
    // PHN-467: a phone reads the Mac's facts (private) but sends its own device record by a phone method (T-1214)
    never(
        "device.report",
        "is FERB.app's report of where this Mac is, from the Mac's own location; a phone's would move it",
    ),
    // ---- stage 3 (T-1202): the phone session's own calls (`auth::PHONE_SESSION`), answered by the daemon's phone gate,
    //      which checks the proof each carries itself; a locked phone needs them to unlock (PHN-124, PHN-125)
    plain("phone.challenge", PHONE_SESSION_WHY).locked(Locked::Allowed),
    // PHN-646: removing itself only takes this phone's own access away; the gate wants its device key proved
    plain("phone.forget_self", PHONE_SESSION_WHY).locked(Locked::Allowed),
    plain("phone.keys", PHONE_SESSION_WHY).locked(Locked::Allowed),
    plain("phone.lock", PHONE_SESSION_WHY).locked(Locked::Allowed),
    plain("phone.present", PHONE_SESSION_WHY).locked(Locked::Allowed),
    plain("phone.session_end", PHONE_SESSION_WHY).locked(Locked::Allowed),
    plain("phone.unlock", PHONE_SESSION_WHY).locked(Locked::Allowed),
    plain("phone.unlock_statement", PHONE_SESSION_WHY).locked(Locked::Allowed),
    // ---- stage 3 S3b (T-1203): files both ways (`auth::PHONE_FILES`). The window first, like every private call; the
    //      daemon's file area then judges the path as the model's file tools are judged (ferb-policy's protection
    //      lists), asks a key place for its own fresh fingerprint proof and audits each with its path (PHN-113, PHN-114,
    //      PHN-121, PHN-127). A transfer under way goes on after the phone locks: its chunks ride the begin's unlock.
    private("phone.files", "a folder's entries on the Mac (PHN-113); a key place needs its own fresh proof (PHN-121)"),
    private("phone.tree", "a folder's files, to fetch it whole (PHN-113); key places inside are left out"),
    private("phone.read", "one file from the Mac (PHN-113, PHN-115); a key place needs its own fresh proof (PHN-121)")
        .stream(),
    plain("phone.put_group", "starts a folder sent from the phone into its upload folder (PHN-127)"),
    plain("phone.put_begin", "starts a file sent from the phone into its upload folder (PHN-127)"),
    plain("phone.put_chunk", "carries an upload its unlocked begin started (PHN-127)").locked(Locked::Allowed),
    plain("phone.put_end", "finishes, pauses or drops an upload its unlocked begin started (PHN-127)")
        .locked(Locked::Allowed),
    // ---- stage 3 (T-1214, T-1211): the phone's own record and finding the Mac's files (`auth::PHONE_OWN`)
    // decision: PHN-467
    plain(
        "phone.device_report",
        "the phone's own time zone, city and state for FERB (SENSE-420); nothing of FERB's goes back. While the app is \
         locked only from a connection that proved the phone's device key (PHN-126)",
    )
    .locked(Locked::Allowed),
    private("phone.recent", "the files FERB made and this phone fetched or sent (PHN-113); key places are left out"),
    private("phone.search", "file names under a folder the phone may browse (PHN-113); never contents, key places left out"),
    // ---- FERB.app's Settings → Phone: the link's state is the Mac owner's side (PHN-106), except what PHN-419 gives a
    //      second phone
    private(
        "phone.list",
        "the paired phones, so a second phone can remove a lost one (PHN-419)",
    ),
    sensitive(
        "phone.remove",
        "removes a paired phone: a second phone removes a lost one with its fingerprint (PHN-419)",
    ),
    // decision: PHN-581
    never(
        "phone.upload_folder",
        "chooses where files from a phone land on the Mac, which is chosen at the Mac (owner, 2026-11-17)",
    ),
    never(
        "phone.rename",
        "changes the paired-phone list, which is the Mac owner's side (PHN-106)",
    ),
    never(
        "phone.pair_decide",
        "is the owner's Yes at the Mac to a new phone; a phone saying it would pair phones by itself (PHN-102, PHN-106)",
    ),
    never(
        "phone.audit",
        "is the record of what phones did, which no phone call reads or changes (PHN-114)",
    ),
    // ---- stage 3 S4 (T-1204): FERB.app's control of the phone link through the daemon (`auth::PHONE_LINK_ADDITIONS`):
    //      the link's state is the Mac owner's side (PHN-106)
    never("phone.enable", LINK_CONTROL_WHY),
    never("phone.link_status", LINK_CONTROL_WHY),
    never("phone.pair_cancel", LINK_CONTROL_WHY),
    never("phone.pair_start", LINK_CONTROL_WHY),
    never("phone.relay_always", LINK_CONTROL_WHY),
    never("phone.relay_set", LINK_CONTROL_WHY),
    never("phone.relay_test", LINK_CONTROL_WHY),
    // ---- the phone link helper's own calls (role link), never a phone's (PHN-106, PHN-441)
    never("link.answer", LINK_WHY),
    never("link.devices", LINK_WHY),
    never("link.events", LINK_WHY).stream(),
    never("link.pair_cancel", LINK_WHY),
    never("link.pair_request", LINK_WHY),
    never("link.pair_wait", LINK_WHY),
    // ---- not FERB.app's: a model worker's tool bridge, bound to its run token
    never(
        "tools.call",
        "is a model worker's tool bridge, bound to its run token; the phone asks FERB through the chat",
    ),
    never(
        "tools.list",
        "is a model worker's tool bridge, bound to its run token; the phone asks FERB through the chat",
    ),
];

/// The method's row, when it has one.
pub fn phone_method(name: &str) -> Option<&'static PhoneMethod> {
    PHONE_METHODS.iter().find(|row| row.name == name)
}

/// The method's class for the phone role. Default-deny: a method with no row (one FERB adds later, a misspelling)
/// is `never` (PHN-112).
pub fn class(name: &str) -> Class {
    phone_method(name).map_or(Class::Never, |row| row.class)
}

/// What a connection whose unlock the Mac has not verified may do with it (PHN-125); `Refused` for a method with no
/// row.
pub fn locked(name: &str) -> Locked {
    match phone_method(name) {
        Some(row) if row.class != Class::Never => row.locked,
        _ => Locked::Refused,
    }
}

/// The table as data (`engine/fixtures/phone/classes.json`), sorted by name: what the link crate and the phone apps
/// compare against.
pub fn as_json() -> serde_json::Value {
    let mut rows: Vec<&PhoneMethod> = PHONE_METHODS.iter().collect();
    rows.sort_by_key(|row| row.name);
    let methods: serde_json::Map<String, serde_json::Value> = rows
        .into_iter()
        .map(|row| {
            (
                row.name.to_owned(),
                serde_json::json!({
                    "class": row.class.as_str(),
                    "locked": row.locked.as_str(),
                    "stream": row.stream,
                    "on_mac_screen": row.on_mac_screen,
                    "refused_params": row.refused_params,
                    "owner_to_confirm": row.owner_to_confirm,
                    "why": row.why,
                }),
            )
        })
        .collect();
    let counts: serde_json::Map<String, serde_json::Value> = Class::ALL
        .iter()
        .map(|c| {
            let n = PHONE_METHODS.iter().filter(|row| row.class == *c).count();
            (c.as_str().to_owned(), n.into())
        })
        .collect();
    serde_json::json!({ "counts": counts, "methods": methods })
}
