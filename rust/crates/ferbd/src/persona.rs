//! What FERB gathers about this Mac for its profile (`setup.persona` and the weekly Mac-profile refresh):
//! `ferb_device::persona::discover_persona` (persona.py:204, verified against rust/fixtures/device/persona.json) over
//! the real scan of the app folders, Spotlight's use counts (`/usr/bin/mdls`, metadata only: no app is launched), and
//! the browsers' profiles from their Local State only once FERB has Full Disk Access (DMN-507).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Map, Value};

/// persona.py:186 `browser_profiles`: each installed Chromium-family browser's profiles (name, signed-in account,
/// whether it is the one used last).
pub fn browser_profiles(support: &Path) -> Vec<Value> {
    use ferb_browser::profiles as bp;
    let platform = ferb_browser::macos::MacPlatform;
    let mut out = Vec::new();
    for key in bp::installed(&platform, support) {
        let Some(dir) = bp::data_dir(key, support) else {
            continue;
        };
        let label = bp::BROWSERS
            .iter()
            .find(|b| b.key == key)
            .map_or(key, |b| b.label);
        let last = bp::last_used(&dir);
        for p in bp::discover(key, &dir) {
            out.push(
                json!({"browser": label, "name": p.display_name, "email": p.email,
                            "last_used": last.as_deref() == Some(p.folder.as_str())}),
            );
        }
    }
    out
}

/// `mdls` (persona.py:86 `_usage_command`): its stdout, Err when it couldn't run.
fn mdls(argv: &[String]) -> Result<Vec<u8>, ()> {
    let (program, args) = argv.split_first().ok_or(())?;
    std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| o.stdout)
        .map_err(|_| ())
}

/// The daemon's `persona_scan` seam over this Mac.
pub fn scan(
    user_home: &Path,
    browser_support: &Path,
) -> Arc<ferb_daemon::areas_a::seams::PersonaScan> {
    let (user, support): (PathBuf, PathBuf) =
        (user_home.to_path_buf(), browser_support.to_path_buf());
    Arc::new(
        move |home: &Path, device: &str, activity: Vec<Value>, excluded: BTreeSet<String>| {
            let activity: Vec<Map<String, Value>> = activity
                .into_iter()
                .filter_map(|v| v.as_object().cloned())
                .collect();
            let roots = ferb_device::scan::app_roots(&user);
            ferb_device::persona::discover_persona(
                home,
                &activity,
                &excluded,
                ferb_device::persona::Discover {
                    scan: &mut || {
                        // `datetime.now().astimezone().isoformat(timespec="seconds")`
                        ferb_device::scan::scan(
                            device,
                            &roots,
                            &crate::setup_env::Catalog,
                            &crate::areas_c::macos_version(),
                            &ferb_device::scan::now_local(),
                        )
                    },
                    usage: &mut |apps| ferb_device::persona::app_usage(apps, &mut mdls),
                    allowed: &mut || {
                        ferb_mac_hands::native::permissions::full_disk_access()
                            == ferb_mac_hands::permissions::Status::Granted
                    },
                    profiles: &mut || Ok(browser_profiles(&support)),
                },
            )
            .map_err(|e| format!("OSError: {e}"))
        },
    )
}
