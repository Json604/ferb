//! uploads.py's `Uploads` against rust/fixtures/browser/uploads.json: each scenario's steps replayed in a temporary
//! folder (the same files, links, locks and races, at ferb-fs's hook points where Python's tests patched
//! `recovery._fclone` / `copy_into`), and after each step the same result and the same files left on disk. A
//! `sources` step is the driver's `_sources`: the identity read through a handle the protection lists checked as
//! opened (ferb-core's `PathCheck`, the seam ferb-fs's tools use), then the copy.
#![cfg(unix)]

mod common;

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::sync::Arc;

use ferb_browser::staging::{StagingError, Uploads};
use ferb_browser::upload::{unix::UnixFiles, Files};
use ferb_core::types::PathCheck;
use serde_json::{json, Value};

use common::{fixture, Counter};

fn b64(data: &[u8]) -> String {
    ferb_identity::py::b64encode(data)
}

/// Every entry under root, sorted, as more.py's `_files` writes it.
fn files(root: &Path) -> Vec<Value> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<Value>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            let m = std::fs::symlink_metadata(&p).unwrap();
            if m.file_type().is_symlink() {
                let target = std::fs::read_link(&p)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.push(json!([
                    rel,
                    "link",
                    target.replace(&*root.to_string_lossy(), "$ROOT")
                ]));
            } else if m.is_dir() {
                out.push(json!([rel, "dir", m.mode() & 0o7777]));
                walk(root, &p, out);
            } else {
                let data = std::fs::read(&p).ok().map(|d| b64(&d));
                let mut entry = vec![json!(rel), json!("file"), json!(data)];
                if rel.starts_with("ws/uploads/") {
                    entry.push(json!(m.mode() & 0o7777));
                    entry.push(json!(
                        m.mtime() as i128 * 1_000_000_000 + m.mtime_nsec() as i128
                    ));
                }
                out.push(Value::Array(entry));
            }
        }
    }
    let mut out = vec![];
    walk(root, root, &mut out);
    out.sort_by(|a, b| a[0].as_str().cmp(&b[0].as_str()));
    out
}

fn chflags(p: &Path, flags: u32) {
    let c = std::ffi::CString::new(p.to_string_lossy().as_bytes()).unwrap();
    // SAFETY: a valid C string; chflags changes only that file's flags.
    let r = unsafe { libc::chflags(c.as_ptr(), flags as _) };
    assert_eq!(r, 0, "chflags {p:?}");
}

fn sha_hex(data: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn err(e: StagingError) -> Value {
    if e.class == "CleanupLeft" {
        json!({"error": e.class, "message": e.message, "left": e.left})
    } else {
        json!({"error": e.class, "message": e.message})
    }
}

fn outcome<T: serde::Serialize>(r: Result<T, StagingError>) -> Value {
    match r {
        Ok(v) => json!({"ok": v}),
        Err(e) => err(e),
    }
}

const OPEN_FILE: i32 = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;

fn open(p: &Path) -> i32 {
    let c = std::ffi::CString::new(p.to_string_lossy().as_bytes()).unwrap();
    // SAFETY: a valid C string; the fd is closed by the caller.
    let fd = unsafe { libc::open(c.as_ptr(), OPEN_FILE) };
    assert!(fd >= 0, "open {p:?}");
    fd
}

fn replay(f: &Value, scenario: &Value, random: &Arc<Counter>) -> usize {
    let name = scenario["name"].as_str().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let r = |s: &str| s.replace("$ROOT", &root.to_string_lossy());
    let uroot = root.join("ws/uploads");
    let untrusted: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    let u = untrusted.clone();
    let ups = Uploads::new(
        &uroot.to_string_lossy(),
        "/nonexistent-home",
        Arc::new(move || u.lock().unwrap().clone()),
        random.clone(),
    );
    let mut locked: Vec<std::path::PathBuf> = vec![];
    let mut staged: Vec<String> = vec![];
    let mut compared = 0;
    let mtime = f["mtime_ns"].as_u64().unwrap();
    for (n, step) in scenario["steps"].as_array().unwrap().iter().enumerate() {
        let at = format!("{name} step {n}");
        let s = &step["step"];
        let (kind, arg) = s.as_object().unwrap().iter().next().unwrap();
        let mut got = Value::Null;
        let stage = |tree: &str, specs: &[Value], tab: Option<&str>| {
            let fds: Vec<i32> = specs
                .iter()
                .map(|s| open(&root.join(s["source"].as_str().unwrap())))
                .collect();
            let sources: Vec<(i32, String, String)> = fds
                .iter()
                .zip(specs)
                .map(|(fd, s)| {
                    let p = root.join(s["source"].as_str().unwrap());
                    let sha = if s["sha"] == "before" {
                        sha_hex(b"before")
                    } else {
                        sha_hex(&std::fs::read(&p).unwrap())
                    };
                    (
                        *fd,
                        p.canonicalize().unwrap().to_string_lossy().into_owned(),
                        sha,
                    )
                })
                .collect();
            let out = ups.stage_all(tree, &sources, tab);
            for fd in fds {
                // SAFETY: closing the fds opened above.
                unsafe { libc::close(fd) };
            }
            ferb_fs::hooks::clear();
            ferb_fs::hooks::clear_off();
            out
        };
        match kind.as_str() {
            "write" => {
                let p = root.join(arg.as_str().unwrap());
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, s["data"].as_str().unwrap()).unwrap();
                let mode = s.get("mode").and_then(Value::as_u64).unwrap_or(0o644) as u32;
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
                let c = std::ffi::CString::new(p.to_string_lossy().as_bytes()).unwrap();
                let ts = [libc::timespec {
                    tv_sec: (mtime / 1_000_000_000) as _,
                    tv_nsec: (mtime % 1_000_000_000) as _,
                }; 2];
                // SAFETY: a valid C string and two timespecs (atime, mtime), as os.utime(ns=...).
                unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), ts.as_ptr(), 0) };
            }
            "mkdir" => std::fs::create_dir_all(root.join(arg.as_str().unwrap())).unwrap(),
            "symlink" => {
                let a = arg.as_array().unwrap();
                std::os::unix::fs::symlink(
                    r(a[1].as_str().unwrap()),
                    root.join(a[0].as_str().unwrap()),
                )
                .unwrap();
            }
            "rmtree" => std::fs::remove_dir_all(root.join(arg.as_str().unwrap())).unwrap(),
            "chmod" => {
                let a = arg.as_array().unwrap();
                std::fs::set_permissions(
                    root.join(a[0].as_str().unwrap()),
                    std::fs::Permissions::from_mode(a[1].as_u64().unwrap() as u32),
                )
                .unwrap();
            }
            "victim" => {
                for n in ["tree1", "tree2", "outside"] {
                    std::fs::create_dir_all(root.join("Documents").join(n)).unwrap();
                    std::fs::write(
                        root.join("Documents").join(n).join("keep.txt"),
                        "the owner's",
                    )
                    .unwrap();
                }
            }
            "untrusted" => {
                *untrusted.lock().unwrap() = arg
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|u| r(u.as_str().unwrap()))
                    .collect();
            }
            "race" => {
                let race = arg.as_str().unwrap().to_string();
                let way = s
                    .get("way")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let (root2, uroot2) = (root.clone(), uroot.clone());
                if race == "no_clone" || way == "byte copy" {
                    ferb_fs::hooks::set_off(&["fclonefileat"]);
                }
                ferb_fs::hooks::set(move |point| {
                    let Ok(mut leaves) = std::fs::read_dir(uroot2.join("tree1")) else {
                        return;
                    };
                    let leaf = leaves.next().unwrap().unwrap().file_name();
                    let victim = root2.join("Documents");
                    match (race.as_str(), point) {
                        ("swap_folder", "staging.copy") => {
                            std::fs::create_dir(victim.join(&leaf)).unwrap();
                            if way == "byte copy" {
                                let planted = victim.join(&leaf).join("cv.pdf");
                                std::fs::write(&planted, "the owner's other file").unwrap();
                                std::fs::set_permissions(
                                    &planted,
                                    std::fs::Permissions::from_mode(0o640),
                                )
                                .unwrap();
                            }
                            std::fs::rename(uroot2.join("tree1"), uroot2.join("moved")).unwrap();
                            std::os::unix::fs::symlink(&victim, uroot2.join("tree1")).unwrap();
                        }
                        ("plant_link", "staging.copy") => {
                            std::os::unix::fs::symlink(
                                victim.join("planted.pdf"),
                                uroot2.join("tree1").join(&leaf).join("cv.pdf"),
                            )
                            .unwrap();
                        }
                        ("swap_after", "staging.copied") => {
                            let copy = uroot2.join("tree1").join(&leaf).join("cv.pdf");
                            std::fs::remove_file(&copy).unwrap();
                            std::os::unix::fs::symlink(root2.join("elsewhere.pdf"), &copy).unwrap();
                        }
                        _ => {}
                    }
                });
            }
            "stage" => {
                let tab = arg.get("tab").and_then(Value::as_str);
                let out = stage(
                    arg["tree"].as_str().unwrap(),
                    std::slice::from_ref(arg),
                    tab,
                );
                if let Ok(v) = &out {
                    staged.extend(v.iter().cloned());
                }
                got = outcome(out.map(|v| v[0].clone()));
            }
            "stage_all" => {
                let tab = arg.get("tab").and_then(Value::as_str);
                got = outcome(stage(
                    arg["tree"].as_str().unwrap(),
                    arg["sources"].as_array().unwrap(),
                    tab,
                ));
            }
            "forget" => got = outcome(ups.forget_tree(arg.as_str().unwrap())),
            "closed" => got = json!({"ok": ups.closed(arg.as_str().unwrap())}),
            "collect" => got = outcome(ups.collect()),
            "sweep" => got = outcome(ups.sweep()),
            "lock_copy" => {
                let p = std::path::PathBuf::from(&staged[0]);
                chflags(&p, libc::UF_IMMUTABLE);
                locked.push(p);
            }
            "unlock" => {
                for p in &locked {
                    if p.symlink_metadata().is_ok() {
                        chflags(p, 0);
                    }
                }
            }
            "sources" => {
                let paths: Vec<String> = arg["paths"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| {
                        root.join(p.as_str().unwrap())
                            .to_string_lossy()
                            .into_owned()
                    })
                    .collect();
                let check = PathCheck(Arc::new(|p: &Path, _: &str| {
                    p.to_string_lossy()
                        .contains("/.ferb/")
                        .then(|| "closed by ferb_state".to_string())
                }));
                let unix = UnixFiles {
                    home: "/nonexistent-home".into(),
                };
                got = match unix.identify(&paths, Some(&check)) {
                    Err(e) => json!({"error": e.class(), "message": e.message()}),
                    Ok((ids, handles)) => {
                        let sources: Vec<(i32, String, String)> = handles
                            .iter()
                            .zip(&paths)
                            .zip(&ids)
                            .map(|((h, p), i)| {
                                (h.raw(), p.clone(), i["sha256"].as_str().unwrap().into())
                            })
                            .collect();
                        match ups.stage_all(
                            arg["tree"].as_str().unwrap(),
                            &sources,
                            arg["tab"].as_str(),
                        ) {
                            Ok(given) => {
                                let ids: Vec<Value> = ids
                                    .into_iter()
                                    .map(|mut i| {
                                        i.insert("owner_uid".into(), "$UID".into());
                                        Value::Object(i)
                                    })
                                    .collect();
                                json!({"ok": {"identities": ids, "given": given}})
                            }
                            Err(e) => json!({"error": "Refused", "message": format!(
                                "FERB couldn't make its own copy of the file to upload ({}); nothing was given to the browser",
                                e.message)}),
                        }
                    }
                };
            }
            other => panic!("{at}: unknown step {other}"),
        }
        let got: Value =
            serde_json::from_str(&got.to_string().replace(&*root.to_string_lossy(), "$ROOT"))
                .unwrap();
        assert_eq!(got, step["out"], "{at}: {s}");
        assert_eq!(json!(files(&root)), step["files"], "{at}: files after {s}");
        compared += 2;
    }
    for p in &locked {
        if p.symlink_metadata().is_ok() {
            chflags(p, 0);
        }
    }
    compared
}

// decisions: HAND-500, HAND-501
#[test]
fn upload_copies_match_python() {
    let f = fixture("uploads.json");
    let mut total = 0;
    // one counter for the whole file, as make.py resets it once before uploads_fixture
    let random = Arc::new(Counter::new());
    for scenario in f["scenarios"].as_array().unwrap() {
        total += replay(&f, scenario, &random);
    }
    assert!(total > 150, "{total}");
}

/// Python's test_concurrent_cleanups_report_no_failure_that_didnt_happen: closing a window of several tabs runs
/// several collects at once; one removing what another is removing is no failure.
#[test]
fn concurrent_cleanups_report_no_failure_that_didnt_happen() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let src = root.join("a.txt");
    std::fs::write(&src, "x").unwrap();
    let sha = sha_hex(b"x");
    let uroot = root.join("uploads");
    for round in 0..5 {
        let ups = Arc::new(Uploads::new(
            &uroot.to_string_lossy(),
            "/nonexistent-home",
            Arc::new(Vec::new),
            Arc::new(ferb_browser::pairing::SystemRandom),
        ));
        let tabs: Vec<String> = (0..6).map(|i| format!("p#{i}")).collect();
        for tab in &tabs {
            for _ in 0..4 {
                let fd = open(&src);
                ups.stage_all(
                    "tree1",
                    &[(fd, src.to_string_lossy().into_owned(), sha.clone())],
                    Some(tab),
                )
                .unwrap();
                // SAFETY: the fd opened above.
                unsafe { libc::close(fd) };
            }
        }
        ups.forget_tree("tree1").unwrap();
        for tab in &tabs {
            ups.closed(tab);
        }
        let barrier = Arc::new(std::sync::Barrier::new(6));
        let workers: Vec<_> = (0..6)
            .map(|_| {
                let (ups, barrier) = (ups.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    ups.collect()
                })
            })
            .collect();
        let failures: Vec<StagingError> = workers
            .into_iter()
            .filter_map(|w| w.join().unwrap().err())
            .collect();
        assert!(
            failures.is_empty() && !uroot.join("tree1").exists(),
            "{round}: {failures:?}"
        );
    }
}
