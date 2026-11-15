//! The owner's permissions and FERB's health as this daemon's own process holds them (`areas/rpc.py`):
//! `doctor.check`, `setup.permissions`, `setup.permission_request`, `setup.finish`, `clipboard.request`,
//! `automation.request`, `identity.check`; and FERB.app's `ui.click_through` (`areas/chat.py`). Nothing here prompts
//! but the owner's own Grant (`setup.permission_request`, `clipboard.request`, `automation.request`).

use std::time::Duration;

use ferb_mac_hands::permissions as perm;
use ferb_rpc::{HandlerError, Params, Server, Session};
use serde_json::{json, Map, Value};

use super::{ferb_error, ok, redact_text, Deps};
use crate::basics::lock;

type Handler =
    fn(
        Deps,
        Session,
        Params,
    ) -> futures_util::future::BoxFuture<'static, Result<Map<String, Value>, HandlerError>>;

fn unary(server: &Server, d: &Deps, method: &str, f: Handler) {
    let d = d.clone();
    server.register_unary(method, move |s: Session, p: Params| f(d.clone(), s, p));
}

pub fn register(server: &Server, d: &Deps) {
    unary(server, d, "doctor.check", |d, s, p| {
        Box::pin(doctor_check(d, s, p))
    });
    unary(server, d, "setup.permissions", |d, _s, p| {
        Box::pin(setup_permissions(d, p))
    });
    unary(server, d, "setup.permission_request", |d, _s, p| {
        Box::pin(setup_permission_request(d, p))
    });
    unary(server, d, "setup.finish", |d, _s, p| {
        Box::pin(setup_finish(d, p))
    });
    unary(server, d, "clipboard.request", |d, _s, _p| {
        Box::pin(clipboard_request(d))
    });
    unary(server, d, "automation.request", |d, _s, p| {
        Box::pin(automation_request(d, p))
    });
    unary(server, d, "ui.click_through", |d, s, p| {
        Box::pin(click_through(d, s, p))
    });
    unary(server, d, "identity.check", |d, _s, p| {
        Box::pin(identity_check(d, p))
    });
}

/// Blocking Mac work off the daemon's threads (DMN-13).
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, HandlerError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| HandlerError::fault("RuntimeError", e.to_string()))
}

/// `params.get(key, default)` as Python's truth value.
fn flag(p: &Params, key: &str, default: bool) -> bool {
    p.get(key).map_or(default, ferb_py::value::truthy)
}

/// `_doctor_check` (rpc.py:420): permissions and the self-test as this process sees them, the runtimes, the
/// containment (sandbox, proxy, snapshots), hearing, the clipboard's access, the tokenizer, the store and the days
/// whose learning failed, and this program's facts.
pub async fn doctor_check(
    d: Deps,
    _s: Session,
    p: Params,
) -> Result<Map<String, Value>, HandlerError> {
    let selftest = flag(&p, "selftest", true);
    let hearing_probe = match p.get("hearing") {
        Some(v) => ferb_py::value::truthy(v),
        None => selftest,
    };
    let mac = d.mac.clone();
    let (grants, tests) = blocking(move || {
        let tests = if selftest { mac.selftest() } else { Vec::new() };
        (mac.grants(), tests)
    })
    .await?;
    let mut out = Map::new();
    out.insert("permissions".into(), Value::Object(grants));
    out.insert("selftest".into(), Value::Array(tests));
    out.insert(
        "runtimes".into(),
        Value::Object(d.runtimes.runtime_status().await?),
    );
    let mut containment = {
        let c = d.containment.clone();
        blocking(move || c(true)).await?
    };
    let (snapshots, failure) = (d.snapshots)();
    containment.insert("snapshots".into(), json!(snapshots));
    containment.insert("snapshot_failure".into(), failure);
    out.insert("containment".into(), Value::Object(containment));
    out.insert("hearing".into(), (d.hearing)(hearing_probe).await);
    let mac = d.mac.clone();
    out.insert(
        "clipboard".into(),
        blocking(move || mac.clipboard_access())
            .await?
            .map_or(Value::Null, Value::from),
    );
    out.insert("tokenizer".into(), json!(d.facts.tokenizer));
    let path = lock(&d.db).path().to_path_buf();
    let (bytes, pending) = blocking(move || {
        (
            ferb_store::store_size(&path),
            ferb_store::cleanup::pending(&path),
        )
    })
    .await?;
    out.insert(
        "database".into(),
        json!({"bytes": bytes, "cleanup_pending": pending}),
    );
    // MEM-421: days whose learning pass failed and wait for the next hourly try; MEM-422: cards stuck on the same
    // inputs
    let undistilled: Vec<Value> = {
        let db = lock(&d.db);
        let mut rows = Vec::new();
        if let Ok(mut st) = db.conn().prepare(
            "SELECT day, detail, attempts FROM distill_runs WHERE status = 'failed' ORDER BY day",
        ) {
            if let Ok(found) = st.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            }) {
                for (day, detail, attempts) in found.flatten() {
                    let e: String = redact_text(&d, &detail.unwrap_or_default())
                        .chars()
                        .take(300)
                        .collect();
                    rows.push(json!({"day": day, "error": e, "attempts": attempts}));
                }
            }
        }
        rows
    };
    let stuck =
        d.a.call(|st| async move { st.learning.stuck_cards().unwrap_or_default() })
            .await
            .unwrap_or_default();
    let stuck: Vec<Value> = stuck
        .into_iter()
        .map(|c| {
            let e: String = redact_text(&d, &ferb_py::value::py_str(&c["error"]))
                .chars()
                .take(300)
                .collect();
            json!({"task_id": c["task_id"], "error": e})
        })
        .collect();
    out.insert(
        "learning".into(),
        json!({"undistilled": undistilled, "stuck_cards": stuck}),
    );
    out.insert("pid".into(), json!(std::process::id()));
    out.insert("executable".into(), json!(d.facts.executable));
    out.insert("version".into(), json!(d.facts.version));
    out.insert("code_hash".into(), json!(d.facts.code_hash));
    out.insert("home".into(), json!(d.home.to_string_lossy()));
    out.insert("stopped".into(), json!(d.stop.engaged()));
    Ok(out)
}

/// `_setup_containment`: the containment facts setup's Full Disk Access row reads (no live probe per poll).
fn setup_containment(d: &Deps) -> Map<String, Value> {
    (d.containment)(false)
}

/// The device profile's apps (`_device_profile().apps`): (name, bundle id, scriptable).
fn profile_apps(d: &Deps) -> Vec<(String, String, bool)> {
    let device = match d.config.get("device", "id") {
        Ok(Some(Value::String(s))) => s,
        _ => return Vec::new(),
    };
    match ferb_device::profile::ProfileStore::new(&d.home).load(&device) {
        Ok(Some(p)) => p
            .apps
            .iter()
            .map(|a| (a.name.clone(), a.bundle_id.clone(), a.scriptable))
            .collect(),
        _ => Vec::new(),
    }
}

/// `permissions.grant_target`: what the owner switches on in Privacy & Security for the daemon's grants: FERB's
/// agent app inside FERB.app, or this program itself when it runs from elsewhere.
fn grant_target(d: &Deps) -> String {
    match &d.app_bundle {
        Some(b) => b
            .join(ferb_device::install::BUNDLE_AGENT_APP)
            .to_string_lossy()
            .into_owned(),
        None => d.facts.executable.clone(),
    }
}

/// rpc.py:505-510: each app's Automation status, every app at once, each at most `limit` (then "unknown"), through
/// the shared check, so a stalled native call is shared by the next poll, never stacked.
pub async fn automation_rows(
    checks: &ferb_mac_hands::permissions::AutomationChecks,
    apps: &[(String, String)],
    limit: Duration,
) -> Vec<Value> {
    let statuses = futures_util::future::join_all(apps.iter().map(|(_, bundle)| {
        let checks = checks.clone();
        let b = bundle.clone();
        async move {
            match tokio::time::timeout(limit, checks.check(&b, false)).await {
                Ok(status) => status.value().to_string(),
                Err(_) => "unknown".to_string(),
            }
        }
    }))
    .await;
    apps.iter()
        .zip(statuses)
        .map(
            |((name, bundle), status)| json!({"name": name, "bundle_id": bundle, "status": status}),
        )
        .collect()
}

/// `_setup_permissions` (rpc.py:493): each grant as this daemon's process holds it, never a prompt; the scriptable
/// running apps' Automation; a grant FERB.app waits on (`fresh`) read again by a fresh process.
async fn setup_permissions(d: Deps, p: Params) -> Result<Map<String, Value>, HandlerError> {
    let mac = d.mac.clone();
    let grants = blocking(move || mac.grants()).await?;
    let mac = d.mac.clone();
    let running = blocking(move || mac.running()).await?;
    let apps: Vec<(String, String)> = profile_apps(&d)
        .into_iter()
        .filter(|(_, b, scriptable)| *scriptable && running.contains(b))
        .map(|(n, b, _)| (n, b))
        .collect();
    let automation = automation_rows(&d.automation_checks, &apps, Duration::from_secs(2)).await;
    let mac = d.mac.clone();
    let clipboard = perm::clipboard_row(blocking(move || mac.clipboard_access()).await?.as_deref());
    let waiting: Vec<String> = match p.get("fresh") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(String::from)
            .collect(),
        _ => Vec::new(),
    };
    let granted = Value::from("granted");
    let pending: Vec<String> = waiting
        .into_iter()
        .filter(|w| perm::FRESH_READ.contains(&w.as_str()) && grants.get(w) != Some(&granted))
        .collect();
    let restart: Vec<String> = if pending.is_empty() {
        Vec::new()
    } else {
        perm::restart_needed(&grants, &d.mac.fresh_grants().await)
            .into_iter()
            .filter(|r| pending.contains(r))
            .collect()
    };
    let mut rows = perm::rows(&grants, &setup_containment(&d), &restart);
    rows.push(clipboard);
    let done = d
        .config
        .get("setup", "permissions_version")
        .ok()
        .flatten()
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        >= perm::SETUP_VERSION;
    ok(json!({"rows": rows, "automation": automation,
              "automation_pane": perm::pane_url("automation"), "grant_target": grant_target(&d),
              "app_panes": perm::app_panes(ferb_device::install::APP_BUNDLE_ID), "setup_done": done}))
}

/// `_setup_permission_request` (rpc.py:543): the owner's Grant on one row: macOS's own prompt, made by this daemon
/// so the grant attaches to it. Journaled.
async fn setup_permission_request(d: Deps, p: Params) -> Result<Map<String, Value>, HandlerError> {
    let permission = ferb_daemon::shared::text_or(&p, "permission", "");
    if !perm::REQUESTABLE.contains(&permission.as_str()) {
        let named = if permission.is_empty() {
            "That permission".to_string()
        } else {
            permission.clone()
        };
        return Err(ferb_error(format!(
            "{named} has no request; add FERB in its System Settings pane"
        )));
    }
    let status = if permission == "clipboard" {
        let mac = d.mac.clone();
        let access = blocking(move || mac.clipboard_request()).await?;
        d.journal.note(
            "clipboard.access",
            json!({"access": access, "requested": true}),
        );
        ferb_py::value::py_str(&perm::clipboard_row(Some(&access))["status"])
    } else {
        let (mac, which) = (d.mac.clone(), permission.clone());
        blocking(move || mac.request(&which)).await?
    };
    d.journal.note(
        "setup.permission_requested",
        json!({"permission": permission, "status": status}),
    );
    ok(json!({"permission": permission, "status": status}))
}

/// `_setup_finish` (rpc.py:559, FERB.app only): setup is recorded as done only when every grant is: this daemon's
/// own and the ones FERB.app reports for itself (`app`); else `missing` names what isn't. Journaled either way.
async fn setup_finish(d: Deps, p: Params) -> Result<Map<String, Value>, HandlerError> {
    let mac = d.mac.clone();
    let (grants, access) = blocking(move || (mac.grants(), mac.clipboard_access())).await?;
    let mut rows = perm::rows(&grants, &setup_containment(&d), &[]);
    rows.push(perm::clipboard_row(access.as_deref()));
    if let Some(Value::Object(app)) = p.get("app") {
        for (name, status) in app {
            rows.push(json!({"id": name, "status": ferb_py::value::py_str(status)}));
        }
    }
    let missing = perm::missing(&rows);
    if missing.is_empty() {
        // permissions.record_done: never lowered
        let current = d
            .config
            .get("setup", "permissions_version")
            .ok()
            .flatten()
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        d.config
            .set(
                "setup",
                "permissions_version",
                &json!(current.max(perm::SETUP_VERSION)),
            )
            .map_err(|e| HandlerError::fault("OSError", e.to_string()))?;
    }
    d.journal.note(
        "setup.finished",
        json!({"by": "app", "done": missing.is_empty(),
               "missing": missing.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>()}),
    );
    ok(json!({"setup_done": missing.is_empty(),
              "missing": missing.iter().map(|(n, s)| json!({"id": n, "status": s})).collect::<Vec<_>>()}))
}

/// `_clipboard_request` (rpc.py:479): one read by this daemon so macOS shows its paste alert now and lists FERB in
/// Paste from Other Apps; nothing read is kept. Journaled.
async fn clipboard_request(d: Deps) -> Result<Map<String, Value>, HandlerError> {
    let mac = d.mac.clone();
    let state = blocking(move || mac.clipboard_request()).await?;
    d.journal.note(
        "clipboard.access",
        json!({"access": state, "requested": true}),
    );
    ok(json!({"access": state}))
}

/// `_automation_request` (rpc.py:578): macOS's Apple Events prompt for each app (only a running app can be asked).
async fn automation_request(d: Deps, p: Params) -> Result<Map<String, Value>, HandlerError> {
    // `[str(b) for b in params.get("bundle_ids", [])]`: a list's items, a text's characters, an object's keys
    let bundles: Vec<String> = match p.get("bundle_ids") {
        None | Some(Value::Null) if !p.contains_key("bundle_ids") => Vec::new(),
        Some(Value::Array(a)) => a.iter().map(ferb_py::value::py_str).collect(),
        Some(Value::String(s)) => s.chars().map(String::from).collect(),
        Some(Value::Object(o)) => o.keys().cloned().collect(),
        // Python's TypeError left the handler as an internal error; Rust refuses in plain words (named fix)
        _ => return Err(ferb_error("bundle_ids must be a list of app bundle ids")),
    };
    let mut results = Map::new();
    for b in bundles {
        let (mac, which) = (d.mac.clone(), b.clone());
        let status = blocking(move || mac.automation(&which, true)).await?;
        results.insert(b, json!(status));
    }
    d.journal
        .note("automation.requested", Value::Object(results.clone()));
    ok(json!({"results": results}))
}

/// `_ui_click_through` (chat.py:146): the notch's click-through windows by number, recorded only from one of FERB's
/// own running apps (the caller known by its socket's pid); the Mac tools' point check then skips exactly those.
async fn click_through(d: Deps, s: Session, p: Params) -> Result<Map<String, Value>, HandlerError> {
    let windows: Option<Vec<i64>> = match p.get("windows") {
        Some(Value::Array(a)) => a
            .iter()
            .map(|w| match w {
                // `type(w) is int and w > 0`: a JSON whole number, never a bool or a float
                Value::Number(n) if n.is_i64() || n.is_u64() => n.as_i64().filter(|n| *n > 0),
                _ => None,
            })
            .collect(),
        _ => None,
    };
    let Some(windows) = windows else {
        return Err(HandlerError::refusal(
            "Refused",
            "windows must be a list of window numbers",
        ));
    };
    let reported = match s.pid {
        Some(pid) => {
            let (mac, w) = (d.mac.clone(), windows.clone());
            blocking(move || mac.click_through(pid, &w)).await?
        }
        None => false,
    };
    if !reported {
        return Err(HandlerError::refusal(
            "Refused",
            "only FERB's own app reports its click-through windows",
        ));
    }
    ok(json!({"windows": windows.len()}))
}

/// `_identity_check` (rpc.py:388): FERB's own Chrome profile holds exactly FERB's account, and each of FERB's
/// secrets is in the Keychain (no value read); the session through the extension when asked.
async fn identity_check(d: Deps, p: Params) -> Result<Map<String, Value>, HandlerError> {
    let get = |section: &str, key: &str| match d.config.get(section, key) {
        Ok(Some(v)) if ferb_py::value::truthy(&v) => ferb_py::value::py_str(&v),
        _ => String::new(),
    };
    let address = get("mailbox", "address");
    let folder = get("identity", "profile");
    let specs =
        ferb_identity::secrets::specs_from_config(d.config.section("secrets").ok().as_ref());
    let mut secrets = Vec::new();
    for s in specs {
        let (present, key) = (d.secret_present.clone(), s.secret.key.clone());
        let held = blocking(move || present(&key)).await?;
        secrets.push(json!({"key": s.secret.key, "in_keychain": held}));
    }
    let mut out = Map::new();
    out.insert("address".into(), json!(address));
    out.insert("profile".into(), json!(folder));
    out.insert("profile_ok".into(), json!(false));
    out.insert("error".into(), Value::Null);
    out.insert("session".into(), Value::Null);
    out.insert("secrets".into(), Value::Array(secrets));
    let cx = d.browsers.clone();
    if address.is_empty() {
        out.insert(
            "error".into(),
            json!(
                "FERB has no account of its own set up (Settings → Browsers → FERB's own account)"
            ),
        );
    } else if folder.is_empty() {
        out.insert(
            "error".into(),
            json!("FERB's own account has no browser profile chosen (Settings → Browsers → FERB's own account)"),
        );
    } else if !cx.as_ref().is_some_and(|c| c.other_app_data()) {
        out.insert(
            "error".into(),
            json!(ferb_daemon::areas_b::browsers::WAITS_FOR_FULL_DISK_ACCESS),
        );
    } else if let Some(c) = &cx {
        let browser = match d.config.get("identity", "browser") {
            Ok(Some(Value::String(s))) => s,
            _ => "chrome".into(),
        };
        let data = c
            .browser_data()
            .into_iter()
            .find(|(b, _)| *b == browser)
            .map_or_else(|| c.deps.chrome_data.clone(), |(_, d)| d);
        let (a, f) = (address.clone(), folder.clone());
        match blocking(move || ferb_identity::seed::verify(&a, &f, &data)).await? {
            Ok(profile) => {
                out.insert("profile_ok".into(), json!(true));
                out.insert("profile_name".into(), json!(profile.name));
            }
            Err(e) => {
                out.insert("error".into(), json!(e.0));
            }
        }
    }
    let paired = match &cx {
        Some(c) => c.attach_identity_if_paired()?,
        None => false,
    };
    out.insert("paired".into(), json!(paired));
    let profile_ok = out["profile_ok"] == json!(true);
    if profile_ok && paired && flag(&p, "session", true) {
        if let Some(check) = &d.session_ok {
            match check().await {
                Ok(ok) => {
                    out.insert("session".into(), json!(ok));
                }
                Err(e) => {
                    out.insert("session".into(), json!(false));
                    out.insert("error".into(), json!(e));
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod automation_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use ferb_mac_hands::permissions::{AutomationChecks, Status};

    /// rpc.py:505-510 with permissions.py:185-221: six apps whose native read stalls answer together within the limit
    /// ("unknown"), not one after another; a second poll while those reads still run starts none of its own; reads
    /// that answer give their status.
    #[tokio::test(flavor = "multi_thread")]
    async fn every_app_is_read_at_once_and_a_stalled_read_is_shared() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let checks = AutomationChecks::new(Arc::new(move |bundle: &str, _ask: bool| {
            c.fetch_add(1, Ordering::SeqCst);
            if bundle.starts_with("slow") {
                std::thread::sleep(Duration::from_millis(600));
            }
            if bundle.ends_with("granted") {
                Status::Granted
            } else {
                Status::NotDetermined
            }
        }));
        let apps: Vec<(String, String)> = (0..6)
            .map(|i| (format!("App {i}"), format!("slow.app{i}")))
            .chain([("Fast".to_string(), "fast.granted".to_string())])
            .collect();
        let began = Instant::now();
        let rows = super::automation_rows(&checks, &apps, Duration::from_millis(200)).await;
        assert!(
            began.elapsed() < Duration::from_millis(500),
            "{:?}",
            began.elapsed()
        );
        assert_eq!(rows.len(), 7);
        assert!(
            rows[..6].iter().all(|r| r["status"] == "unknown"),
            "{rows:?}"
        );
        assert_eq!(rows[6]["status"], "granted");
        assert_eq!(rows[6]["bundle_id"], "fast.granted");
        assert_eq!(calls.load(Ordering::SeqCst), 7);
        // the next poll while the six stalled reads still run: they are shared, none started again
        let _ = super::automation_rows(&checks, &apps[..6], Duration::from_millis(50)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 7);
        // once they answered, a poll reads again and gets their status
        tokio::time::sleep(Duration::from_millis(700)).await;
        let rows = super::automation_rows(&checks, &apps[..1], Duration::from_secs(2)).await;
        assert_eq!(rows[0]["status"], "not_determined");
    }
}
