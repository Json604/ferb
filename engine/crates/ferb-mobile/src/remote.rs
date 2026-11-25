//! The phone's calls beyond the chat (kb T-1221): the app's lock and Stop everything, what FERB has waiting, files both
//! ways, the live preview, FERB's settings, the task list, what the Mac says about itself, and keys made after pairing.
//! Every one goes now or not at all (nothing here is queued), through the same transport as the chat, and every
//! refusal comes back with the Mac's code and words (`MobileError`).
//!
//! **Signed calls.** A sensitive call (a setting, a key's registration) and a key place's file (PHN-121) carry the
//! phone's proof: the app's `Signer` signs `{challenge, method, params}` over a fresh `link.challenge` (ferb-link
//! `Phone::call_payload`), after its own prompt. The Mac takes only the fingerprint-only key where the phone registered
//! one (PHN-133), so the app hands the strong signer for these.
//!
//! **The public relay** (PHN-444, PHN-454): files, folders and the live preview cross only a direct path or the owner's
//! relay. Where the link already says its path carries no bulk (`LinkState::bulk_allowed`), they are refused here
//! before anything is asked (`NeedsBetterPath`); the link and the Mac refuse them there too.
//!
//! **Pictures** (PHN-454): a preview frame is the Mac's window as captured, unmasked, for the owner's paired, unlocked
//! phone. It is returned in memory only: never written to the store, a file, a log or a listener's change.

use std::sync::Arc;

use base64::Engine;
use serde::Serialize;
use serde_json::{json, Value};

use crate::api::{
    lock, parse, sorted, Change, ErrorCode, Mobile, MobileError, MobileResult, Shared, Signer,
};
use crate::status::{Insomnia, PhoneStatus, ScreenUse};
use crate::transport::{codes, TransferSink, Transport};

/// What a phone sees of one entry of a folder on the Mac (`phone.files`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileEntry {
    pub name: String,
    /// The whole path on the Mac (the folder's path and the name).
    pub path: String,
    pub dir: bool,
    pub link: bool,
    /// Bytes (0 for a folder).
    pub size: u64,
    /// Seconds since 1970 (UTC).
    pub modified: i64,
    /// A key place (keys, passwords, private data): each access needs a fresh fingerprint for that very path
    /// (PHN-121, PHN-131).
    pub locked: bool,
}

/// One folder's listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Folder {
    /// The folder's real path on the Mac (as the Mac resolved what was asked).
    pub path: String,
    pub parent: Option<String>,
    /// The folder itself is a key place.
    pub locked: bool,
    pub entries: Vec<FileEntry>,
    /// How many entries it has (more than `entries` when `cut`).
    pub total: u64,
    /// More entries remain after this page (true exactly when `next` is given).
    pub cut: bool,
    /// The index of this page's first entry in the folder's whole listing (0 for the first page).
    pub from: u64,
    /// "Load more" (PHN-589): the marker that asks for the next page (`files`' `after`), valid only for this phone
    /// and this folder; none on the last page.
    pub next: Option<String>,
}

/// One file of a folder fetched whole (`phone.tree`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TreeFile {
    /// Its path under the folder.
    pub rel: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Tree {
    pub path: String,
    pub name: String,
    pub files: Vec<TreeFile>,
    pub bytes: u64,
    /// Files left out (key places, FERB's own data): each asks by itself when fetched.
    pub skipped: u64,
    pub cut: bool,
}

/// A file fetched (or the part of it so far, after a pause).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fetched {
    /// The file's name on the Mac.
    pub name: String,
    /// Where it was saved on the phone.
    pub saved_to: String,
    /// Bytes this fetch received (after `offset`).
    pub received: u64,
    /// The file's whole size.
    pub total: u64,
}

/// A file sent to the Mac.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Uploaded {
    /// Its id on the Mac: a message's attachment (`send_with`) and what `resume` takes.
    pub upload_id: String,
    /// Where the Mac put it.
    pub saved: String,
    pub size: u64,
    /// The folder it landed in, said plainly for the owner ("Downloads › FERB from phone", PHN-581); none from a Mac
    /// that doesn't say.
    pub saved_in: Option<String>,
}

/// A folder sent from the phone, begun on the Mac (`link.put_group`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GroupStarted {
    /// The id its files carry as `group`.
    pub group: String,
    /// The folder it lands in, said plainly (PHN-581).
    pub saved_in: Option<String>,
}

/// Where files sent from the phone land on the Mac (`phone.list`'s `upload_folder`, PHN-580): chosen in FERB.app's
/// Settings → Phone, never from a phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct UploadFolder {
    /// Its whole path on the Mac.
    pub path: String,
    /// The path said plainly ("Downloads › FERB from phone").
    pub shown: String,
    /// It is FERB's own default folder (the owner chose none). (`is_default`: Swift keeps `default` for itself.)
    pub is_default: bool,
}

/// What `screen_use.end` did (GATE-615).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScreenUseEnd {
    /// A "until I'm back" was in force and is ended now (false: there was none).
    pub ended: bool,
    /// Who ended it: "mac", or the phone's endpoint id.
    pub by: String,
    /// What is in force after it.
    pub screen_use: ScreenUse,
}

/// What `insomnia.start` did (APP-980): `already` (it was on: nothing changed) and the mode as it stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InsomniaStarted {
    pub already: bool,
    pub insomnia: Insomnia,
}

/// What `insomnia.end` did: `ended` (it was on), `by` (who ended it: "mac" or a phone's endpoint id; none from a Mac
/// that doesn't say), and the mode after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InsomniaEnded {
    pub ended: bool,
    pub by: Option<String>,
    pub insomnia: Insomnia,
}

/// What Lock my Mac did (`insomnia.lock`, APP-983: the owner's button only): `locked`, the Mac's words, and the mode
/// after it when the Mac gave it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MacLocked {
    pub locked: bool,
    pub text: String,
    pub insomnia: Option<Insomnia>,
}

/// One answer of the Mac's camera in insomnia mode (`insomnia.camera` through `link.get what: camera`, APP-983).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CameraFrame {
    /// The frame (JPEG), when the Mac sent a new one; none when the phone already has it (`have`) or there is none.
    /// Memory only: never stored, logged or put in a listener's change.
    #[serde(skip)]
    pub jpeg: Option<Vec<u8>>,
    /// The frame's number (what `have` takes next time).
    pub seq: Option<i64>,
    /// starting, live, off, denied, failed.
    pub state: String,
    /// The Mac's words for the state ("Starting your Mac's camera…").
    pub text: Option<String>,
}

/// One live preview answer (`tasks.preview` through `link.get what: preview`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PreviewFrame {
    /// The frame (JPEG), when the Mac sent a new one; none when the phone already has it (`have`) or there is none.
    #[serde(skip)]
    pub jpeg: Option<Vec<u8>>,
    /// The frame's number (what `have` takes next time).
    pub seq: Option<i64>,
    /// working, waiting, … (the preview's state, APP-404).
    pub state: String,
    /// The short line over the frame.
    pub line: Option<String>,
    /// A fresh capture (else the last frame, kept while the window was busy).
    pub fresh: bool,
    /// The surface was busy: ask again soon.
    pub busy: bool,
    /// Nothing to show now (the task ended or its surface is gone): the last frame goes.
    pub gone: bool,
    /// The words for an empty preview.
    pub empty_text: Option<String>,
}

impl PreviewFrame {
    /// How long to wait before asking again (FERB.app's rule, `PreviewAnswer.nextRequestDelay`): at once after a fresh
    /// frame, 200 ms while the surface is busy, else a second. The link adds its own least time between two pictures.
    pub fn next_after_ms(&self) -> u64 {
        if self.fresh && self.jpeg.is_some() {
            0
        } else if self.busy {
            200
        } else {
            1000
        }
    }
}

/// One of FERB's settings as `settings.get` lists it: its key and its value as JSON text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingValue {
    pub key: String,
    pub value_json: String,
}

/// One root task as the Mac's task list holds it (`tasks.list`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskRow {
    pub task_id: String,
    pub name: String,
    pub goal: String,
    /// running, done, failed, stopped, …
    pub status: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    /// What FERB answered (its result), when it finished.
    pub answer: String,
    /// The checklist's count, when it has one.
    pub steps_done: Option<u32>,
    pub steps_total: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskPage {
    pub tasks: Vec<TaskRow>,
    /// Older tasks exist: ask again with `before` the last row's `created_at`.
    pub more: bool,
}

/// What the Mac says about itself and this phone (kept, so it shows offline too).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct MacInfo {
    /// The Mac's computer name.
    pub name: Option<String>,
    /// When this phone was paired, as the Mac recorded it (ISO 8601).
    pub paired_at: Option<String>,
    /// This phone's name on the Mac.
    pub phone_name: Option<String>,
    /// Where files sent from the phone land (PHN-580); none from a Mac that doesn't say (and in what an older app
    /// kept).
    #[serde(default)]
    pub upload_folder: Option<UploadFolder>,
    /// PHN-846: the Mac has this phone's app lock off (it gives chat and tasks without the unlock); none from a Mac that
    /// doesn't keep it (and in what an older app kept).
    #[serde(default)]
    pub lock_off: Option<bool>,
}

/// The link's own "cancelled" from a transfer the phone stopped.
fn bad_reply() -> MobileError {
    MobileError::new(ErrorCode::BadReply, "FERB's reply could not be read.")
}

fn text(v: &Value) -> String {
    v.as_str().map(str::to_string).unwrap_or_default()
}

fn opt(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

/// A transfer's progress, from the link's events.
struct Progress(Arc<dyn TransferSink>);

impl TransferSink for Progress {
    fn progress(&self, done: u64, of: u64) {
        self.0.progress(done, of);
    }
    fn accepted(&self, upload_id: &str, offset: u64) {
        self.0.accepted(upload_id, offset);
    }
}

impl Shared {
    /// Reads `chat.status` (allowed while locked, PHN-125) and keeps it as the last status.
    pub(crate) fn read_status(
        this: &Arc<Shared>,
        t: &Arc<dyn Transport>,
    ) -> MobileResult<PhoneStatus> {
        let v = Self::call(t, "chat.status", json!({}))?;
        let status = PhoneStatus::read(&v);
        if this.closed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(MobileError::closed());
        }
        Ok(Self::keep_status(this, status))
    }

    /// FERB's use of the screen as just read: kept in the last status (when there is one), told when it changed.
    fn note_screen_use(&self, now: &ScreenUse) {
        let changed = {
            let mut last = lock(&self.status);
            match last.as_mut() {
                Some(s) if s.screen_use.as_ref() != Some(now) => {
                    s.screen_use = Some(now.clone());
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.notify(Change::Status);
        }
    }

    /// Insomnia mode as just read: kept in the last status (when there is one), told when it changed.
    fn note_insomnia(&self, now: &Insomnia) {
        let changed = {
            let mut last = lock(&self.status);
            match last.as_mut() {
                Some(s) if s.insomnia.as_ref() != Some(now) => {
                    s.insomnia = Some(now.clone());
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.notify(Change::Status);
        }
    }

    /// The transport, when the link is up and its path carries files and pictures (PHN-444): else refused here, before
    /// anything is asked.
    pub(crate) fn transport_bulk(&self, what: &str) -> MobileResult<Arc<dyn Transport>> {
        let t = self.transport_online()?;
        if !self.state.lock().link.bulk_allowed {
            return Err(MobileError::new(
                ErrorCode::NeedsBetterPath,
                format!("{what} is available when connected directly or through your relay."),
            ));
        }
        Ok(t)
    }

    /// The proof for one exact call: a fresh challenge from the Mac signed by the app's key over
    /// `{challenge, method, params}` (ferb-link `Phone::call_payload`).
    fn proof(
        t: &Arc<dyn Transport>,
        method: &str,
        params: &Value,
        signer: &Arc<dyn Signer>,
    ) -> MobileResult<Value> {
        let r = Self::call(t, "link.challenge", json!({}))?;
        let challenge = r["challenge"]
            .as_str()
            .ok_or_else(|| MobileError::new(ErrorCode::BadReply, "The link gave no challenge."))?;
        let payload = sorted(vec![
            ("challenge", json!(challenge)),
            ("method", json!(method)),
            ("params", params.clone()),
        ])
        .to_string();
        let sig = signer
            .sign(&payload)
            .ok_or_else(|| MobileError::new(ErrorCode::NotSigned, "Nothing was sent."))?;
        Ok(json!({"payload": payload, "sig": sig}))
    }

    /// A call with its proof, when a signer is given.
    pub(crate) fn call_signed(
        t: &Arc<dyn Transport>,
        method: &str,
        params: Value,
        signer: Option<&Arc<dyn Signer>>,
    ) -> MobileResult<Value> {
        let proof = match signer {
            Some(s) => Some(Self::proof(t, method, &params, s)?.to_string()),
            None => None,
        };
        let reply = t
            .call(method, &params.to_string(), proof.as_deref())
            .map_err(MobileError::from_link)?;
        parse(&reply)
    }
}

fn read_folder(v: &Value) -> Folder {
    let path = text(&v["path"]);
    let entries = v["entries"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .map(|e| {
            let name = text(&e["name"]);
            let child = if path.ends_with('/') {
                format!("{path}{name}")
            } else {
                format!("{path}/{name}")
            };
            FileEntry {
                path: child,
                name,
                dir: e["dir"].as_bool().unwrap_or(false),
                link: e["link"].as_bool().unwrap_or(false),
                size: e["size"].as_u64().unwrap_or(0),
                modified: e["modified"]
                    .as_i64()
                    .or_else(|| e["modified"].as_f64().map(|f| f as i64))
                    .unwrap_or(0),
                locked: e["locked"].as_bool().unwrap_or(false),
            }
        })
        .collect::<Vec<_>>();
    Folder {
        parent: opt(&v["parent"]),
        locked: v["high_value"].as_bool().unwrap_or(false),
        total: v["total"].as_u64().unwrap_or(entries.len() as u64),
        cut: v["cut"].as_bool().unwrap_or(false),
        from: v["from"].as_u64().unwrap_or(0),
        next: opt(&v["next"]),
        entries,
        path,
    }
}

impl Mobile {
    // ---- the lock and Stop ----------------------------------------------------------------------------------------

    /// The app locked (PHN-15): the Mac is told (`link.lock`: this phone's unlock window ends now) and the watch
    /// stops, so a locked phone receives nothing of the conversation until the next unlock's sync (PHN-125). What
    /// waits still arrives as the Mac's status (PHN-126). Offline, the Mac's window ends by itself after its idle time;
    /// returns whether the Mac was told.
    pub fn lock(&self) -> MobileResult<bool> {
        let watch = {
            let mut st = self.shared.open_state()?;
            st.locked = true;
            st.link.watching = false;
            st.generation_bump();
            st.take_watch()
        };
        if let Some(h) = watch {
            h.close();
        }
        self.shared.notify(Change::Link);
        let Ok(t) = self.shared.transport_online() else {
            return Ok(false);
        };
        t.call("link.lock", "{}", None)
            .map_err(MobileError::from_link)?;
        Ok(true)
    }

    /// Whether the app said it locked and has not synced since (the next `sync`, after the unlock, clears it).
    pub fn is_locked(&self) -> bool {
        self.shared.state.lock().locked
    }

    /// "Stop everything on the Mac" (`stop.engage`, DMN-1): FERB stops all it does and starts nothing until the owner
    /// lifts the stop on the Mac. Works while the phone is locked (PHN-125); never queued.
    pub fn stop_everything(&self) -> MobileResult<()> {
        let t = self.shared.transport_online()?;
        t.call("stop.engage", "{}", None)
            .map_err(MobileError::from_link)?;
        Ok(())
    }

    // ---- what FERB has waiting ----------------------------------------------------------------------------------

    /// Reads what FERB has waiting now (`chat.status`, allowed while locked): each question with what its answer
    /// needs (routine, the fingerprint, only on the Mac, "until I'm back"), what runs, what finished, FERB's use of the
    /// screen. Also kept as `last_status`.
    pub fn status(&self) -> MobileResult<PhoneStatus> {
        let t = self.shared.transport_online()?;
        Shared::read_status(&self.shared, &t)
    }

    /// The last status pushed by the Mac or read (memory only; none after a start until one arrives).
    pub fn last_status(&self) -> MobileResult<Option<PhoneStatus>> {
        if self.shared.closed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(MobileError::closed());
        }
        Ok(lock(&self.shared.status).clone())
    }

    // ---- FERB using the screen "until I'm back" (GATE-615) ------------------------------------------------------

    /// Whether FERB uses the Mac's screen on a phone's "until I'm back at my Mac" now, since when and whose, and
    /// whether the Mac offers it (`screen_use.status`; allowed while locked, no unlock). Also kept in `last_status`.
    pub fn screen_use(&self) -> MobileResult<ScreenUse> {
        let t = self.shared.transport_online()?;
        let v = Shared::call(&t, "screen_use.status", json!({}))?;
        let now = ScreenUse::read(&v);
        self.shared.note_screen_use(&now);
        Ok(now)
    }

    /// Ends FERB's use of the screen "until I'm back" now (`screen_use.end`; allowed while locked, no unlock; never
    /// queued): later screen-use questions are asked again.
    pub fn end_screen_use(&self) -> MobileResult<ScreenUseEnd> {
        let t = self.shared.transport_online()?;
        let v = Shared::call(&t, "screen_use.end", json!({}))?;
        let now = ScreenUse::read(&v);
        self.shared.note_screen_use(&now);
        Ok(ScreenUseEnd {
            ended: v["ended"].as_bool().unwrap_or(false),
            by: text(&v["by"]),
            screen_use: now,
        })
    }

    // ---- insomnia mode (APP-980..APP-985) --------------------------------------------------------------------------

    /// Insomnia mode as the Mac has it now (`insomnia.status`; needs the unlock). Also kept in `last_status`, where the
    /// Mac's pushed `chat.status` (allowed while locked) brings it too.
    pub fn insomnia_status(&self) -> MobileResult<Insomnia> {
        let t = self.shared.transport_online()?;
        let v = Shared::call(&t, "insomnia.status", json!({}))?;
        let now = Insomnia::read(&v);
        self.shared.note_insomnia(&now);
        Ok(now)
    }

    /// Turns insomnia mode on from the phone, after the app's warning (APP-980): `variant` "shield" (FERB.app covers
    /// the screen, the Mac stays unlocked) or "locked"; `screen_yes` the one Yes to bring apps forward under the cover
    /// (APP-984). The sensitive class: signed over `{challenge, method, params}` by `signer` (the fingerprint-only key
    /// where the phone registered one, PHN-133), like a setting. Never queued.
    pub fn start_insomnia(
        &self,
        variant: &str,
        screen_yes: bool,
        signer: Arc<dyn Signer>,
    ) -> MobileResult<InsomniaStarted> {
        if variant != "shield" && variant != "locked" {
            return Err(MobileError::new(
                ErrorCode::Refused,
                "That way of keeping the Mac working is not one FERB knows.",
            ));
        }
        let t = self.shared.transport_online()?;
        let v = Shared::call_signed(
            &t,
            "insomnia.start",
            json!({"variant": variant, "screen_yes": screen_yes}),
            Some(&signer),
        )?;
        let now = Insomnia::read(&v);
        self.shared.note_insomnia(&now);
        Ok(InsomniaStarted {
            already: v["already"].as_bool().unwrap_or(false),
            insomnia: now,
        })
    }

    /// Ends insomnia mode now (`insomnia.end`; allowed while locked, no unlock: it only takes something away). The
    /// Mac records a phone's end as "turned_off".
    pub fn end_insomnia(&self) -> MobileResult<InsomniaEnded> {
        let t = self.shared.transport_online()?;
        let v = Shared::call(&t, "insomnia.end", json!({}))?;
        let now = Insomnia::read(&v);
        self.shared.note_insomnia(&now);
        Ok(InsomniaEnded {
            ended: v["ended"].as_bool().unwrap_or(false),
            by: opt(&v["by"]),
            insomnia: now,
        })
    }

    /// Lock my Mac (`insomnia.lock`, APP-983: only on the owner's word; allowed while the phone is locked).
    pub fn lock_mac(&self) -> MobileResult<MacLocked> {
        let t = self.shared.transport_online()?;
        let v = Shared::call(&t, "insomnia.lock", json!({}))?;
        let now = v
            .get("insomnia")
            .filter(|s| s.is_object())
            .map(Insomnia::read);
        if let Some(n) = now.as_ref() {
            self.shared.note_insomnia(n);
        }
        Ok(MacLocked {
            locked: v["locked"].as_bool().unwrap_or(false),
            text: text(&v["text"]),
            insomnia: now,
        })
    }

    /// It's fine: the notice `notice_id` is answered (`insomnia.dismiss`; allowed while the phone is locked).
    pub fn dismiss_insomnia_notice(&self, notice_id: &str) -> MobileResult<Insomnia> {
        let t = self.shared.transport_online()?;
        let v = Shared::call(&t, "insomnia.dismiss", json!({"notice_id": notice_id}))?;
        let now = Insomnia::read(&v);
        self.shared.note_insomnia(&now);
        Ok(now)
    }

    /// One answer of the Mac's camera in insomnia mode (APP-983: Show me), for this paired, unlocked phone, in memory
    /// only. Each answer keeps the camera on a few seconds more on the Mac; it goes off when the phone stops asking.
    /// `have`: the frame number the phone shows (the Mac leaves the picture out when it is the same). On the public
    /// relay it is refused here before anything is asked, like the live preview (PHN-444, PHN-454).
    pub fn mac_camera(&self, have: Option<i64>) -> MobileResult<CameraFrame> {
        let t = self.shared.transport_bulk("Your Mac's camera")?;
        let mut params = json!({"what": "camera"});
        if let Some(h) = have {
            params["have"] = json!(h);
        }
        struct Quiet;
        impl TransferSink for Quiet {
            fn progress(&self, _done: u64, _of: u64) {}
        }
        let id = format!("camera-{}", self.shared.env.ids.new_id());
        let reply = t
            .get(&id, &params.to_string(), None, false, Arc::new(Quiet))
            .map_err(MobileError::from_link)?;
        let mut v = parse(&reply)?;
        let jpeg = match v.get_mut("data").map(Value::take) {
            Some(Value::String(d)) if !d.is_empty() => base64::engine::general_purpose::STANDARD
                .decode(d.as_bytes())
                .ok()
                .filter(|b| !b.is_empty()),
            _ => None,
        };
        let meta = &v["meta"];
        Ok(CameraFrame {
            jpeg,
            seq: meta["seq"].as_i64(),
            state: text(&meta["state"]),
            text: opt(&meta["text"]),
        })
    }

    // ---- files (PHN-113, PHN-115) -----------------------------------------------------------------------------------

    /// One folder's entries (`link.files`; `~` when no path), a page at a time: `after` is the last page's `next`
    /// ("load more", PHN-589; a marker of another folder or phone is refused). A key place's own folder needs `signer`
    /// (the fingerprint for this very path, PHN-121): without one the Mac answers `NeedsUnlock` with link code
    /// `fresh_unlock_required`. The proof is over the path alone, whichever page.
    pub fn files(
        &self,
        path: Option<&str>,
        signer: Option<Arc<dyn Signer>>,
        after: Option<&str>,
    ) -> MobileResult<Folder> {
        let t = self.shared.transport_online()?;
        let mut params = json!({"path": path.unwrap_or("~")});
        // the Mac checks a key place's proof over `{path}` (its `fresh`), so the marker rides beside it, unsigned
        let proof = match signer.as_ref() {
            Some(s) => Some(Shared::proof(&t, "link.files", &params, s)?.to_string()),
            None => None,
        };
        if let Some(a) = after {
            params["after"] = json!(a);
        }
        let reply = t
            .call("link.files", &params.to_string(), proof.as_deref())
            .map_err(MobileError::from_link)?;
        Ok(read_folder(&parse(&reply)?))
    }

    /// Every file under a folder, to fetch it whole (`link.tree`; direct or the owner's relay only).
    pub fn tree(&self, path: &str, signer: Option<Arc<dyn Signer>>) -> MobileResult<Tree> {
        let t = self.shared.transport_bulk("Fetching a folder")?;
        let v = Shared::call_signed(&t, "link.tree", json!({"path": path}), signer.as_ref())?;
        Ok(Tree {
            path: text(&v["path"]),
            name: text(&v["name"]),
            files: v["files"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[])
                .iter()
                .map(|f| TreeFile {
                    rel: text(&f["rel"]),
                    size: f["size"].as_u64().unwrap_or(0),
                })
                .collect(),
            bytes: v["bytes"].as_u64().unwrap_or(0),
            skipped: v["skipped"].as_u64().unwrap_or(0),
            cut: v["cut"].as_bool().unwrap_or(false),
        })
    }

    /// Fetches one file of the Mac into `save_to` on the phone, chunk by chunk (never whole in memory), from `offset`
    /// (what an earlier, paused fetch already saved: appended). `id` names the transfer for `cancel_transfer` (a
    /// pause: what arrived stays). A key place's file needs `signer` (PHN-121). Direct or the owner's relay only.
    pub fn fetch(
        &self,
        id: &str,
        path: &str,
        save_to: &str,
        offset: u64,
        signer: Option<Arc<dyn Signer>>,
        sink: Arc<dyn TransferSink>,
    ) -> MobileResult<Fetched> {
        let mut params = json!({"what": "path", "path": path, "offset": offset});
        if let Some(s) = signer.as_ref() {
            let t = self.shared.transport_bulk("This file")?;
            // the key place's proof is over the path alone (the Mac's `fresh`: `{path: as sent}`, method link.get)
            params["proof"] = Shared::proof(&t, "link.get", &json!({"path": path}), s)?;
        }
        self.fetch_what(id, params, save_to, sink)
    }

    /// One file into `save_to` by the link's `get` (`params` name it: a path, or a task's file), resumed from the
    /// params' `offset` when there is one.
    pub(crate) fn fetch_what(
        &self,
        id: &str,
        params: Value,
        save_to: &str,
        sink: Arc<dyn TransferSink>,
    ) -> MobileResult<Fetched> {
        let t = self.shared.transport_bulk("This file")?;
        let offset = params["offset"].as_u64().unwrap_or(0);
        let reply = t
            .get(
                id,
                &params.to_string(),
                Some(save_to),
                offset > 0,
                Arc::new(Progress(sink)),
            )
            .map_err(MobileError::from_link)?;
        let v = parse(&reply)?;
        let meta = &v["meta"];
        let received = v["received"].as_u64().unwrap_or(0);
        let path = params["path"].as_str().unwrap_or("");
        Ok(Fetched {
            name: meta["name"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| {
                    path.rsplit('/')
                        .next()
                        .filter(|n| !n.is_empty())
                        .unwrap_or("file")
                        .to_string()
                }),
            saved_to: save_to.to_string(),
            received,
            total: meta["total"].as_u64().unwrap_or(offset + received),
        })
    }

    /// Starts a folder sent from the phone (`link.put_group`): the id its files carry as `group`.
    pub fn put_group(&self, name: &str) -> MobileResult<String> {
        Ok(self.begin_group(name)?.group)
    }

    /// Starts a folder sent from the phone (`link.put_group`): its group id and the folder it lands in (PHN-581).
    pub fn begin_group(&self, name: &str) -> MobileResult<GroupStarted> {
        let t = self.shared.transport_bulk("Sending a folder")?;
        let v = Shared::call_signed(&t, "link.put_group", json!({"name": name}), None)?;
        Ok(GroupStarted {
            group: v["group"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(bad_reply)?,
            saved_in: opt(&v["saved_in"]),
        })
    }

    /// Sends a file from the phone's storage (`local_path`, read piece by piece) to the Mac's upload folder
    /// (PHN-127): `resume` an upload that stopped part way (its id; the Mac says where to go on), `group` and `rel`
    /// a file inside a folder being sent. `id` names the transfer for `cancel_transfer` (a pause: the Mac keeps what
    /// arrived for a while, to go on with). The upload's id rides with a message (`send_with`). Direct or the owner's
    /// relay only.
    #[allow(clippy::too_many_arguments)]
    pub fn upload(
        &self,
        id: &str,
        local_path: &str,
        name: &str,
        resume: Option<&str>,
        group: Option<&str>,
        rel: Option<&str>,
        sink: Arc<dyn TransferSink>,
    ) -> MobileResult<Uploaded> {
        let t = self.shared.transport_bulk("Sending a file")?;
        let opts = json!({"name": name, "resume": resume, "group": group, "rel": rel});
        let reply = t
            .put(id, local_path, &opts.to_string(), Arc::new(Progress(sink)))
            .map_err(MobileError::from_link)?;
        let v = parse(&reply)?;
        Ok(Uploaded {
            upload_id: v["upload_id"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(bad_reply)?,
            saved: text(&v["saved"]),
            size: v["size"].as_u64().unwrap_or(0),
            saved_in: opt(&v["saved_in"]),
        })
    }

    /// Stops a transfer under way (a pause or a cancel: the caller of `fetch` or `upload` gets `Cancelled`).
    pub fn cancel_transfer(&self, id: &str) -> MobileResult<()> {
        let t = self.shared.transport()?;
        t.cancel(id);
        Ok(())
    }

    // ---- the live preview (PHN-454) -----------------------------------------------------------------------------

    /// One answer of a task's live preview: the Mac's window as captured, unmasked (PHN-454), for this paired,
    /// unlocked phone, in memory only. `have`: the frame number the phone shows (the Mac leaves the picture out when it
    /// is the same). On the public relay it is refused here before anything is asked (the link and the Mac refuse it
    /// there too). The link keeps at least its least time between two pictures (200 ms).
    pub fn preview(&self, task_id: &str, have: Option<i64>) -> MobileResult<PreviewFrame> {
        self.preview_of(task_id, None, have)
    }

    /// The same for one step of the task (`worker`: the step task id a card row or checklist line watches, FERB.app's
    /// RowPreview, APP-888; the Mac's `tasks.preview` takes it, DMN-404); none: the step that acted last.
    pub fn preview_of(
        &self,
        task_id: &str,
        worker: Option<&str>,
        have: Option<i64>,
    ) -> MobileResult<PreviewFrame> {
        let t = self.shared.transport_bulk("Watch live")?;
        let mut params = json!({"what": "preview", "live": true, "task_id": task_id});
        if let Some(w) = worker.filter(|w| !w.is_empty()) {
            params["worker"] = json!(w);
        }
        if let Some(h) = have {
            params["have"] = json!(h);
        }
        struct Quiet;
        impl TransferSink for Quiet {
            fn progress(&self, _done: u64, _of: u64) {}
        }
        let id = format!("preview-{}", self.shared.env.ids.new_id());
        let reply = t
            .get(&id, &params.to_string(), None, false, Arc::new(Quiet))
            .map_err(MobileError::from_link)?;
        let mut v = parse(&reply)?;
        let jpeg = match v.get_mut("data").map(Value::take) {
            Some(Value::String(d)) if !d.is_empty() => base64::engine::general_purpose::STANDARD
                .decode(d.as_bytes())
                .ok()
                .filter(|b| !b.is_empty()),
            _ => None,
        };
        let meta = &v["meta"];
        Ok(PreviewFrame {
            jpeg,
            seq: meta["seq"].as_i64(),
            state: text(&meta["state"]),
            line: opt(&meta["line"]),
            fresh: meta["fresh"].as_bool().unwrap_or(false),
            busy: meta["busy"].as_bool().unwrap_or(false),
            // FERB.app's `dropsLastFrame`: the Mac has nothing at all to show (no surface yet, or the task ended)
            gone: meta["state"] == "none" && meta["seq"].is_null(),
            // the Mac's own words for an empty or failed preview (its `problem.text`), else its line
            empty_text: opt(&meta["problem"]["text"]).or_else(|| opt(&meta["line"])),
        })
    }

    // ---- FERB's settings (PHN-404) ------------------------------------------------------------------------------

    /// FERB's settings as the Mac lists them (`settings.get`; needs the unlock).
    pub fn settings(&self) -> MobileResult<Vec<SettingValue>> {
        let t = self.shared.transport_online()?;
        let v = Shared::call(&t, "settings.get", json!({}))?;
        Ok(v["values"]
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| SettingValue {
                        key: k.clone(),
                        value_json: v.to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Changes one setting (`settings.set`, the sensitive class: signed by the fingerprint-only key where the phone
    /// registered one, PHN-112, PHN-133). Returns the value the Mac stored, as JSON text.
    pub fn set_setting(
        &self,
        key: &str,
        value_json: &str,
        signer: Arc<dyn Signer>,
    ) -> MobileResult<String> {
        let t = self.shared.transport_online()?;
        let value: Value = serde_json::from_str(value_json).map_err(|_| {
            MobileError::new(ErrorCode::Refused, "That value is not one FERB can take.")
        })?;
        let v = Shared::call_signed(
            &t,
            "settings.set",
            json!({"key": key, "value": value}),
            Some(&signer),
        )?;
        Ok(v["value"].to_string())
    }

    // ---- the Mac's tasks --------------------------------------------------------------------------------------------

    /// The Mac's root tasks, newest first (`tasks.list`; needs the unlock): every task, not only the ones this
    /// conversation started. `before`: the last row's `created_at`, for the next page.
    pub fn tasks(&self, limit: u32, before: Option<&str>) -> MobileResult<TaskPage> {
        let t = self.shared.transport_online()?;
        let mut params = json!({"limit": limit.clamp(1, 50)});
        if let Some(b) = before {
            params["before"] = json!(b);
        }
        let v = Shared::call(&t, "tasks.list", params)?;
        Ok(TaskPage {
            tasks: v["tasks"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[])
                .iter()
                .map(|r| TaskRow {
                    task_id: text(&r["task_id"]),
                    name: text(&r["name"]),
                    goal: text(&r["goal"]),
                    status: text(&r["status"]),
                    created_at: opt(&r["created_at"]),
                    updated_at: opt(&r["updated_at"]),
                    answer: text(&r["answer"]),
                    steps_done: r["steps"]["done"].as_u64().map(|n| n as u32),
                    steps_total: r["steps"]["total"].as_u64().map(|n| n as u32),
                })
                .collect(),
            more: v["more"].as_bool().unwrap_or(false),
        })
    }

    // ---- FERB's alerts ----------------------------------------------------------------------------------------------

    /// Puts one of FERB's alerts away (`attention.ack`, kb APP-1106, PHN-791): it leaves the status on every device, the
    /// Mac's included. An id the Mac no longer knows is a refusal.
    pub fn dismiss_alert(&self, id: &str) -> MobileResult<()> {
        let t = self.shared.transport_online()?;
        Shared::call(&t, "attention.ack", json!({"ids": [id]}))?;
        Ok(())
    }

    // ---- the Mac and this phone -----------------------------------------------------------------------------------

    /// What the Mac says about itself and this phone (`phone.list`, needs the unlock: the Mac's name, this phone's
    /// pairing date and name, where files sent from the phone land), kept in the store so it shows offline too.
    pub fn mac_info(&self) -> MobileResult<MacInfo> {
        let t = self.shared.transport_online()?;
        let v = Shared::call(&t, "phone.list", json!({}))?;
        let me = v["phones"]
            .as_array()
            .and_then(|a| a.iter().find(|p| p["this"] == json!(true)))
            .cloned()
            .unwrap_or(Value::Null);
        let info = MacInfo {
            name: opt(&v["mac"]["name"]),
            paired_at: opt(&me["paired_at"]),
            phone_name: opt(&me["name"]),
            upload_folder: v["upload_folder"].is_object().then(|| UploadFolder {
                path: text(&v["upload_folder"]["path"]),
                shown: text(&v["upload_folder"]["shown"]),
                is_default: v["upload_folder"]["default"].as_bool().unwrap_or(false),
            }),
            lock_off: me["lock_off"].as_bool(),
        };
        let changed = {
            let st = self.shared.open_state()?;
            let store = st.store.as_ref().ok_or_else(MobileError::closed)?;
            let text = serde_json::to_string(&info).unwrap_or_default();
            let changed = store.meta("mac")?.as_deref() != Some(text.as_str());
            if changed {
                store.set_meta("mac", &text)?;
            }
            changed
        };
        if changed {
            self.shared.notify(Change::Mac);
        }
        Ok(info)
    }

    /// PHN-828: binds this phone to its FERB account on the Mac (`phone.bind_account`): `assertion` is the site's signed
    /// statement for this phone's own link endpoint (`/api/app/assertion`). A phone paired before accounts is bound when
    /// it signs in; the Mac's answer `bound` (true once this phone is the Mac's account's). The Mac refuses with the link
    /// codes `account_mismatch` (it removed this phone: another account than the Mac's), `mac_signed_out` or
    /// `account_unconfirmed` (nothing changed), each with the words to show. Works while locked: the site's signature
    /// proves it.
    pub fn bind_account(&self, assertion: &str) -> MobileResult<bool> {
        let t = self.shared.transport_online()?;
        let v = Shared::call(&t, "phone.bind_account", json!({"assertion": assertion}))?;
        Ok(v["bound"].as_bool().unwrap_or(false))
    }

    /// PHN-846: the owner turns this phone's app lock off on the Mac (`phone.lock_off`, the sensitive class: signed over
    /// this exact call by `signer`, the fingerprint-only key where the phone registered one, else the lock key) or on
    /// again (`phone.lock_on`, no proof: only ever stricter; works while locked). The Mac's answer, `lock_off`. While
    /// off, the Mac gives this phone chat and tasks without the unlock; everything else keeps its proof. A Mac too old
    /// to keep it refuses with `mac_link_older`. The kept `mac_info` follows, so the switch shows the Mac's state.
    pub fn set_lock_off(&self, off: bool, signer: Option<Arc<dyn Signer>>) -> MobileResult<bool> {
        let t = self.shared.transport_online()?;
        let v = if off {
            let signer = signer
                .ok_or_else(|| MobileError::new(ErrorCode::NotSigned, "Nothing was sent."))?;
            Shared::call_signed(&t, "phone.lock_off", json!({}), Some(&signer))?
        } else {
            Shared::call(&t, "phone.lock_on", json!({}))?
        };
        let now = v["lock_off"].as_bool().unwrap_or(off);
        let kept = {
            let st = self.shared.open_state()?;
            let store = st.store.as_ref().ok_or_else(MobileError::closed)?;
            let mut info = store
                .meta("mac")?
                .and_then(|t| serde_json::from_str::<MacInfo>(&t).ok());
            if let Some(i) = info.as_mut() {
                i.lock_off = Some(now);
                store.set_meta("mac", &serde_json::to_string(&*i).unwrap_or_default())?;
            }
            info.is_some()
        };
        if kept {
            self.shared.notify(Change::Mac);
        }
        Ok(now)
    }

    /// The last `mac_info` kept (no network).
    pub fn cached_mac_info(&self) -> MobileResult<Option<MacInfo>> {
        let st = self.shared.open_state()?;
        let store = st.store.as_ref().ok_or_else(MobileError::closed)?;
        Ok(store
            .meta("mac")?
            .and_then(|t| serde_json::from_str(&t).ok()))
    }

    /// Registers a key made after pairing (`link.keys`): the device key or the fingerprint-only key of a phone paired
    /// before it had one, under a proof signed for exactly these keys (the fingerprint-only key where one is already
    /// registered, else the lock key). The Mac fills only keys it does not hold yet.
    pub fn register_keys(
        &self,
        device_pub: Option<&str>,
        strong_pub: Option<&str>,
        signer: Arc<dyn Signer>,
    ) -> MobileResult<()> {
        let t = self.shared.transport_online()?;
        let mut params = serde_json::Map::new();
        if let Some(k) = device_pub {
            params.insert("device_pub".into(), json!(k));
        }
        if let Some(k) = strong_pub {
            params.insert("strong_pub".into(), json!(k));
        }
        if params.is_empty() {
            return Err(MobileError::new(ErrorCode::Refused, "No key was given."));
        }
        Shared::call_signed(&t, "link.keys", Value::Object(params), Some(&signer))?;
        Ok(())
    }
}

/// The codes this module's callers act on (re-exported for the apps' tests).
pub const FRESH_UNLOCK_REQUIRED: &str = codes::FRESH_UNLOCK;

#[cfg(test)]
mod tests {
    use super::*;

    /// What an older app kept of the Mac (before the upload folder) still reads.
    // decision: PHN-580
    #[test]
    fn mac_info_kept_by_an_older_app_still_reads() {
        let old: MacInfo =
            serde_json::from_str(r#"{"name":"MacBook","paired_at":null,"phone_name":"Pixel"}"#)
                .unwrap();
        assert_eq!(old.upload_folder, None);
        assert_eq!(old.name.as_deref(), Some("MacBook"));
    }
}
