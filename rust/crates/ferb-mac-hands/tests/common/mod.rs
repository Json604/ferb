//! Shared helpers: the fixtures and a fake element tree for the Accessibility walk (the same spec make.py's
//! `build` turns into Python objects).
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ferb_mac_hands::ax::{AxSource, AxValue, CgWindow, El};
use serde_json::Value;

pub fn load(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/mac-hands")
        .join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("run make.py: {}", path.display()));
    serde_json::from_str(&text).expect("json")
}

/// A fake element: its id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Id(pub String);

/// An element, its attributes and its actions.
pub type FakeEl = (El, HashMap<String, Value>, Vec<String>);

pub struct FakeTree {
    pub els: HashMap<String, FakeEl>,
    pub apps: HashMap<i32, String>,
    pub system: Option<String>,
    pub cg: HashMap<i32, Vec<CgWindow>>,
    pub clock: Mutex<f64>,
    pub step: f64,
    pub calls: Mutex<Vec<String>>,
    pub sets: Mutex<HashMap<(String, String), i32>>,
    pub at: Mutex<HashMap<(String, i64, i64), String>>,
    reads: AtomicU64,
}

impl FakeTree {
    pub fn from_spec(spec: &Value, step: f64) -> FakeTree {
        let mut els = HashMap::new();
        for (k, v) in spec["els"].as_object().expect("els") {
            let attrs: HashMap<String, Value> = v["attrs"]
                .as_object()
                .map(|m| m.iter().map(|(a, b)| (a.clone(), b.clone())).collect())
                .unwrap_or_default();
            let actions: Vec<String> = v["actions"]
                .as_array()
                .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
                .unwrap_or_default();
            els.insert(k.clone(), (El::new(Id(k.clone())), attrs, actions));
        }
        let apps = spec["apps"]
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(p, id)| (p.parse().unwrap(), id.as_str().unwrap().to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let mut cg = HashMap::new();
        if let Some(m) = spec.get("cg").and_then(Value::as_object) {
            for (pid, ws) in m {
                let list = ws
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|w| {
                        let b: Vec<f64> = w["bounds"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|x| x.as_f64().unwrap())
                            .collect();
                        CgWindow {
                            id: w["id"].as_i64().unwrap(),
                            title: w["title"].as_str().unwrap().into(),
                            onscreen: w["onscreen"].as_bool().unwrap(),
                            bounds: (b[0], b[1], b[2], b[3]),
                        }
                    })
                    .collect();
                cg.insert(pid.parse().unwrap(), list);
            }
        }
        FakeTree {
            els,
            apps,
            system: spec.get("system").and_then(Value::as_str).map(String::from),
            cg,
            clock: Mutex::new(0.0),
            step,
            calls: Mutex::new(vec![]),
            sets: Mutex::new(HashMap::new()),
            at: Mutex::new(HashMap::new()),
            reads: AtomicU64::new(0),
        }
    }

    pub fn el(&self, id: &str) -> El {
        self.els[id].0.clone()
    }

    pub fn id_of(el: &El) -> String {
        el.get::<Id>().map(|i| i.0.clone()).unwrap_or_default()
    }

    pub fn now(&self) -> f64 {
        *self.clock.lock().unwrap()
    }

    fn conv(&self, v: &Value) -> AxValue {
        match v {
            Value::Null => AxValue::None,
            Value::Bool(b) => AxValue::Bool(*b),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    AxValue::Int(i)
                } else {
                    AxValue::Float(n.as_f64().unwrap())
                }
            }
            Value::String(s) => AxValue::Str(s.clone()),
            Value::Array(a) => AxValue::List(a.iter().map(|x| self.conv(x)).collect()),
            Value::Object(o) => {
                if let Some(id) = o.get("el") {
                    AxValue::El(self.el(id.as_str().unwrap()))
                } else if let Some(ids) = o.get("els") {
                    AxValue::List(
                        ids.as_array()
                            .unwrap()
                            .iter()
                            .map(|i| AxValue::El(self.el(i.as_str().unwrap())))
                            .collect(),
                    )
                } else if let Some(p) = o.get("pt") {
                    AxValue::Point(p[0].as_f64().unwrap(), p[1].as_f64().unwrap())
                } else if let Some(p) = o.get("sz") {
                    AxValue::Size(p[0].as_f64().unwrap(), p[1].as_f64().unwrap())
                } else if let Some(r) = o.get("range") {
                    AxValue::Range(r[0].as_i64().unwrap(), r[1].as_i64().unwrap())
                } else if let Some(u) = o.get("url") {
                    AxValue::Url {
                        absolute: u.as_str().unwrap().into(),
                        path: o["path"].as_str().map(String::from),
                    }
                } else if let Some(s) = o.get("strs") {
                    AxValue::List(
                        s.as_array()
                            .unwrap()
                            .iter()
                            .map(|x| AxValue::Str(x.as_str().unwrap().into()))
                            .collect(),
                    )
                } else {
                    AxValue::None
                }
            }
        }
    }
}

/// The fake tree as an AxSource.
pub struct Src(pub Arc<FakeTree>);

impl AxSource for Src {
    fn system(&self) -> El {
        self.0.el(self.0.system.as_deref().unwrap_or("app"))
    }
    fn app(&self, pid: i32) -> El {
        self.0.el(&self.0.apps[&pid])
    }
    fn attr(&self, el: &El, name: &str) -> AxValue {
        self.0.reads.fetch_add(1, Ordering::Relaxed);
        let id = FakeTree::id_of(el);
        match self.0.els.get(&id).and_then(|e| e.1.get(name)) {
            Some(v) => self.0.conv(v),
            None => AxValue::None,
        }
    }
    fn actions(&self, el: &El) -> Vec<String> {
        let id = FakeTree::id_of(el);
        self.0.els.get(&id).map(|e| e.2.clone()).unwrap_or_default()
    }
    fn perform(&self, el: &El, action: &str) -> i32 {
        self.0
            .calls
            .lock()
            .unwrap()
            .push(format!("{action} {}", FakeTree::id_of(el)));
        0
    }
    fn set(&self, el: &El, name: &str, _value: AxValue) -> i32 {
        let id = FakeTree::id_of(el);
        self.0
            .calls
            .lock()
            .unwrap()
            .push(format!("set {name} {id}"));
        *self
            .0
            .sets
            .lock()
            .unwrap()
            .get(&(id, name.to_string()))
            .unwrap_or(&0)
    }
    fn element_at(&self, _root: &El, x: f64, y: f64) -> Option<El> {
        self.0
            .at
            .lock()
            .unwrap()
            .get(&(String::new(), x as i64, y as i64))
            .map(|id| self.0.el(id))
    }
    fn pid_of(&self, _el: &El) -> Option<i32> {
        self.0.apps.keys().next().copied()
    }
    fn same(&self, a: &El, b: &El) -> bool {
        FakeTree::id_of(a) == FakeTree::id_of(b)
    }
    fn hash(&self, el: &El) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        FakeTree::id_of(el).hash(&mut h);
        h.finish()
    }
    fn cg_windows(&self, pid: i32) -> Vec<CgWindow> {
        self.0.cg.get(&pid).cloned().unwrap_or_default()
    }
    fn monotonic(&self) -> f64 {
        let mut c = self.0.clock.lock().unwrap();
        *c += self.0.step;
        *c
    }
    fn sleep(&self, seconds: f64) {
        *self.0.clock.lock().unwrap() += seconds;
    }
}

/// make.py's `deep_tree(depth, wide)` as a spec.
pub fn deep_spec(depth: usize, wide: usize) -> Value {
    use serde_json::json;
    let pos = |x: f64, y: f64, w: f64, h: f64| json!({"AXPosition": {"pt": [x, y]}, "AXSize": {"sz": [w, h]}});
    let mut els = serde_json::Map::new();
    els.insert("app".into(), json!({"attrs": {"AXChildren": {"els": ["win"]}, "AXWindows": {"els": ["win"]}}, "actions": []}));
    let mut win = pos(0.0, 0.0, 500.0, 500.0);
    win["AXRole"] = json!("AXWindow");
    win["AXTitle"] = json!("Deep");
    win["AXChildren"] = json!({"els": ["d0", "wide"]});
    els.insert("win".into(), json!({"attrs": win, "actions": []}));
    for i in 0..depth {
        let kids = if i + 1 < depth {
            vec![format!("d{}", i + 1)]
        } else {
            vec!["deep_pw".to_string()]
        };
        let actions: Vec<&str> = if i % 7 == 0 { vec!["AXPress"] } else { vec![] };
        els.insert(format!("d{i}"), json!({"attrs": {"AXRole": "AXGroup", "AXChildren": {"els": kids}}, "actions": actions}));
    }
    let mut pw = pos(10.0, 10.0, 50.0, 10.0);
    pw["AXRole"] = json!("AXSecureTextField");
    els.insert("deep_pw".into(), json!({"attrs": pw, "actions": []}));
    let mut items: Vec<String> = (0..wide).map(|i| format!("w{i}")).collect();
    items.push("w0".into());
    els.insert(
        "wide".into(),
        json!({"attrs": {"AXRole": "AXList", "AXChildren": {"els": items}}, "actions": []}),
    );
    for i in 0..wide {
        els.insert(format!("w{i}"), json!({"attrs": {"AXRole": "AXButton", "AXTitle": format!("Item {i}")}, "actions": ["AXPress"]}));
    }
    json!({"els": els, "apps": {"5": "app"}, "cg": {"5": [{"id": 50, "title": "Deep", "onscreen": true, "bounds": [0.0, 0.0, 500.0, 500.0]}]}})
}

/// Python's `json.dumps(value, ensure_ascii=True)` then SHA-256 (make.py `sha`).
pub fn py_sha(v: &Value) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(ferb_py::json::dumps(v).as_bytes()))
}
