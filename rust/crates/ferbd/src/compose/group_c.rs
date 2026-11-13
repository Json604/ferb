//! The seams of `areas_c` (the task records, permissions and health, `logs.subscribe`) filled from the real parts.

use std::sync::Arc;

use serde_json::{json, Map, Value};

use super::areas::GroupB;
use super::Options;
use crate::areas_c::{self, Deps, Mac};
use crate::basics::{Journal, SharedDb, Stop};

/// What [`deps`] reads off the composition.
pub struct Parts<'a> {
    pub opts: &'a Options,
    pub config: Arc<ferb_core::config::Config>,
    pub redactor: Arc<ferb_sanitize::redact::Redactor>,
    pub journal: Arc<Journal>,
    pub db: SharedDb,
    pub stop: Arc<Stop>,
    pub a: ferb_daemon::shared::Local<ferb_daemon::areas_a::State>,
    pub gateway: Arc<ferb_gateway::gateway::Gateway>,
    pub b: &'a GroupB,
    pub tools: super::tools::Parts,
    pub bus: ferb_core::bus::Bus,
}

/// The real Mac when the daemon's Mac parts are on, else nothing read or asked.
pub fn mac(opts: &Options, tools: &super::tools::Parts) -> Arc<dyn Mac> {
    if opts.native {
        let real = areas_c::mac::RealMac::default();
        // ui.click_through: the Mac driver's `report_click_through`
        if let Some(m) = tools.mac() {
            let driver = m.tools.driver.clone();
            let _ = real.click.set(Arc::new(move |pid: i32, windows: &[i64]| {
                driver.report_click_through(pid, windows)
            }));
        }
        Arc::new(real)
    } else {
        Arc::new(areas_c::NoMac)
    }
}

pub fn deps(p: Parts<'_>) -> Deps {
    let opts = p.opts;
    let home = opts.home.clone();
    // containment: setup's three facts, or doctor's live check (`sandbox.available()` once at start, the proxy probed)
    let egress = opts.egress.clone();
    let shell_ok = p.tools.shells().is_some();
    let containment: areas_c::Containment = Arc::new(move |live: bool| {
        let (contained, why) = egress
            .containment
            .get()
            .cloned()
            .unwrap_or((false, "not built".into()));
        let shell = shell_ok && contained;
        if !live {
            return crate::basics::object(
                json!({"profile_ok": contained, "shell": shell, "proxy": egress.running() && contained}),
            );
        }
        let probe = egress.probe();
        let get = |k: &str| probe.get(k).cloned().unwrap_or(Value::Null);
        crate::basics::object(json!({
            "sandbox_exec": std::path::Path::new(ferb_shell::sandbox::SANDBOX_EXEC).exists(),
            "profile_ok": contained, "reason": why, "shell": shell, "shell_reason": why,
            "proxy": egress.running() && contained,
            "proxy_alive": get("alive").as_bool().unwrap_or(false),
            "proxy_restarted": get("restarted").as_bool().unwrap_or(false),
            "proxy_last_error": get("last_error"),
            "local_reports": Value::Null,
        }))
    });
    // `_hearing_status` (voice.py:26, L26): FERB.app installed and signed as recorded, and (probe) its Speech
    // Recognition state and on-device support, asked of FERB.app itself in the background; never a prompt
    let voice = ferb_device::install::voice_app(&home, opts.app_bundle.as_deref());
    let asking = p
        .tools
        .mac()
        .map(|m| (m.transcriber.clone(), m.hearing.folder.clone()));
    let red = p.redactor.clone();
    let hearing: areas_c::Hearing = Arc::new(move |probe: bool| {
        let installed = voice.exists();
        let asking = asking.clone();
        let red = red.clone();
        Box::pin(async move {
            let mut out = Map::new();
            out.insert("installed".into(), json!(installed));
            if !installed || !probe {
                return Value::Object(out);
            }
            let Some((transcriber, folder)) = asking else {
                out.insert("signed".into(), json!(true));
                out.insert(
                    "error".into(),
                    json!("HearingUnavailable: FERB's Mac parts aren't running"),
                );
                return Value::Object(out);
            };
            match transcriber.status(&folder).await {
                Ok(status) => {
                    out.extend(status);
                    out.insert("signed".into(), json!(true));
                }
                Err(ferb_sense::transcribe::Hearing::NotSigned(_)) => {
                    out.insert("signed".into(), json!(false));
                }
                Err(e) => {
                    // the check passed; FERB.app's answer didn't come
                    out.insert("signed".into(), json!(true));
                    out.insert(
                        "error".into(),
                        json!(red.text(&format!("{}: {}", e.kind(), e.message()))),
                    );
                }
            }
            Value::Object(out)
        })
    });
    let sensing = p.b.deps.sensing.as_ref().map(|s| {
        (
            s.sense.clone(),
            s.owner_input_running.clone(),
            s.watch_problems.clone(),
            s.perception_status.clone(),
        )
    });
    let sense_status: areas_c::SenseStatus = Arc::new(move || {
        let Some((sense, running, problems, perception)) = &sensing else {
            return Ok(json!({}));
        };
        let owner_input = running();
        let paused = sense.as_ref().is_some_and(|s| s.paused());
        let failed: Map<String, Value> = sense
            .as_ref()
            .map(|s| s.failed().into_iter().map(|(k, v)| (k, json!(v))).collect())
            .unwrap_or_default();
        let why = match sense {
            Some(s) if !s.paused() && !owner_input => s.tap_down(),
            _ => String::new(),
        };
        let perception = match perception {
            Some(f) => f().map_err(|e| ferb_rpc::HandlerError::fault("DatabaseError", e))?,
            None => json!({}),
        };
        let problems = crate::basics::lock(problems).clone();
        Ok(
            json!({"paused": paused, "watchers": sense.as_ref().map(|s| s.running()).unwrap_or_default(),
                  "failed": failed, "owner_input": owner_input, "folder_problems": problems,
                  "owner_input_why": why, "perception": perception}),
        )
    });
    let hub = p.tools.browsers().map(|b| b.hub.clone());
    let extensions = Arc::new(move || -> Vec<String> {
        hub.as_ref()
            .map(|h| {
                h.bridges()
                    .iter()
                    .filter_map(|b| {
                        let v = b.version();
                        (!v.is_empty())
                            .then(|| format!("{} {}: {v}", b.binding.browser, b.binding.folder))
                    })
                    .collect()
            })
            .unwrap_or_default()
    });
    // `_path_closed("recovery.restore", owner=True)`: the owner is the one acting, so only the never-lifted rules
    // (FERB's own state, code, service, the network folders) close a restore
    let protection = super::gateway::protection(
        &p.config,
        &opts.user_home,
        &home,
        opts.app_bundle.as_deref(),
    )
    .ok()
    .map(ferb_gateway::gateway::SyncProtection);
    let protection = Arc::new(protection);
    let user_home = opts.user_home.to_string_lossy().into_owned();
    let closed: areas_c::Closed = Arc::new(move |path: &str, access: &str| {
        use ferb_gateway::words as w;
        use ferb_policy::protection as pr;
        let prot = &protection.as_ref().as_ref()?.0;
        let rules = prot.fs_rules(path, access);
        let rule = rules.first()?.as_str();
        if !pr::NEVER_LIFTED.contains(&rule) {
            return None;
        }
        let name = "recovery.restore";
        let short = ferb_fs::recovery::short(path, &user_home);
        Some(match rule {
            pr::FERB_STATE => w::fill(w::FS_FERB_STATE, &[("name", name), ("path", &short)]),
            pr::FERB_APP => w::fill(w::FS_FERB_APP, &[("name", name), ("path", &short)]),
            pr::FERB_SERVICE => w::fill(w::FS_FERB_SERVICE, &[("name", name), ("path", &short)]),
            _ => {
                let store = prot
                    .entry(rule)
                    .map(|e| e.shown.clone())
                    .unwrap_or_else(|| rule.to_string());
                let text = if rule.starts_with("read:") {
                    w::FS_NEVER_READ
                } else {
                    w::FS_NEVER_WRITE
                };
                w::fill(text, &[("name", name), ("path", &short), ("store", &store)])
            }
        })
    });
    let helper = Arc::new(super::areas::helper(opts, &home));
    let secret_present: Arc<dyn Fn(&str) -> bool + Send + Sync> = Arc::new(move |key: &str| {
        helper
            .present(ferb_identity::secrets::SERVICE, key)
            .unwrap_or(false)
    });
    let browsers =
        p.b.deps
            .browsers
            .as_ref()
            .map(|b| ferb_daemon::areas_b::browsers::Cx::new(&p.b.deps.host, b));
    Deps {
        home,
        user_home: opts.user_home.clone(),
        app_bundle: opts.app_bundle.clone(),
        config: p.config,
        redactor: p.redactor,
        journal: p.journal,
        db: p.db,
        clock: opts.clock.clone(),
        ids: opts.ids.clone(),
        stop: p.stop,
        a: p.a,
        gateway: p.gateway,
        // the runtimes area as registered (its deps after any test's replacement), sharing its state
        runtimes: match &p.b.deps.runtimes {
            Some(r) => {
                ferb_daemon::areas_b::runtimes::Runtimes::new(p.b.deps.host.clone(), r.clone())
            }
            None => p.b.runtimes.clone(),
        },
        browsers,
        extensions,
        mac: mac(opts, &p.tools),
        facts: areas_c::Facts::real(),
        containment,
        hearing,
        sense_status,
        closed,
        secret_present,
        session_ok: None,
        bus: p.bus,
        relay_default: ferb_daemon::areas_b::reports::RELAY_URL.into(),
        // S1's snapshots (the tools' `Snapshots`, also the gateway's before shell work): on, and the last failure
        snapshots: {
            let s = p.tools.snapshots();
            Arc::new(move || match &s {
                Some(s) => (
                    s.take.is_some(),
                    s.last_failure()
                        .map(serde_json::Value::Object)
                        .unwrap_or(serde_json::Value::Null),
                ),
                None => (false, json!({"off": true})),
            })
        },
        tools: p.tools.clone(),
    }
}
