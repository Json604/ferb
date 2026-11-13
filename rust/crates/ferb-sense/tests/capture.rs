//! Capture guards (`ferb/sense/capture.py`) and today's activity (`ferb/sense/daily.py`) against
//! `rust/fixtures/sense/capture.json` and `daily.json`.

mod common;

use std::sync::{Arc, Mutex};

use common::{fixture, tmp, Diffs};
use ferb_clock::zone::Zone;
use ferb_sense::capture::{clipboard_observation, password_element, Field, TypedFields};
use ferb_sense::daily::{terms, values, without_queries, DailyActivity};
use ferb_sense::observation::Observation;
use serde_json::{json, Value};

fn field(v: &Value) -> Option<Field> {
    let a = v.as_array()?;
    Some(Field {
        bundle_id: a[0].as_str()?.into(),
        app: a[1].as_str()?.into(),
        pid: a[2].as_i64()?,
        window_id: a[3].as_str()?.into(),
        field_id: a[4].as_str()?.into(),
        role: a[5].as_str()?.into(),
        label: a[6].as_str()?.into(),
        masked: a[7].as_bool()?,
    })
}

fn obs(o: &Observation) -> Value {
    json!([o.source, o.data, o.hints])
}

// decision: SENSE-401
#[test]
fn typed_bursts_and_clipboard_text_are_kept_as_python_keeps_them() {
    let f = fixture("capture.json");
    let mut d = Diffs::default();
    let redactor: Arc<dyn ferb_sense::Redact> = Arc::new(ferb_sanitize::Redactor::default());
    for (n, script) in f["typed"].as_array().unwrap().iter().enumerate() {
        let emitted: Arc<Mutex<Vec<Value>>> = Arc::default();
        let e = emitted.clone();
        let tf = TypedFields::new(move |o| e.lock().unwrap().push(obs(&o)), redactor.clone());
        for (k, st) in script.as_array().unwrap().iter().enumerate() {
            let before = emitted.lock().unwrap().len();
            let fl = field(&st["field"]);
            match st["op"].as_str().unwrap() {
                "feed" => tf.feed(
                    fl.as_ref(),
                    st["text"].as_str().unwrap(),
                    st["actor"].as_str().unwrap(),
                    st["now"].as_f64().unwrap(),
                    st["secure"].as_bool().unwrap(),
                ),
                "flush" => tf.flush(
                    fl.as_ref(),
                    st["secure"].as_bool().unwrap(),
                    st["now"].as_f64().unwrap(),
                ),
                "leave" => tf.leave(fl.as_ref()),
                "clear" => tf.clear(),
                "holding" => d.check(
                    &format!("script {n} step {k} holding"),
                    &st["holding"],
                    &json!(tf.holding()),
                ),
                other => panic!("{other}"),
            }
            let got = Value::Array(emitted.lock().unwrap()[before..].to_vec());
            d.check(&format!("script {n} step {k} {st}"), &st["emitted"], &got);
        }
    }
    for c in f["clipboard"].as_array().unwrap() {
        let types: Vec<String> = c["types"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap().into())
            .collect();
        let text = c["text"].as_str().map(String::from);
        let ctx = c["context"].as_object().cloned();
        let o = clipboard_observation(
            c["count"].as_i64().unwrap(),
            &types,
            &|| text.clone(),
            redactor.as_ref(),
            c["ferb_count"].as_i64(),
            c["task"].as_str(),
            ctx.as_ref(),
        );
        d.check(
            &format!("clipboard {c}"),
            &c["out"],
            &o.as_ref().map_or(Value::Null, obs),
        );
    }
    for p in f["password_element"].as_array().unwrap() {
        let meta = p[1].as_object().unwrap().clone();
        let attr = |name: &str| meta.get(name).and_then(Value::as_str).map(String::from);
        d.check(
            &format!("password_element {p}"),
            &p[2],
            &json!(password_element(p[0].as_str().unwrap(), &attr)),
        );
    }
    d.finish("capture");
}

// decision: SENSE-1, MEM-27
#[test]
fn todays_activity_file_and_search_match_python() {
    let f = fixture("daily.json");
    let mut d = Diffs::default();
    let case = &f[0];
    let path = tmp().join("activity/today.jsonl");
    let now = Arc::new(Mutex::new(
        ferb_core::when::parse(case["start"].as_str().unwrap()).unwrap(),
    ));
    let n = now.clone();
    let daily = DailyActivity::new(
        path.clone(),
        move || *n.lock().unwrap(),
        Zone::named("Asia/Kolkata").unwrap(),
    );
    for (k, st) in case["steps"].as_array().unwrap().iter().enumerate() {
        let a = &st["args"];
        let what = format!("step {k} {st}");
        match st["op"].as_str().unwrap() {
            "record" => daily.record(a[0].as_object().unwrap()).unwrap(),
            "search" => {
                let got: Vec<Value> = daily
                    .search(a[0].as_str().unwrap(), a[1].as_u64().unwrap() as usize)
                    .into_iter()
                    .map(Value::Object)
                    .collect();
                d.check(&format!("{what} out"), &st["out"], &Value::Array(got));
            }
            "raw_append" => {
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .unwrap();
                file.write_all(a[0].as_str().unwrap().as_bytes()).unwrap();
            }
            "raw_write" => std::fs::write(&path, a[0].as_str().unwrap()).unwrap(),
            "redact_source" => daily.redact_source(a[0].as_str().unwrap(), &a[1]).unwrap(),
            "forget" => {
                let ids: Vec<String> = a[0]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().into())
                    .collect();
                daily.forget_sources(&ids).unwrap()
            }
            "advance" => {
                let mut t = now.lock().unwrap();
                *t += chrono::TimeDelta::seconds(a[0].as_i64().unwrap());
            }
            "roll_over" => daily.roll_over(),
            other => panic!("{other}"),
        }
        let file = std::fs::read_to_string(&path).ok();
        d.check(&format!("{what} file"), &st["file"], &json!(file));
    }
    let extra = &case["extra"];
    for t in extra["terms"].as_array().unwrap() {
        let mut got = terms(t[0].as_str().unwrap());
        got.sort();
        d.check(&format!("terms {}", t[0]), &t[1], &json!(got));
    }
    for v in extra["values"].as_array().unwrap() {
        let mut out = Vec::new();
        values(&v[0], &mut out);
        d.check(&format!("values {}", v[0]), &v[1], &json!(out));
    }
    for v in extra["without_queries"].as_array().unwrap() {
        d.check(
            &format!("without_queries {}", v[0]),
            &v[1],
            &without_queries(&v[0]),
        );
    }
    d.finish("daily");
}
