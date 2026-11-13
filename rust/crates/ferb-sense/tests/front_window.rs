//! The front window at send (`ferb/sense/front_window.py`) against `rust/fixtures/sense/front_window.json`, over
//! scripted sources: nothing here reads a real window, takes a screenshot or brings anything forward.

mod common;

use std::collections::HashMap;
use std::sync::Mutex;

use common::{fixture, Diffs};
use ferb_core::interfaces::BoxFuture;
use ferb_sense::front_window::{
    capture, excluded_app_text, ferb_process, sending_app, Shot, ShotError, Sources, TabLookup,
    Window, FERB_APPS,
};
use ferb_sense::observation::{Dict, Redact};
use serde_json::{json, Value};

struct Fake {
    sc: Value,
    windows: Vec<Window>,
    apps: HashMap<i64, (String, String)>,
    parents: HashMap<i64, i64>,
    daemon: i64,
    redactor: ferb_sanitize::Redactor,
    calls: Mutex<Vec<Value>>,
}

fn known(b: &str) -> bool {
    common::known_browser(b)
}

impl Sources for Fake {
    fn windows(&self) -> Vec<Window> {
        self.windows.clone()
    }
    fn app_of(&self, pid: i64) -> Option<(String, String)> {
        self.apps.get(&pid).cloned()
    }
    fn own(&self, pid: i64, bundle: &str) -> bool {
        ferb_process(pid, bundle, self.daemon, &|p| self.parents.get(&p).copied())
    }
    fn excluded(&self, bundle: &str, _name: &str, url: Option<&str>) -> bool {
        bundle == "com.example.vault" || url.is_some_and(|u| u.contains("bank.example"))
    }
    fn tab(
        &self,
        bundle: &str,
        w: &Window,
        after: bool,
    ) -> Result<Option<(String, String)>, TabLookup> {
        self.calls
            .lock()
            .unwrap()
            .push(json!(["tab", bundle, w.id, after]));
        let v = if after {
            self.sc.get("tab_after")
        } else {
            self.sc.get("tab")
        };
        match v {
            Some(Value::String(s)) if s == "ambiguous" => Err(TabLookup {
                message: "several could be it".into(),
                ambiguous: true,
            }),
            Some(Value::String(s)) if s == "lookup" => Err(TabLookup {
                message: "no window is there".into(),
                ambiguous: false,
            }),
            Some(Value::Array(a)) => Ok(Some((
                a[0].as_str().unwrap().into(),
                a[1].as_str().unwrap().into(),
            ))),
            _ => Ok(None),
        }
    }
    fn shoot<'a>(&'a self, w: &'a Window) -> BoxFuture<'a, Result<Shot, ShotError>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(json!(["shoot", w.id]));
            match self.sc.get("shoot").and_then(Value::as_str) {
                Some("refused") => Err(ShotError::Refused("the window is on another Space".into())),
                Some("error") => Err(ShotError::Failed("OSError".into())),
                _ => {
                    let named = self.sc.get("named").and_then(Value::as_array).map(|a| {
                        (
                            a[0].as_str().unwrap().to_string(),
                            a[1].as_str().unwrap().to_string(),
                        )
                    });
                    let masking = self
                        .sc
                        .get("masking")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    Ok((format!("jpeg-{}", w.id).into_bytes(), masking, named))
                }
            }
        })
    }
    fn redact_image(&self, raw: &[u8]) -> Result<Vec<u8>, String> {
        if self.sc.get("redact_image").is_some() {
            return Err("ValueError".into());
        }
        Ok([raw, b"-redacted"].concat())
    }
    fn redact(&self) -> &dyn Redact {
        &self.redactor
    }
    fn keep<'a>(
        &'a self,
        image: &'a [u8],
        facts: &'a Dict,
        origin: &'a str,
    ) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(json!([
                "keep",
                String::from_utf8_lossy(image),
                facts,
                origin
            ]));
            if self.sc.get("keep").is_some() {
                return Err("KeyError".into());
            }
            Ok("ev-1".into())
        })
    }
    fn client(&self) -> Option<i64> {
        self.sc.get("client").and_then(Value::as_i64)
    }
    fn parent(&self, pid: i64) -> Option<i64> {
        self.parents.get(&pid).copied()
    }
    fn main_frame(&self, pid: i64) -> Option<(f64, f64, f64, f64)> {
        let m = self.sc.get("main")?.get(pid.to_string())?.as_array()?;
        Some((
            m[0].as_f64()?,
            m[1].as_f64()?,
            m[2].as_f64()?,
            m[3].as_f64()?,
        ))
    }
    fn is_browser(&self, bundle: &str) -> bool {
        known(bundle)
    }
    fn read_sources(&self, bundle: &str) -> Vec<(String, String)> {
        if bundle == "com.apple.MobileSMS" {
            vec![
                ("message".into(), bundle.into()),
                ("owner_account".into(), format!("{bundle}|")),
            ]
        } else if known(bundle) {
            vec![("web".into(), bundle.into())]
        } else {
            vec![("app".into(), bundle.into())]
        }
    }
}

// decision: SENSE-403
#[test]
fn front_window_captures_match_python() {
    let f = fixture("front_window.json");
    let mut d = Diffs::default();
    let apps: HashMap<i64, (String, String)> = f["apps"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| {
            (
                k.parse().unwrap(),
                (v[0].as_str().unwrap().into(), v[1].as_str().unwrap().into()),
            )
        })
        .collect();
    let parents: HashMap<i64, i64> = f["parents"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.parse().unwrap(), v.as_i64().unwrap()))
        .collect();
    let daemon = f["daemon_pid"].as_i64().unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    for c in f["captures"].as_array().unwrap() {
        let sc = c["scenario"].clone();
        let windows = sc["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| {
                let b = w[2].as_array().unwrap();
                Window {
                    id: w[0].as_i64().unwrap(),
                    pid: w[1].as_i64().unwrap(),
                    bounds: (
                        b[0].as_f64().unwrap(),
                        b[1].as_f64().unwrap(),
                        b[2].as_f64().unwrap(),
                        b[3].as_f64().unwrap(),
                    ),
                    title: w[3].as_str().unwrap().into(),
                    layer: 0,
                    alpha: 1.0,
                }
            })
            .collect();
        let fake = Fake {
            sc: sc.clone(),
            windows,
            apps: apps.clone(),
            parents: parents.clone(),
            daemon,
            redactor: ferb_sanitize::Redactor::default(),
            calls: Mutex::default(),
        };
        let stage = Mutex::new(Vec::new());
        let got = rt.block_on(capture(&fake, &stage));
        let out = got.map_or(Value::Null, |g| {
            json!({"app": g.app, "bundle_id": g.bundle_id, "title": g.title, "url": g.url, "tab_title": g.tab_title,
                   "image_ref": g.image_ref, "origin": g.origin, "withheld": g.withheld,
                   "image": g.image.as_ref().map(|i| String::from_utf8_lossy(i).into_owned()),
                   "tab_note": g.tab_note, "tab_source": g.tab_source, "masking": g.masking, "facts": g.facts()})
        });
        let name = sc["name"].as_str().unwrap();
        d.check(
            &format!("{name} stage"),
            &c["stage"],
            &json!(*stage.lock().unwrap()),
        );
        d.check(
            &format!("{name} calls"),
            &c["calls"],
            &json!(*fake.calls.lock().unwrap()),
        );
        d.check(&format!("{name} out"), &c["out"], &out);
    }
    let loops: HashMap<i64, i64> = [(40, 41), (41, 40)].into_iter().collect();
    for p in f["ferb_process"].as_array().unwrap() {
        let pid = p[0].as_i64().unwrap();
        let parent: Box<dyn Fn(i64) -> Option<i64>> = if pid == 40 {
            Box::new(|x| loops.get(&x).copied())
        } else {
            Box::new(|x| parents.get(&x).copied())
        };
        d.check(
            &format!("ferb_process {p}"),
            &p[2],
            &json!(ferb_process(pid, p[1].as_str().unwrap(), daemon, &parent)),
        );
    }
    for p in f["sending_app"].as_array().unwrap() {
        let pid = p[0].as_i64().unwrap();
        let got = if pid == 40 {
            sending_app(pid, &|_| None, &|x| loops.get(&x).copied())
        } else {
            sending_app(pid, &|x| apps.get(&x).cloned(), &|x| {
                parents.get(&x).copied()
            })
        };
        d.check(&format!("sending_app {p}"), &p[1], &json!(got));
    }
    d.check(
        "excluded app text",
        &f["excluded_app_text"],
        &json!(excluded_app_text()),
    );
    let mut ours: Vec<&str> = FERB_APPS.to_vec();
    ours.sort();
    d.check("ferb apps", &f["ferb_apps"], &json!(ours));
    d.finish("front window");
}
