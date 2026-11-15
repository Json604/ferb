//! Output paging, the send line check, the clean environment, the admin argv, the texts agents see and every tool
//! result's composition, against what the Python code produced (pty.json, constants.json, results.json).
// decision: HAND-203, HAND-217, HAND-429, HAND-210

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use common::{fixture, strs, Root, Tally};
use ferb_shell::manager::{clean_env_from, line_too_long, ProcInfo};
use ferb_shell::protection::{Entry, LocalPort, Protection};
use ferb_shell::pty::{admin_argv, read_log, ADMIN_SUPERVISOR};
use ferb_shell::sandbox as sb;
use ferb_shell::tools::{
    self, list_result, output_result, rules_result, run_result, send_result, start_result,
};
use serde_json::{json, Map, Value};

#[test]
fn output_pages_never_split_a_character_as_python() {
    let fx = fixture("pty.json");
    let dir = tempfile::tempdir().unwrap();
    let mut tally = Tally::default();
    for (name, hexed) in fx["logs"].as_object().unwrap() {
        let data = hex::decode(hexed.as_str().unwrap()).unwrap();
        std::fs::write(dir.path().join(format!("{name}.log")), &data).unwrap();
    }
    for case in fx["reads"].as_array().unwrap() {
        let name = case[0].as_str().unwrap();
        let log = dir.path().join(format!("{name}.log"));
        let total = std::fs::metadata(&log).unwrap().len();
        let (text, offset) = read_log(&log, total, case[1].as_i64().unwrap(), case[2].as_u64());
        tally.check(
            &format!("{case}"),
            (text, offset),
            (
                case[3].as_str().unwrap().to_string(),
                case[4].as_u64().unwrap(),
            ),
        );
    }
    for case in fx["sends"].as_array().unwrap() {
        let raw = hex::decode(case[0].as_str().unwrap()).unwrap();
        tally.check(
            &format!("send {case}"),
            line_too_long(&raw, case[1].as_i64().unwrap()),
            case[2].as_bool().unwrap(),
        );
    }
    tally.done("pty");
}

#[test]
fn the_clean_environment_and_the_admin_argv_are_pythons() {
    let fx = fixture("pty.json");
    let root = Root::new();
    std::fs::create_dir_all(Path::new(&root.root).join("user/.local/bin")).unwrap();
    let mut tally = Tally::default();
    for case in fx["env"].as_array().unwrap() {
        let environ: BTreeMap<String, String> = case["environ"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), root.sub(v.as_str().unwrap())))
            .collect();
        for (key, cert) in [("clean", false), ("with_sandbox", true)] {
            let got: BTreeMap<String, String> = clean_env_from(&|k| environ.get(k).cloned(), cert)
                .into_iter()
                .collect();
            let want: BTreeMap<String, String> = case[key]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), root.sub(v.as_str().unwrap())))
                .collect();
            tally.check(key, got, want);
        }
    }
    let argv: Vec<String> = [
        "/usr/bin/sandbox-exec",
        "-f",
        "/p/x.sb",
        "/bin/sh",
        "-c",
        "id -u",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let env = vec![
        ("PATH".to_string(), "/usr/bin".to_string()),
        ("A".to_string(), "b c".to_string()),
    ];
    tally.check(
        "admin",
        admin_argv(&argv, &env, "@TTY@").unwrap(),
        strs(&fx["admin"]),
    );
    tally.check(
        "supervisor",
        ADMIN_SUPERVISOR,
        fx["admin_supervisor"].as_str().unwrap(),
    );
    tally.check(
        "admin outside the sandbox",
        admin_argv(&argv[3..], &env, "x").unwrap_err(),
        "an admin command must keep FERB's sandbox".to_string(),
    );
    tally.check(
        "cert",
        Path::new(sb::CERT_BUNDLE).is_file(),
        fx["cert_bundle_exists"].as_bool().unwrap(),
    );
    tally.done("env/admin");
}

#[test]
fn the_texts_agents_see_are_pythons() {
    let fx = fixture("constants.json");
    let mut tally = Tally::default();
    let s = |k: &str| fx[k].as_str().unwrap().to_string();
    for (k, v) in [
        ("SANDBOX_EXEC", sb::SANDBOX_EXEC),
        ("SECURITY_SERVER", sb::SECURITY_SERVER),
        ("CERT_BUNDLE", sb::CERT_BUNDLE),
        ("NESTED_SANDBOX", sb::NESTED_SANDBOX),
        ("APP_REFUSED", sb::APP_REFUSED),
        ("NO_APPS", sb::NO_APPS),
        ("NETWORK_FACT", sb::NETWORK_FACT),
        ("OSASCRIPT", sb::OSASCRIPT),
        ("VOLUMES", sb::VOLUMES),
        ("DATA_VOLUME", sb::DATA_VOLUME),
        ("SCOPE_FACT", sb::SCOPE_FACT),
        ("SCOPE_REFUSED", sb::SCOPE_REFUSED),
        ("OUTPUT_ORIGIN", tools::OUTPUT_ORIGIN),
    ] {
        tally.check(k, v.to_string(), s(k));
    }
    let list = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    tally.check(
        "NETWORK_SERVICES",
        list(&sb::NETWORK_SERVICES),
        strs(&fx["NETWORK_SERVICES"]),
    );
    tally.check(
        "TRUST_SERVICES",
        list(&sb::TRUST_SERVICES),
        strs(&fx["TRUST_SERVICES"]),
    );
    tally.check(
        "APP_SERVICES",
        list(&sb::APP_SERVICES),
        strs(&fx["APP_SERVICES"]),
    );
    tally.check(
        "APPS_LOCKDOWN",
        list(&sb::APPS_LOCKDOWN),
        strs(&fx["APPS_LOCKDOWN"]),
    );
    tally.check(
        "APP_PREFERENCE_DOMAINS",
        list(&sb::APP_PREFERENCE_DOMAINS),
        strs(&fx["APP_PREFERENCE_DOMAINS"]),
    );
    tally.check(
        "WINDOW_SERVER",
        list(&sb::WINDOW_SERVER),
        strs(&fx["WINDOW_SERVER"]),
    );
    tally.check("TRASHES", list(&sb::TRASHES), strs(&fx["TRASHES"]));
    tally.check(
        "CLEAN_ENV_KEYS",
        list(&ferb_shell::manager::CLEAN_ENV_KEYS),
        strs(&fx["CLEAN_ENV_KEYS"]),
    );
    tally.check(
        "SEND_WAIT_S",
        ferb_shell::manager::SEND_WAIT_S,
        fx["SEND_WAIT_S"].as_f64().unwrap(),
    );
    tally.check(
        "WATCH_S",
        ferb_shell::manager::WATCH_S,
        fx["WATCH_S"].as_f64().unwrap(),
    );
    tally.check("TAIL", tools::TAIL, fx["TAIL"].as_u64().unwrap());
    for (name, text) in fx["descriptions"].as_object().unwrap() {
        tally.check(
            name,
            tools::description(name),
            text.as_str().map(str::to_string),
        );
    }
    tally.done("constants");
}

struct Rules {
    home: PathBuf,
    pins: Vec<LocalPort>,
}

impl Protection for Rules {
    fn home(&self) -> &Path {
        &self.home
    }
    fn ferb_home(&self) -> &Path {
        &self.home
    }
    fn workspace(&self) -> &Path {
        &self.home
    }
    fn uploads(&self) -> &Path {
        &self.home
    }
    fn reads(&self) -> &[Entry] {
        &[]
    }
    fn writes(&self) -> &[Entry] {
        &[]
    }
    fn app_entries(&self) -> Vec<PathBuf> {
        vec![]
    }
    fn service_files(&self) -> Vec<PathBuf> {
        vec![]
    }
    fn guarded_parents(&self) -> Vec<PathBuf> {
        vec![]
    }
    fn forms_now(&self, p: &Path) -> Vec<PathBuf> {
        vec![p.to_path_buf()]
    }
    fn app_data(&self, _: &Path) -> bool {
        false
    }
    fn local_ports(&self) -> &[LocalPort] {
        &self.pins
    }
    /// The policy crate's rules(); here the ones Python listed (its own part, not this one's).
    fn rules(&self) -> Vec<Value> {
        vec![]
    }
}

fn info(v: &Value) -> ProcInfo {
    ProcInfo {
        id: "p-1".into(),
        owner_task: "t".into(),
        command: "cmd".into(),
        cwd: "/w".into(),
        started: 0.0,
        running: v["running"].as_bool().unwrap(),
        exit_code: v["exit_code"].as_i64().map(|c| c as i32),
        long_lived: false,
        network: false,
        capture_error: v
            .get("capture_error")
            .and_then(Value::as_str)
            .map(str::to_string),
        sandboxed: true,
        lifted: None,
        read_path: v
            .get("read_path")
            .and_then(Value::as_str)
            .map(str::to_string),
        sandbox_ms: v.get("sandbox_ms").and_then(Value::as_f64),
        direct: vec![],
        stopped: v.get("stopped").and_then(Value::as_str).map(str::to_string),
    }
}

fn sources(v: &[(String, String)]) -> Value {
    Value::Array(v.iter().map(|(a, b)| json!([a, b])).collect())
}

#[test]
fn every_tool_result_is_composed_as_python_composes_it() {
    let fx = fixture("results.json");
    let mut tally = Tally::default();
    for case in fx.as_array().unwrap() {
        let tool = case["tool"].as_str().unwrap();
        let sc = &case["scenario"];
        let what = format!("{tool} {sc}");
        let net =
            || -> Map<String, Value> { sc["network"].as_object().cloned().unwrap_or_default() };
        let read = |k: &str| sc["read"][k].clone();
        let (result, told): (ferb_core::types::ToolResult, Vec<String>) = match tool {
            _ if sc.get("start_refused").is_some() => {
                let mut c = Map::new();
                c.insert("error".into(), sc["start_refused"].clone());
                (
                    ferb_core::types::ToolResult {
                        content: c,
                        is_error: true,
                        ..Default::default()
                    },
                    vec![],
                )
            }
            "shell.run" => {
                let i = info(&sc["info"]);
                let (r, ended) = run_result(
                    "p-1",
                    i.exit_code,
                    read("text").as_str().unwrap(),
                    read("offset").as_u64().unwrap(),
                    read("total").as_u64().unwrap(),
                    &i,
                    net(),
                );
                (r, if ended { vec!["p-1".into()] } else { vec![] })
            }
            "shell.start" => {
                let (r, ended) = start_result("p-1", sc["info"]["running"].as_bool().unwrap());
                (r, if ended { vec!["p-1".into()] } else { vec![] })
            }
            "shell.wait" | "shell.read" => {
                let i = info(&sc["info"]);
                let (r, ended) = output_result(
                    "p-1",
                    read("text").as_str().unwrap(),
                    read("offset").as_u64().unwrap(),
                    read("total").as_u64().unwrap(),
                    &i,
                    net(),
                );
                (r, if ended { vec!["p-1".into()] } else { vec![] })
            }
            "shell.send" => match sc.get("send_refused") {
                Some(msg) => {
                    let mut c = Map::new();
                    c.insert("error".into(), msg.clone());
                    (
                        ferb_core::types::ToolResult {
                            content: c,
                            is_error: true,
                            ..Default::default()
                        },
                        vec![],
                    )
                }
                None => (
                    send_result(
                        sc["send"][0].as_u64().unwrap() as usize,
                        sc["send"][1].as_u64().unwrap() as usize,
                    ),
                    vec![],
                ),
            },
            "shell.kill" => match sc.get("kill_refused") {
                Some(msg) => {
                    let mut c = Map::new();
                    c.insert("error".into(), msg.clone());
                    (
                        ferb_core::types::ToolResult {
                            content: c,
                            is_error: true,
                            ..Default::default()
                        },
                        vec![],
                    )
                }
                None => {
                    let mut c = Map::new();
                    c.insert("killed".into(), json!(true));
                    (
                        ferb_core::types::ToolResult {
                            content: c,
                            ..Default::default()
                        },
                        vec!["p-1".into()],
                    )
                }
            },
            "shell.list" => {
                let all: Vec<ProcInfo> = sc["list"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| ProcInfo {
                        id: p["id"].as_str().unwrap().into(),
                        owner_task: p["task"].as_str().unwrap().into(),
                        command: p["command"].as_str().unwrap().into(),
                        running: p["running"].as_bool().unwrap(),
                        exit_code: p["exit_code"].as_i64().map(|c| c as i32),
                        long_lived: p["background"].as_bool().unwrap(),
                        ..info(&json!({"running": true, "exit_code": null}))
                    })
                    .collect();
                list_result(&all, "t")
            }
            "shell.rules" => {
                let port = if sc.get("sandbox") == Some(&json!(false)) {
                    None
                } else {
                    sc["proxy_port"].as_u64().map(|p| p as u16)
                };
                let prot = Rules {
                    home: PathBuf::from("/h"),
                    pins: vec![LocalPort {
                        port: 5432,
                        executable: PathBuf::from("/bin/sh"),
                    }],
                };
                let mut r = rules_result(&prot, port);
                // the rules list itself is the policy part's (protection.rules()); Python's is taken as given
                r.content
                    .insert("rules".into(), case["content"]["rules"].clone());
                (r, vec![])
            }
            other => panic!("{other}"),
        };
        tally.check(
            &format!("{what} content"),
            Value::Object(result.content),
            case["content"].clone(),
        );
        tally.check(
            &format!("{what} is_error"),
            result.is_error,
            case["is_error"].as_bool().unwrap(),
        );
        tally.check(
            &format!("{what} sources"),
            sources(&result.read_sources),
            case["read_sources"].clone(),
        );
        let want_told: Vec<String> = strs(&case["told"]);
        tally.check(&format!("{what} told"), told, want_told);
    }
    tally.done("results");
}
