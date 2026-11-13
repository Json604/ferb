//! The setup window's Browsers step as `ferb/device/extensions.py` words it (L34 C; HAND-411, HAND-412): why a profile
//! can't be paired as its extension stands (`not_paired_step`) and `setup.browsers`'s rows (`browser_rows`). Ported
//! here: `ferb-device` has `store_page`, `profile_state` and `extension_path`, not these two.

use std::collections::BTreeMap;
use std::path::Path;

use ferb_browser::autoinstall::{self, BrowserState, ProfileStatus};
use ferb_browser::profiles::{self, Unsupported};
use ferb_browser::protocol::{version_ok, MIN_EXTENSION_VERSION};
use ferb_device::extensions::{profile_state, store_page};
use serde_json::{json, Map, Value};

/// extensions.py:68: Chromium's reason code for a copy it turned off until the owner accepts new permissions.
pub const PERMISSIONS_INCREASE: i64 = 4;

/// `version_ok(v)` against the connection minimum; a version Python's `int()` refuses counts as not ok (Python
/// raised there: it never reaches these rows, whose versions come from `extension_version`).
pub fn new_enough(version: &str) -> bool {
    version_ok(version, MIN_EXTENSION_VERSION).unwrap_or(false)
}

/// `autoinstall.UNCONFIRMED`: a browser FERB sends its install request to, unconfirmed.
fn unconfirmed(browser: &str) -> &'static str {
    autoinstall::INSTALL
        .iter()
        .find(|(b, way, _)| *b == browser && *way == "unconfirmed")
        .map(|(_, _, note)| *note)
        .unwrap_or("")
}

/// `autoinstall.INSTALL.get(browser, ("store", ""))[1]`.
fn install_note(browser: &str) -> &'static str {
    autoinstall::INSTALL
        .iter()
        .find(|(b, ..)| *b == browser)
        .map(|(_, _, note)| *note)
        .unwrap_or("")
}

/// extensions.py:71 `not_paired_step` (HAND-411, HAND-412): (reason, the owner's step), or None when FERB's
/// extension is on there and new enough.
pub fn not_paired_step(
    p: &ProfileStatus,
    label: &str,
    requested: bool,
    store_url: &str,
    extension_dir: &Path,
) -> Option<(String, String)> {
    let add = if store_url.is_empty() {
        format!(
            "add FERB's extension in {label} with Load unpacked ({})",
            extension_dir.display()
        )
    } else {
        format!("add FERB from its Chrome Web Store page in {label} ({store_url})")
    };
    if p.extension_id.is_empty() {
        if p.removed {
            return Some((
                "removed".into(),
                format!("FERB's extension was removed there, so {label} won't add it again: {add}"),
            ));
        }
        if requested {
            let step = format!(
                "{label} adds FERB the next time it starts (then turn FERB on when it asks)"
            );
            if !store_url.is_empty() {
                return Some(("requested".into(), format!("{step}; faster: {add}")));
            }
            let tail = if unconfirmed(&p.browser).is_empty() {
                String::new()
            } else {
                format!("; if FERB isn't there then, {add}")
            };
            return Some(("requested".into(), step + &tail));
        }
        return Some(("not_added".into(), add));
    }
    let old = !p.version.is_empty() && !new_enough(&p.version);
    let update = format!(
        "FERB's extension there is version {} and FERB needs {MIN_EXTENSION_VERSION} or later: open \
         chrome://extensions in that profile, turn on Developer mode and press Update, or quit and reopen {label}",
        p.version
    );
    if !p.enabled {
        let step = if p.disabled_for.contains(&PERMISSIONS_INCREASE) {
            format!(
                "{label} turned FERB off until you accept its new permissions: open chrome://extensions in that \
                 profile and turn FERB on"
            )
        } else {
            format!("turn FERB on in {label}'s extensions (chrome://extensions in that profile)")
        };
        let tail = if old {
            // f"{update[0].lower()}{update[1:]}"
            let mut c = update.chars();
            let first: String = c
                .next()
                .map(|f| f.to_lowercase().collect())
                .unwrap_or_default();
            format!("; {first}{}", c.as_str())
        } else {
            String::new()
        };
        return Some(("off".into(), step + &tail));
    }
    if old {
        return Some(("too_old".into(), update));
    }
    None
}

/// extensions.py:108 `browser_rows`: `setup.browsers`'s reply. `connected`: each connected profile and the version
/// its hello said; `unpaired`: browser -> the version of an unpaired copy connected there (`Hub.unpaired_copies`).
pub fn browser_rows(
    states: &[BrowserState],
    store_id: &str,
    extension_dir: &Path,
    ferb_identity: Option<&(String, String)>,
    connected: &[((String, String), String)],
    unsupported: &[Unsupported],
    unpaired: &BTreeMap<String, String>,
) -> Map<String, Value> {
    let valid = autoinstall::valid_id(store_id);
    let version_of = |key: &(String, String)| {
        connected
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    };
    let row = |p: &ProfileStatus| -> Value {
        let key = (p.browser.clone(), p.folder.clone());
        let state = profile_state(
            &p.extension_id,
            p.removed,
            p.enabled,
            p.paired == Some(true),
        );
        let is_connected = version_of(&key).is_some();
        let needs = state == "paired" && !is_connected && unpaired.contains_key(&p.browser);
        let said = if needs {
            unpaired.get(&p.browser).cloned().unwrap_or_default()
        } else {
            version_of(&key).unwrap_or_default()
        };
        let version = if said.is_empty() {
            p.version.clone()
        } else {
            said
        };
        json!({"folder": p.folder, "name": p.name, "email": p.email, "state": state,
               "ferb_identity": Some(&key) == ferb_identity, "connected": is_connected,
               "needs_pairing": needs, "version": version,
               "too_old": !version.is_empty() && !new_enough(&version)})
    };
    let page = store_page(store_id);
    let mut out = Map::new();
    out.insert(
        "store_url".into(),
        if page.is_empty() {
            Value::Null
        } else {
            json!(page)
        },
    );
    out.insert(
        "extension_dir".into(),
        json!(extension_dir.to_string_lossy()),
    );
    out.insert(
        "unsupported".into(),
        Value::Array(
            unsupported
                .iter()
                .map(|u| {
                    json!({"app": u.app, "bundle_id": u.bundle_id,
                           "message": autoinstall::unsupported_line(&u.app)})
                })
                .collect(),
        ),
    );
    out.insert(
        "browsers".into(),
        Value::Array(
            states
                .iter()
                .map(|s| {
                    json!({"browser": s.browser,
                           "label": profiles::browser(&s.browser).map_or(s.browser.as_str(), |b| b.label),
                           "requested": s.request.is_some(), "manual": !valid,
                           "unconfirmed": unconfirmed(&s.browser), "note": install_note(&s.browser),
                           "profiles": s.profiles.iter().map(row).collect::<Vec<_>>()})
                })
                .collect(),
        ),
    );
    out
}
