//! Connection roles, the method table and scoped run tokens. Port of `ferb/daemon/auth.py`.
//!
//! - cli: proven by socket ownership: the peer's uid must be the owner's.
//! - mcp_shim: proven by a run token (scoped to one worker, expiring, revocable; only its hash is kept).
//! - native_host: the relay a browser launches for the FERB extension. The socket peer must be the owner's uid
//!   and the caller origin one of FERB's extension ids; after that the connection is a message channel whose
//!   every message is authenticated by pairing, not by method lists.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

/// auth.py:21-24.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Cli,
    McpShim,
    NativeHost,
    /// A paired phone (stage 3, PHN-111, PHN-112): given only by the phone link's authenticated session, never by the
    /// local socket's hello (it is not in [`Role::ALL`]). What it may call is `crate::classes`, not `Method::roles`.
    Phone,
    /// The phone link's own helper program (stage 3, PHN-441): FERB's signed helper inside FERB.app, proven by its code
    /// signature as FERB.app is for `app_only` (`crate::link_peer`). Not in [`Role::ALL`]: a local hello naming "link"
    /// gets it only from that helper. Its methods are the `link.*` rows (pairing, the device list); it opens phone
    /// sessions (`Role::Phone`) and never reaches FERB.app's methods.
    Link,
}

impl Role {
    /// The roles a local connection may name in its hello (Python's three). Never [`Role::Phone`].
    pub const ALL: [Role; 3] = [Role::Cli, Role::McpShim, Role::NativeHost];

    /// The role's name on the wire (`Role.value`).
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Cli => "cli",
            Role::McpShim => "mcp_shim",
            Role::NativeHost => "native_host",
            Role::Phone => "phone",
            Role::Link => "link",
        }
    }

    /// The role a local hello names; `Role::Phone` is never one of them.
    pub fn from_wire(name: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|role| role.as_str() == name)
    }
}

/// One RPC method: who may call it, and whether only FERB.app's own process may (auth.py:61-69).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Method {
    pub name: &'static str,
    pub roles: &'static [Role],
    pub app_only: bool,
}

const fn m(name: &'static str, roles: &'static [Role], app_only: bool) -> Method {
    Method {
        name,
        roles,
        app_only,
    }
}

/// The fixed method list (auth.py:28-50 `METHODS`, auth.py:61-69 `APP_ONLY`), sorted by name. New RPC methods
/// declare their roles here, never per connection. The native host has none: after its hello the connection
/// is the extension's message channel. Checked against `engine/fixtures/rpc/table.json`, which Python wrote.
pub const METHODS: &[Method] = &[
    m("about.get", &[Role::Cli], false),
    m("about.guide", &[Role::Cli], true),
    // PHN-828, APP-1138: FERB.app's own sign-in to the owner's FERB account, kept in the Keychain through FERB's signed
    // helper; the account a phone must share to pair. Not Python's: ACCOUNT_ADDITIONS.
    m("account.clear", &[Role::Cli], true),
    m("account.get", &[Role::Cli], true),
    m("account.save", &[Role::Cli], true),
    m("attention.ack", &[Role::Cli], false),
    m("automation.request", &[Role::Cli], false),
    m("browser.show_handoff", &[Role::Cli], false),
    m("browsers.installed", &[Role::Cli], false),
    m("browsers.list", &[Role::Cli], false),
    m("browsers.pair", &[Role::Cli], false),
    m("browsers.unpair", &[Role::Cli], false),
    m("chat.answer", &[Role::Cli], true),
    m("chat.approve", &[Role::Cli], true),
    m("chat.attach", &[Role::Cli], false),
    m("chat.correct", &[Role::Cli], true),
    // stage 2 (ENG-10): the chat as entries, read like from a server; FERB.app's own for now, the phone role's in
    // stage 3 (reads Private, `mark` Plain). Not Python's: STAGE2_ADDITIONS names them.
    m("chat.entries.changes", &[Role::Cli], true),
    m("chat.entries.days", &[Role::Cli], true),
    m("chat.entries.get", &[Role::Cli], true),
    m("chat.entries.mark", &[Role::Cli], true),
    m("chat.entries.page", &[Role::Cli], true),
    m("chat.entries.watch", &[Role::Cli], true),
    m("chat.history.days", &[Role::Cli], true),
    m("chat.history.import", &[Role::Cli], true),
    m("chat.history.page", &[Role::Cli], true),
    m("chat.history.put", &[Role::Cli], true),
    m("chat.send", &[Role::Cli], true),
    m("chat.status", &[Role::Cli], false),
    // ORCH-699: the temporary chat, held in the daemon's memory: read, and the owner's icon (TEMPORARY_ADDITIONS,
    // FERB.app's own; a phone by class)
    m("chat.temporary.get", &[Role::Cli], true),
    m("chat.temporary.set", &[Role::Cli], true),
    m("clipboard.request", &[Role::Cli], false),
    m("conversations.change", &[Role::Cli], false),
    m("conversations.list", &[Role::Cli], false),
    // STORE-423, STORE-424 (owner STORE-422): FERB.app's stored-data viewer and the owner's delete of a turn, FERB.app's
    // own (app_only): never a model's tools (mcp_shim), the extension or a command line. Not Python's: DATA_ADDITIONS.
    m("data.cell", &[Role::Cli], true),
    m("data.context", &[Role::Cli], true),
    m("data.delete", &[Role::Cli], true),
    m("data.row", &[Role::Cli], true),
    m("data.rows", &[Role::Cli], true),
    m("data.search", &[Role::Cli], true),
    m("data.tables", &[Role::Cli], true),
    // T-1130 (APP-925): FERB.app reports the Location permission, the city and its exact fixes. Not Python's:
    // DEVICE_ADDITIONS.
    m("device.report", &[Role::Cli], true),
    m("doctor.check", &[Role::Cli], false),
    m("identity.check", &[Role::Cli], false),
    m("identity.choose", &[Role::Cli], true),
    m("identity.find", &[Role::Cli], true),
    m("identity.pair", &[Role::Cli], true),
    // T-1274 (APP-980..APP-985): insomnia mode: its state, start and end, the owner's Lock my Mac and It's fine, the
    // camera for Show me, and FERB.app's reports from its cover (INSOMNIA_ADDITIONS, FERB.app's own; a phone by class)
    m("insomnia.camera", &[Role::Cli], true),
    m("insomnia.camera_frame", &[Role::Cli], true),
    m("insomnia.dismiss", &[Role::Cli], true),
    m("insomnia.end", &[Role::Cli], true),
    m("insomnia.lock", &[Role::Cli], true),
    m("insomnia.report", &[Role::Cli], true),
    m("insomnia.start", &[Role::Cli], true),
    m("insomnia.status", &[Role::Cli], true),
    m("intro.preview", &[Role::Cli], true),
    m("intro.suggest", &[Role::Cli], true),
    m("jev.enable", &[Role::Cli], false),
    m("jev.reload", &[Role::Cli], false),
    m("jev.stats", &[Role::Cli], false),
    m("keys.delete", &[Role::Cli], true),
    m("keys.store", &[Role::Cli], true),
    // stage 3 (T-1202): the phone link helper's own calls (role link): a pairing the owner says Yes to on the Mac, the
    // device list and its changes. Not Python's: LINK_ADDITIONS.
    m("link.answer", &[Role::Link], false),
    m("link.devices", &[Role::Link], false),
    m("link.events", &[Role::Link], false),
    m("link.pair_cancel", &[Role::Link], false),
    m("link.pair_request", &[Role::Link], false),
    m("link.pair_wait", &[Role::Link], false),
    m("logs.subscribe", &[Role::Cli], false),
    m("memory.add", &[Role::Cli], false),
    m("memory.edit", &[Role::Cli], false),
    m("memory.forget", &[Role::Cli], false),
    m("memory.import.cancel", &[Role::Cli], false),
    m("memory.import.estimate", &[Role::Cli], false),
    m("memory.import.lines", &[Role::Cli], false),
    m("memory.import.offer", &[Role::Cli], false),
    m("memory.import.scope", &[Role::Cli], false),
    m("memory.import.start", &[Role::Cli], false),
    m("memory.import.status", &[Role::Cli], false),
    m("memory.import.switch", &[Role::Cli], false),
    m("memory.learning", &[Role::Cli], false),
    m("memory.list", &[Role::Cli], false),
    m("memory.profile", &[Role::Cli], false),
    m("memory.repeats", &[Role::Cli], false),
    m("memory.restore", &[Role::Cli], false),
    m("memory.retract", &[Role::Cli], false),
    m("memory.review", &[Role::Cli], false),
    m("memory.review.answer", &[Role::Cli], false),
    m("memory.show", &[Role::Cli], false),
    m("notices.list", &[Role::Cli], false),
    m("notifications.categories", &[Role::Cli], false),
    m("notifications.subscribe", &[Role::Cli], false),
    // stage 3 (T-1202): FERB.app's Settings → Phone (PHONE_ADDITIONS, FERB.app's own), and a phone session's own calls
    // (PHONE_SESSION: no local role; the phone gate answers them with the proof each carries)
    m("phone.audit", &[Role::Cli], true),
    // PHN-828: a paired phone binds itself to its FERB account (a phone paired before accounts, as it signs in);
    // PHONE_ACCOUNT: no local role
    m("phone.bind_account", &[], false),
    m("phone.challenge", &[], false),
    // T-1214: the phone's own record (time, zone, city, battery) and the Mac's files it may find (PHONE_OWN)
    m("phone.device_report", &[], false),
    // stage 3 S4 (T-1204): FERB.app's control of the phone link, relayed by the daemon to its helper
    // (PHONE_LINK_ADDITIONS, FERB.app's own)
    m("phone.enable", &[Role::Cli], true),
    m("phone.files", &[], false),
    // PHN-646: a phone's "Forget this Mac" removes that phone itself (its own session, its device key proved)
    m("phone.forget_self", &[], false),
    m("phone.keys", &[], false),
    m("phone.link_status", &[Role::Cli], true),
    m("phone.list", &[Role::Cli], true),
    m("phone.lock", &[], false),
    m("phone.pair_cancel", &[Role::Cli], true),
    m("phone.pair_decide", &[Role::Cli], true),
    m("phone.pair_start", &[Role::Cli], true),
    m("phone.present", &[], false),
    m("phone.put_begin", &[], false),
    m("phone.put_chunk", &[], false),
    m("phone.put_end", &[], false),
    m("phone.put_group", &[], false),
    m("phone.read", &[], false),
    m("phone.recent", &[], false),
    m("phone.relay_always", &[Role::Cli], true),
    m("phone.relay_set", &[Role::Cli], true),
    m("phone.relay_test", &[Role::Cli], true),
    m("phone.remove", &[Role::Cli], true),
    m("phone.rename", &[Role::Cli], true),
    m("phone.search", &[], false),
    m("phone.session_end", &[], false),
    m("phone.tree", &[], false),
    m("phone.unlock", &[], false),
    // PHN-649: the unlock the phone's lock key signed in the opening prompt before the link was up
    m("phone.unlock_statement", &[], false),
    // PHN-581: where a phone's files land, chosen in FERB.app's Settings → Phone (PHONE_ADDITIONS, FERB.app's own)
    m("phone.upload_folder", &[Role::Cli], true),
    m("ping", &[Role::Cli, Role::McpShim], false),
    // HAND-625: the owner's project folders in Settings, FERB.app's own. Not Python's: PROJECT_ADDITIONS.
    m("projects.list", &[Role::Cli], true),
    m("projects.remove", &[Role::Cli], true),
    m("ps.attach", &[Role::Cli], false),
    m("ps.kill", &[Role::Cli], false),
    m("ps.list", &[Role::Cli], false),
    m("ps.send", &[Role::Cli], false),
    m("questions.picture", &[Role::Cli], true),
    m("recovery.list", &[Role::Cli], false),
    m("recovery.restore", &[Role::Cli], false),
    m("reminders.cancel", &[Role::Cli], false),
    m("reminders.list", &[Role::Cli], false),
    m("reports.build", &[Role::Cli], false),
    m("reports.diagnostic", &[Role::Cli], false),
    m("reports.send", &[Role::Cli], false),
    m("results.ack", &[Role::Cli], false),
    m("runtimes.check", &[Role::Cli], false),
    m("runtimes.check_calls", &[Role::Cli], true),
    m("runtimes.model", &[Role::Cli], false),
    m("runtimes.models", &[Role::Cli], false),
    m("runtimes.sign_in", &[Role::Cli], false),
    m("runtimes.status", &[Role::Cli], false),
    m("runtimes.switch_task", &[Role::Cli], false),
    m("runtimes.use", &[Role::Cli], false),
    m("schedules.change", &[Role::Cli], false),
    m("schedules.list", &[Role::Cli], false),
    // GATE-615: the phone's "until I'm back at my Mac": its state and its end (SCREEN_USE_ADDITIONS, FERB.app's own;
    // a phone reaches them by their class)
    m("screen_use.end", &[Role::Cli], true),
    m("screen_use.status", &[Role::Cli], true),
    m("secrets.delete", &[Role::Cli], true),
    // GATE-602: the owner's "don't mask this" for a saved identifier, FERB.app's own. Not Python's:
    // IDENTIFIER_ADDITIONS names them.
    m("secrets.mask", &[Role::Cli], true),
    m("secrets.saved", &[Role::Cli], true),
    m("secrets.unmask", &[Role::Cli], true),
    m("seen.image", &[Role::Cli], true),
    m("seen.list", &[Role::Cli], true),
    m("sense.pause", &[Role::Cli], false),
    m("sense.resume", &[Role::Cli], false),
    m("sense.status", &[Role::Cli], false),
    m("settings.get", &[Role::Cli], false),
    m("settings.set", &[Role::Cli], false),
    m("setup.browser_open", &[Role::Cli], false),
    m("setup.browser_pair", &[Role::Cli], false),
    m("setup.browsers", &[Role::Cli], false),
    m("setup.browsers_pair_all", &[Role::Cli], false),
    m("setup.browsers_pairing", &[Role::Cli], false),
    m("setup.finish", &[Role::Cli], true),
    m("setup.permission_request", &[Role::Cli], false),
    m("setup.permissions", &[Role::Cli], false),
    m("setup.persona", &[Role::Cli], false),
    // DMN-598: FERB.app's Uninstall has the daemon remove FERB's browser files. Not Python's: UNINSTALL_ADDITIONS.
    m("setup.uninstall_browser_files", &[Role::Cli], true),
    m("stop.engage", &[Role::Cli], false),
    m("stop.lift", &[Role::Cli], false),
    m("task.stop", &[Role::Cli], false),
    m("tasks.list", &[Role::Cli], false),
    m("tasks.preview", &[Role::Cli], true),
    m("timeline.get", &[Role::Cli], false),
    m("timers.repeat", &[Role::Cli], false),
    m("timers.states", &[Role::Cli], false),
    m("tools.call", &[Role::McpShim], false),
    m("tools.list", &[Role::McpShim], false),
    m("trace.get", &[Role::Cli], false),
    m("ui.click_through", &[Role::Cli], false),
    m("update.prepare", &[Role::Cli], false),
    m("verdict.set", &[Role::Cli], false),
];

/// The methods stage 2 added to Python's table (ENG-10: the chat as entries, read like from a server), all FERB.app's
/// own (`app_only`) until stage 3 gives them to the phone role. The method-table golden test is Python's table plus
/// exactly these.
pub const STAGE2_ADDITIONS: [&str; 6] = [
    "chat.entries.changes",
    "chat.entries.days",
    "chat.entries.get",
    "chat.entries.mark",
    "chat.entries.page",
    "chat.entries.watch",
];

/// ORCH-699: the temporary chat's two calls, FERB.app's own (`app_only`); a phone reaches them by class. Not Python's.
pub const TEMPORARY_ADDITIONS: [&str; 2] = ["chat.temporary.get", "chat.temporary.set"];

/// PHN-828, APP-1138: FERB.app's sign-in to the owner's FERB account (the account, its tokens), kept by the daemon in
/// the Keychain through FERB's signed helper; a phone pairs only with a Mac signed in to the same account. FERB.app's
/// own (`app_only`): never a tool's, the extension's, a command line's or a phone's.
pub const ACCOUNT_ADDITIONS: [&str; 3] = ["account.clear", "account.get", "account.save"];

/// The methods the owner's identifier choices added to Python's table (GATE-601, GATE-602): FERB.app's own
/// (`app_only`), never a tool's or the phone's.
pub const IDENTIFIER_ADDITIONS: [&str; 2] = ["secrets.mask", "secrets.unmask"];

/// GATE-612: the owner's own words and answers (a chat message, a question's Yes or No, its typed answer, a safety
/// check's box) come only from FERB.app (`app_only`), never from any other program running as the owner. Python had
/// them for the cli role without the app check (the one named difference from its recorded table).
pub const OWNER_APP_ONLY: [&str; 4] = ["chat.answer", "chat.approve", "chat.correct", "chat.send"];

/// The methods of FERB.app's stored-data viewer and the owner's delete (STORE-423, STORE-424): FERB.app's own
/// (`app_only`), never a tool's, the phone's or a command line's.
pub const DATA_ADDITIONS: [&str; 7] = [
    "data.cell",
    "data.context",
    "data.delete",
    "data.row",
    "data.rows",
    "data.search",
    "data.tables",
];

/// DMN-598: FERB.app's Uninstall asks the daemon, which holds Full Disk Access, to remove FERB's own files from the
/// browsers' folders before the service is unregistered: FERB.app's own (`app_only`), never a tool's or a command
/// line's.
pub const UNINSTALL_ADDITIONS: [&str; 1] = ["setup.uninstall_browser_files"];

/// The owner's project folders in Settings (HAND-625): FERB.app's own (`app_only`), never a tool's or the phone's.
pub const PROJECT_ADDITIONS: [&str; 2] = ["projects.list", "projects.remove"];

/// FERB.app's reports of this Mac's Location permission, city, Wi-Fi name and exact fixes (APP-925): FERB.app's own
/// (`app_only`): a model's tools, the extension or another program never set where FERB thinks the Mac is.
pub const DEVICE_ADDITIONS: [&str; 1] = ["device.report"];

/// Stage 3 (T-1202): FERB.app's Settings → Phone: the paired phones, their removal and renaming, the owner's Yes to a
/// pairing, the audit. FERB.app's own (`app_only`); from a phone only `phone.list` (private) and `phone.remove`
/// (sensitive: a second phone removes a lost one with its fingerprint, PHN-419), by their classes.
pub const PHONE_ADDITIONS: [&str; 6] = [
    "phone.audit",
    "phone.list",
    "phone.pair_decide",
    "phone.remove",
    "phone.rename",
    "phone.upload_folder",
];

/// GATE-615: the phone's "until I'm back at my Mac" (a Yes to screen use that lasts until the owner is back): what is in
/// force and its end. FERB.app's own (`app_only`); from a phone by their classes (plain, allowed while locked: ending it
/// only takes something away).
pub const SCREEN_USE_ADDITIONS: [&str; 2] = ["screen_use.end", "screen_use.status"];

/// T-1274 (APP-980..APP-985): insomnia mode. FERB.app's own (`app_only`); from a phone by their classes: the state
/// (private), the start (sensitive: signed with the fingerprint, as a setting), the end, Lock my Mac and It's fine
/// (plain, allowed while locked: each only takes something away), the camera (private, never on the shared relay);
/// never the cover's reports or the camera's frames, which are FERB.app's alone.
pub const INSOMNIA_ADDITIONS: [&str; 8] = [
    "insomnia.camera",
    "insomnia.camera_frame",
    "insomnia.dismiss",
    "insomnia.end",
    "insomnia.lock",
    "insomnia.report",
    "insomnia.start",
    "insomnia.status",
];

/// MEM-442, MEM-444: the owner's word on an imported memory: where it applies (`memory.import.scope`) and his
/// answer to its offer (`memory.import.offer`), sensitive; and what the Memory page shows of them
/// (`memory.import.lines`, private). MEM-459: the owner's two switches (`memory.import.switch`), sensitive. MEM-460:
/// the owner's review of what an import brought in (`memory.review`) and his answers to it item by item
/// (`memory.review.answer`), both private: an answer to FERB's own question, as `chat.answer` is.
pub const MEMORY_IMPORT_ADDITIONS: [&str; 6] = [
    "memory.import.lines",
    "memory.import.offer",
    "memory.import.scope",
    "memory.import.switch",
    "memory.review",
    "memory.review.answer",
];

/// Stage 3 (T-1202): a phone session's own calls: a challenge, the unlock and lock, the device key's proof, keys
/// registered late, the session's end. No local role may call them; on a phone session the phone gate answers each,
/// checking the proof it carries itself (CONTRACT §12).
pub const PHONE_SESSION: [&str; 8] = [
    "phone.challenge",
    "phone.forget_self",
    "phone.keys",
    "phone.lock",
    "phone.present",
    "phone.session_end",
    "phone.unlock",
    "phone.unlock_statement",
];

/// PHN-828: a paired phone's own statement of its FERB account (the site's signed assertion for its own endpoint),
/// which binds a phone paired before accounts to the Mac's account, or removes it when the accounts differ. No local
/// role may call it; the phone gate admits it by its row (plain, allowed while locked: the site's signature proves it).
pub const PHONE_ACCOUNT: [&str; 1] = ["phone.bind_account"];

/// Stage 3 S3b (T-1203): a phone session's file calls: browse, fetch (a stream) and upload. No local role may call
/// them; on a phone session the phone gate leaves each to the daemon's file area, which checks the unlock window, a
/// key place's fresh proof and the path itself (`ferb-daemon` `phone::files`), and audits it with its path.
pub const PHONE_FILES: [&str; 7] = [
    "phone.files",
    "phone.put_begin",
    "phone.put_chunk",
    "phone.put_end",
    "phone.put_group",
    "phone.read",
    "phone.tree",
];

/// Stage 3 (T-1214, T-1211): a phone session's calls the daemon's phone area answers beside the files: the phone's own
/// device record (its time zone, city, battery; and the one precise fix the owner said Yes to), the Mac's recent files
/// (what FERB made, what this phone fetched or sent) and a search of file names. No local role may call them; the
/// phone gate admits them by their rows, and the phone area judges every path as `PHONE_FILES` does.
pub const PHONE_OWN: [&str; 3] = ["phone.device_report", "phone.recent", "phone.search"];

/// Stage 3 S4 (T-1204): FERB.app's control of the phone link (Settings → Phone): the pairing code, the owner's relay,
/// Always use my relay, how each phone is connected, the phone feature on or off. The daemon answers each, asking its
/// helper over `link.events` where only the helper knows (kb PHN-441: the app reaches the helper only through the
/// daemon). FERB.app's own (`app_only`); never a phone's, a tool's or the helper's.
pub const PHONE_LINK_ADDITIONS: [&str; 7] = [
    "phone.enable",
    "phone.link_status",
    "phone.pair_cancel",
    "phone.pair_start",
    "phone.relay_always",
    "phone.relay_set",
    "phone.relay_test",
];

/// Stage 3 (T-1202): the phone link helper's own calls (`Role::Link`): never FERB.app's, a tool's or a phone's.
/// S4 adds `link.answer` (the helper's answer to one of the daemon's asks).
pub const LINK_ADDITIONS: [&str; 6] = [
    "link.answer",
    "link.devices",
    "link.events",
    "link.pair_cancel",
    "link.pair_request",
    "link.pair_wait",
];

/// The method's row, when FERB has such a method at all.
pub fn method(name: &str) -> Option<&'static Method> {
    METHODS
        .binary_search_by(|m| m.name.cmp(name))
        .ok()
        .map(|at| &METHODS[at])
}

/// auth.py:72-73. The phone role's methods are those its class does not refuse (`crate::classes`).
pub fn allowed(role: Role, name: &str) -> bool {
    match role {
        Role::Phone => {
            method(name).is_some() && crate::classes::class(name) != crate::classes::Class::Never
        }
        _ => method(name).is_some_and(|m| m.roles.contains(&role)),
    }
}

/// Whether only FERB.app's own process may call it (`method in APP_ONLY`; DMN-23, DMN-402).
pub fn app_only(name: &str) -> bool {
    method(name).is_some_and(|m| m.app_only)
}

/// A connection or run token failed authentication (`core.errors.AuthError`); its words go to the caller with
/// `UNAUTHORIZED`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct AuthError(pub String);

fn refused<T>(why: &str) -> Result<T, AuthError> {
    Err(AuthError(why.to_owned()))
}

/// auth.py:76-78.
pub fn check_cli_peer(peer_uid: u32, owner_uid: u32) -> Result<(), AuthError> {
    if peer_uid != owner_uid {
        return refused("cli role requires the owner's uid");
    }
    Ok(())
}

/// auth.py:155-160.
pub fn check_native_host(
    peer_uid: u32,
    owner_uid: u32,
    origin: &str,
    extension_ids: &HashSet<String>,
) -> Result<(), AuthError> {
    if peer_uid != owner_uid {
        return refused("native_host role requires the owner's uid");
    }
    const SCHEME: &str = "chrome-extension://";
    match origin.strip_prefix(SCHEME) {
        Some(rest) if extension_ids.contains(rest.trim_end_matches('/')) => Ok(()),
        _ => refused("unknown extension origin"),
    }
}

/// Whose browser profile and apps a worker acts with (`core.types.Identity`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Identity {
    /// FERB's own profile and credentials.
    Ferb,
    /// The owner's logged-in browsers and apps; no credentials ever.
    Owner,
}

/// The monotonic clock run tokens expire by (`core.interfaces.Clock.monotonic`), in seconds.
pub trait Clock: Send + Sync {
    fn monotonic(&self) -> f64;
}

/// The process's own monotonic clock.
#[derive(Debug)]
pub struct SystemClock(std::time::Instant);

impl Default for SystemClock {
    fn default() -> Self {
        SystemClock(std::time::Instant::now())
    }
}

impl Clock for SystemClock {
    fn monotonic(&self) -> f64 {
        self.0.elapsed().as_secs_f64()
    }
}

/// What a run token allows (auth.py:81-92).
#[derive(Clone, Debug, PartialEq)]
pub struct RunGrant {
    pub task_id: String,
    pub worker_id: String,
    pub scopes: BTreeSet<String>,
    pub identity: Identity,
    /// `Clock::monotonic()` deadline.
    pub expires_at: f64,
    /// The final checker (V3, W4): the gateway allows observation tools only.
    pub observe_only: bool,
    /// The planner's lookups (ORCH-516): when not empty, the gateway offers and runs only these tools, at most
    /// `max_calls` calls for this grant (0: no limit).
    pub tools: BTreeSet<String>,
    pub max_calls: u64,
}

/// What `RunTokens::issue` takes besides the task and worker.
#[derive(Clone, Debug, PartialEq)]
pub struct GrantSpec {
    pub scopes: BTreeSet<String>,
    pub identity: Identity,
    pub ttl_s: f64,
    pub observe_only: bool,
    pub tools: BTreeSet<String>,
    pub max_calls: u64,
}

impl GrantSpec {
    /// A grant with no scopes, tool list or call limit, as `issue(task, worker, set(), identity, ttl_s)`.
    pub fn new(identity: Identity, ttl_s: f64) -> Self {
        GrantSpec {
            scopes: BTreeSet::new(),
            identity,
            ttl_s,
            observe_only: false,
            tools: BTreeSet::new(),
            max_calls: 0,
        }
    }
}

/// Issue / validate / expire / revoke (auth.py:95-152). Tokens are returned once; only SHA-256 hashes are kept.
pub struct RunTokens {
    clock: Arc<dyn Clock>,
    grants: Mutex<HashMap<String, Arc<RunGrant>>>, // sha256(token) in hex -> grant
    source: Option<Arc<TokenSource>>,
}

/// Where a new token's text comes from instead of the system's random bytes (`secrets.token_urlsafe(32)`): a test's
/// recorded tokens. A daemon never sets one.
pub type TokenSource = dyn Fn() -> String + Send + Sync;

impl RunTokens {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        RunTokens {
            clock,
            grants: Mutex::new(HashMap::new()),
            source: None,
        }
    }

    /// As [`RunTokens::new`], with each token's text from `source` (Python's `secrets.token_urlsafe` patched in a
    /// recorder): the tests that replay recorded token values.
    pub fn with_source(clock: Arc<dyn Clock>, source: Arc<TokenSource>) -> Self {
        RunTokens {
            clock,
            grants: Mutex::new(HashMap::new()),
            source: Some(source),
        }
    }

    fn digest(token: &str) -> String {
        hex::encode(Sha256::digest(token.as_bytes()))
    }

    fn grants(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<RunGrant>>> {
        self.grants
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A new token for `worker_id` of `task_id`: 32 random bytes as URL-safe base64 without padding, like
    /// `secrets.token_urlsafe(32)` (auth.py:106-112).
    pub fn issue(&self, task_id: &str, worker_id: &str, spec: GrantSpec) -> String {
        let token = match &self.source {
            Some(source) => source(),
            None => {
                let mut random = [0u8; 32];
                // No token is better than a guessable one: without the system's random source the daemon must not
                // go on.
                getrandom::fill(&mut random).expect("the system's random source is unavailable");
                base64_url(&random)
            }
        };
        let grant = RunGrant {
            task_id: task_id.to_owned(),
            worker_id: worker_id.to_owned(),
            scopes: spec.scopes,
            identity: spec.identity,
            expires_at: self.clock.monotonic() + spec.ttl_s,
            observe_only: spec.observe_only,
            tools: spec.tools,
            max_calls: spec.max_calls,
        };
        self.grants().insert(Self::digest(&token), Arc::new(grant));
        token
    }

    /// auth.py:114-123. An expired token is forgotten when it is seen.
    pub fn validate(&self, token: &str) -> Result<Arc<RunGrant>, AuthError> {
        let digest = Self::digest(token);
        let mut grants = self.grants();
        // every stored digest is compared, each in constant time (don't leak which prefix matched)
        let mut found = None;
        for (stored, grant) in grants.iter() {
            if same(stored.as_bytes(), digest.as_bytes()) {
                found = Some(Arc::clone(grant));
            }
        }
        let Some(grant) = found else {
            return refused("unknown or revoked run token");
        };
        if self.clock.monotonic() >= grant.expires_at {
            grants.remove(&digest);
            return refused("run token expired");
        }
        Ok(grant)
    }

    /// For an already-authenticated connection: the grant must still be registered (this very grant, not an
    /// equal one) and unexpired, so a revoke or expiry takes effect on open connections at their next call
    /// (auth.py:125-132).
    pub fn recheck(&self, grant: &Arc<RunGrant>) -> Result<Arc<RunGrant>, AuthError> {
        if !self.grants().values().any(|held| Arc::ptr_eq(held, grant)) {
            return refused("run token revoked");
        }
        if self.clock.monotonic() >= grant.expires_at {
            return refused("run token expired");
        }
        Ok(Arc::clone(grant))
    }

    fn revoke(&self, doomed: impl Fn(&RunGrant) -> bool) -> usize {
        let mut grants = self.grants();
        let before = grants.len();
        grants.retain(|_, grant| !doomed(grant));
        before - grants.len()
    }

    /// auth.py:134-138.
    pub fn revoke_worker(&self, worker_id: &str) -> usize {
        self.revoke(|grant| grant.worker_id == worker_id)
    }

    /// A task tree ended: every token of its tasks goes, so a token never outlives its task (auth.py:140-147).
    pub fn revoke_tree(&self, tree_id: &str) -> usize {
        let root = tree_id.split('.').next().unwrap_or(tree_id);
        let children = format!("{root}.");
        self.revoke(|grant| grant.task_id == root || grant.task_id.starts_with(&children))
    }

    /// auth.py:149-152.
    pub fn revoke_all(&self) -> usize {
        self.revoke(|_| true)
    }

    /// How many tokens are live (for tests and `ferb stats`-like facts; never the tokens themselves).
    pub fn len(&self) -> usize {
        self.grants().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl std::fmt::Debug for RunTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunTokens")
            .field("live", &self.len())
            .finish()
    }
}

/// `hmac.compare_digest`: the time taken depends on the length only.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

fn base64_url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |n, b| (n << 8) | u32::from(*b)) << (8 * (3 - chunk.len()));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeClock(Mutex<f64>);

    impl Clock for FakeClock {
        fn monotonic(&self) -> f64 {
            *self.0.lock().unwrap()
        }
    }

    fn tokens() -> (Arc<FakeClock>, RunTokens) {
        let clock = Arc::new(FakeClock(Mutex::new(1000.0)));
        (Arc::clone(&clock), RunTokens::new(clock))
    }

    fn owner(ttl_s: f64) -> GrantSpec {
        GrantSpec::new(Identity::Owner, ttl_s)
    }

    // tests/test_auth.py:11-15
    #[test]
    fn each_role_only_gets_its_own_methods() {
        assert!(allowed(Role::Cli, "trace.get"));
        assert!(!allowed(Role::McpShim, "trace.get"));
        assert!(!allowed(Role::McpShim, "logs.subscribe"));
        assert!(allowed(Role::McpShim, "ping") && allowed(Role::Cli, "ping"));
        assert!(METHODS.iter().all(|m| !m.roles.contains(&Role::NativeHost)));
        assert!(!allowed(Role::Cli, "auth.hello") && !allowed(Role::Cli, ""));
    }

    /// GATE-604: `chat.send` seals the IDs in the text it is given, so only the owner's own connection may call it: a
    /// model's tools reach the daemon as `mcp_shim` (tools.call only), the extension's relay has no methods, and the
    /// shell's sandbox refuses the daemon's socket (ferb-shell `sandbox.rs`, the profiles fixture).
    // decision: GATE-604
    #[test]
    fn only_the_owners_connection_may_send_a_chat_message() {
        assert!(allowed(Role::Cli, "chat.send"));
        assert!(!allowed(Role::McpShim, "chat.send"));
        assert!(!allowed(Role::NativeHost, "chat.send"));
    }

    /// GATE-612: a chat message, a question's answer and a safety check's box are FERB.app's alone: any other program
    /// running as the owner (a planted git hook, a launch agent) is refused them, as it is the stored data.
    // decision: GATE-612
    #[test]
    fn the_owners_words_and_answers_are_ferb_app_s_alone() {
        for name in OWNER_APP_ONLY {
            assert!(allowed(Role::Cli, name), "{name}");
            assert!(app_only(name), "{name}");
            assert!(
                !allowed(Role::McpShim, name) && !allowed(Role::NativeHost, name),
                "{name}"
            );
        }
    }

    #[test]
    fn the_table_is_sorted_so_lookups_find_every_row() {
        assert!(METHODS.windows(2).all(|pair| pair[0].name < pair[1].name));
        assert!(METHODS.iter().all(|m| method(m.name) == Some(m)));
    }

    // decision: DMN-33
    #[test]
    fn restore_stays_on_the_cli_role_and_is_not_app_only() {
        for name in ["recovery.list", "recovery.restore"] {
            assert!(allowed(Role::Cli, name) && !app_only(name));
        }
    }

    /// STORE-423, STORE-424: the viewer's reads and the owner's delete are FERB.app's alone: a model's tools reach the
    /// daemon as mcp_shim, the extension's relay has no methods, and a cli connection that isn't FERB.app is refused.
    // decision: STORE-423
    // decision: STORE-424
    #[test]
    fn the_data_viewer_and_delete_are_ferb_app_s_own() {
        for name in DATA_ADDITIONS {
            assert!(app_only(name), "{name}");
            assert!(allowed(Role::Cli, name), "{name}");
            assert!(!allowed(Role::McpShim, name), "{name}");
            assert!(!allowed(Role::NativeHost, name), "{name}");
        }
    }

    /// The owner's project list and its Remove are FERB.app's alone: no model's tools, extension or command line.
    // decision: HAND-625
    #[test]
    fn the_project_list_is_ferb_app_s_own() {
        for name in PROJECT_ADDITIONS {
            assert!(app_only(name), "{name}");
            assert!(allowed(Role::Cli, name), "{name}");
            assert!(!allowed(Role::McpShim, name), "{name}");
            assert!(!allowed(Role::NativeHost, name), "{name}");
        }
    }

    /// The owner's FERB account and its tokens are FERB.app's alone: no model's tools, extension or command line.
    // decision: APP-1138
    #[test]
    fn the_account_is_ferb_app_s_own() {
        for name in ACCOUNT_ADDITIONS {
            assert!(app_only(name), "{name}");
            assert!(allowed(Role::Cli, name), "{name}");
            assert!(!allowed(Role::McpShim, name), "{name}");
            assert!(!allowed(Role::NativeHost, name), "{name}");
            assert!(!allowed(Role::Link, name), "{name}");
            assert!(!allowed(Role::Phone, name), "{name}");
        }
    }

    /// Where FERB thinks the Mac is comes only from FERB.app: no model's tools, extension or command line.
    // decision: APP-925
    #[test]
    fn the_device_report_is_ferb_app_s_own() {
        for name in DEVICE_ADDITIONS {
            assert!(app_only(name), "{name}");
            assert!(allowed(Role::Cli, name), "{name}");
            assert!(!allowed(Role::McpShim, name), "{name}");
            assert!(!allowed(Role::NativeHost, name), "{name}");
        }
    }

    // decision: DMN-23
    #[test]
    fn app_only_methods_are_cli_methods() {
        for name in [
            "secrets.saved",
            "secrets.delete",
            "identity.find",
            "identity.choose",
            "identity.pair",
            "about.guide",
            "setup.finish",
            "seen.list",
            "seen.image",
            "tasks.preview",
        ] {
            assert!(app_only(name), "{name}");
        }
        assert!(METHODS
            .iter()
            .filter(|m| m.app_only)
            .all(|m| m.roles == [Role::Cli]));
        assert!(!app_only("ping") && !app_only("no.such.method"));
    }

    /// Stage 3: the link helper's calls are its own, a phone session's own calls belong to no local role, and Settings →
    /// Phone is FERB.app's alone.
    // decision: PHN-441
    // decision: PHN-106
    #[test]
    fn the_phone_link_s_methods_belong_to_their_own_callers() {
        for name in LINK_ADDITIONS {
            assert!(allowed(Role::Link, name), "{name}");
            assert!(!app_only(name), "{name}");
            for role in Role::ALL {
                assert!(!allowed(role, name), "{name}");
            }
        }
        for name in PHONE_SESSION.iter().chain(&PHONE_OWN).chain(&PHONE_ACCOUNT) {
            assert!(method(name).is_some_and(|m| m.roles.is_empty()), "{name}");
            for role in [Role::Cli, Role::McpShim, Role::NativeHost, Role::Link] {
                assert!(!allowed(role, name), "{name}");
            }
        }
        for name in PHONE_ADDITIONS.iter().chain(&PHONE_LINK_ADDITIONS) {
            assert!(app_only(name) && allowed(Role::Cli, name), "{name}");
            assert!(!allowed(Role::Link, name), "{name}");
            assert!(
                !allowed(Role::McpShim, name) && !allowed(Role::NativeHost, name),
                "{name}"
            );
        }
        // the helper reaches nothing of FERB.app's
        for m in METHODS {
            assert_eq!(
                allowed(Role::Link, m.name),
                LINK_ADDITIONS.contains(&m.name),
                "{}",
                m.name
            );
        }
        assert!(!Role::ALL.contains(&Role::Link));
        assert_eq!(Role::from_wire("link"), None);
    }

    // tests/test_auth.py:18-21
    #[test]
    fn cli_requires_the_owner_uid() {
        assert!(check_cli_peer(501, 501).is_ok());
        assert_eq!(
            check_cli_peer(502, 501),
            refused("cli role requires the owner's uid")
        );
    }

    // tests/test_auth.py:24-31
    #[test]
    fn native_host_needs_the_owner_uid_and_a_known_extension_origin() {
        let id = "jaclnphefmmgmpofnabffahdkbjjompm";
        let ids: HashSet<String> = [id.to_owned()].into();
        assert!(check_native_host(501, 501, &format!("chrome-extension://{id}/"), &ids).is_ok());
        assert!(check_native_host(501, 501, &format!("chrome-extension://{id}"), &ids).is_ok());
        assert!(check_native_host(501, 501, &format!("chrome-extension://{id}///"), &ids).is_ok());
        for (uid, origin) in [
            (502, format!("chrome-extension://{id}/")),
            (501, format!("chrome-extension://{}/", "a".repeat(32))),
            (501, format!("https://{id}/")),
            (501, format!("chrome-extension://{id}/x")),
            (501, id.to_owned()),
            (501, String::new()),
        ] {
            assert!(
                check_native_host(uid, 501, &origin, &ids).is_err(),
                "{origin}"
            );
        }
    }

    // tests/test_auth.py:34-50
    #[test]
    fn run_token_lifecycle_expire_and_revoke() {
        let (clock, tokens) = tokens();
        let mut spec = owner(60.0);
        spec.scopes.insert("fs:read".to_owned());
        let t = tokens.issue("task-1", "w-1", spec);
        let g = tokens.validate(&t).unwrap();
        assert_eq!(
            (g.task_id.as_str(), g.worker_id.as_str(), g.identity),
            ("task-1", "w-1", Identity::Owner)
        );
        assert!(g.scopes.contains("fs:read") && g.expires_at == 1060.0);
        *clock.0.lock().unwrap() += 61.0;
        assert_eq!(tokens.validate(&t), refused("run token expired"));
        assert_eq!(tokens.validate(&t), refused("unknown or revoked run token")); // forgotten when seen expired
        let t2 = tokens.issue("task-1", "w-2", owner(60.0));
        let g2 = tokens.validate(&t2).unwrap();
        assert_eq!(tokens.revoke_all(), 1);
        assert!(tokens.validate(&t2).is_err());
        // an open connection loses access at its next call
        assert_eq!(tokens.recheck(&g2), refused("run token revoked"));
    }

    #[test]
    fn a_token_expires_at_its_deadline_and_recheck_wants_the_very_grant() {
        let (clock, tokens) = tokens();
        let t = tokens.issue("t", "w", owner(60.0));
        let g = tokens.validate(&t).unwrap();
        assert!(tokens.recheck(&g).is_ok());
        assert_eq!(
            tokens.recheck(&Arc::new((*g).clone())),
            refused("run token revoked")
        );
        *clock.0.lock().unwrap() += 60.0;
        assert_eq!(tokens.recheck(&g), refused("run token expired"));
    }

    // tests/test_auth.py:53-60
    #[test]
    fn revoke_one_worker_only() {
        let (_, tokens) = tokens();
        let a = tokens.issue("t", "w-a", owner(60.0));
        let b = tokens.issue("t", "w-b", owner(60.0));
        assert_eq!(tokens.revoke_worker("w-a"), 1);
        assert!(tokens.validate(&a).is_err());
        assert!(tokens.validate(&b).is_ok());
    }

    #[test]
    fn a_tree_s_tokens_go_with_the_tree_and_no_other() {
        let (_, tokens) = tokens();
        let root = tokens.issue("abc", "w1", owner(60.0));
        let child = tokens.issue("abc.2.1", "w2", owner(60.0));
        let other = tokens.issue("abcd", "w3", owner(60.0));
        assert_eq!(tokens.revoke_tree("abc.2"), 2); // named by any of its tasks
        assert!(tokens.validate(&root).is_err() && tokens.validate(&child).is_err());
        assert!(tokens.validate(&other).is_ok());
    }

    // tests/test_auth.py:63-68
    #[test]
    fn token_value_is_never_stored() {
        let (_, tokens) = tokens();
        let t = tokens.issue("t", "w", owner(60.0));
        assert_eq!(t.len(), 43); // secrets.token_urlsafe(32)
        assert!(t
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        assert!(!tokens.grants().contains_key(&t));
        let stored = format!("{:?}", tokens.grants());
        assert!(!stored.contains(&t) && !format!("{tokens:?}").contains(&t));
        assert!(tokens.validate(&format!("{t}x")).is_err());
        assert_ne!(t, tokens.issue("t", "w", owner(60.0)));
    }

    #[test]
    fn base64_is_the_url_safe_alphabet_without_padding() {
        assert_eq!(base64_url(b""), "");
        assert_eq!(base64_url(b"f"), "Zg");
        assert_eq!(base64_url(b"fo"), "Zm8");
        assert_eq!(base64_url(b"foo"), "Zm9v");
        assert_eq!(base64_url(&[0xfb, 0xff, 0xfe]), "-__-");
    }
}
