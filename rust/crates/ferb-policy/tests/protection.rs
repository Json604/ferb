//! The protection lists against what Python's did (decisions: ENG-3, GATE-1, GATE-200, GATE-203, GATE-224, GATE-505):
//! `rust/fixtures/policy/make.py` built three file trees in a temporary folder (links into protected folders, a link
//! to FERB's state, a link loop, a relative link, a FIFO, another app's container, FERB.app), loaded the lists and
//! asked every check about each path. The same trees are built here in a temporary folder and must get the same
//! answers. The kernel's own names come from a name-only open and `F_GETPATH` (macOS).

#![cfg(target_os = "macos")]

mod common;

use std::ffi::CString;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

use common::*;
use ferb_policy::ppath::PosixFiles;
use ferb_policy::protection::{self, Lists, Protection};
use serde_json::{json, Value};

struct Root {
    _dir: tempfile::TempDir,
    tmp: String,
    real: String,
}

impl Root {
    fn new() -> Root {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().to_str().unwrap().to_string();
        let real = fs::canonicalize(dir.path())
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        Root {
            _dir: dir,
            tmp,
            real,
        }
    }

    /// The fixture's placeholders as this run's folder.
    fn fill(&self, v: &Value) -> Value {
        let text = v
            .to_string()
            .replace("<RTMP>", &self.real)
            .replace("<TMP>", &self.tmp);
        serde_json::from_str(&text).unwrap()
    }

    /// A folder above the temporary root, in any spelling (left out of the fixture).
    fn above(&self, path: &str) -> bool {
        let mut roots = vec![self.tmp.clone(), self.real.clone()];
        roots.extend([
            format!("/System/Volumes/Data{}", self.tmp),
            format!("/System/Volumes/Data{}", self.real),
        ]);
        roots
            .iter()
            .any(|r| r.starts_with(&format!("{}/", path.trim_end_matches('/'))))
    }
}

fn mkfifo(p: &Path) {
    let c = CString::new(p.to_str().unwrap()).unwrap();
    // SAFETY: `c` is a valid NUL-terminated path that outlives the call.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
}

fn build(root: &Path, layout: &str) {
    let home = root.join("home");
    let ferb = home.join(".ferb");
    for d in [
        home.join("Documents"),
        home.join(".ssh"),
        ferb.join("workspace/uploads"),
        home.join("Library/Keychains"),
        home.join("Library/Containers/com.apple.Safari/Data"),
        home.join("Library/LaunchAgents"),
        home.join("notes"),
        root.join("FERB.app/Contents/Library/LaunchAgents"),
        root.join("other"),
    ] {
        fs::create_dir_all(d).unwrap();
    }
    fs::write(home.join(".ssh/id_test"), "x").unwrap();
    fs::write(home.join("Documents/a.txt"), "a").unwrap();
    fs::write(home.join(".zshrc"), "x").unwrap();
    fs::write(ferb.join("ferb.db"), "x").unwrap();
    fs::write(
        root.join("FERB.app/Contents/Library/LaunchAgents/com.ferb.daemon.plist"),
        "x",
    )
    .unwrap();
    fs::write(home.join("Library/LaunchAgents/com.ferb.daemon.plist"), "x").unwrap();
    symlink(home.join(".ssh"), home.join("Documents/sshlink")).unwrap();
    symlink(&ferb, home.join("notes/ferblink")).unwrap();
    symlink(home.join("loop2"), home.join("loop1")).unwrap();
    symlink(home.join("loop1"), home.join("loop2")).unwrap();
    symlink("relative/../../.ssh", home.join("Documents/rel")).unwrap();
    mkfifo(&home.join("Documents/fifo"));
    if layout == "linked" {
        symlink(home.join("Documents"), root.join("other/docs")).unwrap();
        symlink(
            root.join("other"),
            home.join("Library/Containers/linkedcontainer"),
        )
        .unwrap();
    }
}

fn entry_json(e: &protection::Entry) -> Value {
    json!({"id": e.id, "path": e.path, "shown": e.shown, "typed": e.typed, "kernel": e.kernel, "app_data": e.app_data})
}

// decision: GATE-1, GATE-200, GATE-203, GATE-224, GATE-505
#[test]
fn protection_matches_python() {
    let fx = fixture("protection.json");
    let fx = &fx[0];
    let mut d = Diffs::new();
    let defaults = json!([
        fx["browser_stores"],
        fx["default_never_read"],
        fx["never_read_offered"]
    ]);
    d.check(
        "defaults",
        &Value::Null,
        &defaults,
        &json!([
            protection::BROWSER_STORES,
            protection::default_never_read(),
            protection::never_read_offered()
        ]),
    );
    let files = PosixFiles;
    for (n, tree) in fx["trees"].as_array().unwrap().iter().enumerate() {
        let r = Root::new();
        let tree = r.fill(tree);
        let layout = s(&tree["layout"]);
        let root = Path::new(&r.tmp).join(format!("prot{n}"));
        build(&root, &layout);
        let home = root.join("home");
        let ferb = home.join(".ferb");
        let lists = &tree["lists"];
        let p = Protection::new(
            home.to_str().unwrap(),
            ferb.to_str().unwrap(),
            &Lists {
                never_read: &lists["never_read"],
                never_write: &lists["never_write"],
                terminal_apps: &lists["terminal_apps"],
                local_ports: &lists["local_ports"],
            },
            tree["app"].as_str(),
            &files,
        )
        .expect("the lists load");
        for res in tree["results"].as_array().unwrap() {
            let probe = s(&res["path"]);
            let mut got = json!({"path": probe, "read_denied": {"ok": p.read_denied(&probe)},
                "write_denied": {"ok": p.write_denied(&probe)}, "ferb_state": {"ok": p.ferb_state(&probe)},
                "app_data": p.app_data(&probe)});
            for access in [
                "read",
                "write",
                "remove",
                "metadata",
                "trash",
                "delete_permanent",
            ] {
                got[format!("fs_{access}")] = json!({"ok": p.fs_rules(&probe, access)});
            }
            d.check(&format!("{layout}: {probe}"), &res["path"], res, &got);
        }
        let want = &tree["state"];
        let pairs = |v: Vec<(String, bool)>| {
            json!(v.iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>())
        };
        let mut terminal_apps = p.terminal_apps.clone();
        terminal_apps.sort();
        let liftable: serde_json::Map<String, Value> = want["liftable"]
            .as_object()
            .unwrap()
            .keys()
            .map(|k| (k.clone(), json!(p.liftable(k))))
            .collect();
        let forms_now: Vec<Value> = [
            "home/.ssh",
            "home/Library/Containers/com.apple.Safari",
            "home/Documents/sshlink",
        ]
        .iter()
        .map(|x| json!(p.forms_now(root.join(x).to_str().unwrap())))
        .collect();
        let got = json!({"home": p.home, "ferb_home": p.ferb_home, "workspace": p.workspace, "uploads": p.uploads,
            "reads": p.reads.iter().map(entry_json).collect::<Vec<_>>(),
            "writes": p.writes.iter().map(entry_json).collect::<Vec<_>>(), "terminal_apps": terminal_apps,
            "service_files": p.service_files,
            "local_ports": p.local_ports.iter().map(|l| json!({"port": l.port, "executable": l.executable})).collect::<Vec<_>>(),
            "read_entries": pairs(p.read_entries()), "write_entries": pairs(p.write_entries()), "app_entries": p.app_entries(),
            "guarded_parents": p.guarded_parents().into_iter().filter(|g| !r.above(g)).collect::<Vec<_>>(),
            "rules": p.rules(), "liftable": liftable, "forms_now": forms_now,
            "workspace_problem": protection::workspace_problem(ferb.to_str().unwrap(), &files)});
        d.check(&format!("{layout}: state"), &json!(layout), want, &got);
    }
    let r = Root::new();
    for c in fx["workspace"].as_array().unwrap() {
        let c = r.fill(c);
        let kind = s(&c["kind"]);
        let root = Path::new(&r.tmp).join(format!("ws-{kind}"));
        let ferb = root.join(".ferb");
        fs::create_dir_all(&ferb).unwrap();
        match kind.as_str() {
            "link" => {
                fs::create_dir(root.join("elsewhere")).unwrap();
                symlink(root.join("elsewhere"), ferb.join("workspace")).unwrap();
            }
            "file" => fs::write(ferb.join("workspace"), "x").unwrap(),
            "fine" => fs::create_dir(ferb.join("workspace")).unwrap(),
            _ => {}
        }
        d.check(
            "workspace_problem",
            &c["kind"],
            &c["out"],
            &json!(protection::workspace_problem(
                ferb.to_str().unwrap(),
                &files
            )),
        );
    }
    let r = Root::new();
    fs::create_dir_all(Path::new(&r.tmp).join("home")).unwrap();
    for c in fx["errors"].as_array().unwrap() {
        let c = r.fill(c);
        let l = &c["lists"];
        let got = match Protection::new(
            &format!("{}/home", r.tmp),
            &format!("{}/ferbhome", r.tmp),
            &Lists {
                never_read: &l["never_read"],
                never_write: &l["never_write"],
                terminal_apps: &l["terminal_apps"],
                local_ports: &l["local_ports"],
            },
            None,
            &files,
        ) {
            Ok(p) => json!({"ok": p.rules()}),
            Err(_) => json!({"raises": "BadProtection"}),
        };
        d.check("bad lists", &c["lists"], &c["out"], &got);
    }
    d.finish("protection");
}
