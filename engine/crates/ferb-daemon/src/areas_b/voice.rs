//! Hearing's FERB.app plumbing: where the app is, its recorded signature, its Speech Recognition probe, ending its
//! hearing before an update. Port of `ferb/daemon/areas/voice.py`. Must not listen (L26) and must not speak
//! (VOICE-409): FERB replies only in text.

use std::path::{Path, PathBuf};

use ferb_rpc::HandlerError;
use ferb_sense::transcribe::{Hearing, Transcriber};
use serde_json::{json, Map, Value};

use super::core::Host;

/// `voice_bundle`: ~/.ferb/FERB.app from a checkout install; FERB.app itself when the daemon runs from it.
pub fn voice_bundle(home: &Path, app_bundle: Option<&Path>) -> PathBuf {
    ferb_device::install::voice_app(home, app_bundle)
}

/// `_hearing_requirement`: FERB.app's signature as setup recorded it (`[hearing] requirement`, or `[voice]` from older
/// setups: the same app).
pub fn hearing_requirement(h: &Host) -> Result<Option<String>, HandlerError> {
    for section in ["hearing", "voice"] {
        if let Some(v) = h.get(section, "requirement")? {
            if ferb_py::value::truthy(&v) {
                return Ok(Some(ferb_py::value::py_str(&v)));
            }
        }
    }
    Ok(None)
}

/// `_hearing_status` (L26, for doctor and setup): FERB.app installed and signed as recorded, and (`probe`) its
/// Speech Recognition state and on-device support, asked of FERB.app itself. Never a prompt.
pub async fn hearing_status(
    h: &Host,
    bundle: &Path,
    transcriber: &Transcriber,
    folder: &Path,
    probe: bool,
) -> Map<String, Value> {
    let mut out = Map::new();
    let installed = bundle.exists();
    out.insert("installed".into(), json!(installed));
    if !installed || !probe {
        return out;
    }
    match transcriber.status(folder).await {
        Ok(facts) => {
            out.extend(facts);
            out.insert("signed".into(), json!(true));
        }
        Err(Hearing::NotSigned(_)) => {
            out.insert("signed".into(), json!(false));
        }
        Err(e @ Hearing::Unavailable(_)) => {
            // the check passed; FERB.app's answer didn't come
            out.insert("signed".into(), json!(true));
            out.insert(
                "error".into(),
                json!(h.redact_text(&format!("{}: {}", e.kind(), e.message()))),
            );
        }
    }
    out
}

/// `_quit_voice_app` (DMN-401): one transcribing is never ended (it stops through its lease); otherwise each
/// hearing instance (FERB.app's executable run with `transcribe`/`status` on a file in FERB's audio folder) gets
/// SIGTERM. `pids` is the process scan (`hearing_pids_now`), `kill` the signal.
pub fn quit_voice_app(
    transcriber: &Transcriber,
    folder: &Path,
    pids: &dyn Fn(&Path, &Path) -> Vec<i64>,
    kill: &dyn Fn(i64),
) {
    if transcriber.busy() {
        return;
    }
    for pid in pids(&transcriber.app, folder) {
        kill(pid);
    }
}
