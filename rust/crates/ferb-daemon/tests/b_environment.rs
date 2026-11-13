//! `ferb/daemon/areas/environment.py` against `rust/fixtures/daemon-b/environment.json` (made by `environment.py`
//! beside it): each case's recorded daemon state handed to the Rust functions through fake seams, comparing the
//! answer, the journal rows, the drivers' calls and the state left behind byte for byte (`json.dumps`).

mod b_support;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use b_support::{fixture, Home};
use ferb_browser::pairing::Binding;
use ferb_browser::profiles::BrowserProfile;
use ferb_core::interfaces::BoxFuture;
use ferb_core::types::Identity;
use ferb_daemon::areas_b::environment::{
    self as env, Bridge, Hub, ReleaseError, Seams, SecretSpec, TabDriver, TaskRec,
};
use ferb_identity::Secret;
use ferb_sanitize::untrusted::NoRandomness;
use serde_json::{json, Map, Value};

fn s(v: &Value) -> String {
    v.as_str().map(str::to_owned).unwrap_or_default()
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().map(s).collect()
}

fn pairs(v: &Value) -> Vec<(String, String)> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|p| (s(&p[0]), s(&p[1])))
        .collect()
}

struct Driver {
    st: Value,
    name: &'static str,
    log: Arc<Mutex<Vec<Value>>>,
}

impl TabDriver for Driver {
    fn task_tabs(&self, task_id: &str) -> Vec<Map<String, Value>> {
        self.st["tabs"][task_id]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| t.as_object().cloned())
            .collect()
    }
    fn urls_of(&self, task_id: &str) -> Vec<String> {
        strs(&self.st["urls"][task_id])
    }
    fn keep_tabs_open(&self, task_id: &str) {
        self.log
            .lock()
            .unwrap()
            .push(json!([self.name, "keep_tabs_open", task_id]));
    }
    fn rest<'a>(&'a self, task_id: &'a str, resting: bool) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.log
                .lock()
                .unwrap()
                .push(json!([self.name, "rest", task_id, resting]));
        })
    }
    fn release<'a>(&'a self, _task_id: &'a str) -> BoxFuture<'a, Result<Vec<Value>, ReleaseError>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn has_wake(&self) -> bool {
        self.st["wake"].as_bool().unwrap_or(false)
    }
}

struct FakeHub(Value);

impl Hub for FakeHub {
    fn bridge(&self, browser: &str, folder: &str) -> Option<Bridge> {
        let b = self.0["bridges"].get(format!("{browser}/{folder}"))?;
        Some(Bridge {
            connected: b["conn"].as_bool().unwrap_or(false),
            version: s(&b["version"]),
        })
    }
    fn unpaired(&self) -> Vec<String> {
        self.0["unpaired"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(_, v)| s(v))
            .collect()
    }
    fn problem(&self, b: &Binding) -> Option<String> {
        self.0["problems"]
            .get(format!("{}/{}", b.browser, b.folder))
            .map(s)
    }
}

struct Fake {
    st: Value,
    support: PathBuf,
    log: Arc<Mutex<Vec<Value>>>,
    missing: Mutex<Vec<String>>,
    providers: Mutex<Vec<String>>,
}

fn profile(p: &Value) -> BrowserProfile {
    BrowserProfile::new(
        &s(&p["browser"]),
        &s(&p["folder"]),
        &s(&p["display_name"]),
        &s(&p["email"]),
    )
}

impl Fake {
    fn driver(&self, key: &str, name: &'static str) -> Option<Arc<dyn TabDriver>> {
        let st = self.st["drivers"].get(key)?;
        if st.is_null() {
            return None;
        }
        Some(Arc::new(Driver {
            st: st.clone(),
            name,
            log: self.log.clone(),
        }))
    }
}

impl Seams for Fake {
    fn identity_profile(&self) -> Option<(String, String)> {
        let p = self.st.get("identity_profile")?;
        p.as_array().map(|a| (s(&a[0]), s(&a[1])))
    }
    fn pairings(&self) -> Option<Vec<Binding>> {
        let list = self.st.get("pairings")?.as_array()?;
        Some(
            list.iter()
                .map(|b| Binding {
                    install_id: s(&b["install_id"]),
                    browser: s(&b["browser"]),
                    folder: s(&b["folder"]),
                    identity: if b["identity"] == "ferb" {
                        Identity::Ferb
                    } else {
                        Identity::Owner
                    },
                })
                .collect(),
        )
    }
    fn pairing_version(&self, install_id: &str) -> String {
        s(&self.st["pairing_versions"][install_id])
    }
    fn alignment(&self) -> Vec<Value> {
        self.st["alignment"].as_array().cloned().unwrap_or_default()
    }
    fn profiles(&self) -> Vec<BrowserProfile> {
        self.st["profiles"]
            .as_array()
            .into_iter()
            .flatten()
            .map(profile)
            .collect()
    }
    fn fresh_profiles(&self) -> Vec<BrowserProfile> {
        self.profiles()
    }
    fn installed_apps(&self) -> Vec<(String, String)> {
        pairs(&self.st["installed_apps"])
    }
    fn running_bundle_ids(&self) -> Option<BTreeSet<String>> {
        match &self.st["running"] {
            Value::String(_) => None,
            v => Some(strs(v).into_iter().collect()),
        }
    }
    fn macos_version(&self) -> String {
        match self.st.get("macos") {
            Some(v) => s(v),
            None => "26.1".into(),
        }
    }
    fn browser_data_dir_override(&self, _browser: &str) -> Option<PathBuf> {
        None
    }
    fn support(&self) -> PathBuf {
        self.support.clone()
    }
    fn browser_support(&self) -> PathBuf {
        self.support.clone()
    }
    fn profile_state(&self, p: &BrowserProfile, extension: Option<&str>) -> String {
        match self.st["profile_states"].get(format!("{}/{}", p.browser, p.folder)) {
            Some(v) => s(v),
            None => format!("ext:{}", extension.unwrap_or("None")),
        }
    }
    fn worker_types(&self) -> Value {
        self.st.get("worker_types").cloned().unwrap_or(json!([]))
    }
    fn task(&self, task_id: &str) -> Option<TaskRec> {
        let t = self.st["tasks"].get(task_id)?;
        let opt = |v: &Value| v.as_str().map(str::to_owned);
        Some(TaskRec {
            id: task_id.into(),
            tree_id: opt(&t["tree_id"]),
            role: s(&t["role"]),
            parent_id: opt(&t["parent_id"]),
            scopes: strs(&t["scopes"]).into_iter().collect(),
            request_id: opt(&t["request_id"]),
            identity: t["identity"].as_str().unwrap_or("owner").into(),
            device_id: t["device_id"].as_str().unwrap_or("dev").into(),
        })
    }
    fn readable_tasks(&self, tree: &str) -> Option<BTreeSet<String>> {
        let r = self.st.get("readable")?;
        if r.is_null() {
            return None;
        }
        Some(match r.get(tree) {
            Some(v) => strs(v).into_iter().collect(),
            None => BTreeSet::from([tree.to_owned()]),
        })
    }
    fn router(&self) -> Option<Arc<dyn TabDriver>> {
        self.driver("router", "router")
    }
    fn ferb_identity(&self) -> Option<Arc<dyn TabDriver>> {
        self.driver("ferb_identity", "ferb_identity")
    }
    fn activity_search(&self, _words: &str) -> Vec<Map<String, Value>> {
        self.st["activity"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| r.as_object().cloned())
            .collect()
    }
    fn wrap_untrusted(&self, text: &str) -> Result<String, NoRandomness> {
        Ok(ferb_sanitize::untrusted::wrap_untrusted_with_id(
            text,
            "0123456789ab",
        ))
    }
    fn registered_scopes(&self) -> BTreeSet<String> {
        self.st["registered"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(strs)
            .collect()
    }
    fn identity_browser(&self) -> bool {
        self.st["identity_browser"].as_bool().unwrap_or(false)
    }
    fn browsing(&self) -> Map<String, Value> {
        self.st["browsing"].as_object().cloned().unwrap_or_default()
    }
    fn permissions(&self) -> BTreeMap<String, bool> {
        self.st["permissions"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, v)| (k.clone(), v.as_bool().unwrap_or(false)))
            .collect()
    }
    fn runtime_for(&self, role: &str) -> String {
        s(&self.st["runtime_for"][role])
    }
    fn stored_api_keys(
        &self,
        providers: &[String],
        problems: &mut Vec<Value>,
    ) -> Result<Vec<Secret>, String> {
        self.providers
            .lock()
            .unwrap()
            .extend(providers.iter().cloned());
        let keys = &self.st["keys"];
        problems.extend(keys["problems"].as_array().cloned().unwrap_or_default());
        if keys["raise"].as_bool().unwrap_or(false) {
            return Err("RuntimeError".into());
        }
        Ok(strs(&keys["values"]).into_iter().map(Secret::new).collect())
    }
    fn about_summary(&self) -> String {
        self.st
            .get("about")
            .map(s)
            .unwrap_or_else(|| "FERB about.".into())
    }
    fn secret_specs(&self) -> Vec<SecretSpec> {
        self.st["secrets"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|x| SecretSpec {
                key: s(&x["key"]),
                account_hint: s(&x["account_hint"]),
                allowed_origins: strs(&x["allowed_origins"]),
            })
            .collect()
    }
    fn missing_dev_tools(&self) -> Vec<String> {
        self.missing.lock().unwrap().clone()
    }
    fn clear_missing_dev_tools(&self) {
        self.missing.lock().unwrap().clear();
    }
    fn dev_tools_installed(&self) -> bool {
        self.st["dev_tools_installed"].as_bool().unwrap_or(false)
    }
    fn time_meanings(&self) -> Vec<(String, String)> {
        pairs(&self.st["time_meanings"])
    }
    fn models(&self) -> Vec<(String, String)> {
        match self.st.get("models") {
            Some(m) => m
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, v)| (k.clone(), s(v)))
                .collect(),
            None => vec![("planner".into(), "model-a".into())],
        }
    }
    fn schedule_facts(&self, last_runs: bool) -> Option<Vec<String>> {
        let f = self.st.get("schedule_facts").filter(|v| !v.is_null())?;
        Some(strs(&f[if last_runs { "last_runs" } else { "plain" }]))
    }
    fn reminder_facts(&self) -> Option<Vec<String>> {
        self.st
            .get("reminder_facts")
            .filter(|v| !v.is_null())
            .map(strs)
    }
    fn hub(&self) -> Option<Arc<dyn Hub>> {
        let h = self.st.get("hub").filter(|v| !v.is_null())?;
        Some(Arc::new(FakeHub(h.clone())))
    }
    fn unpaired_note(&self, browser: &str) -> String {
        s(&self.st["unpaired_notes"][browser])
    }
}

fn dumps(v: &Value) -> String {
    ferb_py::json::dumps(v)
}

fn run_case(c: &Value) -> Vec<String> {
    let home = Home::new(c["config"].as_str().unwrap_or(""));
    let support = home.dir.path().join("support");
    for (rel, doc) in c["files"].as_object().into_iter().flatten() {
        let p = support.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let text = match doc {
            Value::String(t) => t.clone(),
            other => ferb_py::json::dumps(other),
        };
        std::fs::write(&p, text).unwrap();
    }
    let f = Fake {
        st: c["state"].clone(),
        support,
        log: Arc::new(Mutex::new(vec![])),
        missing: Mutex::new(strs(&c["state"]["missing_dev_tools"])),
        providers: Mutex::new(vec![]),
    };
    let host = &home.host;
    let args = c["args"].as_array().cloned().unwrap_or_default();
    let a0 = args.first().map(s).unwrap_or_default();
    let flag = |i: usize, d: bool| args.get(i).and_then(Value::as_bool).unwrap_or(d);
    let out: Value = match c["method"].as_str().unwrap() {
        "_environment" => {
            let e = env::environment(host, &f, &a0).unwrap();
            json!({"stable": e.stable, "live": e.live})
        }
        "_task_root" => json!(env::task_root(&f, &a0)),
        "_inspection_parent" => json!(env::inspection_parent(&f, &a0)),
        "_conversation_tasks" => json!(env::conversation_tasks(&f, &a0)),
        "_task_tabs" => json!(env::task_tabs(host, &f, &a0)),
        "_browser_context" => json!(env::browser_context(host, &f, &a0)),
        "_urls_of" => json!(env::urls_of(&f, &a0)),
        "_keep_tabs_open" => {
            env::keep_tabs_open(&f, &a0);
            Value::Null
        }
        "_loop_rest" => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            rt.block_on(env::loop_rest(&f, &a0, flag(1, false)));
            Value::Null
        }
        "_relevant_context" => {
            let r = env::relevant_context(&f, &a0).unwrap();
            json!([r.text, r.origin, r.data, r.refs])
        }
        "_availability" => {
            let a = env::availability(&f);
            json!({"scopes": a.scopes, "text": a.text, "missing": a.missing})
        }
        "_facts" => json!(env::facts(host, &f).unwrap()),
        "_live_facts" => json!(env::live_facts(&f, flag(0, true), flag(1, false))),
        "_keychain_values" => {
            let mut problems = vec![];
            let (values, n) = env::keychain_values(&f, &mut problems);
            let values: Vec<&str> = values.iter().map(Secret::expose).collect();
            json!([values, n])
        }
        "_preload_secrets" => {
            env::preload_secrets(host, &f).unwrap();
            Value::Null
        }
        m => panic!("unknown method {m}"),
    };
    let journal: Vec<Value> = home
        .journal_rows()
        .into_iter()
        .map(|r| json!([r[0], r[1]]))
        .collect();
    let mut got = vec![
        format!("out {}", dumps(&out)),
        format!("journal {}", dumps(&Value::Array(journal))),
        format!(
            "log {}",
            dumps(&Value::Array(f.log.lock().unwrap().clone()))
        ),
    ];
    if c.get("missing_dev_tools").is_some() {
        got.push(format!(
            "missing_dev_tools {}",
            dumps(&json!(f.missing.lock().unwrap().clone()))
        ));
    }
    if c.get("providers").is_some() {
        got.push(format!(
            "providers {}",
            dumps(&json!(f.providers.lock().unwrap().clone()))
        ));
        let probe = s(&c["state"]["probe"]);
        got.push(format!("redacted_after {}", host.redact_text(&probe)));
    }
    got
}

fn expected(c: &Value) -> Vec<String> {
    let mut out = vec![
        format!("out {}", dumps(&c["out"])),
        format!("journal {}", dumps(&c["journal"])),
        format!("log {}", dumps(&c["log"])),
    ];
    if let Some(m) = c.get("missing_dev_tools") {
        out.push(format!("missing_dev_tools {}", dumps(m)));
    }
    if let Some(p) = c.get("providers") {
        out.push(format!("providers {}", dumps(p)));
        out.push(format!("redacted_after {}", s(&c["redacted_after"])));
    }
    out
}

#[test]
fn environment_matches_python() {
    let fx = fixture("daemon-b/environment.json");
    let cases = fx["cases"].as_array().unwrap();
    let (mut same, mut diffs) = (0usize, Vec::new());
    for c in cases {
        let (want, got) = (expected(c), run_case(c));
        for (i, w) in want.iter().enumerate() {
            match got.get(i) {
                Some(g) if g == w => same += 1,
                g => diffs.push(format!(
                    "{} ({}):\n  python: {w}\n  rust:   {}",
                    c["name"],
                    c["method"],
                    g.map_or("(none)", String::as_str)
                )),
            }
        }
    }
    eprintln!(
        "environment: {} cases, {same} comparisons equal",
        cases.len()
    );
    assert!(
        diffs.is_empty(),
        "{} difference(s):\n{}",
        diffs.len(),
        diffs.join("\n")
    );
    assert!(cases.len() >= 70);
}
