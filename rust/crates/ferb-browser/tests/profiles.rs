//! Profiles, the resolver and the extension's install requests against
//! rust/fixtures/browser/profiles.json (hands.py: the Python build on an invented support folder,
//! its paths written "$ROOT"). The same tree is written in a temporary folder here.

mod common;

use std::path::{Path, PathBuf};

use common::fixture;
use ferb_browser::autoinstall::{self, BrowserState, ProfileStatus};
use ferb_browser::profiles::{self, BrowserProfile, Paired, Platform, Resolver};
use serde_json::{json, Value};

struct Mac;

impl Platform for Mac {
    fn has_app(&self, bundle_id: &str) -> bool {
        ["com.google.Chrome", "com.brave.Browser"].contains(&bundle_id)
    }
}

fn p(v: &BrowserProfile) -> Value {
    json!({"browser": v.browser, "folder": v.folder, "display_name": v.display_name, "email": v.email})
}

fn status(s: &ProfileStatus) -> Value {
    json!({"browser": s.browser, "folder": s.folder, "name": s.name, "extension_id": s.extension_id,
           "enabled": s.enabled, "removed": s.removed, "paired": s.paired, "version": s.version,
           "disabled_for": s.disabled_for, "email": s.email})
}

struct Root(PathBuf);

impl Root {
    fn norm(&self, path: &Path) -> String {
        path.to_string_lossy()
            .replace(&*self.0.to_string_lossy(), "$ROOT")
    }
    fn states(&self, states: &[BrowserState]) -> Value {
        json!(states
            .iter()
            .map(|s| json!({"browser": s.browser, "request": s.request.as_ref().map(|r| self.norm(r)),
                            "profiles": s.profiles.iter().map(status).collect::<Vec<_>>(), "error": s.error}))
            .collect::<Vec<_>>())
    }
    fn paths(&self, ps: &[PathBuf]) -> Value {
        json!(ps.iter().map(|x| self.norm(x)).collect::<Vec<_>>())
    }
}

// decisions: HAND-2, HAND-247, HAND-407, HAND-412, HAND-414, HAND-415, HAND-416, HAND-424
#[test]
fn profiles_resolver_and_install_requests_are_pythons() {
    let f = fixture("profiles.json");
    let dir = tempfile::tempdir().unwrap();
    let root = Root(dir.path().canonicalize().unwrap());
    for (rel, text) in f["tree"].as_object().unwrap() {
        let path = root.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            text.as_str()
                .unwrap()
                .replace("$ROOT", &root.0.to_string_lossy()),
        )
        .unwrap();
    }
    let (support, home) = (root.0.join("support"), root.0.join("home"));
    let store = f["store"].as_str().unwrap();
    let ids = [f["unpacked"].as_str().unwrap(), store];
    assert_eq!(json!(profiles::installed(&Mac, &support)), f["installed"]);
    let mut every = vec![];
    for b in ["chrome", "brave", "edge", "opera"] {
        let found = profiles::discover(b, &profiles::data_dir(b, &support).unwrap());
        assert_eq!(
            json!(found.iter().map(p).collect::<Vec<_>>()),
            f["discover"][b],
            "{b}"
        );
        every.extend(found);
    }
    for b in ["chrome", "brave", "opera"] {
        assert_eq!(
            json!(profiles::last_used(
                &profiles::data_dir(b, &support).unwrap()
            )),
            f["last_used"][b]
        );
    }
    let chrome: Vec<BrowserProfile> = every
        .iter()
        .filter(|x| x.browser == "chrome")
        .cloned()
        .collect();
    let pairs: Vec<(String, String)> = [
        ("brave", "Profile 7"),
        ("chrome", "X"),
        ("edge", "Default"),
        ("nosuch", "A"),
        ("brave", "Profile 7"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(
        json!(profiles::with_paired(&Mac, &chrome, &pairs)
            .iter()
            .map(p)
            .collect::<Vec<_>>()),
        f["with_paired"]
    );
    for (prof, want) in every.iter().zip(f["entries"].as_array().unwrap()) {
        let folder = profiles::data_dir(&prof.browser, &support)
            .unwrap()
            .join(&prof.folder);
        let e = profiles::extension_entry(&folder, &ids);
        assert_eq!(
            json!(e.as_ref().map(|(i, m)| json!([i, m]))),
            want["entry"],
            "{prof:?}"
        );
        assert_eq!(
            profiles::extension_state(&folder, &ids),
            want["state"],
            "{prof:?}"
        );
        let version = e.as_ref().map_or(String::new(), |(i, m)| {
            profiles::extension_version(m, &folder, i)
        });
        assert_eq!(version, want["version"], "{prof:?}");
        assert_eq!(
            json!(e
                .as_ref()
                .map_or(vec![], |(_, m)| profiles::disable_reasons(m))),
            want["reasons"],
            "{prof:?}"
        );
    }
    let aliases = vec![(
        "my alias".to_string(),
        ("chrome".to_string(), "Profile 3".to_string()),
    )];
    for case in f["resolve"].as_array().unwrap() {
        let spoken = case["spoken"].as_str().unwrap();
        let r = profiles::resolve(spoken, &every, &aliases, case["browser"].as_str());
        let got = json!({"kind": r.kind, "step": r.step, "candidates": r.candidates.iter().map(p).collect::<Vec<_>>()});
        assert_eq!(got, case["r"], "{case}");
        assert_eq!(r.message(spoken, &every), case["message"], "{case}");
    }
    let mut shown = vec![];
    for x in &every {
        for label in [None, Some(""), Some("Chrome")] {
            shown.push(profiles::shown(x, label));
        }
    }
    assert_eq!(json!(shown), f["shown"]);
    assert_eq!(
        json!([
            p(&profiles::named(&every, "chrome", "Profile 1")),
            p(&profiles::named(&every, "chrome", "Gone"))
        ]),
        f["named"]
    );
    for case in f["open_command"].as_array().unwrap() {
        let a = &case["args"];
        let got = match profiles::open_command(
            a[0].as_str().unwrap(),
            a[1].as_str().unwrap(),
            a[2].as_str(),
            &every,
            case["binary"].as_str().unwrap(),
            a[3].as_bool().unwrap(),
            a[4].as_bool().unwrap(),
            a[5].as_bool().unwrap(),
        ) {
            Ok(c) => json!({ "ok": c }),
            Err(e) => json!({ "error": e }),
        };
        assert_eq!(got, case["out"], "{case}");
    }
    let bindings = vec![
        Paired {
            browser: "chrome".into(),
            folder: "Profile 1".into(),
            owner: true,
        },
        Paired {
            browser: "chrome".into(),
            folder: "Profile 2".into(),
            owner: false,
        },
        Paired {
            browser: "brave".into(),
            folder: "Default".into(),
            owner: true,
        },
    ];
    for case in f["choose"].as_array().unwrap() {
        let pair = |v: &Value| {
            v.as_array().map(|a| {
                (
                    a[0].as_str().unwrap().to_string(),
                    a[1].as_str().unwrap().to_string(),
                )
            })
        };
        let r = Resolver {
            platform: &Mac,
            profiles: if case["profiles"] == true {
                every.clone()
            } else {
                vec![]
            },
            bindings: if case["bindings"] == 3 {
                bindings.clone()
            } else {
                vec![]
            },
            identity_profile: pair(&case["identity"]),
            aliases: aliases.clone(),
            owner_default: pair(&case["default"]),
            unseen: case["unseen"].as_str().map(String::from),
        };
        let got = match r.choose(case["named"].as_str()) {
            Ok(c) => json!({"ok": {"browser": c.browser, "folder": c.folder, "reason": c.reason}}),
            Err(m) => json!({"error": "NeedsOwner", "message": m}),
        };
        assert_eq!(got, case["out"], "{case}");
    }
    // the install requests (HAND-247)
    for (id, want) in f["valid_id"].as_object().unwrap() {
        assert_eq!(json!(autoinstall::valid_id(id)), *want, "{id}");
    }
    assert_eq!(
        root.paths(&autoinstall::native_host_dirs(
            &support,
            &["chrome", "brave", "opera", "chrome"]
        )),
        f["native_host_dirs"]
    );
    let paired = [("chrome".to_string(), "Profile 1".to_string())];
    assert_eq!(
        root.states(&autoinstall::read_states(
            &Mac,
            &support,
            store,
            &ids,
            Some(&paired),
            None
        )),
        f["read_states_before"]
    );
    let prepared =
        autoinstall::prepare(&Mac, &support, store, &ids, Some(&paired), Some(&home)).unwrap();
    assert_eq!(root.states(&prepared), f["prepare"]);
    let record: Value = serde_json::from_str(
        &std::fs::read_to_string(home.join(autoinstall::REQUEST_RECORD))
            .unwrap()
            .replace(&*root.0.to_string_lossy(), "$ROOT"),
    )
    .unwrap();
    assert_eq!(record, f["record"]);
    assert_eq!(
        std::fs::read_to_string(
            support
                .join("Google/Chrome/External Extensions")
                .join(format!("{store}.json"))
        )
        .unwrap(),
        f["request_text"]
    );
    let states = autoinstall::read_states(&Mac, &support, store, &ids, None, None);
    assert_eq!(root.states(&states), f["read_states"]);
    let ext = root.0.join("ext");
    let once = [("chrome".to_string(), "Default".to_string())];
    let again = autoinstall::prepare(&Mac, &support, store, &ids, Some(&once), None).unwrap();
    let describe = |st: &[BrowserState], sid: &str, w: &[PathBuf], u: &[String]| {
        json!(autoinstall::describe(st, sid, &ext, w, u)
            .iter()
            .map(|l| l.replace(&*root.0.to_string_lossy(), "$ROOT"))
            .collect::<Vec<_>>())
    };
    let got = json!([
        describe(&states, store, &[], &[]),
        describe(&states, "bad", &[], &["Arc".into()]),
        describe(&states, "", &[root.0.join("x.json")], &[]),
        describe(&[], store, &[], &["Comet".into()]),
        describe(&again, store, &[], &[]),
    ]);
    assert_eq!(got, f["describe"]);
    assert_eq!(
        root.paths(&autoinstall::withdraw(&home, store).unwrap()),
        f["withdraw_keep"]
    );
    let record: Value = serde_json::from_str(
        &std::fs::read_to_string(home.join(autoinstall::REQUEST_RECORD))
            .unwrap()
            .replace(&*root.0.to_string_lossy(), "$ROOT"),
    )
    .unwrap();
    assert_eq!(record, f["record_after"]);
    assert_eq!(
        root.paths(&autoinstall::remove_requests(&home, &support, store).unwrap()),
        f["remove_requests"]
    );
    assert_eq!(
        json!(home.join(autoinstall::REQUEST_RECORD).exists()),
        f["record_gone"]
    );
    // the Web Store package: the same names, order and contents (Python's zip bytes change with the time it runs)
    let (zipped, notes) = autoinstall::package(&ext, &root.0.join("out")).unwrap();
    assert_eq!(root.norm(&zipped), f["package"]["zip"]);
    assert_eq!(json!(notes), f["package"]["notes"]);
    let mut z = zip::ZipArchive::new(std::fs::File::open(&zipped).unwrap()).unwrap();
    let mut files = vec![];
    for i in 0..z.len() {
        let mut e = z.by_index(i).unwrap();
        let mut text = String::new();
        std::io::Read::read_to_string(&mut e, &mut text).unwrap();
        files.push(json!({"name": e.name(), "text": text}));
    }
    assert_eq!(json!(files), f["package"]["files"]);
}
