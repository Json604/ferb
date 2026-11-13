//! Usage, prices, limits, the runtime table's rules and the small helpers against rust/fixtures/runtimes/tables.json.

mod support;

use std::path::Path;

use ferb_runtimes::catalog::{self, Config};
use ferb_runtimes::{api, codex, tooltrace, usage};
use serde_json::{json, Value};
use support::*;

struct Cfg(Value);

impl Config for Cfg {
    fn get(&self, section: &str, key: &str) -> Option<Value> {
        self.0.get(section).and_then(|s| s.get(key)).cloned()
    }
}

fn configs() -> Vec<Value> {
    vec![
        json!({}),
        json!({"models": {"codex": "gpt-6.1-sol", "claude": ""}, "runtimes": {"claude_model": "opus"}}),
        json!({"efforts": {"codex": " high ", "claude": 5, "openai": ""},
            "role_models": {"orchestrator": {"runtime": "codex", "model": " gpt-6-sol ", "effort": "medium"},
                "checker": {"runtime": "claude", "model": "", "effort": "max"},
                "worker": {"runtime": "openai", "model": "gpt-6-luna"}}}),
        json!({"role_models": {"orchestrator": "not a table", "worker": {"runtime": "codex", "effort": "  "}},
            "efforts": {"codex": "xhigh"}}),
    ]
}

fn opt_str(v: &Value) -> Option<&str> {
    v.as_str()
}

fn check(name: &str, got: Value, want: &Value, failures: &mut Vec<String>) {
    if let Some(d) = first_diff(&got, want, "") {
        failures.push(format!("{name}: {d}"));
    }
}

fn dumps_f(v: Option<f64>) -> Value {
    v.and_then(serde_json::Number::from_f64)
        .map_or(Value::Null, Value::Number)
}

// decision: RUN-404, RUN-405, RUN-417, RUN-422, RUN-423, RUN-428, RUN-429, RUN-436, RUN-437, RUN-438, RUN-460, RUN-461, RUN-462, RUN-463
#[test]
fn tables_match_python() {
    let fx = fixture("tables.json");
    let mut failures = Vec::new();
    let mut n = 0;
    for row in fx["costs"].as_array().unwrap() {
        let got = usage::cost_usd(
            row[0].as_str().unwrap(),
            row[1].as_i64().unwrap(),
            row[2].as_i64().unwrap(),
            row[3].as_i64().unwrap(),
            row[4].as_i64().unwrap(),
            row[5].as_str().unwrap(),
            row[6].as_i64().unwrap(),
        );
        // exact float equality: the same operations in the same order
        let want = row[7].as_f64();
        if got.map(f64::to_bits) != want.map(f64::to_bits) {
            failures.push(format!("cost {row}: {got:?}"));
        }
        n += 1;
    }
    for row in fx["usages"].as_array().unwrap() {
        let (kind, model, data) = (
            row[0].as_str().unwrap(),
            row[1].as_str().unwrap(),
            row[2].as_object().unwrap(),
        );
        let day = "2026-11-13";
        let u = match kind {
            "openai" => usage::from_openai(model, data, "api", day),
            "codex" => usage::from_codex(model, data, day),
            "anthropic" => usage::from_anthropic(model, data, "api", None, day),
            "anthropic_cost" => usage::from_anthropic(model, data, "subscription", Some(0.25), day),
            _ => usage::from_gemini(model, data, "api", day),
        }
        .unwrap();
        let got = ferb_py::json::dumps(&Value::Object(u.as_dict()));
        if got != row[3].as_str().unwrap() {
            failures.push(format!("usage {kind} {model}: {got} != {}", row[3]));
        }
        n += 1;
    }
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    write_files(
        &home,
        &json!({"codex-home/auth.json": "{\"tokens\":{}}",
        "run/codex-bundled-0.166.0.json": serde_json::to_string(&json!({"models": [
        {"slug": "gpt-5.6-sol", "context_window": 272000, "effective_context_window_percent": 95},
        {"slug": "gpt-x", "context_window": 100000, "effective_context_window_percent": 87.5},
        {"slug": "gpt-new", "context_window": 1, "effective_context_window_percent": 1, "minimal_client_version": [0, 170, 0]},
        {"slug": "gpt-broken", "context_window": 0, "effective_context_window_percent": 95},
        {"slug": "gpt-true", "context_window": true, "effective_context_window_percent": 95}]})).unwrap()}),
    );
    let paths = Paths {
        root: dir.path().to_path_buf(),
        home: home.clone(),
        tmp: dir.path().join("tmp"),
    };
    for row in fx["limits"].as_array().unwrap() {
        let got =
            match catalog::limits(row[0].as_str().unwrap(), opt_str(&row[1]), Path::new(&home)) {
                Ok(l) => json!([
                    l.model,
                    l.context_window,
                    l.max_output,
                    l.source,
                    l.window_includes_output,
                    l.max_input_chars
                ]),
                Err(e) => json!([e.class(), e.to_string()]),
            };
        check(
            &format!("limits {} {}", row[0], row[1]),
            paths.normalize(&got),
            &row[2],
            &mut failures,
        );
        n += 1;
    }
    for row in fx["call_limit"].as_array().unwrap() {
        let got = catalog::call_limit(row[0].as_str().unwrap(), row[1].as_bool().unwrap());
        check("call_limit", dumps_f(got), &row[2], &mut failures);
        n += 1;
    }
    let cfgs = configs();
    for row in fx["models"].as_array().unwrap() {
        let c = Cfg(cfgs[row[0].as_u64().unwrap() as usize].clone());
        let name = row[1].as_str().unwrap();
        let roles: Vec<String> = ["orchestrator", "worker", "checker:x"]
            .iter()
            .map(|r| catalog::role_model(&c, name, r))
            .collect();
        let efforts: Vec<String> = ["orchestrator", "worker", "checker"]
            .iter()
            .map(|r| catalog::role_effort(&c, name, r))
            .collect();
        let got = json!([
            row[0],
            name,
            catalog::configured_model(&c, name),
            catalog::model_for(&c, name),
            roles,
            catalog::configured_effort(&c, name),
            efforts
        ]);
        check("models", got, row, &mut failures);
        n += 1;
    }
    for row in fx["efforts"].as_array().unwrap() {
        let c = Cfg(cfgs[row[0].as_u64().unwrap() as usize].clone());
        let got = catalog::run_effort(
            &c,
            row[1].as_str().unwrap(),
            row[2].as_str().unwrap(),
            row[3].as_str().unwrap(),
            row[4].as_bool().unwrap(),
            opt_str(&row[5]),
        );
        check(&format!("effort {row}"), json!(got), &row[6], &mut failures);
        n += 1;
    }
    for row in fx["effort_problem"].as_array().unwrap() {
        check(
            &format!("effort_problem {row}"),
            json!(catalog::effort_problem(
                row[0].as_str().unwrap(),
                row[1].as_str().unwrap()
            )),
            &row[2],
            &mut failures,
        );
        n += 1;
    }
    for row in fx["sign_in_fix"].as_array().unwrap() {
        check(
            "sign_in_fix",
            json!(catalog::sign_in_fix(row[0].as_str().unwrap())),
            &row[1],
            &mut failures,
        );
        n += 1;
    }
    for row in fx["labels"].as_array().unwrap() {
        check(
            "label",
            json!(catalog::label(row[0].as_str().unwrap())),
            &row[1],
            &mut failures,
        );
        n += 1;
    }
    check(
        "api_providers",
        json!(catalog::api_providers()),
        &fx["api_providers"],
        &mut failures,
    );
    let table: Vec<Value> = catalog::CATALOG
        .iter()
        .map(|r| {
            json!([
                r.name,
                r.default_model,
                match r.sign_in {
                    catalog::SignIn::Codex => "codex",
                    catalog::SignIn::Claude => "claude",
                    catalog::SignIn::Key => "key",
                },
                r.billing
            ])
        })
        .collect();
    check("catalog", json!(table), &fx["catalog"], &mut failures);
    for row in fx["http_cause"].as_array().unwrap() {
        check(
            "http_cause",
            json!(api::http_cause(row[0].as_u64().unwrap() as u16)),
            &row[1],
            &mut failures,
        );
        n += 1;
    }
    for row in fx["g"].as_array().unwrap() {
        check(
            &format!("g {row}"),
            json!(api::py_g(row[0].as_f64().unwrap())),
            &row[1],
            &mut failures,
        );
        n += 1;
    }
    for row in fx["traces"].as_array().unwrap() {
        let got = match tooltrace::tools_in(row[0].as_str().unwrap()) {
            Ok(names) => json!(names),
            Err(r) => json!([r.class, r.message]),
        };
        check(&format!("trace {}", row[0]), got, &row[1], &mut failures);
        n += 1;
    }
    for row in fx["inline_refs"].as_array().unwrap() {
        check(
            "inline_refs",
            api::inline_refs(&row[0]),
            &row[1],
            &mut failures,
        );
        n += 1;
    }
    let plain: codex::Redact = std::sync::Arc::new(|s: &str| s.to_string());
    for row in fx["codex_errors"].as_array().unwrap() {
        let e = (!row[0].is_null()).then_some(&row[0]);
        check(
            &format!("codex_error {}", row[0]),
            json!([
                row[0],
                codex::codex_error("P", e, &plain),
                codex::signed_out(e)
            ]),
            row,
            &mut failures,
        );
        n += 1;
    }
    for row in fx["logins"].as_array().unwrap() {
        check(
            &format!("login {}", row[0]),
            json!(codex::chatgpt_login(row[0].as_str().unwrap().as_bytes())),
            &row[1],
            &mut failures,
        );
        n += 1;
    }
    for row in fx["cache_ids"].as_array().unwrap() {
        check(
            "cache_id",
            json!(codex::cache_thread_id(
                row[0].as_str().unwrap(),
                row[1].as_str().unwrap(),
                row[2].as_str().unwrap()
            )),
            &row[3],
            &mut failures,
        );
        n += 1;
    }
    for row in fx["model_lists"].as_array().unwrap() {
        check(
            "model_list",
            json!(ferb_runtimes::models::parse(
                row[0].as_str().unwrap(),
                &row[1]
            )),
            &row[2],
            &mut failures,
        );
        n += 1;
    }
    let home2 = dir.path().join("home2");
    write_files(
        &home2,
        &json!({"codex-home/auth.json": "{}", "codex-home/models_cache.json": serde_json::to_string(&json!({"models": [
        {"slug": "gpt-a", "supported_reasoning_levels": [{"effort": "low"}, {"effort": "ultra"}, {"x": 1}],
         "default_reasoning_level": "low"}, {"slug": "gpt-b"}, {"nope": 1}]})).unwrap()}),
    );
    let names: Vec<String> = [
        "claude-opus-5-5",
        "claude-opus-5-5-20261101",
        "claude-opus-4-5",
        "claude-x",
        "gemini-3.8-flash",
        "gemini-3-pro-preview-001",
        "gpt-5.6-sol",
        "gpt-6-luna",
        "gpt-7",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for row in fx["effort_levels"].as_array().unwrap() {
        let (l, d) = ferb_runtimes::models::effort_levels(row[0].as_str().unwrap(), &home2, &names);
        check(
            &format!("effort_levels {}", row[0]),
            json!([l, d]),
            &row[1],
            &mut failures,
        );
        n += 1;
    }
    for row in fx["known_models"].as_array().unwrap() {
        check(
            "known_models",
            json!(ferb_runtimes::models::known_models(
                row[0].as_str().unwrap(),
                &home2
            )),
            &row[1],
            &mut failures,
        );
        n += 1;
    }
    let toml = json!([
        codex::toml(&codex::provider_entry()),
        codex::toml(&json!("q\"é")),
        codex::toml(&json!(true)),
        codex::toml(&json!(1.5))
    ]);
    check("toml", toml, &fx["toml"], &mut failures);
    assert!(
        failures.is_empty(),
        "{} of {n} differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
