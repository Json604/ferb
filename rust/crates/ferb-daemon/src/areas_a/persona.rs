//! What FERB saw on this Mac (MEM-13), shared by the app's setup step and chat. Port of `ferb/daemon/persona.py`
//! (`PersonaReview`) and the daemon's side of it in `areas/memory.py` (`_mac_profile_service`, `_prepare_persona`,
//! `_build_mac_profile`, `_tell_mac_joins`, `_tell_mac_profile`, `_refresh_mac_profile`).
//!
//! `start` gathers the facts (the scan seam), has a model interpret them and stores the conclusions as observed memory,
//! once; `status` only reads it. The summary is presented once: by the setup step when it asked, else in chat.

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use ferb_device::persona::{facts_fingerprint, PersonaState};
use serde_json::{json, Map, Value};

use super::seams::PersonaScan;
use super::State;

/// `PersonaReview`'s state (its task runs on the loop).
pub struct PersonaReview {
    state: PersonaState,
    scan: Option<Arc<PersonaScan>>,
    /// `self.persona is not None`: the service was asked for once.
    built: bool,
    running: Option<tokio::task::JoinHandle<()>>,
    error: Option<String>,
    pub closed: bool,
}

impl PersonaReview {
    pub fn new(home: &Path, scan: Option<Arc<PersonaScan>>) -> PersonaReview {
        PersonaReview {
            state: PersonaState::new(home),
            scan,
            built: false,
            running: None,
            error: None,
            closed: false,
        }
    }

    pub fn built(&self) -> bool {
        self.built
    }

    fn busy(&self) -> bool {
        self.running.as_ref().is_some_and(|t| !t.is_finished())
    }

    fn seen(&self) -> Map<String, Value> {
        self.state.seen().unwrap_or_default()
    }

    /// `status`.
    pub fn status(&self) -> Value {
        if self.closed {
            return json!({"state": "closed"});
        }
        if self.busy() {
            return json!({"state": "scanning"});
        }
        let seen = self.seen();
        if !seen.is_empty() {
            let shown = seen.get("shown").filter(|v| ferb_py::value::truthy(v));
            return json!({"state": "seen",
                          "summary": shown.or(seen.get("summary")).cloned().unwrap_or(json!("")),
                          "facts": seen.get("facts").cloned().unwrap_or(json!([])),
                          "at": seen.get("at").cloned().unwrap_or(json!(""))});
        }
        if let Some(e) = &self.error {
            return json!({"state": "failed", "error": e});
        }
        json!({"state": "not_started"})
    }

    /// `needs_browser_part` (DMN-507).
    pub fn needs_browser_part(&self) -> bool {
        let seen = self.seen();
        !seen.is_empty() && seen.get("profiles_read") == Some(&json!(false))
    }
}

/// `_prepare_persona(refresh, shown_by)` → `PersonaReview.start`.
pub fn start(st: &Rc<State>, refresh: bool, shown_by: &str) -> Value {
    let mut p = st.persona.borrow_mut();
    p.built = true;
    if p.closed {
        return json!({"state": "closed"});
    }
    if p.busy() {
        return p.status();
    }
    if p.scan.is_none() {
        return json!({"state": "unavailable"});
    }
    if refresh || p.seen().is_empty() {
        p.error = None;
        let (me, shown_by) = (st.clone(), shown_by.to_owned());
        p.running = Some(tokio::task::spawn_local(async move {
            discover(&me, &shown_by).await;
        }));
    }
    p.status()
}

/// `collect` (memory.py:1140-1151): owner-attributed focus counts, the device id (made once), the scan.
fn gather(st: &State) -> Result<Value, String> {
    let scan = st
        .persona
        .borrow()
        .scan
        .clone()
        .ok_or_else(|| "no scan".to_owned())?;
    let activity: Vec<Value> = st
        .store
        .query(
            "SELECT json_extract(data_json,'$.bundle_id') AS bundle_id, json_extract(data_json,'$.app') AS app, \
             COUNT(*) AS focus_events, MAX(at) AS last_used FROM trail WHERE source='app_focus' AND actor='owner' \
             GROUP BY bundle_id",
            Vec::new(),
        )
        .map_err(|e| e.described())?
        .iter()
        .map(|r| {
            let mut m = Map::new();
            for (n, name) in r.names.iter().enumerate() {
                m.insert(name.clone(), ferb_learning::store::to_json(r.at(n)));
            }
            Value::Object(m)
        })
        .collect();
    let device = match st.config_get("device", "id").filter(ferb_py::value::truthy) {
        Some(d) => ferb_py::value::py_str(&d),
        None => {
            let d = st.ids.new_id();
            st.config
                .set("device", "id", &json!(d))
                .map_err(|e| e.to_string())?;
            d
        }
    };
    let exclude = st.config.exclude_apps().map_err(|e| e.to_string())?;
    let facts = scan(&st.home, &device, activity, exclude)?;
    Ok(st.redactor.value(&facts))
}

async fn discover(st: &Rc<State>, shown_by: &str) {
    let out: Result<(), String> = async {
        let facts = gather(st)?;
        let pick = |k: &str| facts.get(k).cloned().unwrap_or(Value::Null);
        st.journal(
            "setup.persona_scanned",
            json!({"scanned_at": pick("scanned_at"), "installed_apps": pick("installed_apps"),
                   "apps_with_use": pick("apps_with_use")}),
            None,
        )
        .map_err(|e| e.message)?;
        store_seen(st, &facts, shown_by, false).await
    }
    .await;
    if let Err(e) = out {
        let error = st.redact_text(&e);
        st.persona.borrow_mut().error = Some(error.clone());
        let _ = st.journal("setup.persona_failed", json!({"error": error}), None);
    }
}

/// `_build_mac_profile`: a model interprets the gathered facts; code stores them as observed memory, joining the core
/// profile (MEM-423); `joined` lists the facts newly in it.
async fn build(st: &Rc<State>, facts: &Value) -> Result<Value, String> {
    let make = st
        .make_runtime
        .clone()
        .ok_or_else(|| "RuntimeError: no model runtime is set up".to_owned())?;
    let answer = ferb_learning::mac_profile::interpret(
        &move |a: &str, b: &str, c: &[String], d: &Value| make(a, b, c, d),
        &st.prompts,
        facts,
        &st.device_id(),
        None,
        &|_: &str, _: &Value| {},
    )
    .await
    .map_err(|e| e.described())?;
    let before = st.profile.ids().map_err(|e| e.described())?;
    let append = |kind: &str, data: Value| st.journal.append(kind, Some(data), None);
    let (stored, _) = st
        .profile
        .holding_joins(|| ferb_learning::mac_profile::store(&st.memory, &append, facts, &answer));
    let mut stored = stored.map_err(|e| e.described())?;
    let now = st.profile.ids().map_err(|e| e.described())?;
    let joined: Vec<Value> = stored["facts"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|f| {
            let id = ferb_py::value::py_str(&f["id"]);
            (now.contains(&id) && !before.contains(&id)).then(|| json!(id))
        })
        .collect();
    stored["joined"] = json!(joined);
    Ok(stored)
}

/// `PersonaReview._store`.
async fn store_seen(
    st: &Rc<State>,
    facts: &Value,
    shown_by: &str,
    quiet: bool,
) -> Result<(), String> {
    let stored = build(st, facts).await?;
    let now = ferb_clock::py::iso(&st.now());
    let previous = st.persona.borrow().seen();
    let shown_by_kept = if quiet {
        previous
            .get("shown_by")
            .map(ferb_py::value::py_str)
            .unwrap_or_else(|| shown_by.to_owned())
    } else {
        shown_by.to_owned()
    };
    let get = |k: &str, d: Value| stored.get(k).cloned().unwrap_or(d);
    let seen = json!({"summary": get("summary", json!("")), "shown": get("shown", json!("")),
                      "facts": get("facts", json!([])), "at": now, "checked_at": now,
                      "fingerprint": facts_fingerprint(facts),
                      "profiles_read": facts.get("profiles_read").cloned().unwrap_or(json!(true)),
                      "shown_by": shown_by_kept});
    {
        let p = st.persona.borrow();
        p.state
            .set_seen(crate::shared::object(seen.clone()))
            .map_err(|e| e.to_string())?;
        p.state.set_reviewed(true).map_err(|e| e.to_string())?;
    }
    let count = seen["facts"].as_array().map_or(0, Vec::len);
    st.journal(
        "memory.mac_profile_seen",
        json!({"facts": count, "quiet": quiet, "shown_by": shown_by}),
        None,
    )
    .map_err(|e| e.message)?;
    let shown_text = ferb_py::value::py_str(&seen["shown"]);
    let has_shown = ferb_py::value::truthy(&seen["shown"]);
    if !quiet && shown_by != "setup" && has_shown {
        tell_mac_profile(st, &shown_text).await;
    }
    let shown = !quiet && (shown_by == "setup" || has_shown);
    let joined: Vec<String> = stored["joined"]
        .as_array()
        .map(|a| a.iter().map(ferb_py::value::py_str).collect())
        .unwrap_or_default();
    if !joined.is_empty() && !shown {
        tell_mac_joins(st, &joined).await;
    }
    Ok(())
}

/// `PersonaReview.refresh` via `_refresh_mac_profile` (MEM-13): weekly, gather again; interpret and store again only
/// when the facts changed. True when it changed what FERB remembers.
pub async fn refresh(st: &Rc<State>) -> Result<bool, String> {
    if st.shutting_down.get() {
        return Ok(false);
    }
    let seen = {
        let p = st.persona.borrow();
        if p.closed || p.scan.is_none() || p.busy() {
            return Ok(false);
        }
        p.seen()
    };
    if seen.is_empty() {
        return Ok(false);
    }
    let checked = seen
        .get("checked_at")
        .filter(|v| ferb_py::value::truthy(v))
        .or(seen.get("at"))
        .map(ferb_py::value::py_str)
        .unwrap_or_default();
    if let Some(at) = ferb_clock::py::parse(&checked) {
        let days = ferb_learning::profile::PASS_DAYS as f64 * 86400.0;
        if ferb_clock::py::total_seconds(&st.now(), &at) < days {
            return Ok(false);
        }
    }
    let facts = gather(st)?;
    if json!(facts_fingerprint(&facts)) == seen.get("fingerprint").cloned().unwrap_or(Value::Null) {
        let mut kept = seen.clone();
        kept.insert("checked_at".into(), json!(ferb_clock::py::iso(&st.now())));
        kept.insert(
            "profiles_read".into(),
            facts.get("profiles_read").cloned().unwrap_or(json!(true)),
        );
        st.persona
            .borrow()
            .state
            .set_seen(kept)
            .map_err(|e| e.to_string())?;
        return Ok(false);
    }
    store_seen(st, &facts, "quiet", true).await?;
    Ok(true)
}

/// `_tell_mac_joins` (MEM-427): a re-check's facts newly in the core profile, told as one quiet notice.
async fn tell_mac_joins(st: &Rc<State>, ids: &[String]) {
    let items: Vec<ferb_learning::items::Item> = ids
        .iter()
        .filter_map(|i| st.memory.get(i).ok().flatten())
        .collect();
    let Some(first) = items.first() else {
        return;
    };
    let texts: Vec<String> = items
        .iter()
        .map(|it| it.content.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    let line = format!(
        "Now always on: {} ({})",
        texts
            .iter()
            .map(|t| format!("“{t}”"))
            .collect::<Vec<_>>()
            .join(", "),
        ferb_learning::items::origin_label(&first.origin)
    );
    let made: Vec<Value> = items
        .iter()
        .zip(&texts)
        .map(|(it, t)| {
            let mut c = ferb_orchestrator::cards::note(&it.id, t, "saved", &line);
            c["label"] = json!(ferb_learning::items::origin_label(&it.origin));
            c["origin"] = json!(it.origin);
            c["always_on"] = json!(true);
            c
        })
        .collect();
    st.owner
        .tell_owner(
            &format!("memory-always-on-scan-{}", ferb_clock::py::iso(&st.now())),
            items.iter().map(|it| format!("memory:{}", it.id)).collect(),
            &line,
            &ferb_sanitize::origin::join(["ferb", "inferred"]),
            Some(Value::Array(made)),
        )
        .await;
}

/// `_tell_mac_profile`: shown once in chat when the setup step didn't show it.
async fn tell_mac_profile(st: &Rc<State>, text: &str) {
    st.owner
        .tell_owner(
            &format!("memory-mac-profile-{}", ferb_clock::py::iso(&st.now())),
            Vec::new(),
            text,
            &ferb_sanitize::origin::join(["ferb", "private"]),
            None,
        )
        .await;
}

/// At shutdown (`PersonaReview.close`).
pub fn close(st: &State) {
    let mut p = st.persona.borrow_mut();
    p.closed = true;
    if let Some(t) = p.running.take() {
        t.abort();
    }
}
