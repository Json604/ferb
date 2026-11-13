//! Replays rust/fixtures/device/cli.json: ferb/cli/setup.py's commands (setup --service, a checkout's setup,
//! setup --cli, uninstall, the sign-ins and sign-outs, the clean-up, doctor) with the same commands, daemon calls,
//! questions, printed lines, exit statuses and files as Python.
// decision: DMN-513, DMN-515, DMN-537, DMN-589, DMN-20, DMN-24, DMN-502, DMN-506, DMN-21

mod support;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ferb_device::cli::setup::{self as S, Env, PermissionFacts, RunOpts, RuntimeFacts, StoreFacts};
use ferb_device::cli::{CliError, NoSignals, Rpc};
use ferb_device::install::{CodexPin, Installer, Output, Sources};
use ferb_device::scan::BrowserCatalog;
use serde_json::{json, Value};
use support::*;

fn s(v: &Value) -> String {
    subst(v.as_str().expect("a string"))
}

fn sv(v: &Value) -> Value {
    match v {
        Value::String(t) => json!(subst(t)),
        Value::Array(a) => Value::Array(a.iter().map(sv).collect()),
        Value::Object(m) => Value::Object(m.iter().map(|(k, x)| (k.clone(), sv(x))).collect()),
        other => other.clone(),
    }
}

struct Catalog(Value);

impl BrowserCatalog for Catalog {
    fn known(&self, id: &str) -> bool {
        self.0[id]["known"]
            .as_bool()
            .unwrap_or_else(|| panic!("no catalog answer for {id}"))
    }
    fn pair_key(&self, id: &str) -> String {
        self.0[id]["pair_key"].as_str().unwrap().into()
    }
    fn paired(&self, id: &str) -> bool {
        self.0[id]["paired"].as_bool().unwrap()
    }
}

struct ReplayRpc(Shared);

fn rpc_error(e: &Value) -> CliError {
    let message = s(&e["message"]);
    match e["type"].as_str().unwrap() {
        "NotRunning" => CliError::NotRunning(message),
        "RpcError" => CliError::Rpc {
            code: e["code"].as_i64().unwrap(),
            message,
        },
        other => CliError::Ferb {
            kind: other.into(),
            message,
        },
    }
}

impl Rpc for ReplayRpc {
    fn call(&mut self, method: &str, params: Option<Value>) -> Result<Value, CliError> {
        let mut sc = self.0.borrow_mut();
        let ev = sc.next("rpc", method);
        assert_eq!(
            ev["op"], "call",
            "{}: Python's next daemon event was {ev}, Rust called {method}",
            sc.context
        );
        assert_eq!(ev["method"], json!(method), "{}: method", sc.context);
        assert_eq!(
            sv(&ev["params"]),
            params.unwrap_or(Value::Null),
            "{}: params of {method}",
            sc.context
        );
        match ev.get("error") {
            Some(e) => Err(rpc_error(e)),
            None => Ok(sv(&ev["result"])),
        }
    }
    fn close(&mut self) {
        let mut sc = self.0.borrow_mut();
        let ev = sc.next("rpc", "close");
        assert_eq!(
            ev["op"], "close",
            "{}: Python's next daemon event was {ev}, Rust closed",
            sc.context
        );
    }
}

struct Perms(Shared);

impl PermissionFacts for Perms {
    fn missing(&mut self, _report: &Value) -> Vec<(String, String)> {
        let ev = self
            .0
            .borrow_mut()
            .next("perm_missing", "permissions.missing");
        ev["result"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (s(&p[0]), s(&p[1])))
            .collect()
    }
    fn fda_ready(&mut self, containment: &Value) -> bool {
        let mut sc = self.0.borrow_mut();
        let ev = sc.next("fda_ready", "permissions.fda_ready");
        assert_eq!(
            ev["containment"], *containment,
            "{}: fda_ready's containment",
            sc.context
        );
        ev["result"].as_bool().unwrap()
    }
}

struct Store(Shared);

impl StoreFacts for Store {
    fn preview(&mut self, path: &Path) -> Value {
        let ev = self.0.borrow_mut().next("store", "preview");
        assert_eq!(
            (ev["op"].as_str(), s(&ev["path"])),
            (Some("preview"), path.to_string_lossy().into_owned())
        );
        sv(&ev["result"])
    }
    fn apply(
        &mut self,
        home: &Path,
        ok: &str,
        stop: &dyn Fn() -> bool,
        _say: &mut dyn FnMut(&str),
    ) -> Result<Value, (String, String)> {
        let mut sc = self.0.borrow_mut();
        let open = sc.next("store", "open");
        assert_eq!(
            (open["op"].as_str(), s(&open["path"])),
            (
                Some("open"),
                home.join("ferb.db").to_string_lossy().into_owned()
            )
        );
        let ev = sc.next("store", "apply");
        assert_eq!(
            (ev["op"].as_str(), ev["ok"].as_str()),
            (Some("apply"), Some(ok)),
            "{}",
            sc.context
        );
        let out = match ev.get("error") {
            Some(e) => Err((s(&e[0]), s(&e[1]))),
            None => {
                assert_eq!(ev["stopped_asked"], json!(stop()), "{}: stop", sc.context);
                Ok(sv(&ev["result"]))
            }
        };
        let close = sc.next("store", "close");
        assert_eq!(close["op"], "close");
        out
    }
    fn pending(&mut self, path: &Path) -> bool {
        let ev = self.0.borrow_mut().next("store", "pending");
        assert_eq!(
            (ev["op"].as_str(), s(&ev["path"])),
            (Some("pending"), path.to_string_lossy().into_owned())
        );
        ev["result"].as_bool().unwrap()
    }
    fn size(&mut self, path: &Path) -> u64 {
        let ev = self.0.borrow_mut().next("store", "size");
        assert_eq!(
            (ev["op"].as_str(), s(&ev["path"])),
            (Some("size"), path.to_string_lossy().into_owned())
        );
        ev["result"].as_u64().unwrap()
    }
}

struct Runtimes(Shared);

impl RuntimeFacts for Runtimes {
    fn available(&mut self, name: &str, _home: &Path) -> Option<(bool, String)> {
        let mut sc = self.0.borrow_mut();
        let ev = sc.next("runtime_available", name);
        assert_eq!(ev["name"], json!(name), "{}", sc.context);
        Some((ev["result"][0].as_bool().unwrap(), s(&ev["result"][1])))
    }
}

/// A Write that keeps what was printed.
#[derive(Clone, Default)]
struct Out(Rc<RefCell<Vec<u8>>>);

impl std::io::Write for Out {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn run_seam(script: Shared) -> S::Run<'static> {
    Box::new(move |argv: &[String], opts: &RunOpts| {
        let mut sc = script.borrow_mut();
        let ev = sc.next("run", &format!("{argv:?}"));
        let expected: Vec<String> = ev["argv"].as_array().unwrap().iter().map(s).collect();
        assert_eq!(argv, expected.as_slice(), "{}: program", sc.context);
        let env: BTreeMap<String, String> = opts.env.iter().cloned().collect();
        let want: BTreeMap<String, String> = ev["env"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), s(v)))
            .collect();
        assert_eq!(env, want, "{}: environment of {argv:?}", sc.context);
        let mut flags = Vec::new();
        if opts.capture {
            flags.extend(["capture_output", "text"]);
        }
        if opts.timeout.is_some() {
            flags.push("timeout");
        }
        flags.sort();
        let want_flags: Vec<String> = ev["flags"].as_array().unwrap().iter().map(s).collect();
        assert_eq!(flags, want_flags, "{}: flags of {argv:?}", sc.context);
        let root = sc.root.clone();
        apply(&root, &ev["effects"], &[]);
        Ok(Output {
            code: ev["rc"].as_i64().unwrap() as i32,
            stdout: s(&ev["stdout"]),
            stderr: s(&ev["stderr"]),
        })
    })
}

struct World {
    script: Shared,
    clock: Stepped,
    root: PathBuf,
    user_home: PathBuf,
    codex: CodexPin,
    catalog: Value,
    scanned_at: String,
    os_version: String,
    claude_cli: PathBuf,
    out: Out,
}

impl World {
    fn installer(&self, cfg: &Value) -> Installer {
        let mut inst = Installer::new(
            PathBuf::from(s(&cfg["home"])),
            PathBuf::from(s(&cfg["launch_agents"])),
            Box::new(ScriptRunner(self.script.clone(), "cmd")),
            Box::new(self.clock.clone()),
            self.user_home.clone(),
            self.codex.clone(),
            Sources {
                repo_root: self.root.join("src/repo"),
                helper_source: PathBuf::from("$REPO/ferb/identity/secret_helper.swift"),
                extension: self.root.join("src/extension"),
            },
        );
        inst.label = s(&cfg["label"]);
        inst.uid = UID;
        inst.native_hosts_dirs = cfg["native_hosts_dirs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| PathBuf::from(s(d)))
            .collect();
        inst.extension_ids = cfg["extension_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(s)
            .collect();
        inst.bundle = cfg["bundle"].as_str().map(PathBuf::from);
        inst.cli_dir = cfg["cli_dir"].as_str().map(PathBuf::from);
        inst.app_service = cfg["app_service"].as_bool();
        inst
    }

    fn env(&self, inst: Installer) -> Env<'static> {
        let script = self.script.clone();
        let (a, b, c, d, e, f, g, h) = (
            script.clone(),
            script.clone(),
            script.clone(),
            script.clone(),
            script.clone(),
            script.clone(),
            script.clone(),
            script.clone(),
        );
        let apps = self.root.join("Applications");
        let catalog = Catalog(self.catalog.clone());
        let (scanned_at, os_version) = (self.scanned_at.clone(), self.os_version.clone());
        let claude_cli = self.claude_cli.clone();
        Env {
            inst,
            connect: Box::new(move || {
                let ev = a.borrow_mut().next("rpc", "connect");
                assert_eq!(ev["op"], "connect");
                match ev.get("error") {
                    Some(e) => Err(CliError::NotRunning(s(e))),
                    None => Ok(Box::new(ReplayRpc(a.clone())) as Box<dyn Rpc>),
                }
            }),
            ask: Box::new(move |prompt: &str| {
                let mut sc = b.borrow_mut();
                let ev = sc.next("ask", prompt);
                assert_eq!(s(&ev["prompt"]), prompt, "{}: question", sc.context);
                s(&ev["answer"])
            }),
            out: Box::new(self.out.clone()),
            sleep: Box::new(move |secs: f64| {
                let ev = c.borrow_mut().next("sleep", "sleep");
                assert_eq!(ev["seconds"].as_f64(), Some(secs));
            }),
            run: run_seam(script.clone()),
            tokenizer: Box::new(move |home: &Path| {
                let ev = d.borrow_mut().next("tokenizer", "tokenizer");
                assert_eq!(s(&ev["home"]), home.to_string_lossy());
                "Tokenizer: FERB's pinned copy is in place.".into()
            }),
            scan: Box::new(move |id: &str| {
                ferb_device::scan::scan(
                    id,
                    std::slice::from_ref(&apps),
                    &catalog,
                    &os_version,
                    &scanned_at,
                )
            }),
            browser_support: None,
            browsers: None,
            permissions: Box::new(Perms(e)),
            store: Box::new(Store(f)),
            runtimes: Box::new(Runtimes(g)),
            protection: Box::new(move |_home: &Path| {
                let ev = h.borrow_mut().next("protection", "protection.load");
                if ev["fail"].as_bool().unwrap() {
                    Err("[protection] never_read entry 3 isn't a path".into())
                } else {
                    Ok((1, 2))
                }
            }),
            codex_catalog: {
                let sc = script.clone();
                Box::new(move |home: &Path| {
                    let ev = sc.borrow_mut().next("catalog", "bundled_catalog");
                    assert_eq!(s(&ev["home"]), home.to_string_lossy());
                    ev["result"].as_bool().unwrap()
                })
            },
            claude_cli: {
                let sc = script.clone();
                Box::new(move || {
                    let ev = sc.borrow_mut().next("claude_cli", "cli_binary");
                    if ev["installed"].as_bool().unwrap() {
                        Ok(claude_cli.clone())
                    } else {
                        Err("the Claude Agent SDK's bundled Claude Code is missing".into())
                    }
                })
            },
            checkout_code_hash: None,
            signals: Box::new(NoSignals),
        }
    }
}

fn result_json(r: Result<i32, CliError>) -> Result<Value, Value> {
    match r {
        Ok(rc) => Ok(json!({"rc": rc})),
        Err(CliError::Interrupted) => Ok(json!({"rc": 130, "interrupted": true})),
        Err(e) => Err(json!({"type": e.type_name(), "message": e.to_string()})),
    }
}

#[test]
fn every_cli_scenario_matches_python() {
    // snapshot_check and restore read local times: the fixtures were made in UTC
    std::env::set_var("TZ", "UTC");
    let fx = fixture("cli.json");
    let pure = fixture("pure.json");
    let codex = CodexPin {
        version: fx["constants"]["codex_version"].as_str().unwrap().into(),
        native_bin: PathBuf::from(fx["constants"]["native_bin"].as_str().unwrap()),
    };
    let (mut steps, mut events) = (0, 0);
    for sc in fx["scenarios"].as_array().unwrap() {
        let name = sc["name"].as_str().unwrap();
        let root = PathBuf::from(sc["root"].as_str().unwrap());
        materialize(&root, &sc["initial"]);
        let world = World {
            script: Shared::default(),
            clock: Stepped::default(),
            root: root.clone(),
            user_home: PathBuf::from(sc["user_home"].as_str().unwrap()),
            codex: codex.clone(),
            catalog: pure["scan"]["catalog"].clone(),
            scanned_at: fx["constants"]["scanned_at"].as_str().unwrap().into(),
            os_version: fx["constants"]["os_version"].as_str().unwrap().into(),
            claude_cli: PathBuf::from(sc.get("claude_cli").and_then(Value::as_str).unwrap_or("")),
            out: Out::default(),
        };
        world.script.borrow_mut().root = root.clone();
        let mut env: Option<Env<'static>> = None;
        for (n, step) in sc["steps"].as_array().unwrap().iter().enumerate() {
            let op = step["op"].as_str().unwrap();
            let context = format!("{name} step {n} ({op})");
            apply(&root, &step["edits"], &[]);
            {
                let mut s = world.script.borrow_mut();
                s.events = step["events"].as_array().unwrap().iter().cloned().collect();
                s.context = context.clone();
            }
            world.out.0.borrow_mut().clear();
            let before = snap(&root);
            let args = &step["args"];
            let got: Result<Value, Value> = match op {
                "configure" => {
                    env = Some(world.env(world.installer(args)));
                    Ok(Value::Null)
                }
                "set_app_service" => {
                    env.as_mut().unwrap().inst.app_service = args["app_service"].as_bool();
                    Ok(Value::Null)
                }
                _ => {
                    let e = env.as_mut().expect("configured");
                    result_json(match op {
                        "service_setup" => S::service_setup(e, args["update"].as_bool().unwrap()),
                        "setup" => S::setup(e, args["update"].as_bool().unwrap()),
                        "doctor" => S::doctor(e),
                        "cli_setup" => S::cli_setup(e),
                        "uninstall" => S::uninstall(
                            e,
                            args["purge"].as_bool().unwrap(),
                            args["confirmed"].as_bool().unwrap(),
                        ),
                        "codex_logout" => S::codex_logout(e),
                        "codex_login" => S::codex_login(e),
                        "claude_logout" => S::claude_logout(e),
                        "cleanup_preview" => S::cleanup_preview(e),
                        "cleanup_apply" => S::cleanup_apply(e, args["ok"].as_str().unwrap()),
                        other => panic!("unknown op {other}"),
                    })
                }
            };
            match (&got, step.get("error")) {
                (Ok(v), None) => assert_eq!(*v, sv(&step["result"]), "{context}: result"),
                (Err(e), Some(want)) => assert_eq!(*e, sv(want), "{context}: error"),
                (Ok(v), Some(want)) => panic!("{context}: Python raised {want}, Rust answered {v}"),
                (Err(e), None) => panic!(
                    "{context}: Rust raised {e}, Python answered {}",
                    step["result"]
                ),
            }
            if let Some(want) = step.get("out") {
                let printed = String::from_utf8(world.out.0.borrow().clone()).unwrap();
                assert_eq!(printed, s(want), "{context}: printed");
            }
            let left: Vec<Value> = world.script.borrow().events.iter().cloned().collect();
            assert!(
                left.is_empty(),
                "{context}: Rust made {} fewer calls than Python; next: {}",
                left.len(),
                left[0]
            );
            if let (Some(notes), Some(e)) = (step.get("notes"), env.as_ref()) {
                if op != "configure" {
                    assert_eq!(json!(e.inst.notes), sv(notes), "{context}: notes");
                }
            }
            let after = snap(&root);
            assert_eq!(
                changes(&before, &after),
                expected_changes(&step["changes"]),
                "{context}: files"
            );
            steps += 1;
            events += step["events"].as_array().unwrap().len();
        }
        let _ = std::fs::remove_dir_all(&root);
    }
    assert_eq!((steps, events), (71, 863));
}
