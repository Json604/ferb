//! The browser area (`ferb/daemon/areas/browsers.py`) against the Python daemon's own lines: the recorded
//! `browsers` scenario of `rust/fixtures/daemon/rpc_transcripts.json` (no Full Disk Access) and
//! `rust/fixtures/daemon-b/browsers.json` (made by `browsers.py`: invented Chrome and Brave folders, pairings,
//! connected profiles, FERB's own profile, Pair all, a handoff). Every line both ways, the journal rows, every browser
//! command FERB ran, what it sent a connected profile, and config.toml, the browser folders and the pairing rows at
//! the end.

mod b_support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use b_support::{fixture, line_text, replay, scenario, Daemon, Home};
use ferb_browser::pairing::{
    Binding, InstallRow, InstallStore, PairingKey, Pairings, Random, StoreError,
};
use ferb_browser::profiles::{Binaries, Platform};
use ferb_browser::protocol::{ExtMessage, ExtType};
use ferb_core::types::Identity;
use ferb_daemon::areas_b::browsers::{
    self, BoxFut, BridgeView, Cx, Deps, Extensions, IdentityBrowser, NativeHost, Notifier, State,
    Tabs, BAD_TIMEOUT,
};
use serde_json::{json, Value};

const METHODS: [&str; 13] = [
    "browsers.list",
    "browsers.pair",
    "browsers.installed",
    "browsers.unpair",
    "setup.browser_pair",
    "setup.browsers_pair_all",
    "setup.browsers_pairing",
    "setup.browsers",
    "setup.browser_open",
    "browser.show_handoff",
    "identity.find",
    "identity.choose",
    "identity.pair",
];

// ---------- fakes ----------

/// `ext_installs` in memory: (install_id, browser, folder, identity, ext_version).
type InstallRecord = (String, String, String, Identity, String);

#[derive(Default)]
struct MemInstalls(Mutex<Vec<InstallRecord>>);

impl InstallStore for MemInstalls {
    fn row(&self, install_id: &str) -> Result<Option<InstallRow>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.0 == install_id)
            .map(|r| InstallRow {
                browser: r.1.clone(),
                folder: r.2.clone(),
                identity: r.3,
            }))
    }
    fn install_for(&self, browser: &str, folder: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.1 == browser && r.2 == folder)
            .map(|r| r.0.clone()))
    }
    fn installs(&self) -> Result<Vec<String>, StoreError> {
        let mut rows = self.0.lock().unwrap().clone();
        rows.sort_by(|a, b| (&a.1, &a.2).cmp(&(&b.1, &b.2)));
        Ok(rows.into_iter().map(|r| r.0).collect())
    }
    fn key(&self, _: &str) -> Result<Option<PairingKey>, StoreError> {
        Ok(None)
    }
    fn key_and_seq(&self, _: &str) -> Result<Option<(PairingKey, i64)>, StoreError> {
        Ok(None)
    }
    fn version(&self, install_id: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.0 == install_id)
            .map(|r| r.4.clone()))
    }
    fn set_version(&self, install_id: &str, version: &str) -> Result<(), StoreError> {
        for r in self
            .0
            .lock()
            .unwrap()
            .iter_mut()
            .filter(|r| r.0 == install_id)
        {
            r.4 = version.into();
        }
        Ok(())
    }
    fn set_last_seq(&self, _: &str, _: i64) -> Result<(), StoreError> {
        Ok(())
    }
    fn unpair(&self, browser: &str, folder: &str) -> Result<bool, StoreError> {
        let mut rows = self.0.lock().unwrap();
        let before = rows.len();
        rows.retain(|r| !(r.1 == browser && r.2 == folder));
        Ok(rows.len() != before)
    }
    fn pair(
        &self,
        install_id: &str,
        row: &InstallRow,
        _: &PairingKey,
        _: &str,
    ) -> Result<(), StoreError> {
        let mut rows = self.0.lock().unwrap();
        rows.retain(|r| !((r.1 == row.browser && r.2 == row.folder) || r.0 == install_id));
        rows.push((
            install_id.into(),
            row.browser.clone(),
            row.folder.clone(),
            row.identity,
            String::new(),
        ));
        Ok(())
    }
}

/// `secrets.token_urlsafe(24)` as the fixture stubs it: 24 bytes of a counter's value; any other draw is zeros.
#[derive(Default)]
struct CountingRandom(AtomicUsize);

impl Random for CountingRandom {
    fn bytes(&self, n: usize) -> Vec<u8> {
        if n == 24 {
            vec![(self.0.fetch_add(1, Ordering::SeqCst) + 1) as u8; n]
        } else {
            vec![0; n]
        }
    }
}

/// Launch Services knows every browser at no path; no app opens web links (the harness's stubs).
struct FakeMac;

impl Platform for FakeMac {
    fn has_app(&self, _: &str) -> bool {
        true
    }
    fn locate(&self, _: &str) -> Result<Option<PathBuf>, String> {
        Ok(None)
    }
    fn https_handlers(&self) -> Result<Vec<PathBuf>, String> {
        Ok(vec![])
    }
}

struct FakeBridge {
    view: BridgeView,
    install: String,
}

/// The hub as the fixture leaves it: bridges made with a live connection, unpaired copies, nothing refused.
struct FakeHub {
    pairings: Mutex<Option<Arc<Pairings>>>,
    bridges: Mutex<Vec<FakeBridge>>,
    unpaired: Vec<(String, String, String)>,
    next: AtomicUsize,
    calls: Mutex<Vec<Value>>,
    /// The open tabs of each profile ("chrome/Profile 1"), as the browsers would hold them.
    tabs: OpenTabs,
}

/// Each profile's open tabs by address: the owner's, and what FERB's launches opened.
type OpenTabs = Arc<Mutex<BTreeMap<String, Vec<String>>>>;

/// The profile a browser command names ("chrome/Profile 1"): the browser from its executable (or `open -b`'s bundle
/// id), the folder from `--profile-directory`.
fn launched_profile(argv: &[String]) -> String {
    let all = argv.join(" ");
    let browser = if all.contains("Brave") || all.contains("com.brave.") {
        "brave"
    } else {
        "chrome"
    };
    let folder = argv
        .iter()
        .find_map(|a| a.strip_prefix("--profile-directory="))
        .unwrap_or("Default");
    format!("{browser}/{folder}")
}

impl FakeHub {
    fn pairings(&self) -> Arc<Pairings> {
        self.pairings.lock().unwrap().clone().unwrap()
    }

    /// `hub.bridge(browser, folder)` (a new bridge when the binding changed), connected, one more hello.
    fn connect(&self, browser: &str, folder: &str, version: &str) {
        let b = self
            .pairings()
            .for_profile(browser, folder)
            .unwrap()
            .expect("paired");
        let mut bridges = self.bridges.lock().unwrap();
        let at = bridges
            .iter()
            .position(|x| x.view.browser == browser && x.view.folder == folder);
        if at.is_none_or(|i| bridges[i].install != b.install_id) {
            let fresh = FakeBridge {
                view: BridgeView {
                    browser: browser.into(),
                    folder: folder.into(),
                    identity: b.identity,
                    id: self.next.fetch_add(1, Ordering::SeqCst) + 1,
                    connected: false,
                    version: String::new(),
                    hellos: 0,
                },
                install: b.install_id.clone(),
            };
            match at {
                Some(i) => bridges[i] = fresh,
                None => bridges.push(fresh),
            }
        }
        let br = bridges
            .iter_mut()
            .find(|x| x.view.browser == browser && x.view.folder == folder)
            .unwrap();
        br.view.connected = true;
        br.view.version = version.into();
        br.view.hellos += 1;
    }
}

impl Extensions for FakeHub {
    fn bridges(&self) -> Vec<BridgeView> {
        self.bridges
            .lock()
            .unwrap()
            .iter()
            .map(|b| b.view.clone())
            .collect()
    }
    fn unpaired(&self) -> Vec<String> {
        self.unpaired.iter().map(|u| u.1.clone()).collect()
    }
    fn unpaired_copies(&self) -> BTreeMap<String, String> {
        let mut out: BTreeMap<String, String> = BTreeMap::new();
        for (_, browser, version) in &self.unpaired {
            let same = out.get(browser).is_none_or(|v| v == version);
            out.insert(
                browser.clone(),
                if same { version.clone() } else { String::new() },
            );
        }
        out
    }
    fn problem(&self, binding: &Binding) -> Option<String> {
        let version = self
            .pairings()
            .version(&binding.install_id)
            .unwrap_or_default();
        (!version.is_empty() && !browsers::steps::new_enough(&version))
            .then(|| ferb_browser::rules::too_old(&version))
    }
    fn call(
        &self,
        browser: &str,
        folder: &str,
        op: &str,
        args: Value,
        timeout: f64,
    ) -> BoxFut<Result<Value, String>> {
        self.calls.lock().unwrap().push(json!([
            format!("{browser}/{folder}"),
            op,
            args.clone(),
            timeout
        ]));
        // the extension's `wake_close`: every tab of that profile at exactly that address closes
        if op == "wake_close" {
            if let Some(tabs) = self
                .tabs
                .lock()
                .unwrap()
                .get_mut(&format!("{browser}/{folder}"))
            {
                tabs.retain(|t| Some(t.as_str()) != args["url"].as_str());
            }
        }
        Box::pin(async { Ok(json!({})) })
    }
}

#[derive(Default)]
struct FakeIdentity(AtomicBool);

impl IdentityBrowser for FakeIdentity {
    fn ready(&self) -> bool {
        true
    }
    fn attached(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
    fn attach(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    fn detach(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct Notes(Mutex<Vec<Value>>);

impl Notifier for Notes {
    fn notify(
        &self,
        title: &str,
        text: &str,
        urgent: bool,
        category: &str,
        target: Value,
        ident: &str,
    ) -> BoxFut<()> {
        self.0.lock().unwrap().push(
            json!({"title": title, "text": text, "urgent": urgent, "category": category,
                                            "target": target, "ident": ident}),
        );
        Box::pin(async {})
    }
}

/// The router's tab for a handoff: the first look shows the check; later looks wait for `go`, then don't.
struct FakeTabs {
    looks: Mutex<Vec<Value>>,
    shown: Mutex<Vec<Value>>,
    go: tokio::sync::watch::Receiver<bool>,
}

impl Tabs for FakeTabs {
    fn shows(&self, tab: &Value, message: &str) -> BoxFut<Result<Value, String>> {
        let n = {
            let mut looks = self.looks.lock().unwrap();
            looks.push(json!([tab, message]));
            looks.len()
        };
        let mut go = self.go.clone();
        Box::pin(async move {
            let page = json!({"url": "https://www.example.test/check", "title": "Check"});
            let mut out = json!({"open": true});
            if n == 1 {
                out["shows"] = json!(true);
            } else {
                let _ = go.wait_for(|g| *g).await;
                out["shows"] = json!(false);
            }
            out["url"] = page["url"].clone();
            out["title"] = page["title"].clone();
            Ok(out)
        })
    }
    fn show_tab(&self, tab: &Value) -> BoxFut<bool> {
        let n = {
            let mut shown = self.shown.lock().unwrap();
            shown.push(tab.clone());
            shown.len()
        };
        Box::pin(async move { n == 1 })
    }
    fn tab_profile(&self, _: &Value) -> (String, String) {
        ("chrome".into(), "Default".into())
    }
}

// ---------- the world one scenario starts from ----------

struct World {
    home: Home,
    support: PathBuf,
    deps: Deps,
    hub: Arc<FakeHub>,
    launched: Arc<Mutex<Vec<Value>>>,
    answer: Arc<AtomicBool>,
    identity: Arc<FakeIdentity>,
    notes: Arc<Notes>,
    tabs: Arc<FakeTabs>,
    go: tokio::sync::watch::Sender<bool>,
    waits: Arc<Mutex<Vec<Value>>>,
    store: Arc<MemInstalls>,
    open_tabs: OpenTabs,
}

const DIRS: [(&str, &str); 2] = [
    ("chrome", "Google/Chrome"),
    ("brave", "BraveSoftware/Brave-Browser"),
];

fn identity_of(text: &str) -> Identity {
    *Identity::ALL.iter().find(|i| i.as_str() == text).unwrap()
}

fn world(setup: &Value, access: bool) -> World {
    let config = if setup["config"].is_null() {
        String::new()
    } else {
        line_text(&setup["config"])
    };
    let home = Home::new(&config);
    let support = home.dir.path().join("user/Library/Application Support");
    std::fs::create_dir_all(&support).unwrap();
    if let Some(files) = setup["files"].as_object() {
        for (rel, text) in files {
            let path = support.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, line_text(text)).unwrap();
        }
    }
    let store = Arc::new(MemInstalls::default());
    for r in setup["installs"].as_array().into_iter().flatten() {
        let s = |i: usize| r[i].as_str().unwrap().to_string();
        store
            .0
            .lock()
            .unwrap()
            .push((s(0), s(1), s(2), identity_of(&s(3)), s(4)));
    }
    let pairings = Arc::new(Pairings::new(
        store.clone(),
        home.clock.clone(),
        Arc::new(CountingRandom::default()),
    ));
    let hub = Arc::new(FakeHub {
        pairings: Mutex::new(Some(pairings.clone())),
        bridges: Mutex::new(vec![]),
        unpaired: setup["unpaired"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|u| {
                let s = |i: usize| u[i].as_str().unwrap().to_string();
                (s(0), s(1), s(2))
            })
            .collect(),
        next: AtomicUsize::new(0),
        calls: Mutex::new(vec![]),
        tabs: OpenTabs::default(),
    });
    for b in setup["bridges"].as_array().into_iter().flatten() {
        hub.connect(
            b[0].as_str().unwrap(),
            b[1].as_str().unwrap(),
            b[2].as_str().unwrap(),
        );
    }
    let launched = Arc::new(Mutex::new(vec![]));
    let answer = Arc::new(AtomicBool::new(false));
    let installs = Arc::new(AtomicUsize::new(0));
    let launch: browsers::Launch = {
        let (launched, answer, pairings, hub) = (
            launched.clone(),
            answer.clone(),
            pairings.clone(),
            hub.clone(),
        );
        Arc::new(move |argv: &[String]| {
            launched.lock().unwrap().push(json!(argv));
            let url = argv.last().cloned().unwrap_or_default();
            // a command whose last word is an address opens it as a tab of that profile
            let opened = !url.starts_with("--") && argv.len() > 1;
            if opened {
                hub.tabs
                    .lock()
                    .unwrap()
                    .entry(launched_profile(argv))
                    .or_default()
                    .push(url.clone());
            }
            if answer.load(Ordering::SeqCst) {
                if let Some((_, token)) = url.split_once("/pair.html#token=") {
                    let n = installs.fetch_add(1, Ordering::SeqCst) + 1;
                    let m = ExtMessage {
                        install_id: format!("inst-pair-{n:04}"),
                        mtype: ExtType::Pair,
                        body_json: json!({"token": token}).to_string(),
                        seq: None,
                        mac: None,
                    };
                    let verdict = pairings.handle(&m).unwrap();
                    assert!(verdict.accepted, "{}", verdict.reason);
                    let b = verdict.binding.unwrap();
                    hub.connect(&b.browser, &b.folder, "0.19.5");
                    // pair.js: the page hands over its token, then closes itself
                    if let Some(tabs) = hub.tabs.lock().unwrap().get_mut(&launched_profile(argv)) {
                        tabs.retain(|t| *t != url);
                    }
                }
            }
            Ok(())
        })
    };
    let identity = Arc::new(FakeIdentity::default());
    let notes = Arc::new(Notes::default());
    let (go, go_rx) = tokio::sync::watch::channel(false);
    let tabs = Arc::new(FakeTabs {
        looks: Mutex::new(vec![]),
        shown: Mutex::new(vec![]),
        go: go_rx,
    });
    let waits = Arc::new(Mutex::new(vec![]));
    let deps = Deps {
        pairings,
        hub: hub.clone(),
        data_access: Arc::new(move || access),
        platform: Arc::new(FakeMac),
        binaries: Arc::new(Binaries::default()),
        browser_support: support.clone(),
        browser_data_dirs: Some(
            DIRS.iter()
                .map(|(b, rel)| (b.to_string(), support.join(rel)))
                .collect(),
        ),
        launch,
        tabs: Some(tabs.clone()),
        notifier: notes.clone(),
        activity: Some({
            let waits = waits.clone();
            Arc::new(move |worker: &str, on: bool| waits.lock().unwrap().push(json!([worker, on])))
        }),
        identity_browser: identity.clone(),
        native_host: NativeHost {
            launcher: home.path().join("bin/ferb-native-host"),
            exec_part: "/FX/venv/bin/python -P -m ferb.native_host.main".into(),
        },
        chrome_data: home
            .dir
            .path()
            .join("user/Library/Application Support/Google/Chrome"),
        new_handoff_id: Arc::new(|| "abcdef012345".to_string()),
        pair_load_wait_s: 0.0,
        pair_timeout_s: 60.0,
        handoff_poll_s: 0.0,
        state: Arc::new(State::default()),
    };
    let open_tabs = hub.tabs.clone();
    World {
        home,
        support,
        deps,
        hub,
        launched,
        answer,
        identity,
        notes,
        tabs,
        go,
        waits,
        open_tabs,
        store,
    }
}

/// The recorded `/FX/NN/home` is this test's home.
fn localize(text: &str, home: &Path) -> String {
    let re = regex::Regex::new(r"/FX/\d\d/home").unwrap();
    re.replace_all(text, regex::NoExpand(&home.to_string_lossy()))
        .into_owned()
}

/// The steps with every line written out (long ones from bodies) and its paths made this test's.
fn local_steps(steps: &[Value], home: &Path) -> Vec<Value> {
    steps
        .iter()
        .map(|st| {
            let mut st = st.clone();
            if let Some(lines) = st.get_mut("lines").and_then(Value::as_array_mut) {
                for l in lines {
                    for k in ["send", "recv"] {
                        if let Some(v) = l.get(k).cloned() {
                            l[k] = json!(localize(&line_text(&v), home));
                        }
                    }
                }
            }
            st
        })
        .collect()
}

/// Python's `float()` refused these `timeout_s`: an internal error there (-32603, or "ValueError: …" on the stream),
/// a refusal naming the parameter here (ENG-6).
fn bad_timeout(request: &str, expected: &str) -> Option<String> {
    let req: Value = serde_json::from_str(request).ok()?;
    let t = req["params"].get("timeout_s")?;
    let numeric = matches!(t, Value::Number(_))
        || t.as_str().is_some_and(|s| s.trim().parse::<f64>().is_ok());
    if numeric || !expected.contains("\"error\"") {
        return None;
    }
    let message = if req["method"] == "setup.browsers_pairing" {
        format!("FerbError: {BAD_TIMEOUT}")
    } else {
        BAD_TIMEOUT.to_string()
    };
    Some(ferb_py::json::dumps(
        &json!({"jsonrpc": "2.0", "id": req["id"], "error": {"code": -32004, "message": message}}),
    ))
}

/// Asserts a replayed part clean and adds up its (same, named) lines.
fn tally(counts: &mut (usize, usize), out: b_support::Outcome) {
    out.assert_clean();
    counts.0 += out.same.values().sum::<usize>();
    counts.1 += out.named.values().sum::<usize>();
}

fn keep(m: &str) -> bool {
    METHODS.contains(&m)
}

async fn start(w: &World) -> Daemon {
    let (host, deps) = (w.home.host.clone(), w.deps.clone());
    Daemon::start(move |server| browsers::register(server, &host, &deps)).await
}

fn tree(root: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
                out.insert(rel, std::fs::read_to_string(&p).unwrap());
            }
        }
    }
    out
}

fn installs_now(store: &MemInstalls) -> Value {
    let mut rows = store.0.lock().unwrap().clone();
    rows.sort_by(|a, b| (&a.1, &a.2).cmp(&(&b.1, &b.2)));
    json!(rows
        .iter()
        .map(|r| json!([r.0, r.1, r.2, r.3.as_str(), r.4]))
        .collect::<Vec<_>>())
}

/// The journal rows Python wrote, without its `daemon.internal_error` rows (the server's record of the crashes the
/// bad `timeout_s` calls were: refusals here).
fn python_journal(sc: &Value, home: &Path) -> Vec<Value> {
    sc["journal"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r[0] != "daemon.internal_error")
        .map(|r| serde_json::from_str(&localize(&r.to_string(), home)).unwrap())
        .collect()
}

/// Fixed against Python (ENG-6): a profile that isn't connected is loaded for its pairing page with no window
/// (`--no-startup-window`) instead of with Python's blank local wake page, so there is no page to close afterwards
/// (`wake_close`). Python's recording opened seven wake pages and closed two: the five pairings that didn't succeed left
/// theirs open. Python's launches and bridge calls with that one difference.
const WAKE_PAGE: &str = "http://127.0.0.1:9/wake#w1";

fn without_wake_pages(launched: &Value, calls: &Value) -> (Value, Value) {
    let launched: Vec<Value> = launched
        .as_array()
        .unwrap()
        .iter()
        .map(|argv| {
            let mut argv = argv.as_array().unwrap().clone();
            if argv.last().and_then(Value::as_str) == Some(WAKE_PAGE) {
                *argv.last_mut().unwrap() = json!("--no-startup-window");
            }
            json!(argv)
        })
        .collect();
    let calls: Vec<Value> = calls
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c[1] != "wake_close")
        .cloned()
        .collect();
    (json!(launched), json!(calls))
}

fn same_calls(python: &Value, rust: &[Value]) {
    let p = python.as_array().unwrap();
    assert_eq!(p.len(), rust.len(), "bridge calls: {python} vs {rust:?}");
    for (a, b) in p.iter().zip(rust) {
        assert_eq!(a[0], b[0]);
        assert_eq!(a[1], b[1]);
        assert_eq!(a[2], b[2]);
        assert_eq!(a[3].as_f64(), b[3].as_f64());
    }
}

// ---------- tests ----------

// decision: DMN-507
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_recorded_calls_without_full_disk_access_answer_as_python() {
    let file = fixture("daemon/rpc_transcripts.json");
    let sc = scenario(&file, "browsers");
    let w = world(&json!({}), false);
    let d = start(&w).await;
    let steps = local_steps(sc["steps"].as_array().unwrap(), w.home.path());
    let out = replay(&d, &steps, &keep, &[], &|_, _| None).await;
    out.assert_clean();
    assert_eq!(out.same.values().sum::<usize>(), 32, "{:?}", out.same); // 31 calls' lines and the hello
}

// decision: DMN-507, HAND-407, DMN-25
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_full_disk_access_only_paired_profiles_are_known_by_their_folder() {
    let file = fixture("daemon-b/browsers.json");
    let sc = scenario(&file, "without_access");
    let w = world(&sc["setup"], false);
    let d = start(&w).await;
    let steps = local_steps(sc["steps"].as_array().unwrap(), w.home.path());
    let out = replay(&d, &steps, &keep, &[], &|_, _| None).await;
    out.assert_clean();
    assert_eq!(out.same.values().sum::<usize>(), 15, "{:?}", out.same);
    assert_eq!(w.home.journal_rows(), python_journal(sc, w.home.path()));
    assert!(w.launched.lock().unwrap().is_empty());
    let config = std::fs::read_to_string(w.home.path().join("config.toml")).unwrap();
    assert_eq!(config, line_text(&sc["after"]["config"]));
    assert_eq!(installs_now(&w.store), sc["after"]["installs"]);
}

// decision: DMN-507, DMN-508, HAND-406, HAND-407, HAND-411, HAND-412, HAND-413, HAND-424, HAND-449, DMN-25, DMN-546
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_full_disk_access_every_call_answers_writes_and_launches_as_python() {
    let file = fixture("daemon-b/browsers.json");
    let sc = scenario(&file, "with_access");
    let w = world(&sc["setup"], true);
    let d = start(&w).await;
    let home = w.home.path().to_path_buf();
    let steps = local_steps(sc["steps"].as_array().unwrap(), &home);
    let wait = steps
        .iter()
        .position(|s| s["do"] == "wait")
        .expect("the handoff's wait");
    let hello: Vec<Value> = steps[..2].to_vec(); // connect + auth.hello, again for each later part
    let fixes = |req: &str, exp: &str| bad_timeout(req, exp);

    // the launcher answers pairing pages from call 28 to call 32, as the recorder's did
    let at = |id: u64| {
        steps
            .iter()
            .position(|s| {
                s["do"] == "call"
                    && serde_json::from_str::<Value>(&line_text(&s["lines"][0]["send"])).unwrap()
                        ["id"]
                        == id
            })
            .unwrap()
    };
    let (from, to) = (at(28), at(33));
    let mut counts = (0usize, 0usize);
    tally(
        &mut counts,
        replay(&d, &steps[..from], &keep, &[], &fixes).await,
    );
    w.answer.store(true, Ordering::SeqCst);
    let mut part: Vec<Value> = hello.clone();
    part.extend_from_slice(&steps[from..to]);
    tally(&mut counts, replay(&d, &part, &keep, &[], &fixes).await);
    w.answer.store(false, Ordering::SeqCst);
    let attached_after_pair = w.identity.attached();
    let mut part: Vec<Value> = hello.clone();
    part.extend_from_slice(&steps[to..wait]);
    tally(&mut counts, replay(&d, &part, &keep, &[], &fixes).await);

    // the handoff: the first look, then the owner's clicks while it waits, then the page lets them through
    let cx = Cx::new(&w.home.host, &w.deps);
    let job = tokio::spawn(async move {
        browsers::handoff(
            &cx,
            json!({"tab": 7}),
            "Verify you are human",
            "task-1",
            "w-1",
        )
        .await
    });
    for _ in 0..200 {
        if w.notes.0.lock().unwrap().len() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let shows_end = steps.len() - 1;
    let mut part: Vec<Value> = hello.clone();
    part.extend_from_slice(&steps[wait + 1..shows_end]);
    tally(&mut counts, replay(&d, &part, &keep, &[], &fixes).await);
    w.go.send(true).unwrap();
    let result = job.await.unwrap().unwrap().unwrap();
    let mut part: Vec<Value> = hello.clone();
    part.extend_from_slice(&steps[shows_end..]);
    tally(&mut counts, replay(&d, &part, &keep, &[], &fixes).await);

    // every line both ways; the four bad `timeout_s` answers differ by the named fix
    assert_eq!(
        counts,
        (46, 4),
        "(same, named): 46 recorded lines and the hello again for each later part"
    );
    let h = &sc["handoff"];
    assert_eq!(result, h["result"]);
    assert_eq!(json!(*w.tabs.looks.lock().unwrap()), h["looks"]);
    assert_eq!(json!(*w.tabs.shown.lock().unwrap()), h["shown"]);
    assert_eq!(json!(*w.notes.0.lock().unwrap()), h["notify"]);
    assert_eq!(json!(*w.waits.lock().unwrap()), h["waiting"]);

    assert_eq!(w.home.journal_rows(), python_journal(sc, &home));
    assert_eq!(
        json!(*w.launched.lock().unwrap()),
        without_wake_pages(&sc["launched"], &sc["bridge_calls"]).0
    );
    same_calls(
        &without_wake_pages(&sc["launched"], &sc["bridge_calls"]).1,
        &w.hub.calls.lock().unwrap(),
    );
    let after = &sc["after"];
    let config = std::fs::read_to_string(home.join("config.toml")).unwrap();
    assert_eq!(config, line_text(&after["config"]));
    let files: BTreeMap<String, String> = after["files"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), localize(&line_text(v), &home)))
        .collect();
    assert_eq!(tree(&w.support), files);
    assert_eq!(installs_now(&w.store), after["installs"]);
    assert_eq!(
        json!([attached_after_pair, w.identity.attached()]),
        after["identity_attached"]
    );
    assert!(w.home.events().is_empty());
}

/// The owner (2026-11-14): FERB closes the tabs it opened to pair once the profile is paired, never quits a browser
/// and never closes a tab it didn't open. Pair all over profiles that aren't connected: each is loaded with no window,
/// then gets its pairing page, which closes itself once it has handed over its token; nothing else opens, no command
/// closes any tab, and every profile is left with exactly the owner's own tabs.
// decision: HAND-520, HAND-411, HAND-412
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pair_all_leaves_each_profile_with_only_the_owners_tabs() {
    let file = fixture("daemon-b/browsers.json");
    let sc = scenario(&file, "with_access");
    let w = world(&sc["setup"], true);
    let owners: BTreeMap<String, Vec<String>> = [
        ("chrome/Default", vec!["https://mail.example.test/inbox"]),
        (
            "chrome/Profile 1",
            vec!["https://docs.example.test/a", "https://news.example.test/"],
        ),
        ("chrome/Profile 7", vec!["https://shop.example.test/cart"]),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.into_iter().map(String::from).collect()))
    .collect();
    *w.open_tabs.lock().unwrap() = owners.clone();
    w.answer.store(true, Ordering::SeqCst);
    let d = start(&w).await;
    let mut c = d.connect().await;
    c.send(r#"{"jsonrpc": "2.0", "id": 1, "method": "auth.hello", "params": {"role": "cli"}}"#)
        .await;
    c.recv().await.unwrap();
    c.send(r#"{"jsonrpc": "2.0", "id": 2, "method": "setup.browsers_pair_all", "params": {"timeout_s": 5}}"#)
        .await;
    let reply: Value = serde_json::from_str(&c.recv().await.unwrap()).unwrap();
    let results = reply["result"]["results"].as_array().unwrap().clone();
    let paired: Vec<String> = results
        .iter()
        .filter(|r| r["paired"] == true)
        .map(|r| {
            format!(
                "{}/{}",
                r["browser"].as_str().unwrap(),
                r["folder"].as_str().unwrap()
            )
        })
        .collect();
    assert!(!paired.is_empty(), "{reply}");

    // exactly two commands per profile it paired: the windowless load, then the pairing page; no other page
    let launched: Vec<Vec<String>> = w
        .launched
        .lock()
        .unwrap()
        .iter()
        .map(|a| serde_json::from_value(a.clone()).unwrap())
        .collect();
    assert_eq!(launched.len(), 2 * results.len(), "{launched:?}");
    for profile in &paired {
        let mine: Vec<&Vec<String>> = launched
            .iter()
            .filter(|a| launched_profile(a) == *profile)
            .collect();
        assert_eq!(mine.len(), 2, "{profile}: {mine:?}");
        assert_eq!(
            mine[0].last().unwrap(),
            "--no-startup-window",
            "{profile}: {mine:?}"
        );
        assert!(
            mine[1].last().unwrap().contains("/pair.html#token="),
            "{profile}: {mine:?}"
        );
    }
    // no browser was quit and no tab was closed by a command: the pairing pages closed themselves
    assert!(
        w.hub.calls.lock().unwrap().is_empty(),
        "{:?}",
        w.hub.calls.lock().unwrap()
    );
    let mut left = w.open_tabs.lock().unwrap().clone();
    left.retain(|_, tabs| !tabs.is_empty());
    assert_eq!(left, owners);
}
