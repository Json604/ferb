//! Replays rust/fixtures/device/installer.json: every tests/test_install.py scenario, step by step, with the same
//! commands in the same order, the same answers, the same files changed and the same results and errors.
// decision: DMN-513, DMN-514, DMN-515, DMN-516, DMN-517, DMN-536, DMN-589, DMN-590, DMN-18, DMN-19, DMN-20, DMN-14

mod support;

use std::path::{Path, PathBuf};

use ferb_device::install::{CodexPin, InstallError, Installer, Programs, Sources};
use serde_json::{json, Value};
use support::*;

fn s(v: &Value) -> String {
    subst(v.as_str().expect("a string"))
}

fn opt_path(v: &Value) -> Option<PathBuf> {
    v.as_str().map(PathBuf::from)
}

struct World {
    script: Shared,
    clock: Stepped,
    root: PathBuf,
    user_home: PathBuf,
    codex: CodexPin,
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
        inst.bundle = opt_path(&cfg["bundle"]);
        inst.cli_dir = opt_path(&cfg["cli_dir"]);
        inst.app_service = cfg["app_service"].as_bool();
        if cfg["fetch_model"] == "ok" {
            self.set_fetch(&mut inst, false);
        }
        inst
    }

    fn set_fetch(&self, inst: &mut Installer, fail: bool) {
        let script = self.script.clone();
        inst.fetch_model = Some(Box::new(move |root: &Path| {
            let ev = script
                .borrow_mut()
                .next("fetch", &root.display().to_string());
            assert_eq!(ev["root"], json!(root.to_string_lossy()));
            if fail {
                Err(("OSError".into(), "network unreachable".into()))
            } else {
                Ok(())
            }
        }));
    }
}

fn path_json(p: Option<PathBuf>) -> Value {
    p.map(|p| json!(p.to_string_lossy())).unwrap_or(Value::Null)
}

fn err_json(e: &InstallError) -> Value {
    json!({"type": e.type_name(), "message": e.to_string()})
}

/// One step: the operation on the installer, its answer as Python's `jsonable` writes it.
fn run_op(
    world: &World,
    inst: &mut Option<Installer>,
    op: &str,
    args: &Value,
) -> Result<Value, InstallError> {
    if op == "configure" {
        *inst = Some(world.installer(args));
        return Ok(Value::Null);
    }
    let i = inst.as_mut().expect("configured");
    let work = |fail: bool, script: Shared| {
        move || -> Result<(), InstallError> {
            script.borrow_mut().next("work", "while_stopped work");
            if fail {
                Err(InstallError::Other {
                    type_name: "RuntimeError".into(),
                    message: "disk I/O error".into(),
                })
            } else {
                Ok(())
            }
        }
    };
    Ok(match op {
        "install" => {
            let update_only = args["update_only"].as_bool().unwrap();
            match args["work"].as_str() {
                Some(w) => {
                    let mut f = work(w == "fail", world.script.clone());
                    i.install(update_only, Some(&mut f))?
                }
                None => i.install(update_only, None)?,
            }
            Value::Null
        }
        "while_stopped" => {
            let mut f = work(args["work"] == "fail", world.script.clone());
            i.while_stopped(&mut f)?;
            Value::Null
        }
        "set_fetch_model" => {
            world.set_fetch(i, args["fetch_model"] == "fail");
            Value::Null
        }
        "set_app_service" => {
            i.app_service = args["app_service"].as_bool();
            Value::Null
        }
        "uninstall" => json!(i.uninstall(args["purge"].as_bool().unwrap())?),
        "restore_definition" => json!(i.restore_definition()?),
        "restart" => {
            i.restart()?;
            Value::Null
        }
        "load" => {
            i.load()?;
            Value::Null
        }
        "unload" => {
            i.unload()?;
            Value::Null
        }
        "exiting" => json!(i.exiting()),
        "disabled" => json!(i.disabled()),
        "running" => json!(i.running()?),
        "in_app" => json!(i.in_app()),
        "installed" => json!(i.installed()),
        "active_program" => json!(i.active_program()),
        "recorded" => Value::Object(i.recorded()),
        "plist_bytes" => json!({"hex": hex::encode(i.plist_bytes()?)}),
        "daemon_command" => json!(i.daemon_command()),
        "python_command" => json!(i.python_command()),
        "grant_target" => path_json(i.grant_target()),
        "granted_binary" => path_json(i.granted_binary()),
        "cli_status" => json!(i.cli_status()),
        "definition_current" => json!(i.definition_current()?),
        "signer" => json!(i.signer()),
        "own_requirement" => json!(i.own_requirement()?),
        "install_secret_helper" => {
            i.install_secret_helper()?;
            Value::Null
        }
        "ensure_codex" => {
            i.ensure_codex()?;
            Value::Null
        }
        "write_native_host" => {
            json!(i
                .write_native_host()?
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect::<Vec<_>>())
        }
        "link_cli" => {
            i.link_cli()?;
            Value::Null
        }
        "check_purge_target" => {
            i.check_purge_target()?;
            Value::Null
        }
        "granted" => Value::Null,
        other => panic!("unknown op {other}"),
    })
}

fn subst_value(v: &Value) -> Value {
    match v {
        Value::String(t) => json!(subst(t)),
        Value::Array(a) => Value::Array(a.iter().map(subst_value).collect()),
        Value::Object(m) => {
            Value::Object(m.iter().map(|(k, x)| (k.clone(), subst_value(x))).collect())
        }
        other => other.clone(),
    }
}

#[test]
fn every_installer_scenario_matches_python() {
    let fx = fixture("installer.json");
    let codex = CodexPin {
        version: fx["constants"]["codex_version"].as_str().unwrap().into(),
        native_bin: PathBuf::from(fx["constants"]["native_bin"].as_str().unwrap()),
    };
    let mut steps = 0;
    let mut events = 0;
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
        };
        world.script.borrow_mut().root = root.clone();
        let mut inst: Option<Installer> = None;
        for (n, step) in sc["steps"].as_array().unwrap().iter().enumerate() {
            let op = step["op"].as_str().unwrap();
            let context = format!("{name} step {n} ({op})");
            apply(&root, &step["edits"], &[]);
            {
                let mut sc = world.script.borrow_mut();
                sc.events = step["events"].as_array().unwrap().iter().cloned().collect();
                sc.context = context.clone();
            }
            let before = snap(&root);
            let got = run_op(&world, &mut inst, op, &step["args"]);
            match (&got, step.get("error")) {
                (Ok(v), None) => assert_eq!(*v, subst_value(&step["result"]), "{context}: result"),
                (Err(e), Some(want)) => {
                    assert_eq!(err_json(e), subst_value(want), "{context}: error")
                }
                (Ok(v), Some(want)) => panic!("{context}: Python raised {want}, Rust answered {v}"),
                (Err(e), None) => panic!(
                    "{context}: Rust raised {e} ({}), Python answered {}",
                    e.type_name(),
                    step["result"]
                ),
            }
            let left = world.script.borrow().events.len();
            assert_eq!(
                left,
                0,
                "{context}: Rust made {left} fewer calls than Python: {:?}",
                world.script.borrow().events
            );
            if let (Some(notes), Some(i)) = (step.get("notes"), inst.as_ref()) {
                if op != "configure" {
                    assert_eq!(json!(i.notes), subst_value(notes), "{context}: notes");
                }
            }
            assert_eq!(
                *world.clock.0.borrow(),
                step["clock"].as_f64().unwrap(),
                "{context}: clock"
            );
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
    assert_eq!((steps, events), (210, 1594));
}

#[test]
fn the_rust_build_differs_only_in_the_programs_it_names() {
    // decision: DMN-14
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join(".ferb");
    let bundle = dir.path().join("FERB.app");
    let make = |programs: Programs| {
        let mut i = Installer::new(
            home.clone(),
            dir.path().join("LaunchAgents"),
            Box::new(ferb_device::install::SystemRunner),
            Box::new(Stepped::default()),
            dir.path().to_path_buf(),
            CodexPin {
                version: "0".into(),
                native_bin: "x".into(),
            },
            Sources {
                repo_root: "/r".into(),
                helper_source: "/h".into(),
                extension: "/e".into(),
            },
        );
        i.programs = programs;
        i
    };
    let rust = Programs::Rust(ferb_device::install::RustPrograms {
        bundle_daemon: "Contents/Helpers/FERB.app/Contents/MacOS/FERB".into(),
        bundle_native_host: "Contents/Helpers/ferb-native-host".into(),
        checkout_daemon: "/build/ferbd".into(),
        checkout_native_host: "/build/ferb-native-host".into(),
        daemon_args: vec![],
    });
    let py = make(Programs::Python);
    let rs = make(rust.clone());
    let (mut a, mut b) = (py.plist().unwrap(), rs.plist().unwrap());
    assert_eq!(
        b.remove("ProgramArguments").unwrap(),
        ferb_device::plistlib::Value::strings(["/build/ferbd"])
    );
    a.remove("ProgramArguments");
    assert_eq!(a, b);
    assert_eq!(rs.native_host_exec(), "/build/ferb-native-host");
    assert!(py
        .native_host_exec()
        .ends_with(" -P -m ferb.native_host.main"));
    // FERB.app: the nested app's executable, run with no arguments
    std::fs::create_dir_all(bundle.join("Contents")).unwrap();
    let info = ferb_device::plistlib::dumps(&ferb_device::plistlib::Value::Dict(
        [(
            "CFBundleVersion".to_string(),
            ferb_device::plistlib::Value::string("7"),
        )]
        .into_iter()
        .collect(),
    ))
    .unwrap();
    std::fs::write(bundle.join("Contents/Info.plist"), info).unwrap();
    let mut app = make(rust);
    app.bundle = Some(bundle.clone());
    assert_eq!(
        app.daemon_command(),
        vec![bundle
            .join("Contents/Helpers/FERB.app/Contents/MacOS/FERB")
            .to_string_lossy()
            .into_owned()]
    );
    // a checkout's Rust install isn't defined: it says so and changes nothing
    let mut checkout = make(Programs::Rust(ferb_device::install::RustPrograms {
        bundle_daemon: "d".into(),
        bundle_native_host: "n".into(),
        checkout_daemon: "/build/ferbd".into(),
        checkout_native_host: "/build/ferb-native-host".into(),
        daemon_args: vec![],
    }));
    assert!(checkout.install(false, None).is_err());
    assert!(!home.exists());
}
