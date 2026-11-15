//! HAND-522: a task's cleanup wakes a disconnected profile that holds its tabs the quiet way and closes them once the
//! profile's extension is back; only what still can't be confirmed is left (and closed at the next connection). The
//! keepalive pings while a task tab is open and stops after; a profile's disconnect and reconnect while a task holds
//! tabs in it reach the hub's hook. Through a scripted fake extension on in-memory connections (signed as the real
//! one signs), invented profiles, tokio's paused clock; the wake is a fake that never starts a program.

mod common;

use std::collections::HashSet;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_browser::bridge::{Bridge, Hooks, Hub, Ledger, TaskLink};
use ferb_browser::driver::{Driver, Kind, Tasks, Wake, CLEANUP_WAKE_S};
use ferb_browser::pairing::{InstallRow, InstallStore, PairingKey, Pairings, StoreError};
use ferb_browser::protocol::{sign, MIN_EXTENSION_VERSION, PROTOCOL_VERSION};
use ferb_browser::rules::{BrowserApps, BrowserInfo};
use ferb_core::interfaces::{BoxFuture, Clock};
use ferb_core::types::Identity;
use ferb_core::when::DateTime;
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::task::JoinHandle;

use common::Counter;

const KEY: [u8; 32] = [7; 32];

/// Two paired owner profiles: chrome/Default ("install-chrome") and brave/Work ("install-brave").
struct Installs(Mutex<Vec<(String, i64)>>);

fn profile_of(install: &str) -> Option<(&'static str, &'static str)> {
    match install {
        "install-chrome" => Some(("chrome", "Default")),
        "install-brave" => Some(("brave", "Work")),
        _ => None,
    }
}

impl InstallStore for Installs {
    fn row(&self, install_id: &str) -> Result<Option<InstallRow>, StoreError> {
        Ok(profile_of(install_id).map(|(b, f)| InstallRow {
            browser: b.into(),
            folder: f.into(),
            identity: Identity::Owner,
        }))
    }
    fn install_for(&self, browser: &str, folder: &str) -> Result<Option<String>, StoreError> {
        Ok(["install-chrome", "install-brave"]
            .into_iter()
            .find(|i| profile_of(i) == Some((browser, folder)))
            .map(String::from))
    }
    fn installs(&self) -> Result<Vec<String>, StoreError> {
        Ok(vec!["install-brave".into(), "install-chrome".into()])
    }
    fn key(&self, install_id: &str) -> Result<Option<PairingKey>, StoreError> {
        Ok(profile_of(install_id).map(|_| PairingKey::new(KEY.to_vec())))
    }
    fn key_and_seq(&self, install_id: &str) -> Result<Option<(PairingKey, i64)>, StoreError> {
        let seq = self
            .0
            .lock()
            .unwrap()
            .iter()
            .find(|(i, _)| i == install_id)
            .map_or(0, |(_, s)| *s);
        Ok(profile_of(install_id).map(|_| (PairingKey::new(KEY.to_vec()), seq)))
    }
    fn version(&self, _: &str) -> Result<Option<String>, StoreError> {
        Ok(Some(MIN_EXTENSION_VERSION.into()))
    }
    fn set_version(&self, _: &str, _: &str) -> Result<(), StoreError> {
        Ok(())
    }
    fn set_last_seq(&self, install_id: &str, seq: i64) -> Result<(), StoreError> {
        let mut rows = self.0.lock().unwrap();
        match rows.iter_mut().find(|(i, _)| i == install_id) {
            Some(r) => r.1 = seq,
            None => rows.push((install_id.into(), seq)),
        }
        Ok(())
    }
    fn unpair(&self, _: &str, _: &str) -> Result<bool, StoreError> {
        Ok(false)
    }
    fn pair(&self, _: &str, _: &InstallRow, _: &PairingKey, _: &str) -> Result<(), StoreError> {
        Ok(())
    }
}

struct Clocked;

impl Clock for Clocked {
    fn now(&self) -> DateTime {
        ferb_core::when::parse("2026-01-01T09:00:00+00:00").unwrap()
    }
    fn monotonic(&self) -> f64 {
        0.0
    }
    fn sleep_until(&self, _: DateTime) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

struct NoLedger;

impl Ledger for NoLedger {
    fn state(&self, _: &str) -> Option<String> {
        None
    }
    fn reconcile(&self, _: &str, _: &str, _: Vec<String>) {}
}

struct Apps;

impl BrowserApps for Apps {
    fn known(&self, _: &str) -> bool {
        true
    }
    fn browser(&self, _: &str) -> Option<BrowserInfo> {
        None
    }
}

/// What each fake connection was sent: (connection number, op, args).
type Sent = Arc<Mutex<Vec<(usize, String, Value)>>>;

/// The fake extension of every profile: one connection at a time per profile, its hello listing `tabs`.
struct Ext {
    hub: Arc<Hub>,
    sent: Sent,
    seq: AtomicI64,
    conns: Mutex<Vec<(String, JoinHandle<()>)>>,
}

impl Ext {
    fn frame(&self, install: &str, mtype: &str, body: &Value) -> String {
        let body_json = ferb_py::json::dumps(body);
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let mac = sign(&KEY, install, seq, mtype, &body_json);
        format!(
            "{}\n",
            json!({"install_id": install, "type": mtype, "body_json": body_json, "seq": seq, "mac": mac})
        )
    }

    /// A new connection for `install` whose hello lists `tabs` ({tab_id, task, url, borrowed}); it answers every
    /// command as the extension does when it succeeds.
    fn connect(self: &Arc<Self>, install: &str, tabs: Value) {
        let (ext_w, hub_r) = tokio::io::duplex(1 << 20);
        let (hub_w, ext_r) = tokio::io::duplex(1 << 20);
        tokio::spawn(Arc::clone(&self.hub).serve(BufReader::new(hub_r), hub_w, Some(4242)));
        let me = Arc::clone(self);
        let number = self.conns.lock().unwrap().len() + 1;
        let install_id = install.to_string();
        let job = tokio::spawn(async move {
            let mut w = ext_w;
            let hello = json!({"version": MIN_EXTENSION_VERSION, "protocol": PROTOCOL_VERSION, "browser": "chrome",
                               "paired": true, "nonce": format!("nonce-{number}"), "tabs": tabs});
            w.write_all(me.frame(&install_id, "hello", &hello).as_bytes())
                .await
                .unwrap();
            let mut lines = BufReader::new(ext_r).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let v: Value = serde_json::from_str(&line).unwrap();
                if v["type"] != "cmd" {
                    continue;
                }
                let cmd: Value = serde_json::from_str(v["body"].as_str().unwrap()).unwrap();
                let op = cmd["op"].as_str().unwrap().to_string();
                me.sent
                    .lock()
                    .unwrap()
                    .push((number, op.clone(), cmd["args"].clone()));
                let value = match op.as_str() {
                    "close" => json!({"closed": true}),
                    "release_tab" => json!({"released": true}),
                    _ => json!({}),
                };
                let result =
                    json!({"id": cmd["id"], "gen": cmd["gen"], "status": "ok", "value": value});
                if w.write_all(me.frame(&install_id, "result", &result).as_bytes())
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        self.conns.lock().unwrap().push((install.to_string(), job));
    }

    /// The profile's extension goes away (its connection ends).
    fn drop_profile(&self, install: &str) {
        for (i, job) in self.conns.lock().unwrap().iter() {
            if i == install {
                job.abort();
            }
        }
    }

    fn sent_on(&self, conn: usize) -> Vec<(String, Value)> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, _, _)| *n == conn)
            .map(|(_, op, args)| (op.clone(), args.clone()))
            .collect()
    }

    fn pings_on(&self, conn: usize) -> usize {
        self.sent_on(conn)
            .iter()
            .filter(|(op, _)| op == "ping")
            .count()
    }
}

/// The wake: records each start; when `back` holds the profile's tabs, its extension connects again with them.
struct FakeWake {
    ext: Mutex<Option<Arc<Ext>>>,
    starts: Mutex<Vec<(String, String, bool)>>,
    back: Mutex<Vec<(String, Value)>>,
}

impl Wake for FakeWake {
    fn start(&self, browser: &str, folder: &str, window: bool) {
        self.starts
            .lock()
            .unwrap()
            .push((browser.into(), folder.into(), window));
        let install = if browser == "chrome" {
            "install-chrome"
        } else {
            "install-brave"
        };
        let tabs = self
            .back
            .lock()
            .unwrap()
            .iter()
            .find(|(i, _)| i == install)
            .map(|(_, t)| t.clone());
        if let (Some(tabs), Some(ext)) = (tabs, self.ext.lock().unwrap().clone()) {
            // the profile loads (no window) and its extension says hello a moment later
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(3)).await;
                ext.connect(install, tabs);
            });
        }
    }
}

struct World {
    driver: Arc<Driver>,
    ext: Arc<Ext>,
    wake: Arc<FakeWake>,
    running: Arc<Mutex<HashSet<String>>>,
    links: Arc<Mutex<Vec<(String, TaskLink)>>>,
}

fn world() -> World {
    world_choosing(Arc::new(|_| {
        Ok(("chrome".to_string(), "Default".to_string()))
    }))
}

/// `world` with the resolver `choose`.
fn world_choosing(choose: ferb_browser::driver::Choose) -> World {
    let random = Arc::new(Counter::new());
    let pairings = Arc::new(Pairings::new(
        Arc::new(Installs(Mutex::new(vec![]))),
        Arc::new(Clocked),
        random.clone(),
    ));
    let hub = Hub::new(
        pairings,
        Arc::new(NoLedger),
        Arc::new(|_: &str, _: Map<String, Value>| {}),
        Arc::new(|| "dev-1".to_string()),
        random.clone(),
        Arc::new(Apps),
        Arc::new(|_| Some("com.google.Chrome".to_string())),
    );
    let links: Arc<Mutex<Vec<(String, TaskLink)>>> = Arc::default();
    let l = links.clone();
    hub.set_hooks(Hooks {
        on_task_link: Arc::new(move |br: &Arc<Bridge>, link: &TaskLink| {
            l.lock().unwrap().push((br.name(), link.clone()))
        }),
        ..Hooks::default()
    });
    let running: Arc<Mutex<HashSet<String>>> =
        Arc::new(Mutex::new(HashSet::from(["task-a".to_string()])));
    let r = running.clone();
    let tasks = Tasks {
        running: Arc::new(move |t: &str| r.lock().unwrap().contains(t)),
        ..Tasks::default()
    };
    let wake = Arc::new(FakeWake {
        ext: Mutex::new(None),
        starts: Mutex::default(),
        back: Mutex::default(),
    });
    let driver = Driver::new(
        hub.clone(),
        Kind::Owner {
            choose,
            input: None,
        },
        tasks,
        Some(wake.clone() as Arc<dyn Wake>),
        random,
    );
    let ext = Arc::new(Ext {
        hub,
        sent: Arc::default(),
        seq: AtomicI64::new(0),
        conns: Mutex::default(),
    });
    *wake.ext.lock().unwrap() = Some(ext.clone());
    World {
        driver,
        ext,
        wake,
        running,
        links,
    }
}

fn tabs(list: &[(i64, &str, bool)]) -> Value {
    json!(list
        .iter()
        .map(|(id, task, borrowed)| json!({"tab_id": id, "task": task, "url": format!("https://example.com/{id}"),
                                           "borrowed": borrowed}))
        .collect::<Vec<_>>())
}

/// Lets the connections' tasks run (paused clock: no real wait).
async fn settle() {
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
}

// decision: HAND-522
#[tokio::test(start_paused = true)]
async fn cleanup_wakes_a_disconnected_profile_and_closes_only_the_tabs_ferb_opened() {
    let w = world();
    // task-a holds two tabs in chrome/Default: 11 FERB opened, 12 the owner's own tab it borrowed
    w.ext.connect(
        "install-chrome",
        tabs(&[(11, "task-a", false), (12, "task-a", true)]),
    );
    settle().await;
    assert_eq!(w.driver.tabs().len(), 2);

    // the keepalive pings while FERB's tab is open (every 15 s)
    tokio::time::sleep(Duration::from_secs(31)).await;
    assert!(w.ext.pings_on(1) >= 2, "{:?}", w.ext.sent_on(1));

    // the profile's connection drops while task-a holds its tabs: the hook hears of it
    w.ext.drop_profile("install-chrome");
    settle().await;
    assert_eq!(
        w.links.lock().unwrap().clone(),
        vec![(
            "chrome/Default".to_string(),
            TaskLink {
                connected: false,
                tasks: vec!["task-a".into()],
                tabs: 2,
                gen: 1
            }
        )]
    );

    // the task ends: cleanup wakes the profile (no window), its extension comes back listing both tabs
    w.wake.back.lock().unwrap().push((
        "install-chrome".into(),
        tabs(&[(11, "task-a", false), (12, "task-a", true)]),
    ));
    let began = tokio::time::Instant::now();
    let released = w.driver.release("task-a").await.expect("all confirmed");
    assert!(began.elapsed() < Duration::from_secs_f64(CLEANUP_WAKE_S));
    assert_eq!(
        w.wake.starts.lock().unwrap().clone(),
        vec![("chrome".to_string(), "Default".to_string(), false)]
    );
    let shown: Vec<(Value, Value, Value)> = released
        .iter()
        .map(|r| (r["tab"].clone(), r["closed"].clone(), r["released"].clone()))
        .collect();
    assert_eq!(
        shown,
        vec![
            (json!("t1"), json!(true), json!(true)),
            (json!("t2"), json!(false), json!(true))
        ]
    );
    // on the new connection: FERB's tab closed, the owner's tab only let go of, never closed
    let acts: Vec<(String, Value)> = w
        .ext
        .sent_on(2)
        .into_iter()
        .filter(|(op, _)| op != "ping")
        .collect();
    assert_eq!(
        acts,
        vec![
            ("close".to_string(), json!({"tab_id": 11})),
            (
                "release_tab".to_string(),
                json!({"tab_id": 12, "keep_open": false})
            )
        ]
    );
    // the reconnect while task-a held its tabs is told too
    assert_eq!(
        w.links.lock().unwrap().last().cloned(),
        Some((
            "chrome/Default".to_string(),
            TaskLink {
                connected: true,
                tasks: vec!["task-a".into()],
                tabs: 2,
                gen: 2
            }
        ))
    );

    // no task tab is open any more: the keepalive stops
    w.running.lock().unwrap().remove("task-a");
    let pings = w.ext.pings_on(2);
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(w.ext.pings_on(2), pings);
}

// decision: HAND-522
#[tokio::test(start_paused = true)]
async fn what_a_woken_profile_cant_confirm_is_left_and_closed_at_its_next_connection() {
    let w = world();
    w.ext
        .connect("install-chrome", tabs(&[(11, "task-a", false)]));
    w.ext
        .connect("install-brave", tabs(&[(21, "task-a", false)]));
    settle().await;
    assert_eq!(w.driver.tabs().len(), 2);
    w.ext.drop_profile("install-chrome");
    w.ext.drop_profile("install-brave");
    settle().await;

    // neither profile comes back: both are woken quietly at once, and the cleanup waits one bound, not one each
    let began = tokio::time::Instant::now();
    let err = w.driver.release("task-a").await.expect_err("unconfirmed");
    let waited = began.elapsed();
    assert!(
        waited >= Duration::from_secs_f64(CLEANUP_WAKE_S)
            && waited < Duration::from_secs_f64(CLEANUP_WAKE_S + 1.0),
        "{waited:?}"
    );
    let mut starts = w.wake.starts.lock().unwrap().clone();
    starts.sort();
    assert_eq!(
        starts,
        vec![
            ("brave".to_string(), "Work".to_string(), false),
            ("chrome".to_string(), "Default".to_string(), false)
        ]
    );
    let left = err.facts().unwrap()["tabs"].as_array().unwrap().clone();
    assert_eq!(left.len(), 2, "{left:?}");
    assert!(left.iter().all(|t| t["borrowed"] == json!(false)));

    // the task is over; chrome/Default connects again later: its leftover is closed then (queued, as before)
    w.running.lock().unwrap().remove("task-a");
    w.ext
        .connect("install-chrome", tabs(&[(11, "task-a", false)]));
    settle().await;
    let acts: Vec<(String, Value)> = w
        .ext
        .sent_on(3)
        .into_iter()
        .filter(|(op, _)| op != "ping")
        .collect();
    assert_eq!(acts, vec![("close".to_string(), json!({"tab_id": 11}))]);
}

// decision: HAND-522
/// A task already gone from the running list when its cleanup runs: the woken profile's hello leaves its tabs to the
/// cleanup (one close each), instead of closing them a second time itself.
#[tokio::test(start_paused = true)]
async fn a_woken_profiles_hello_leaves_the_cleanups_tabs_to_the_cleanup() {
    let w = world();
    w.ext
        .connect("install-chrome", tabs(&[(11, "task-a", false)]));
    settle().await;
    w.ext.drop_profile("install-chrome");
    settle().await;
    w.running.lock().unwrap().remove("task-a");
    w.wake
        .back
        .lock()
        .unwrap()
        .push(("install-chrome".into(), tabs(&[(11, "task-a", false)])));
    let released = w.driver.release("task-a").await.expect("confirmed");
    assert_eq!(released.len(), 1);
    assert_eq!(released[0]["closed"], json!(true));
    settle().await;
    let acts: Vec<(String, Value)> = w
        .ext
        .sent_on(2)
        .into_iter()
        .filter(|(op, _)| op != "ping")
        .collect();
    assert_eq!(acts, vec![("close".to_string(), json!({"tab_id": 11}))]);
}

/// HAND-562: with several of the owner's profiles paired and none picked, the resolver can't choose alone; a print of
/// a local HTML file then goes to the profile the task already works in (its newest open tab's), named as
/// browser.open takes it; with no open tab, or when the resolver chooses alone, nothing is guessed. Live 2026-11-15:
/// browser.pdf html_path failed five times with "select a profile from the task context".
// decision: HAND-562
#[tokio::test(start_paused = true)]
async fn a_local_print_goes_to_the_profile_the_task_already_works_in() {
    let w = world_choosing(Arc::new(|named: Option<&str>| match named {
        Some("brave/Work") => Ok(("brave".to_string(), "Work".to_string())),
        Some("chrome/Default") => Ok(("chrome".to_string(), "Default".to_string())),
        _ => Err(ferb_browser::Error::refused(
            "select a profile from the task context; available: brave/Work, chrome/Default",
        )),
    }));
    assert_eq!(w.driver.in_use_profile("task-a"), None, "no tab yet");
    w.ext.connect(
        "install-brave",
        tabs(&[(21, "task-a", false), (22, "task-b", false)]),
    );
    settle().await;
    assert_eq!(
        w.driver.in_use_profile("task-a").as_deref(),
        Some("brave/Work")
    );
    assert_eq!(
        w.driver.in_use_profile("task-c"),
        None,
        "another task's tab is not this one's"
    );
    let profile = w.driver.in_use_profile("task-a");
    // the print's tab is asked of brave/Work's extension (this fake answers with no tab)
    let _ = w
        .driver
        .open_for(
            "http://127.0.0.1:1/print",
            "task-a",
            profile.as_deref(),
            None,
            false,
        )
        .await;
    assert!(
        w.ext.sent_on(1).iter().any(|(op, _)| op == "open"),
        "{:?}",
        w.ext.sent_on(1)
    );

    // when the resolver chooses alone, that choice stands (browser.open's)
    let w = world();
    w.ext
        .connect("install-brave", tabs(&[(31, "task-a", false)]));
    settle().await;
    assert_eq!(w.driver.in_use_profile("task-a"), None);
}
