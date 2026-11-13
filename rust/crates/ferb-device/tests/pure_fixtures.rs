//! Replays rust/fixtures/device/pure.json: codesign parsing, requirement checks, launchctl wording, plists, the
//! device profile and its store, the app scan, persona facts, devtools, the argument parser, the toolbox's printed
//! lines, doctor's checks and the tokenizer copy, each against what the Python code produced.
// decision: DMN-509, DMN-511, DMN-502, DMN-506, DMN-3, DMN-4

mod support;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ferb_device::cli::admin::{self, MemoryArgs};
use ferb_device::cli::args::{parse, Parsed};
use ferb_device::cli::setup as S;
use ferb_device::cli::{plain_error, CliError, Rpc};
use ferb_device::install::{self, Output};
use ferb_device::profile::{diff, summarize, AppInfo, DeviceProfile, ProfileStore};
use ferb_device::scan::{self, BrowserCatalog};
use ferb_device::{devtools, persona, plistlib};
use serde_json::{json, Map, Value};
use support::*;

fn pure() -> Value {
    fixture("pure.json")
}

fn root() -> PathBuf {
    PathBuf::from("/private/tmp/ferb-device-fx/pure")
}

fn tree_under(dir: &Path) -> BTreeMap<String, Value> {
    snap(dir)
}

fn expected_tree(list: &Value) -> BTreeMap<String, Value> {
    expected_changes(list)
}

#[test]
fn codesign_requirements_and_launchctl_words() {
    let p = pure();
    for c in p["requirements"].as_array().unwrap() {
        let r = c["req"].as_str().unwrap();
        assert_eq!(
            install::trusted_requirement(r, false),
            c["trusted"].as_bool().unwrap(),
            "{r}"
        );
        assert_eq!(
            install::trusted_requirement(r, true),
            c["trusted_pinned"].as_bool().unwrap(),
            "{r}"
        );
        assert_eq!(json!(install::certificate_leaf(r)), c["leaf"], "{r}");
    }
    for c in p["codesign"].as_array().unwrap() {
        let o = c["out"].as_str().unwrap();
        assert_eq!(
            install::designated_requirement(o),
            c["designated"].as_str().unwrap(),
            "{o:?}"
        );
        assert_eq!(
            install::codesign_field(o, "CDHash"),
            c["cdhash"].as_str().unwrap(),
            "{o:?}"
        );
        assert_eq!(
            install::codesign_field(o, "TeamIdentifier"),
            c["team"].as_str().unwrap(),
            "{o:?}"
        );
    }
    for c in p["launchctl_words"].as_array().unwrap() {
        let out = Output {
            code: c["rc"].as_i64().unwrap() as i32,
            stdout: c["stdout"].as_str().unwrap_or("").into(),
            stderr: c["stderr"].as_str().unwrap_or("").into(),
        };
        assert_eq!(install::launchctl_words(&out), c["words"].as_str().unwrap());
    }
}

fn to_plist(v: &Value) -> plistlib::Value {
    match v {
        Value::String(s) => plistlib::Value::String(s.clone()),
        Value::Bool(b) => plistlib::Value::Bool(*b),
        Value::Number(n) => plistlib::Value::Integer(n.as_i64().unwrap()),
        Value::Array(a) => plistlib::Value::Array(a.iter().map(to_plist).collect()),
        Value::Object(m) => {
            plistlib::Value::Dict(m.iter().map(|(k, x)| (k.clone(), to_plist(x))).collect())
        }
        Value::Null => panic!("no null in a plist"),
    }
}

#[test]
fn plists_are_written_as_plistlib_writes_them() {
    let p = pure();
    for c in p["plists"].as_array().unwrap() {
        let got = plistlib::dumps(&to_plist(&c["value"])).unwrap();
        assert_eq!(hex::encode(got), c["hex"].as_str().unwrap());
    }
    let bad = &p["plist_errors"][0];
    let mut d = std::collections::BTreeMap::new();
    d.insert(
        "k".to_string(),
        plistlib::Value::String(bad["value"].as_str().unwrap().into()),
    );
    assert_eq!(
        plistlib::dumps(&plistlib::Value::Dict(d))
            .unwrap_err()
            .to_string(),
        bad["error"].as_str().unwrap()
    );
}

#[test]
fn device_profiles_diff_and_their_store() {
    let p = pure()["profile"].clone();
    let load = |k: &str| DeviceProfile::from_json(&p[k].to_string()).unwrap();
    let (a, b, c) = (load("a"), load("b"), load("c"));
    assert_eq!(a.to_json(), p["a_json"].as_str().unwrap());
    let d = diff(&a, &b);
    assert_eq!(
        json!({"added": d.added, "removed": d.removed, "changed": d.changed}),
        p["diff"]
    );
    assert_eq!(summarize(&b), p["summary"]);
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let store = ProfileStore::new(&home);
    let saves = vec![
        store.save(&a, "first").unwrap(),
        store.save(&b, "device scan").unwrap(),
        store.save(&c, "only the time").unwrap(),
    ];
    assert_eq!(json!(saves), p["saves"]);
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(store.history_path())
        .unwrap()
        .write_all(b"not json\n{\"no\": 1}\n")
        .unwrap();
    assert_eq!(json!(store.history()), p["history"]);
    assert_eq!(tree_under(&home), expected_tree(&p["files"]));
}

struct Catalog(Value);

impl BrowserCatalog for Catalog {
    fn known(&self, id: &str) -> bool {
        self.0[id]["known"].as_bool().unwrap()
    }
    fn pair_key(&self, id: &str) -> String {
        self.0[id]["pair_key"].as_str().unwrap().into()
    }
    fn paired(&self, id: &str) -> bool {
        self.0[id]["paired"].as_bool().unwrap()
    }
}

/// One test owns the pure root (the scan, persona and devtools cases read files there).
#[test]
fn scan_persona_and_devtools_on_invented_bundles() {
    let p = pure();
    let base = root();
    let _ = std::fs::remove_dir_all(&base);
    let sroot = base.join("Apps");
    std::fs::create_dir_all(&sroot).unwrap();
    apply(&sroot, &p["scan"]["files"], &[]);
    // the app whose Info.plist is a list (Python's scan then raised AttributeError; ENG-6: Rust skips that app)
    let listed = sroot.join("Listed.app/Contents");
    std::fs::create_dir_all(&listed).unwrap();
    std::fs::write(
        listed.join("Info.plist"),
        plistlib::dumps(&plistlib::Value::Array(vec![plistlib::Value::string(
            "not",
        )]))
        .unwrap(),
    )
    .unwrap();
    for r in p["scan"]["reads"].as_array().unwrap() {
        let bundle = sroot.join(r["bundle"].as_str().unwrap());
        let got = scan::read_app(&bundle)
            .map(|a| serde_json::to_value(a).unwrap())
            .unwrap_or(Value::Null);
        if r.get("error").is_some() {
            // decision: ENG-6 — the one deliberate difference: Python raised, Rust leaves the app out
            assert_eq!(got, Value::Null, "{bundle:?}");
        } else {
            assert_eq!(got, r["app"], "{bundle:?}");
        }
    }
    std::fs::remove_file(listed.join("Info.plist")).unwrap();
    let catalog = Catalog(p["scan"]["catalog"].clone());
    let prof = &p["scan"]["profile"];
    let got = scan::scan(
        "dev2",
        &[sroot.clone(), base.join("Missing")],
        &catalog,
        prof["os_version"].as_str().unwrap(),
        prof["scanned_at"].as_str().unwrap(),
    );
    // Listed.app is still a folder without an Info.plist here, as in Python's run
    assert_eq!(serde_json::to_value(&got).unwrap(), *prof);
    for (name, web) in p["scan"]["web"].as_object().unwrap() {
        assert_eq!(
            scan::registers_web(&sroot.join(name)),
            web.as_bool().unwrap(),
            "{name}"
        );
    }

    // persona
    let pp = &p["persona"];
    let apps: Vec<AppInfo> = serde_json::from_value(pp["apps"].clone()).unwrap();
    let raw = hex::decode(pp["mdls_hex"].as_str().unwrap()).unwrap();
    let rows = persona::usage_rows(&apps, &raw);
    assert_eq!(json!(rows), pp["rows"]);
    assert_eq!(
        json!(persona::usage_rows(&apps, b"not a plist")),
        pp["bad_rows"]
    );
    assert_eq!(json!(persona::usage_command(&apps)), pp["command"]);
    let activity: Vec<Map<String, Value>> = serde_json::from_value(pp["activity"].clone()).unwrap();
    let with = persona::with_activity(&rows, &activity);
    assert_eq!(json!(with), pp["with_activity"]);
    let profiles = vec![
        json!({"browser": "Google Chrome", "name": "Work", "email": "w@example.invalid", "last_used": true}),
    ];
    let facts = persona::mac_facts(
        &with,
        "2026-11-13T09:30:05+05:30",
        &["chrome".to_string()],
        &profiles,
        true,
    );
    assert_eq!(facts, pp["facts"]);
    assert_eq!(
        persona::facts_fingerprint(&facts),
        pp["fingerprint"].as_str().unwrap()
    );

    // devtools
    let dv = &p["devtools"];
    let bins = base.join("bins");
    std::fs::create_dir_all(&bins).unwrap();
    apply(&bins, &dv["files"], &[]);
    let stub = PathBuf::from(dv["stub_folder"].as_str().unwrap());
    for c in dv["cases"].as_array().unwrap() {
        let (rc, out) = (
            c["rc"].as_i64().unwrap() as i32,
            c["out"].as_str().unwrap().to_string(),
        );
        let mut run = |_argv: &[String]| {
            Some(Output {
                code: rc,
                stdout: out.clone(),
                stderr: String::new(),
            })
        };
        assert_eq!(
            json!(devtools::missing_tools(
                c["path"].as_str().unwrap(),
                &stub,
                &mut run
            )),
            c["missing"],
            "{c}"
        );
    }
    assert_eq!(
        devtools::missing_fact(&[]),
        dv["facts"][0].as_str().unwrap()
    );
    assert_eq!(
        devtools::missing_fact(&["python3".into(), "git".into()]),
        dv["facts"][1].as_str().unwrap()
    );
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn the_argument_parser_reads_every_command_line_as_argparse() {
    // decision: DMN-3
    let p = pure();
    for c in p["argv"].as_array().unwrap() {
        let argv: Vec<String> = c["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap().to_string())
            .collect();
        match parse(&argv) {
            Parsed::Version => assert_eq!(c["exit"], 0, "{argv:?}"),
            Parsed::Args(ns) => {
                let want: BTreeMap<String, Value> = c["args"]
                    .as_object()
                    .unwrap_or_else(|| panic!("{argv:?}: Python refused"))
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                assert_eq!(ns, want, "{argv:?}");
            }
            Parsed::Error(e) => assert_eq!(c["exit"], 2, "{argv:?}: Rust refused ({e})"),
        }
    }
    for app in ferb_device::cli::APP_COMMANDS {
        let argv: Vec<String> = app.iter().map(|s| s.to_string()).collect();
        assert!(matches!(parse(&argv), Parsed::Args(_)), "{argv:?}");
    }
}

/// tests' Canned client: answers by method (a list marked `__seq__` answers in turn), records every call.
struct Canned {
    answers: Map<String, Value>,
    calls: RefCell<Vec<Value>>,
}

impl Rpc for Canned {
    fn call(&mut self, method: &str, params: Option<Value>) -> Result<Value, CliError> {
        self.calls.borrow_mut().push(json!([method, params]));
        let a = self
            .answers
            .get_mut(method)
            .unwrap_or_else(|| panic!("no answer for {method}"));
        if let Some(list) = a.as_array_mut() {
            if list.first().is_some_and(|x| x.get("__seq__").is_some()) {
                return Ok(list.remove(0)["value"].clone());
            }
        }
        Ok(a.clone())
    }
    fn set_timeout(&mut self, seconds: Option<f64>) {
        self.calls
            .borrow_mut()
            .push(json!(["set_timeout", seconds]));
    }
}

fn opt(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(String::from)
}

#[test]
fn the_toolbox_prints_what_python_printed() {
    std::env::set_var("TZ", "UTC");
    let p = pure();
    let mut n = 0;
    for t in p["toolbox"].as_array().unwrap() {
        let name = t["name"].as_str().unwrap();
        let mut c = Canned {
            answers: t["answers"].as_object().unwrap().clone(),
            calls: RefCell::new(Vec::new()),
        };
        let mut out: Vec<u8> = Vec::new();
        let args = &t["args"];
        let mut replies: Vec<String> = args
            .get("replies")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
            .unwrap_or_default();
        let mut ask = |_prompt: &str| replies.remove(0);
        let mut sleep = |_s: f64| {};
        let mut copy = |_p: &str| true;
        let rc = if name == "verdict" {
            admin::verdict(&mut c, "t-1", "pass", "fine", &mut out)
        } else if name.starts_with("ps") {
            admin::ps(&mut c, &mut out)
        } else if name == "browsers_list" {
            admin::browsers(&mut c, None, None, None, &mut out, &mut sleep, &mut copy)
        } else if name.starts_with("browsers_unpair") {
            let browser = if name == "browsers_unpair" {
                Some("chrome")
            } else {
                None
            };
            admin::browsers(
                &mut c,
                Some("unpair"),
                Some("Work"),
                browser,
                &mut out,
                &mut sleep,
                &mut copy,
            )
        } else if name.starts_with("browsers_pair") {
            let (profile, browser) = match name {
                "browsers_pair" | "browsers_pair_install" => ("Work", Some("chrome")),
                "browsers_pair_install_gives_up" => ("Home", None),
                _ => ("Work", None),
            };
            admin::browsers(
                &mut c,
                Some("pair"),
                Some(profile),
                browser,
                &mut out,
                &mut sleep,
                &mut copy,
            )
        } else if name.starts_with("restore_list") || name == "restore_none" {
            admin::restore(&mut c, None, &mut out)
        } else if name.starts_with("restore_") {
            admin::restore(&mut c, Some("ab12"), &mut out)
        } else if name.starts_with("memory_") {
            let m = MemoryArgs {
                mcmd: args["mcmd"].as_str().unwrap().into(),
                subject: Some(opt(args, "subject").unwrap_or_else(|| "owner".into())),
                content: opt(args, "content"),
                kind: opt(args, "kind"),
                key: None,
                id: opt(args, "id"),
                query: opt(args, "query"),
                app: opt(args, "app"),
                person: None,
                batch: opt(args, "batch"),
                all: args["all"].as_bool().unwrap_or(false),
                proposed: args["proposed"].as_bool().unwrap_or(false),
                yes: args["yes"].as_bool().unwrap_or(false),
                keep_sources: args["keep_sources"].as_bool().unwrap_or(false),
            };
            admin::memory(&mut c, &m, &mut ask, &mut out)
        } else {
            panic!("unknown toolbox run {name}");
        };
        assert_eq!(json!(rc.unwrap()), t["rc"], "{name}: exit");
        assert_eq!(
            String::from_utf8(out).unwrap(),
            t["out"].as_str().unwrap(),
            "{name}: printed"
        );
        assert_eq!(json!(c.calls.into_inner()), t["calls"], "{name}: calls");
        n += 1;
    }
    assert_eq!(n, 35);
    for e in p["log_events"].as_array().unwrap() {
        let line = ferb_device::cli::logs::format_event(&e["event"], &|_| unreachable!()).unwrap();
        assert_eq!(line, e["line"].as_str().unwrap());
    }
    for e in p["plain_error"].as_array().unwrap() {
        let m = e["message"].as_str().unwrap().to_string();
        let err = match e["code"].as_i64() {
            Some(code) => CliError::Rpc { code, message: m },
            None => CliError::ferb(m),
        };
        assert_eq!(plain_error(&err), e["line"].as_str().unwrap());
    }
    for c in p["shown_bytes"].as_array().unwrap() {
        assert_eq!(
            admin::shown_bytes(c[0].as_f64().unwrap()),
            c[1].as_str().unwrap()
        );
    }
    for c in p["status_label"].as_array().unwrap() {
        assert_eq!(
            admin::status_label(c[0].as_str().unwrap()),
            c[1].as_str().unwrap()
        );
    }
}

fn opt_check(c: Option<S::Check>) -> Value {
    c.map(|c| c.json()).unwrap_or(Value::Null)
}

#[test]
fn doctors_checks_word_each_case_as_python() {
    std::env::set_var("TZ", "UTC");
    let checks = pure()["checks"].clone();
    let roles = ["worker", "planner", "checker"];
    let lims = [
        json!({"runtime": "codex", "model": "gpt-x", "context_window": 272000, "max_output": 128000}),
        json!({"runtime": "codex", "model": "gpt-x", "context_window": 1, "max_output": 2, "max_input_chars": 1048576}),
        json!({"runtime": "claude", "model": "m", "problem": "no documented limits"}),
    ];
    for (i, (r, l)) in roles.iter().zip(lims.iter()).enumerate() {
        assert_eq!(
            S::context_window_check(r, l).json(),
            checks["context_window"][i]
        );
    }
    for (i, st) in [
        None,
        Some("allow"),
        Some("unrestricted"),
        Some("default"),
        Some("ask"),
        Some("deny"),
        Some("odd"),
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(opt_check(S::clipboard_check(*st)), checks["clipboard"][i]);
    }
    let cont = [
        Value::Null,
        json!({}),
        json!({"profile_ok": true, "shell": true}),
        json!({"profile_ok": true, "shell": true, "proxy": true, "proxy_alive": false, "proxy_last_error": "refused"}),
        json!({"profile_ok": true, "shell": true, "proxy_restarted": true}),
        json!({"profile_ok": false, "reason": "sandbox-exec missing"}),
        json!({"profile_ok": true, "shell": false, "shell_reason": "denied"}),
    ];
    for (i, c) in cont.iter().enumerate() {
        let c = (!c.is_null()).then_some(c);
        assert_eq!(
            S::containment_check(c).json(),
            checks["containment"][i],
            "{c:?}"
        );
    }
    let local = [
        Value::Null,
        json!({}),
        json!({"local_reports": {"running": true}}),
        json!({"local_reports": {"running": false}}),
        json!({"local_reports": {"running": false, "stopped": "log stream ended"}}),
    ];
    for (i, c) in local.iter().enumerate() {
        let c = (!c.is_null()).then_some(c);
        assert_eq!(
            opt_check(S::local_reports_check(c)),
            checks["local_reports"][i]
        );
    }
    let snaps = [
        Value::Null,
        json!({"snapshots": false}),
        json!({"snapshot_failure": {"off": true}}),
        json!({"snapshot_failure": {"at": "2026-11-12T08:05:00+05:30", "why": "tmutil failed"}}),
        json!({"snapshot_failure": {"at": "not a time", "why": "x"}}),
        json!({"snapshot_failure": {"why": "y"}}),
        json!({"snapshots": true}),
    ];
    for (i, c) in snaps.iter().enumerate() {
        let c = (!c.is_null()).then_some(c);
        assert_eq!(S::snapshot_check(c).json(), checks["snapshots"][i], "{c:?}");
    }
    for h in checks["hearing"].as_array().unwrap() {
        let mut got = Vec::new();
        let input = (!h["in"].is_null()).then_some(&h["in"]);
        S::add_hearing_check(&mut |c| got.push(c.json()), input);
        assert_eq!(json!(got), h["out"], "{}", h["in"]);
    }
    for (key, f) in [
        ("sip", S::sip_check as fn(&mut S::Run<'_>) -> S::Check),
        ("filevault", S::filevault_check),
    ] {
        for c in checks[key].as_array().unwrap() {
            let (rc, out, raises) = (
                c["rc"].as_i64().unwrap() as i32,
                c["out"].as_str().unwrap().to_string(),
                c["raises"].as_bool().unwrap(),
            );
            let mut run: S::Run<'_> = Box::new(move |_argv: &[String], _o: &S::RunOpts| {
                if raises {
                    Err("no such program".into())
                } else {
                    Ok(Output {
                        code: rc,
                        stdout: out.clone(),
                        stderr: String::new(),
                    })
                }
            });
            assert_eq!(f(&mut run).json(), c["check"], "{key} {c}");
        }
    }
}

#[test]
fn the_pinned_tokenizer_copy_is_put_in_place_as_python_does() {
    let p = pure()["tokenizer"].clone();
    assert_eq!(
        p["url"].as_str().unwrap(),
        ferb_device::cli::tokenizer::TOKENIZER_URL
    );
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("tok");
    let pin = p["pin"].as_str().unwrap();
    let target = ferb_device::cli::tokenizer::tokenizer_path(&home);
    for (i, c) in p["cases"].as_array().unwrap().iter().enumerate() {
        if c["name"] == "cached_copy" {
            std::fs::write(&target, b"tampered").unwrap();
        }
        if c["name"] == "mismatch" {
            let _ = std::fs::remove_file(&target);
        }
        let before = snap(dir.path());
        let cache = c["cache"].as_str().map(|s| s.as_bytes().to_vec());
        let fetched = c["fetched"].clone();
        let mut verified = || cache.clone();
        let mut fetch = |_url: &str| match &fetched {
            Value::String(s) => Ok(s.as_bytes().to_vec()),
            other => Err((
                other[0].as_str().unwrap().to_string(),
                other[1].as_str().unwrap().to_string(),
            )),
        };
        let line = ferb_device::cli::tokenizer::ensure(&home, pin, &mut verified, &mut fetch);
        assert_eq!(line, c["line"].as_str().unwrap(), "case {i}");
        let got = changes(&before, &snap(dir.path()));
        let want = expected_changes(&c["changes"]);
        assert_eq!(got, want, "case {i} files");
    }
}

#[test]
fn persona_state_moves_out_of_config_once() {
    let p = pure()["persona_state"].clone();
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("persona");
    std::fs::create_dir(&home).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(
        home.join("config.toml"),
        p["config_before"].as_str().unwrap(),
    )
    .unwrap();
    std::fs::set_permissions(
        home.join("config.toml"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let state = persona::PersonaState::new(&home);
    let steps = p["steps"].as_array().unwrap();
    // Open: ferb-core's Config.remove refuses to remove a sub-table ([setup.persona_pending]), which tomlkit does;
    // persona.json is written first (as in Python), so the state is moved and read from there, but the old keys
    // stay in config.toml. Python's config text after the move is steps[1].
    match state.pending() {
        Err(persona::PersonaError::Config(ferb_core::config::ConfigError::Unsupported(_))) => {}
        other => panic!("the sub-table removal is refused today, got {other:?}"),
    }
    assert_ne!(
        std::fs::read_to_string(home.join("config.toml")).unwrap(),
        steps[1]["text"].as_str().unwrap()
    );
    assert_eq!(Value::Object(state.pending().unwrap()), steps[0]["result"]);
    let seen: Map<String, Value> = serde_json::from_value(steps[3]["result"].clone()).unwrap();
    state.set_seen(seen).unwrap();
    assert_eq!(json!(state.reviewed().unwrap()), steps[2]["result"]);
    assert_eq!(Value::Object(state.seen().unwrap()), steps[3]["result"]);
    assert_eq!(
        std::fs::read_to_string(home.join("persona.json")).unwrap(),
        steps[4]["text"].as_str().unwrap()
    );
    let mode = std::fs::metadata(home.join("persona.json"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(json!(mode), steps[4]["mode"]);
}

#[test]
fn the_code_hash_is_pythons() {
    let p = pure()["code_hash"].clone();
    let dir = tempfile::tempdir().unwrap();
    apply(dir.path(), &p["files"], &[]);
    let pkg = dir.path().join("ferb");
    assert_eq!(
        install::code_hash(&pkg).unwrap(),
        p["with_extension"].as_str().unwrap()
    );
    std::fs::remove_dir_all(dir.path().join("extensions")).unwrap();
    assert_eq!(
        install::code_hash(&pkg).unwrap(),
        p["without_extension"].as_str().unwrap()
    );
}

/// Spotlight's answer as the fixture's mdls stand-in gives it: every other app (in the order asked) has a use count
/// of 10 + its place and a last use on the (1 + place)th of November.
fn mdls_answer(argv: &[String]) -> Vec<u8> {
    let at = argv
        .iter()
        .position(|a| a == "kMDItemLastUsedDate")
        .unwrap();
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\">\n<array>\n",
    );
    for (n, path) in argv[at + 1..].iter().enumerate() {
        if n % 2 == 0 {
            xml.push_str(&format!(
                "<dict><key>kMDItemPath</key><string>{path}</string><key>kMDItemUseCount</key><integer>{}</integer>\
                 <key>kMDItemLastUsedDate</key><date>2026-11-{:02}T08:00:00Z</date></dict>\n",
                10 + n,
                1 + n
            ));
        }
    }
    xml.push_str("</array>\n</plist>\n");
    xml.into_bytes()
}

/// persona.py:204 `discover_persona` on a fake home against rust/fixtures/device/persona.json (make_persona.py): the
/// scan of invented bundles saved to FERB's home, the mdls command asked, Spotlight's counts with the owner's focus
/// events, the browsers' profiles only with Full Disk Access (DMN-507), a failure reading them as none.
#[test]
fn discover_persona_on_a_fake_home_is_pythons() {
    let f = fixture("persona.json");
    let p = pure();
    let base = PathBuf::from("/private/tmp/ferb-device-fx/persona");
    let _ = std::fs::remove_dir_all(&base);
    let apps = base.join("Apps");
    std::fs::create_dir_all(&apps).unwrap();
    apply(&apps, &p["scan"]["files"], &[]);
    let catalog = Catalog(p["scan"]["catalog"].clone());
    let cases = f["cases"].as_array().unwrap();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let home = base.join(format!("home-{name}"));
        std::fs::create_dir_all(&home).unwrap();
        let activity: Vec<Map<String, Value>> = case["activity"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_object().unwrap().clone())
            .collect();
        let excluded: std::collections::BTreeSet<String> = case["excluded"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e.as_str().unwrap().to_string())
            .collect();
        let asked: RefCell<Vec<Vec<String>>> = RefCell::new(Vec::new());
        let allowed = case["allowed"].as_bool().unwrap();
        let facts = persona::discover_persona(
            &home,
            &activity,
            &excluded,
            persona::Discover {
                scan: &mut || {
                    scan::scan(
                        "dev-fx",
                        std::slice::from_ref(&apps),
                        &catalog,
                        "15.4",
                        "2026-11-13T09:30:05+00:00",
                    )
                },
                usage: &mut |listed: &[AppInfo]| {
                    persona::app_usage(listed, &mut |argv: &[String]| {
                        asked.borrow_mut().push(argv.to_vec());
                        Ok(mdls_answer(argv))
                    })
                },
                allowed: &mut || allowed,
                profiles: &mut || match &case["profiles"] {
                    Value::Array(a) => Ok(a.clone()),
                    _ => Err("Local State unreadable".into()),
                },
            },
        )
        .unwrap();
        assert_eq!(
            ferb_py::json::dumps(&facts),
            ferb_py::json::dumps(&case["facts"]),
            "{name}"
        );
        assert_eq!(json!(asked.into_inner()), case["mdls"], "{name}: mdls");
        for (rel, want) in case["files"].as_object().unwrap() {
            let got = std::fs::read_to_string(home.join(rel)).unwrap();
            assert_eq!(got, want.as_str().unwrap(), "{name}: {rel}");
        }
    }
    let _ = std::fs::remove_dir_all(&base);
    println!("{} cases equal", cases.len());
}
