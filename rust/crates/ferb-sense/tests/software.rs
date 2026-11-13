//! Installed-software facts (`ferb/sense/software.py`) and the daemon side of transcription
//! (`ferb/sense/transcribe.py`) against `rust/fixtures/sense/software.json` and `transcribe.json`. Every program
//! run here is scripted: no codesign, spctl, xattr, `open` or FERB.app is started.

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use common::{fixture, input, input_value, tmp, Diffs};
use ferb_sense::software::{
    app_security_facts, associated_jobs, login_items, privacy_facts, Answer, Readers,
};
use ferb_sense::transcribe::{
    hearing_pids, launch_args, redacted, why, Transcriber, Transcript, Word,
};
use serde_json::{json, Value};

fn write_plist(path: &Path, v: &plist::Value, binary: bool) {
    if binary {
        v.to_file_binary(path).unwrap();
    } else {
        v.to_file_xml(path).unwrap();
    }
}

fn dict(pairs: &[(&str, plist::Value)]) -> plist::Value {
    plist::Value::Dictionary(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}

fn s(v: &str) -> plist::Value {
    plist::Value::String(v.into())
}

#[test]
fn software_facts_match_python() {
    let f = fixture("software.json");
    let mut d = Diffs::default();
    let app = Path::new("/Applications/Example.app");
    for c in f["login_items"].as_array().unwrap() {
        let inv = c[0].as_str().map(|t| t.as_bytes().to_vec());
        d.check(
            &format!("login_items {}", c[0]),
            &c[1],
            &login_items(app, "com.example.app", inv.as_deref()),
        );
    }
    let calls: Mutex<Vec<Value>> = Mutex::default();
    let run = |args: &[String]| -> std::io::Result<Answer> {
        calls.lock().unwrap().push(json!(args));
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        if a[..2] == ["/usr/bin/codesign", "-dv"] {
            return Ok((0, b"Executable=/x\n".to_vec(),
                       b"Identifier=com.example.app\nFormat=app bundle\nAuthority=Developer ID Application: Ex\nAuthority=Developer ID Certification Authority\nAuthority=Apple Root CA\nTeamIdentifier=ABCDE12345\nRuntime Version=14.0.0\nSignature size=9000\nSignature=adhoc\n".to_vec()));
        }
        if a[0] == "/usr/sbin/spctl" {
            return Ok((
                3,
                Vec::new(),
                b"/Applications/Example.app: rejected\nsource=Unnotarized\n".to_vec(),
            ));
        }
        if a[..2] == ["/usr/bin/codesign", "-d"] {
            let mut out = Vec::new();
            dict(&[
                (
                    "com.apple.security.app-sandbox",
                    plist::Value::Boolean(true),
                ),
                (
                    "keychain-access-groups",
                    plist::Value::Array(vec![s("ABCDE12345.x")]),
                ),
            ])
            .to_writer_xml(&mut out)
            .unwrap();
            return Ok((0, out, Vec::new()));
        }
        Err(std::io::Error::other("not found"))
    };
    let run_bad = |args: &[String]| -> std::io::Result<Answer> {
        if args[0] == "/usr/sbin/spctl" {
            return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
        }
        let code = if args.iter().any(|a| a == "-dv") {
            1
        } else {
            0
        };
        let out = if args.iter().any(|a| a == "--entitlements") {
            b"not a plist".to_vec()
        } else {
            Vec::new()
        };
        Ok((code, out, b"code object is not signed at all\n".to_vec()))
    };
    let xattr = |_: &Path, name: &str| -> std::io::Result<Vec<u8>> {
        if name == "com.apple.quarantine" {
            return Ok(b"0083;65f00000;Safari;ABCD-1234".to_vec());
        }
        let mut out = Vec::new();
        plist::Value::Array(vec![
            s("https://example.com/Example.dmg"),
            s("https://example.com/"),
        ])
        .to_writer_binary(&mut out)
        .unwrap();
        Ok(out)
    };
    let xattr_bad = |_: &Path, name: &str| -> std::io::Result<Vec<u8>> {
        if name == "com.apple.quarantine" {
            return Err(std::io::Error::other("no attribute"));
        }
        Ok(b"bplist00 broken".to_vec())
    };
    let privacy = |_: &str, _: &Path| json!({"user": {"status": "read", "grants": []}});
    let jobs = |_: &Path| json!([{"path": "/Library/LaunchAgents/x.plist"}]);
    let good = app_security_facts(
        app,
        "com.example.app",
        &Readers {
            run: &run,
            xattr: &xattr,
            privacy: &privacy,
            jobs: &jobs,
        },
    );
    let bad = app_security_facts(
        app,
        "com.example.app",
        &Readers {
            run: &run_bad,
            xattr: &xattr_bad,
            privacy: &privacy,
            jobs: &jobs,
        },
    );
    d.check("security", &f["security"], &json!([good, bad]));
    d.check(
        "security calls",
        &f["security_calls"],
        &json!(*calls.lock().unwrap()),
    );

    let folders: Vec<PathBuf> = vec![tmp().join("jobs/user"), tmp().join("jobs/sys")];
    for folder in &folders {
        std::fs::create_dir_all(folder).unwrap();
    }
    let (u, sy) = (&folders[0], &folders[1]);
    write_plist(
        &u.join("a.plist"),
        &dict(&[
            ("Label", s("com.example.a")),
            (
                "ProgramArguments",
                plist::Value::Array(vec![
                    s("/Applications/Example.app/Contents/MacOS/a"),
                    s("-x"),
                ]),
            ),
        ]),
        false,
    );
    write_plist(
        &u.join("b.plist"),
        &dict(&[("Label", s("com.other")), ("Program", s("/usr/bin/true"))]),
        false,
    );
    std::fs::write(u.join("c.plist"), b"not a plist").unwrap();
    write_plist(
        &u.join("d.txt"),
        &dict(&[("Program", s("/Applications/Example.app/x"))]),
        false,
    );
    write_plist(
        &sy.join("e.plist"),
        &dict(&[
            ("Label", s("com.example.e")),
            ("Program", s("/Applications/Example.app/b")),
            ("ProgramArguments", plist::Value::Array(vec![s("/ignored")])),
        ]),
        true,
    );
    write_plist(
        &sy.join("f.plist"),
        &dict(&[("ProgramArguments", plist::Value::Array(vec![]))]),
        false,
    );
    write_plist(
        &sy.join("h.plist"),
        &dict(&[("Program", s("Example.app/rel"))]),
        false,
    );
    write_plist(
        &sy.join("i.plist"),
        &dict(&[
            ("Label", plist::Value::Integer(5.into())),
            ("Program", s("/Applications/Example.app/../Other/x")),
        ]),
        false,
    );
    // Fixed against Python (ENG-6): a launch-job plist whose root is a list raised AttributeError out of Python's
    // `associated_jobs` (the fixture's `jobs_list_root`); here it is skipped, so the answer is the one Python gave
    // once that file was gone.
    write_plist(
        &sy.join("g.plist"),
        &plist::Value::Array(vec![s("a list")]),
        false,
    );
    d.eq(
        "python raised on a list-rooted plist",
        Some("AttributeError"),
        f["jobs_list_root"]["error"].as_str(),
    );
    let got = json!([
        associated_jobs(app, &folders),
        associated_jobs(app, &[tmp().join("jobs/missing")])
    ]);
    d.check("jobs", &f["jobs"], &got);

    let tcc = tmp().join("tcc");
    std::fs::create_dir_all(&tcc).unwrap();
    {
        let c = rusqlite::Connection::open(tcc.join("TCC.db")).unwrap();
        c.execute_batch(
            "CREATE TABLE access (service TEXT, client TEXT, auth_value INTEGER, last_modified INTEGER, extra TEXT);
             INSERT INTO access VALUES ('kTCCServiceScreenCapture','com.example.app',2,1700000000,'x'),
             ('kTCCServiceAccessibility','/Applications/Example.app',0,1700000001,NULL),
             ('kTCCServiceCamera','com.other',2,1,NULL),
             ('kTCCServiceAppleEvents','com.example.app',2,1700000002,NULL);",
        )
        .unwrap();
        let o = rusqlite::Connection::open(tcc.join("old.db")).unwrap();
        o.execute_batch("CREATE TABLE access (service TEXT, client TEXT)")
            .unwrap();
    }
    std::fs::write(tcc.join("junk.db"), b"not sqlite at all".repeat(100)).unwrap();
    let got: Vec<Value> = ["TCC.db", "old.db", "missing.db", "junk.db"]
        .iter()
        .map(|n| privacy_facts("com.example.app", app, &tcc.join(n)))
        .collect();
    d.check("privacy", &f["privacy"], &json!(got));
    d.finish("software");
}

// decision: SENSE-400
#[test]
fn transcription_asks_and_redacted_segments_match_python() {
    let f = fixture("transcribe.json");
    let mut d = Diffs::default();
    let app = tmp().join("home/.ferb/FERB.app");
    let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    d.check(
        "launch_args",
        &f["launch_args"],
        &json!([
            launch_args(
                &app,
                "transcribe",
                &args(&["/a.wav", "/ans.json", "en-US"]),
                None
            ),
            launch_args(&app, "status", &[], Some(&tmp().join("fh"))),
        ]),
    );
    for c in f["why"].as_array().unwrap() {
        d.check(
            &format!("why {}", c[0]),
            &c[1],
            &json!(why(c[0].as_object().unwrap())),
        );
    }
    let redactor = ferb_sanitize::Redactor::default();
    let red = |t: &str| redactor.text(t);
    for c in f["redacted"].as_array().unwrap() {
        let t = Transcript {
            text: c["text"].as_str().unwrap().into(),
            words: c["words"]
                .as_array()
                .unwrap()
                .iter()
                .map(|w| Word {
                    text: w[0].as_str().unwrap().into(),
                    start: w[1].as_f64().unwrap(),
                    end: w[2].as_f64().unwrap(),
                })
                .collect(),
            locale: String::new(),
        };
        d.check(
            &format!("redacted {}", c["text"]),
            &c["out"],
            &redacted(&t, &red, c["offset"].as_f64().unwrap()),
        );
    }
    std::fs::create_dir_all(&app).unwrap();
    let folder = tmp().join("home/.ferb/workspace/audio");
    let counter = Arc::new(Mutex::new(0u64));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap();
    for ask in f["asks"].as_array().unwrap() {
        let name = ask["name"].as_str().unwrap().to_string();
        let answer = ask["answer"].clone();
        let launched: Arc<Mutex<Vec<Value>>> = Arc::default();
        let l = launched.clone();
        let late = name == "timeout";
        let launch = move |argv: Vec<String>| -> ferb_core::interfaces::BoxFuture<'static, std::io::Result<i32>> {
            let (l, answer) = (l.clone(), answer.clone());
            Box::pin(async move {
                l.lock().unwrap().push(json!(argv));
                let at = argv.iter().position(|a| a == "transcribe").map_or(argv.len() - 1, |i| i + 2);
                let path = PathBuf::from(&argv[at]);
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
                l.lock().unwrap().push(json!(format!("0o{mode:o}")));
                if late {
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                }
                if answer != json!("") {
                    let text = match &answer {
                        Value::String(s) => s.clone(),
                        other => ferb_py::json::dumps(other),
                    };
                    std::fs::write(&path, text).unwrap();
                }
                Ok(7)
            })
        };
        let signed_ok = ask["signed"].as_bool().unwrap();
        let req = ask["requirement"].as_str().map(String::from);
        let mut tr = Transcriber::new(
            app.clone(),
            move || req.clone(),
            launch,
            move |_, _| signed_ok,
            None,
        );
        let c = counter.clone();
        tr.token = Box::new(move || {
            let mut n = c.lock().unwrap();
            *n += 1;
            format!("{:016x}", *n)
        });
        if late {
            tr.limit_s = 0.05;
        }
        let language = if name == "no locale" {
            None
        } else {
            Some("en-US")
        };
        let got = match rt.block_on(tr.transcribe(&folder.join("clip.wav"), language, 0.0)) {
            Ok(t) => {
                json!({"text": t.text, "words": t.words.iter().map(|w| json!([w.text, w.start, w.end])).collect::<Vec<_>>(),
                            "locale": t.locale})
            }
            Err(e) => json!({"error": e.kind(), "message": e.message()}),
        };
        let mut left: Vec<String> = std::fs::read_dir(&folder)
            .map(|r| {
                r.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        left.sort();
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&folder)
            .ok()
            .map(|m| format!("0o{:o}", m.permissions().mode() & 0o777));
        let want = json!({"launched": ask["launched"], "out": ask["out"], "left": ask["left"],
                          "in_flight": ask["in_flight"], "folder_mode": ask["folder_mode"]});
        let got = json!({"launched": *launched.lock().unwrap(), "out": got, "left": left, "in_flight": tr.in_flight(),
                         "folder_mode": mode});
        d.check(&format!("ask {name}"), &want, &got);
    }
    let status_launched: Arc<Mutex<Vec<Value>>> = Arc::default();
    let sl = status_launched.clone();
    let mut tr = Transcriber::new(app.clone(), || Some("req".into()), move |argv: Vec<String>| -> ferb_core::interfaces::BoxFuture<'static, std::io::Result<i32>> {
        let sl = sl.clone();
        Box::pin(async move {
            sl.lock().unwrap().push(json!(argv));
            std::fs::write(argv.last().unwrap(), r#"{"speech": "authorized", "on_device": true, "locale": "en", "other": 1}"#).unwrap();
            Ok(0)
        })
    }, |_, _| true, Some(tmp().join("fh2")));
    let c = counter.clone();
    tr.token = Box::new(move || {
        let mut n = c.lock().unwrap();
        *n += 1;
        format!("{:016x}", *n)
    });
    let status = rt.block_on(tr.status(&tmp().join("fh2/audio"))).unwrap();
    d.check(
        "status",
        &f["status"],
        &json!({"launched": *status_launched.lock().unwrap(), "out": status}),
    );
    let missing = Transcriber::new(
        tmp().join("nope.app"),
        || Some("r".into()),
        |_| -> ferb_core::interfaces::BoxFuture<'static, std::io::Result<i32>> {
            Box::pin(async { Ok(0) })
        },
        |_, _| true,
        None,
    );
    let got = match rt.block_on(missing.transcribe(Path::new("/x.wav"), None, 0.0)) {
        Ok(_) => json!("ok"),
        Err(e) => json!({"error": e.kind(), "message": e.message()}),
    };
    d.check("missing app", &f["missing_app"], &got);
    let hp = &f["hearing_pids"];
    let own = 4_000_000_i64;
    let ps = input(hp["ps"].as_str().unwrap()).replace("<SELF>", &own.to_string());
    let got = hearing_pids(
        Path::new(&input(hp["app"].as_str().unwrap())),
        Path::new(&input(hp["folder"].as_str().unwrap())),
        &ps,
        own,
    );
    d.check("hearing_pids", &hp["out"], &json!(got));
    let _ = input_value;
    d.finish("transcribe");
}
