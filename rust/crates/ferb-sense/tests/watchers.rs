//! Each watcher's mapping from what macOS reports to observations (`ferb/sense/watchers/*`) against
//! `rust/fixtures/sense/watchers.json`, over invented events and fakes: no window, pasteboard, input tap,
//! Accessibility or macOS database is touched.

#![allow(clippy::type_complexity)]

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use common::{fixture, input, input_value, tmp, Diffs};
use ferb_sense::capture::mark_clipboard_write;
use ferb_sense::exclusions::Exclusions;
use ferb_sense::manager::Emit;
use ferb_sense::observation::{Observation, Redact};
use ferb_sense::watchers::accessibility::{accessibility_observation, element_facts, AxElement};
use ferb_sense::watchers::app_installs::{AppInfo, AppInstallsWatcher};
use ferb_sense::watchers::base::{Focused, FsEvents, OnFsEvent};
use ferb_sense::watchers::browser_tabs::{
    front_tab_script, parse_front_tab, parse_windows, scripts, tab_observation, window_tab,
    windows_script, ScriptedTabsWatcher, ScriptedWindow,
};
use ferb_sense::watchers::clipboard::{access, ClipboardWatcher, Pasteboard};
use ferb_sense::watchers::files::{change_of, noise, valid_folders, FilesWatcher};
use ferb_sense::watchers::idle::IdleWatcher;
use ferb_sense::watchers::notifications::NotificationDatabase;
use ferb_sense::watchers::presence::{MacPresence, Session};
use serde_json::{json, Map, Value};

fn obs(o: &Observation) -> Value {
    json!([o.source, o.data, o.hints])
}

struct Table<'a> {
    table: &'a Map<String, Value>,
    name: String,
}

impl AxElement for Table<'_> {
    fn attr(&self, name: &str) -> Option<Value> {
        self.table
            .get(&self.name)?
            .get(name)
            .filter(|v| !v.is_null())
            .cloned()
    }
    fn window(&self) -> Option<Box<dyn AxElement + '_>> {
        let w = self.attr("AXWindow")?;
        Some(Box::new(Table {
            table: self.table,
            name: w.as_str()?.to_string(),
        }))
    }
    fn hash_id(&self) -> String {
        "hash".into()
    }
}

struct NoStream;
impl FsEvents for NoStream {
    fn start(&self, _: &[String], _: OnFsEvent) -> Option<Box<dyn Send + Sync>> {
        None
    }
}

fn focused(v: &Value) -> Option<Focused> {
    let a = v.as_array()?;
    Some(Focused {
        pid: a[0].as_i64()?,
        bundle_id: a[1].as_str()?.into(),
        name: a[2].as_str()?.into(),
    })
}

#[derive(Default)]
struct Board {
    count: i64,
    kinds: Vec<String>,
    text: Option<String>,
    behavior: Option<i64>,
    bump: bool,
}

struct Shared(Mutex<Board>);
impl Pasteboard for Shared {
    fn change_count(&self) -> i64 {
        self.0.lock().unwrap().count
    }
    fn access_behavior(&self) -> Option<i64> {
        self.0.lock().unwrap().behavior
    }
    fn types(&self) -> Vec<String> {
        self.0.lock().unwrap().kinds.clone()
    }
    fn read_string(&self) -> Option<String> {
        let mut b = self.0.lock().unwrap();
        if b.bump {
            b.count += 1;
        }
        b.text.clone()
    }
}

// decision: SENSE-1, SENSE-5, SENSE-401, SENSE-403, SENSE-404, SENSE-201
#[test]
fn watchers_map_events_as_python_does() {
    let f = fixture("watchers.json");
    let mut d = Diffs::default();
    let redactor = Arc::new(ferb_sanitize::Redactor::default());
    for c in f["accessibility"].as_array().unwrap() {
        let b = c[0].as_str().unwrap();
        let o = accessibility_observation(
            b,
            c[1].as_str().unwrap(),
            c[2].as_i64().unwrap(),
            c[3].as_str().unwrap(),
            c[4].as_object().unwrap(),
            redactor.as_ref(),
            common::known_browser(b),
        );
        d.check(&format!("accessibility {c}"), &c[5], &json!(o.data));
    }
    let table = f["element_table"].as_object().unwrap().clone();
    for c in f["element_facts"].as_array().unwrap() {
        let el = Table {
            table: &table,
            name: c[0].as_str().unwrap().into(),
        };
        d.check(
            &format!("element_facts {c}"),
            &c[2],
            &json!(element_facts(c[1].as_str().unwrap(), &el)),
        );
    }

    // app installs
    for p in ["Apps/Utilities", "Apps/Here.app"] {
        std::fs::create_dir_all(tmp().join(p)).unwrap();
    }
    let here = input("<TMP>/Apps/Here.app");
    let folders = vec![
        input("<TMP>/Apps"),
        input("<TMP>/Apps/Utilities"),
        input("<TMP>/NotThere"),
    ];
    let w = AppInstallsWatcher::new(
        Arc::new(|_, _, _| {}),
        &folders,
        Arc::new(NoStream),
        Arc::new(move |p: &str| {
            (p == here).then(|| AppInfo {
                bundle_id: "com.example.here".into(),
                name: "Here".into(),
                version: "1.0".into(),
                path: p.into(),
                scriptable: false,
            })
        }),
    );
    d.check(
        "app install folders",
        &f["app_install_folders"],
        &json!(w.folders),
    );
    let installs = f["app_installs"].as_array().unwrap();
    for (n, c) in installs.iter().enumerate() {
        if n == installs.len() - 1 {
            std::fs::create_dir_all(tmp().join("Apps/Unknown.app")).unwrap();
        }
        let got = w
            .observe(
                &input(c[0].as_str().unwrap()),
                c[1].as_u64().unwrap() as u32,
            )
            .map(|(o, _)| obs(&o));
        d.check(&format!("app install {c}"), &c[2], &json!(got));
    }

    // browser tabs
    for c in f["tab_observation"].as_array().unwrap() {
        let a = &c[0];
        let o = tab_observation(
            a[0].as_str().unwrap(),
            a[1].as_str().unwrap(),
            a[2].as_bool().unwrap(),
            a[3].as_str().unwrap(),
            a[4].as_str(),
            a[5].as_str(),
            a[6].as_bool(),
            a[7].as_str(),
        );
        d.check(
            &format!("tab_observation {a}"),
            &c[1],
            &json!(o.as_ref().map(obs)),
        );
    }
    let mine: Map<String, Value> = scripts()
        .iter()
        .map(|(b, k, body)| (b.to_string(), json!([k, body])))
        .collect();
    d.check("scripts", &f["scripts"], &Value::Object(mine));
    let ws: Map<String, Value> = scripts()
        .iter()
        .map(|(b, _, _)| (b.to_string(), json!(windows_script(b))))
        .collect();
    d.check("window scripts", &f["window_scripts"], &Value::Object(ws));
    let fs: Map<String, Value> = scripts()
        .iter()
        .map(|(b, _, _)| (b.to_string(), json!(front_tab_script(b))))
        .collect();
    d.check(
        "front tab scripts",
        &f["front_tab_script"],
        &Value::Object(fs),
    );
    let win = |w: &ScriptedWindow| {
        json!([
            w.bounds.map(|b| json!([b.0, b.1, b.2, b.3])),
            w.name,
            w.url,
            w.title
        ])
    };
    for (name, p) in f["parsed"].as_object().unwrap() {
        let (windows, front) = match p["stdout"].as_str() {
            None => (Value::Null, Value::Null),
            Some(out) => (
                Value::Array(parse_windows(out).iter().map(win).collect()),
                json!(parse_front_tab(out).map(|(u, t)| json!([u, t]))),
            ),
        };
        d.check(
            &format!("parsed {name}"),
            &json!([p["windows"], p["front"]]),
            &json!([windows, front]),
        );
    }
    let sets: HashMap<String, Vec<ScriptedWindow>> = f["window_sets"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| {
            let list = v
                .as_array()
                .unwrap()
                .iter()
                .map(|w| ScriptedWindow {
                    bounds: w[0].as_array().map(|b| {
                        (
                            b[0].as_f64().unwrap(),
                            b[1].as_f64().unwrap(),
                            b[2].as_f64().unwrap(),
                            b[3].as_f64().unwrap(),
                        )
                    }),
                    name: w[1].as_str().unwrap().into(),
                    url: w[2].as_str().unwrap().into(),
                    title: w[3].as_str().unwrap().into(),
                })
                .collect();
            (k.clone(), list)
        })
        .collect();
    for c in f["window_tab"].as_array().unwrap() {
        // the invented windows' bounds were whole numbers in Python (ints); written as such
        let whole = |w: &ScriptedWindow| {
            json!([
                w.bounds
                    .map(|b| json!([b.0 as i64, b.1 as i64, b.2 as i64, b.3 as i64])),
                w.name,
                w.url,
                w.title
            ])
        };
        let got = match window_tab(
            &sets[c[0].as_str().unwrap()],
            (0.0, 0.0, 800.0, 600.0),
            c[1].as_str().unwrap(),
        ) {
            Ok(w) => whole(&w),
            Err(e) if e.ambiguous => json!({"ambiguous": e.message}),
            Err(e) => json!({"lookup": e.message}),
        };
        d.check(&format!("window_tab {c}"), &c[2], &got);
    }
    let front: Arc<Mutex<Option<Focused>>> = Arc::default();
    let reads: Arc<Mutex<HashMap<String, Option<(String, String)>>>> = Arc::new(Mutex::new(
        [
            ("com.apple.Safari", Some(("https://s.example/", "S"))),
            ("com.google.Chrome", Some(("https://c.example/p", "C"))),
            ("com.brave.Browser", None),
        ]
        .into_iter()
        .map(|(k, v)| {
            (
                k.to_string(),
                v.map(|(a, b)| (a.to_string(), b.to_string())),
            )
        })
        .collect(),
    ));
    let (fr, rd) = (front.clone(), reads.clone());
    let watcher = ScriptedTabsWatcher::new(
        Arc::new(|b: &str| b != "com.vivaldi.Vivaldi"),
        Arc::new(|br: &str| br == "chrome"),
        Arc::new(move || fr.lock().unwrap().clone()),
        Arc::new(move |b: &str| Ok(rd.lock().unwrap().get(b).cloned().flatten())),
    );
    let steps = f["scripted_tabs"].as_array().unwrap();
    for (n, c) in steps.iter().enumerate() {
        if n == steps.len() - 1 {
            reads.lock().unwrap().insert(
                "com.apple.Safari".into(),
                Some(("https://s.example/2".into(), "S2".into())),
            );
        }
        *front.lock().unwrap() = focused(&c[0]);
        let got = watcher.check().unwrap().as_ref().map(obs);
        d.check(&format!("scripted tabs {}", c[0]), &c[1], &json!(got));
    }

    // clipboard
    for c in f["access"].as_array().unwrap() {
        d.check(
            &format!("access {}", c[0]),
            &c[1],
            &json!(access(c[0].as_i64())),
        );
    }
    let board = Arc::new(Shared(Mutex::new(Board {
        count: 1,
        kinds: vec!["public.utf8-plain-text".into()],
        text: Some("hello".into()),
        behavior: Some(2),
        bump: false,
    })));
    let focus: Arc<Mutex<Option<Focused>>> = Arc::new(Mutex::new(Some(Focused {
        pid: 9,
        bundle_id: "com.apple.Notes".into(),
        name: "Notes".into(),
    })));
    let fc = focus.clone();
    let cw = ClipboardWatcher::new(
        redactor.clone() as Arc<dyn Redact>,
        Arc::new(|b: &str, _: &str| b == "com.example.vault"),
        Arc::new(|_: &str| {}),
        board.clone(),
        Arc::new(move || fc.lock().unwrap().clone()),
    );
    for st in f["clipboard_steps"]["log"].as_array().unwrap() {
        let s = &st["step"];
        {
            let mut b = board.0.lock().unwrap();
            if let Some(c) = s.get("count") {
                b.count = c.as_i64().unwrap();
            }
            if let Some(t) = s.get("text") {
                b.text = t.as_str().map(String::from);
            }
            if let Some(x) = s.get("behavior") {
                b.behavior = x.as_i64();
            }
            if let Some(k) = s.get("kinds") {
                b.kinds = k
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().into())
                    .collect();
            }
            if let Some(x) = s.get("bump") {
                b.bump = x.as_bool().unwrap();
            }
        }
        if let Some(fv) = s.get("focus") {
            *focus.lock().unwrap() = fv.as_array().map(|a| Focused {
                pid: 9,
                bundle_id: a[0].as_str().unwrap().into(),
                name: a[1].as_str().unwrap().into(),
            });
        }
        if let Some(m) = s.get("mark") {
            mark_clipboard_write(m[0].as_i64().unwrap(), m[1].as_str());
        }
        let emitted = Mutex::new(Vec::new());
        let reported = Mutex::new(Vec::new());
        cw.step(&|o| emitted.lock().unwrap().push(obs(&o)), &|r| {
            reported.lock().unwrap().push(r.to_string())
        });
        let (last, state) = cw.seen();
        let got = json!({"step": s, "emitted": *emitted.lock().unwrap(), "reported": *reported.lock().unwrap(),
                         "last": last, "state": state});
        d.check(&format!("clipboard {s}"), st, &got);
    }

    // files
    let home = tmp().join("home");
    for p in ["Docs/sub", ".ssh"] {
        std::fs::create_dir_all(home.join(p)).unwrap();
    }
    std::fs::write(home.join("file.txt"), "x").unwrap();
    let _ = std::os::unix::fs::symlink(home.join("Docs"), home.join("docs-link"));
    std::fs::create_dir_all(tmp().join("outside")).unwrap();
    let excl = Exclusions::build::<&str, &str, &str>(&[], &[], &["~/.ssh", "~/Docs/sub"]);
    let asked: Vec<String> = [
        "~/Docs",
        "~/Docs/sub",
        "~/.ssh",
        "~/missing",
        "~/file.txt",
        "<TMP>/outside",
        "~",
        "~/docs-link",
        "~/Docs/../Docs",
    ]
    .iter()
    .map(|s| input(s))
    .collect();
    let (ok, problems) = valid_folders(&asked, &excl, &home.to_string_lossy());
    d.check("valid_folders", &f["valid_folders"], &json!([ok, problems]));
    std::fs::create_dir_all(tmp().join("fs")).unwrap();
    std::fs::write(tmp().join("fs/exists"), "x").unwrap();
    for c in f["change_of"].as_array().unwrap() {
        let p = tmp().join("fs").join(c[0].as_str().unwrap());
        d.check(
            &format!("change_of {c}"),
            &c[2],
            &json!(change_of(
                &p.to_string_lossy(),
                c[1].as_u64().unwrap() as u32
            )),
        );
    }
    for c in f["noise"].as_array().unwrap() {
        d.check(
            &format!("noise {c}"),
            &c[1],
            &json!(noise(c[0].as_str().unwrap())),
        );
    }
    let mono = Arc::new(Mutex::new(100.0f64));
    let m = mono.clone();
    let fw = FilesWatcher::new(
        Arc::new(Vec::new),
        Arc::new(NoStream),
        Arc::new(move || *m.lock().unwrap()),
    );
    for c in f["files_observe"].as_array().unwrap() {
        *mono.lock().unwrap() += c[2].as_f64().unwrap();
        let p = tmp().join("fs").join(c[0].as_str().unwrap());
        let got = fw
            .observe(&p.to_string_lossy(), c[1].as_u64().unwrap() as u32)
            .map(|o| json!([o.source, o.data]));
        d.check(&format!("files observe {c}"), &c[3], &json!(got));
    }

    // idle
    let state: Arc<Mutex<(Option<f64>, bool, f64)>> = Arc::new(Mutex::new((None, true, 1000.0)));
    let (a, b, c2) = (state.clone(), state.clone(), state.clone());
    let iw = IdleWatcher::new(
        Arc::new(move || a.lock().unwrap().0),
        Arc::new(move || b.lock().unwrap().1),
        120.0,
        Arc::new(move || c2.lock().unwrap().2),
    );
    for c in f["idle"].as_array().unwrap() {
        *state.lock().unwrap() = (
            c[0].as_f64(),
            c[1].as_bool().unwrap(),
            c[2].as_f64().unwrap(),
        );
        let em: Arc<Mutex<Vec<Value>>> = Arc::default();
        let e = em.clone();
        let emit: Emit = Arc::new(move |o| e.lock().unwrap().push(obs(&o)));
        iw.check(&emit);
        d.check(&format!("idle {c}"), &c[3], &json!(*em.lock().unwrap()));
    }

    // Notification Center's database (an invented copy)
    let dir = tmp().join("usernoted");
    std::fs::create_dir_all(&dir).unwrap();
    let ndb = dir.join("db");
    {
        let c = rusqlite::Connection::open(&ndb).unwrap();
        c.execute_batch(
            "CREATE TABLE app (app_id INTEGER PRIMARY KEY, identifier TEXT);
             CREATE TABLE record (rec_id INTEGER PRIMARY KEY, app_id INTEGER, uuid BLOB, data BLOB, delivered_date REAL);
             INSERT INTO app VALUES (1,'com.chat'),(2,'com.example.vault'),(3,'com.mail');",
        )
        .unwrap();
        for r in f["notification_records"].as_array().unwrap() {
            let id = r[0].as_i64().unwrap();
            let uuid: rusqlite::types::Value = if id == 3 {
                rusqlite::types::Value::Text(r[2].as_str().unwrap().into())
            } else {
                rusqlite::types::Value::Blob(hex::decode(r[2].as_str().unwrap()).unwrap())
            };
            c.execute(
                "INSERT INTO record (rec_id, app_id, uuid, data) VALUES (?,?,?,?)",
                rusqlite::params![
                    id,
                    r[1].as_i64().unwrap(),
                    uuid,
                    hex::decode(r[3].as_str().unwrap()).unwrap()
                ],
            )
            .unwrap();
        }
    }
    let reader = NotificationDatabase::new(
        ndb.clone(),
        redactor.clone(),
        Arc::new(|b: &str, _: &str| b == "com.example.vault"),
    );
    let read = |reader: &NotificationDatabase| -> Value {
        let after = *reader.after.lock().unwrap();
        let out: Vec<Value> = reader.read_records().unwrap().iter().map(obs).collect();
        json!({"after": after, "out": out, "after_read": *reader.after.lock().unwrap()})
    };
    let log = f["notifications"].as_array().unwrap();
    d.check("notifications first read", &log[0], &read(&reader));
    {
        let c = rusqlite::Connection::open(&ndb).unwrap();
        let mut bob = Vec::new();
        plist::Value::Dictionary(
            [(
                "req".to_string(),
                plist::Value::Dictionary(
                    [
                        ("titl".to_string(), plist::Value::String("Bob".into())),
                        ("body".to_string(), plist::Value::String("hey".into())),
                    ]
                    .into_iter()
                    .collect(),
                ),
            )]
            .into_iter()
            .collect(),
        )
        .to_writer_xml(&mut bob)
        .unwrap();
        c.execute(
            "INSERT INTO record (rec_id, app_id, uuid, data) VALUES (7, 1, ?, ?)",
            rusqlite::params![b"uuid-7".to_vec(), bob],
        )
        .unwrap();
    }
    d.check("notifications second read", &log[1], &read(&reader));
    rusqlite::Connection::open(&ndb)
        .unwrap()
        .execute("DELETE FROM record WHERE rec_id > 2", [])
        .unwrap();
    d.check("notifications after deletion", &log[2], &read(&reader));
    let bad = dir.join("bad");
    rusqlite::Connection::open(&bad)
        .unwrap()
        .execute_batch("CREATE TABLE record (rec_id INTEGER PRIMARY KEY, uuid BLOB); CREATE TABLE app (app_id INTEGER, identifier TEXT);")
        .unwrap();
    let err = NotificationDatabase::new(bad, redactor.clone(), Arc::new(|_: &str, _: &str| false))
        .read_records()
        .unwrap_err();
    d.check(
        "unsupported schema",
        &log[3]["bad"],
        &json!(err.trim_start_matches("RuntimeError: ")),
    );

    // presence
    presence(&f["presence"], &mut d);
    let _ = input_value;
    d.finish("watchers");
}

fn presence(log: &Value, d: &mut Diffs) {
    let returns: Arc<Mutex<Vec<String>>> = Arc::default();
    let wall = Arc::new(Mutex::new(5000.0f64));
    let sess = Arc::new(Mutex::new(Session {
        id: Some("s".into()),
        locked: Some(false),
        on_console: Some(true),
    }));
    let disp = Arc::new(Mutex::new(Some(false)));
    let (w, s, dd) = (wall.clone(), sess.clone(), disp.clone());
    let mp = MacPresence::new(
        move || *w.lock().unwrap(),
        move || s.lock().unwrap().clone(),
        move || *dd.lock().unwrap(),
        None,
    );
    let r = returns.clone();
    mp.set_on_return(Some(Arc::new(move |sig: &str| {
        r.lock().unwrap().push(sig.to_string())
    })));
    let mut at = 0usize;
    let mut check = |d: &mut Diffs, got: Value| {
        d.check(&format!("presence {}", log[at]["op"]), &log[at], &got);
        at += 1;
    };
    check(d, json!({"op": "away before start", "away": mp.away()}));
    mp.changed("locked", true);
    check(
        d,
        json!({"op": "changed before start", "away": mp.away(), "returns": *returns.lock().unwrap()}),
    );
    *sess.lock().unwrap() = Session {
        id: Some("s".into()),
        locked: Some(true),
        on_console: Some(true),
    };
    mp.start_watch();
    check(
        d,
        json!({"op": "start locked", "away": mp.away(), "returns": *returns.lock().unwrap(),
                    "present_since": mp.present_since()}),
    );
    for (flag, value) in [
        ("asleep", true),
        ("locked", false),
        ("asleep", false),
        ("asleep", false),
        ("nonsense", true),
        ("displays_asleep", true),
        ("switched_away", true),
        ("displays_asleep", false),
        ("switched_away", false),
    ] {
        *wall.lock().unwrap() += 10.0;
        mp.changed(flag, value);
        check(
            d,
            json!({"op": "changed", "flag": flag, "value": value, "away": mp.away(),
                        "returns": *returns.lock().unwrap(), "present_since": mp.present_since()}),
        );
    }
    *sess.lock().unwrap() = Session {
        id: None,
        locked: Some(false),
        on_console: Some(false),
    };
    *disp.lock().unwrap() = Some(true);
    mp.read();
    check(
        d,
        json!({"op": "read", "away": mp.away(), "returns": *returns.lock().unwrap()}),
    );
    *sess.lock().unwrap() = Session::default();
    *disp.lock().unwrap() = None;
    mp.read();
    check(
        d,
        json!({"op": "read nothing", "away": mp.away(), "returns": *returns.lock().unwrap()}),
    );
    *sess.lock().unwrap() = Session {
        id: None,
        locked: Some(false),
        on_console: Some(true),
    };
    *disp.lock().unwrap() = Some(false);
    *wall.lock().unwrap() += 1.0;
    mp.read();
    check(
        d,
        json!({"op": "read back", "away": mp.away(), "returns": *returns.lock().unwrap(),
                    "present_since": mp.present_since()}),
    );
    mp.set_on_return(Some(Arc::new(|_: &str| panic!("telling FERB failed"))));
    mp.changed("locked", true);
    mp.changed("locked", false);
    check(
        d,
        json!({"op": "on_return raises", "away": mp.away(), "present_since": mp.present_since()}),
    );
    mp.stop_watch();
    check(d, json!({"op": "stop", "away": mp.away()}));
    mp.changed("locked", true);
    check(d, json!({"op": "changed after stop", "away": mp.away()}));
}
