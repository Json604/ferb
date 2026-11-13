//! The identity port's calls of the signed helper against the Python code (decision: ENG-3):
//! `rust/fixtures/identity/make.py` ran `ferb/identity` on invented data with the helper replaced
//! by a recorder; these tests make the same calls and must produce the same helper invocations (no
//! argument, the same stdin bytes), the same answers and refusals, and the same
//! `owner-secret-handles.json` bytes.
//!
//! Nothing here touches the login Keychain, the installed helper or `~/.ferb`. The helper is
//! `examples/fake_helper.rs`, copied to `<temporary home>/bin/ferb-secrets` and signed ad hoc
//! (`codesign -s -`: no identity, no keychain); the pin beside it is that copy's own code hash,
//! the form setup pins an ad-hoc FERB.app's helper with (`ferb/device/install.py:153-158`). So
//! the caller's real check runs, with the real `/usr/bin/codesign`, in every test: there is no
//! switch that skips or weakens it, in any build.
#![cfg(target_os = "macos")]

#[path = "../../ferb-identity/tests/common/mod.rs"]
mod common;

use std::{
    collections::BTreeSet,
    fs, io,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, OnceLock},
};

use common::{cases, hex, same, strings, unb64, unb64_text, unhex, Recorded};
use ferb_identity::{
    helper::{Helper, Launcher, Reply, Request},
    owner_secrets::{self, OwnerSecrets},
    py,
    secret_tools::secrets_list,
    secrets::{origin_allowed, specs_from_config, KeychainSecretStore, SecretRef},
    Error, HelperFacts, Secret, Service,
};
use ferb_platform_macos::identity::{
    helper, owner_secrets as mac_owner_secrets, SignedHelper, PASSED_ENV,
};
use serde_json::{json, Map, Value};
use tempfile::TempDir;
use zeroize::Zeroizing;

// ---------- the fake helper ----------

fn codesign(args: &[&str], path: &Path) -> std::process::Output {
    Command::new("/usr/bin/codesign")
        .args(args)
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

/// `ferb/device/install.py:129-135`: the designated requirement from `codesign -d -r-`.
fn designated(path: &Path) -> String {
    let shown = codesign(&["-d", "-r-"], path);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&shown.stdout),
        String::from_utf8_lossy(&shown.stderr)
    );
    text.lines()
        .find_map(|line| {
            line.strip_prefix("# designated =>")
                .or_else(|| line.strip_prefix("designated =>"))
        })
        .map(|pin| pin.trim().to_owned())
        .unwrap_or_else(|| panic!("no designated requirement in: {text}"))
}

/// The fake helper, signed ad hoc once, and the pin of exactly that code.
fn signed_fake() -> &'static (TempDir, PathBuf, String) {
    static FAKE: OnceLock<(TempDir, PathBuf, String)> = OnceLock::new();
    FAKE.get_or_init(|| {
        // Cargo builds a package's examples with its tests, into `<profile>/examples`, above
        // wherever this test binary is.
        let test = std::env::current_exe().unwrap();
        let built = test
            .ancestors()
            .map(|folder| folder.join("examples/fake_helper"))
            .find(|built| built.is_file())
            .expect("examples/fake_helper is not built: run the tests with `cargo test -p ferb-platform-macos`");
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ferb-secrets");
        fs::copy(&built, &path).unwrap();
        let signed = codesign(&["--force", "--sign", "-", "--identifier", "com.ferb.secrets.test-fake"], &path);
        assert!(signed.status.success(), "{}", String::from_utf8_lossy(&signed.stderr));
        let pin = designated(&path);
        assert!(pin.starts_with("cdhash H\""), "an ad-hoc signature is pinned by its code hash, got {pin}");
        (dir, path, pin)
    })
}

#[derive(Debug, PartialEq)]
struct Call {
    args: Vec<String>,
    env: Vec<String>,
    stdin: Vec<u8>,
}

struct Home {
    dir: TempDir,
}

impl Home {
    /// A temporary FERB home with the signed fake helper and its pin, as setup leaves them.
    fn new() -> Self {
        let home = Home::bare();
        let (_, fake, pin) = signed_fake();
        fs::create_dir(home.path().join("bin")).unwrap();
        fs::copy(fake, home.binary()).unwrap();
        fs::set_permissions(home.binary(), fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(
            home.path().join("bin/ferb-secrets.requirement"),
            format!("{pin}\n"),
        )
        .unwrap();
        home
    }

    fn bare() -> Self {
        Home {
            dir: TempDir::new().unwrap(),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn binary(&self) -> PathBuf {
        self.path().join("bin/ferb-secrets")
    }

    /// The replies the fake gives next, in order, from Python's recorded calls of one step.
    fn script(&self, calls: &Value) {
        let lines: Vec<String> = helper_calls(calls)
            .iter()
            .map(|call| {
                format!(
                    "{} {}",
                    call["code"].as_i64().unwrap(),
                    hex(&unb64(&call["reply"]))
                )
            })
            .collect();
        let bin = self.path().join("bin");
        if bin.is_dir() {
            fs::write(bin.join("replies.txt"), lines.join("\n")).unwrap();
            let _ = fs::remove_file(bin.join("replies.pos"));
            let _ = fs::remove_file(bin.join("calls.log"));
        }
    }

    /// Every run of the fake since the last `script`.
    fn calls(&self) -> Vec<Call> {
        let log = fs::read_to_string(self.path().join("bin/calls.log")).unwrap_or_default();
        log.lines()
            .map(|line| {
                let parts: Vec<&str> = line.split('\t').collect();
                let list = |text: &str| -> Vec<String> {
                    text.split(',')
                        .filter(|p| !p.is_empty())
                        .map(|p| String::from_utf8(unhex(p)).unwrap())
                        .collect()
                };
                Call {
                    args: list(parts[0]),
                    env: list(parts[1]),
                    stdin: unhex(parts[2]),
                }
            })
            .collect()
    }

    /// The helper was run exactly as Python ran it: no argument beyond its own path, the same
    /// stdin bytes in the same order, and nothing in its environment but the short list.
    fn assert_calls(&self, python: &Value, what: &str) {
        let want = helper_calls(python);
        let got = self.calls();
        assert_eq!(
            got.len(),
            want.len(),
            "{what}: how many times the helper ran"
        );
        for (got, want) in got.iter().zip(want) {
            assert_eq!(strings(&want["argv"]), ["$HOME/bin/ferb-secrets"], "{what}");
            assert_eq!(got.args.len(), 1, "{what}: the helper takes no argument");
            assert!(
                Path::new(&got.args[0]).ends_with("bin/ferb-secrets"),
                "{what}"
            );
            assert_eq!(
                String::from_utf8_lossy(&got.stdin),
                unb64_text(&want["stdin"]),
                "{what}: stdin"
            );
            assert_eq!(want["stdout"], "pipe", "{what}");
            assert_eq!(want["stderr"], "null", "{what}");
            for name in &got.env {
                assert!(
                    PASSED_ENV.contains(&name.as_str()),
                    "{what}: {name} reached the helper's environment"
                );
            }
        }
    }
}

/// Python's calls of the helper itself (its `codesign` calls are compared in `src/helper.rs`).
fn helper_calls(calls: &Value) -> Vec<&Value> {
    calls
        .as_array()
        .unwrap()
        .iter()
        .filter(|call| call["argv"][0] != "/usr/bin/codesign")
        .collect()
}

fn service(name: &str) -> Service {
    match name {
        "ferb" => Service::Ferb,
        "ferb.owner-secrets" => Service::OwnerSecrets,
        other => panic!("the helper accepts no service {other:?}"),
    }
}

fn recording(home: &Home) -> (Helper, Arc<Mutex<Vec<HelperFacts>>>) {
    let told = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&told);
    (
        helper(home.path()).on_failure(move |facts| sink.lock().unwrap().push(facts.clone())),
        told,
    )
}

fn told_json(told: &Mutex<Vec<HelperFacts>>) -> Value {
    Value::Array(
        told.lock()
            .unwrap()
            .drain(..)
            .map(|facts| facts.to_json())
            .collect(),
    )
}

/// A refusal is Python's, word for word, with Python's facts; and no error ever shows `hidden`.
fn assert_refused(error: &Error, want: &Value, hidden: &[&str], what: &str) {
    assert_eq!(
        error.refusal(),
        want["refused"].as_str(),
        "{what}: {error:?}"
    );
    match (error, want.get("facts")) {
        (Error::HelperFailed { facts, .. }, Some(want)) => same(&facts.to_json(), want, what),
        (Error::Refused(_), None) => {}
        other => panic!("{what}: the wrong kind of refusal: {other:?}"),
    }
    let shown = format!("{error} {error:?}");
    // (a value too short to tell from an ordinary word, like the fixture's "v", proves nothing)
    for text in hidden.iter().filter(|text| {
        text.len() >= 3 && !want["refused"].as_str().unwrap_or_default().contains(*text)
    }) {
        assert!(
            !shown.contains(text),
            "{what}: an error shows {text:?}: {shown}"
        );
    }
}

fn reply_json(reply: Reply) -> Value {
    let mut out = Map::new();
    // Python's dict has "value" where the helper wrote it; the helper writes nothing else with it.
    if let Some(value) = &reply.value {
        out.insert("value".into(), value.expose().into());
    }
    out.extend(reply.fields);
    Value::Object(out)
}

// ---------- the helper caller ----------

#[test]
fn the_helper_is_called_and_read_as_python_does() {
    for case in cases()["helper"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case.get("body_b64").is_some())
    {
        let name = case["name"].as_str().unwrap();
        let home = if case["helper_files"] == true {
            Home::new()
        } else {
            Home::bare()
        };
        if case["codesign"] != 0 {
            // Python's recorder made codesign say no; here the pin names other code, and the
            // real codesign says no.
            fs::write(
                home.path().join("bin/ferb-secrets.requirement"),
                format!("cdhash H\"{}\"\n", "0".repeat(40)),
            )
            .unwrap();
        }
        home.script(&case["calls"]);
        let body: Value = serde_json::from_slice(&unb64(&case["body_b64"])).unwrap();
        let field = |key: &str| body[key].as_str().unwrap_or_default();
        let value = Secret::from(field("value"));
        let key = body.get("key").map(unb64).unwrap_or_default();
        let request = match field("operation") {
            "store" => Request::Store {
                service: service(field("service")),
                account: field("account"),
                value: &value,
            },
            "lookup" => Request::Lookup {
                service: service(field("service")),
                account: field("account"),
            },
            "present" => Request::Present {
                service: service(field("service")),
                account: field("account"),
            },
            "delete" => Request::Delete {
                service: service(field("service")),
                account: field("account"),
            },
            "seal" => Request::Seal {
                key: &key,
                value: &value,
                command: field("command"),
                origin: field("origin"),
            },
            other => panic!("{other}"),
        };
        let (helper, told) = recording(&home);
        let want = &case["result"];
        match helper.request(&request) {
            Ok(reply) => {
                assert!(
                    !format!("{reply:?}").contains("invented"),
                    "{name}: a reply prints its value"
                );
                let want: Value = serde_json::from_slice(&unb64(&want["ok_b64"]))
                    .unwrap_or_else(|_| panic!("{name}: Python refused, Rust answered"));
                same(&reply_json(reply), &want, name);
            }
            Err(error) => assert_refused(&error, want, &[field("value"), field("account")], name),
        }
        home.assert_calls(&case["calls"], name);
        same(&told_json(&told), &case["told"], name);
    }
}

#[test]
fn helper_seal_gives_the_nonce_and_the_ciphertext() {
    let case = cases()["helper"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "helper.seal")
        .unwrap();
    let home = Home::new();
    home.script(&case["calls"]);
    let spec = &case["seal"];
    let sealed = helper(home.path())
        .seal(
            &unb64(&spec["key"]),
            &Secret::new(unb64_text(&spec["value"])),
            spec["command"].as_str().unwrap(),
            spec["origin"].as_str().unwrap(),
        )
        .unwrap();
    same(
        &json!({"nonce": sealed.nonce, "ct": sealed.ct}),
        &case["result"]["ok"],
        "helper.seal",
    );
    home.assert_calls(&case["calls"], "helper.seal");
}

/// The pin is the exact code: anything else at the helper's path is refused and never started.
#[test]
fn code_the_pin_does_not_name_never_runs() {
    let refused = |home: &Home, what: &str| {
        home.script(&json!([]));
        let error = helper(home.path())
            .store(
                Service::OwnerSecrets,
                "account",
                &Secret::from("invented canary"),
            )
            .unwrap_err();
        assert!(matches!(error, Error::Refused(_)), "{what}: {error:?}");
        assert!(home.calls().is_empty(), "{what}: the helper ran");
        error.to_string()
    };
    let signature = "the secret helper signature does not match the installed identity";
    let unavailable = "the signed secret helper is unavailable; owner setup is required";

    // the same program with one byte changed after it was signed
    let home = Home::new();
    let mut bytes = fs::read(home.binary()).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 1;
    fs::write(home.binary(), bytes).unwrap();
    assert_eq!(refused(&home, "a changed byte"), signature);

    // another validly signed program (codesign itself would accept it with no pin)
    let home = Home::new();
    fs::write(home.binary(), fs::read(home.binary()).unwrap()).unwrap();
    let again = codesign(
        &[
            "--force",
            "--sign",
            "-",
            "--identifier",
            "com.ferb.secrets.other",
        ],
        &home.binary(),
    );
    assert!(again.status.success());
    assert_ne!(designated(&home.binary()), signed_fake().2);
    assert_eq!(refused(&home, "other signed code"), signature);

    // an unsigned script
    let home = Home::new();
    fs::write(home.binary(), "#!/bin/sh\necho '{\"ok\":true}'\n").unwrap();
    assert_eq!(refused(&home, "a script"), signature);

    // an empty pin, and a pin that is no requirement
    for pin in ["", "\n", "not a requirement"] {
        let home = Home::new();
        fs::write(home.path().join("bin/ferb-secrets.requirement"), pin).unwrap();
        assert_eq!(refused(&home, "a bad pin"), signature);
    }

    // no pin, no helper, a folder where the helper should be
    let home = Home::new();
    fs::remove_file(home.path().join("bin/ferb-secrets.requirement")).unwrap();
    assert_eq!(refused(&home, "no pin"), unavailable);
    let home = Home::new();
    fs::remove_file(home.binary()).unwrap();
    assert_eq!(refused(&home, "no helper"), unavailable);
    fs::create_dir(home.binary()).unwrap();
    assert_eq!(refused(&home, "a folder"), unavailable);
}

/// The pinned fake does run, and what it is given holds the value only on stdin.
#[test]
fn a_value_travels_only_on_stdin() {
    let home = Home::new();
    home.script(&json!([{"argv": ["x"], "code": 0, "reply": py::b64encode(b"{\"ok\":true}")}]));
    std::env::set_var("FERB_TEST_CANARY_ENV", "invented canary in the environment");
    helper(home.path())
        .store(
            Service::OwnerSecrets,
            "account",
            &Secret::from("invented canary"),
        )
        .unwrap();
    let calls = home.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].args.len(), 1);
    assert!(!calls[0].args[0].contains("canary"));
    assert!(
        !calls[0].env.iter().any(|name| name.contains("CANARY")),
        "{:?}",
        calls[0].env
    );
    assert!(String::from_utf8_lossy(&calls[0].stdin).contains("invented canary"));
}

// ---------- the program that runs is the pinned code ----------

/// The Mac's launcher with a change to the helper's file made after its check and before its run:
/// the window Python leaves (`helper.py:29-43`).
struct SwapAfterCheck {
    swap: Box<dyn Fn(&Path) + Send + Sync>,
}

impl Launcher for SwapAfterCheck {
    fn verified(&self, home: &Path) -> ferb_identity::Result<PathBuf> {
        let program = SignedHelper.verified(home)?;
        (self.swap)(&program);
        Ok(program)
    }

    fn run(&self, program: &Path, body: &[u8]) -> io::Result<(bool, Zeroizing<Vec<u8>>)> {
        SignedHelper.run(program, body)
    }
}

/// Put `bytes` at the helper's path as a new file (a rename, as a swap would), executable.
fn replace(program: &Path, bytes: &[u8]) {
    let next = program.with_file_name("swapped-in");
    fs::write(&next, bytes).unwrap();
    fs::set_permissions(&next, fs::Permissions::from_mode(0o700)).unwrap();
    fs::rename(&next, program).unwrap();
}

/// Store a canary through a helper that `swap` changes after its check; what came back.
fn store_with_swap(home: &Home, swap: impl Fn(&Path) + Send + Sync + 'static) -> Result<(), Error> {
    home.script(&json!([{"argv": ["x"], "code": 0, "reply": py::b64encode(b"{\"ok\":true}")}]));
    Helper::new(
        home.path(),
        Arc::new(SwapAfterCheck {
            swap: Box::new(swap),
        }),
    )
    .store(
        Service::OwnerSecrets,
        "account",
        &Secret::from("invented canary"),
    )
}

// decision: ENG-6 (fixed against Python: helper.py:29-43 runs whatever is at the path after the check)
#[test]
fn a_helper_swapped_after_its_check_is_refused_before_it_reads_anything() {
    let signature = "the secret helper signature does not match the installed identity";
    let refused = |home: &Home, got: Result<(), Error>, what: &str| {
        let error = got.expect_err(what);
        assert!(matches!(error, Error::Refused(_)), "{what}: {error:?}");
        assert_eq!(error.to_string(), signature, "{what}");
        assert!(
            home.calls().is_empty(),
            "{what}: the swapped-in program ran"
        );
        let bin = fs::read_dir(home.path().join("bin")).unwrap();
        for entry in bin {
            let name = entry.unwrap().file_name().into_string().unwrap();
            assert!(
                !name.starts_with("stolen"),
                "{what}: the request reached {name}"
            );
        }
    };

    // the same fake signed as other code: it would log its stdin, canary included, if it ran
    let home = Home::new();
    let got = store_with_swap(&home, |program| {
        let other = program.with_file_name("other");
        fs::copy(&signed_fake().1, &other).unwrap();
        let signed = codesign(
            &[
                "--force",
                "--sign",
                "-",
                "--identifier",
                "com.ferb.secrets.other",
            ],
            &other,
        );
        assert!(signed.status.success());
        fs::rename(&other, program).unwrap();
    });
    refused(&home, got, "other signed code");

    // a script that keeps what it is given: what runs is /bin/sh, which the pin does not name
    let home = Home::new();
    let got = store_with_swap(&home, |program| {
        replace(
            program,
            b"#!/bin/sh\ncat > \"$(dirname \"$0\")/stolen\"\necho '{\"ok\":true}'\n",
        )
    });
    refused(&home, got, "a script");

    // the pinned program with one byte changed and its signature left as it was: the code the
    // kernel loads carries the pinned hash, so only the check of its file's pages refuses it
    let home = Home::new();
    let got = store_with_swap(&home, |program| {
        let mut bytes = fs::read(program).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 1;
        replace(program, &bytes);
    });
    refused(&home, got, "a changed byte under the pinned signature");
}

/// What is checked is the code: the same pinned program put back as a new file still runs.
#[test]
fn the_pinned_code_swapped_for_itself_still_runs() {
    let home = Home::new();
    store_with_swap(&home, |program| {
        replace(program, &fs::read(program).unwrap())
    })
    .unwrap();
    let calls = home.calls();
    assert_eq!(calls.len(), 1);
    assert!(String::from_utf8_lossy(&calls[0].stdin).contains("invented canary"));
}

// ---------- owner secrets ----------

fn run_step(home: &Home, step: &Value, what: &str) {
    home.script(&step["calls"]);
    let (helper, told) = recording(home);
    let store = OwnerSecrets::with_keychain(home.path(), Arc::new(helper), Arc::new(Recorded));
    let text = |key: &str| step[key].as_str().unwrap_or_default();
    let sites = strings(&step["sites"]);
    let held: Option<BTreeSet<String>> = step
        .get("holding")
        .filter(|h| !h.is_null())
        .map(|h| strings(h).into_iter().collect());
    let holds = |source: &str| held.as_ref().is_some_and(|held| held.contains(source));
    let holding: Option<&dyn Fn(&str) -> bool> = if held.is_some() { Some(&holds) } else { None };
    let sources: BTreeSet<String> = strings(&step["sources"]).into_iter().collect();
    let value = step.get("value_b64").map(|v| Secret::new(unb64_text(v)));
    let mut revealed = None;
    let got: Result<Value, Error> = match text("op") {
        "listing" => store.listing(text("place"), &sites, holding),
        "remember" => store
            .remember(
                text("name"),
                text("place"),
                value.as_ref().unwrap(),
                &sites,
                step["kind"].as_str(),
            )
            .map(|()| Value::Null),
        "reveal" => store
            .reveal(text("name"), text("place"), step["kind"].as_str())
            .map(|secret| {
                assert_eq!(format!("{secret:?}"), "<secret>");
                revealed = Some(secret);
                Value::Null
            }),
        "set_username" => store
            .set_username(text("place"), text("username"), text("source"))
            .map(|()| Value::Null),
        "replace_usernames" => store
            .replace_usernames(&sources, step["username"].as_str(), step["source"].as_str())
            .map(Value::from),
        "clear_usernames" => store.clear_usernames(&sources).map(Value::from),
        "inventory" => store.inventory().map(Value::from),
        "records" => store.records().map(Value::Object),
        "unusable" => store.unusable().map(Value::from),
        "delete" => store.delete(text("place"), text("name")),
        "account" => store
            .account(text("place"), &sites, holding)
            .map(|(name, from)| json!([name, from])),
        "username" => store.username(text("place"), &sites).map(Value::from),
        other => panic!("{other}"),
    };
    let want = &step["result"];
    match got {
        Ok(got) => match (want.get("ok_b64"), revealed) {
            (Some(want), Some(secret)) => assert_eq!(
                json!(secret.expose()),
                serde_json::from_slice::<Value>(&unb64(want)).unwrap(),
                "{what}"
            ),
            _ => same(
                &got,
                want.get("ok")
                    .unwrap_or_else(|| panic!("{what}: Python gave {want}, Rust {got}")),
                what,
            ),
        },
        Err(error) if want.get("raised").is_some() => {
            assert!(error.refusal().is_none(), "{what}: {error:?}")
        }
        Err(error) => {
            let hidden = value
                .as_ref()
                .map(|v| v.expose().to_owned())
                .unwrap_or_default();
            assert_refused(&error, want, &[hidden.as_str(), " \u{b7} "], what);
        }
    }
    home.assert_calls(&step["calls"], what);
    same(&told_json(&told), &step["told"], what);
    let file = home.path().join(owner_secrets::FILE_NAME);
    match &step["file"] {
        Value::Null => assert!(!file.is_file(), "{what}: Python wrote no file"),
        bytes => {
            assert_eq!(
                String::from_utf8_lossy(&fs::read(&file).unwrap()),
                String::from_utf8_lossy(&unb64(bytes)),
                "{what}: the file"
            );
            let mode = fs::metadata(&file).unwrap().permissions().mode() & 0o7777;
            assert_eq!(
                format!("0o{mode:o}"),
                step["mode"].as_str().unwrap(),
                "{what}: the file's mode"
            );
        }
    }
    let mut left: Vec<String> = fs::read_dir(home.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(
        left,
        strings(&step["left"]),
        "{what}: what is left in the home"
    );
}

#[test]
fn owner_secrets_flows_match_python_step_by_step() {
    for (name, flow) in cases()["owner"]["flows"].as_object().unwrap() {
        let home = Home::new();
        if let Some(start) = flow.get("start").filter(|s| !s.is_null()) {
            fs::write(home.path().join(owner_secrets::FILE_NAME), unb64(start)).unwrap();
        }
        for (i, step) in flow["steps"].as_array().unwrap().iter().enumerate() {
            run_step(&home, step, &format!("{name}, step {i} ({})", step["op"]));
        }
    }
}

/// Every stored form Python reads is read the same way; every form it refuses is refused in its
/// words and the file is left byte for byte (each probe starts from the file again).
#[test]
fn handle_files_are_read_or_refused_as_python_does() {
    for case in cases()["owner"]["files"].as_array().unwrap() {
        let content = unb64(&case["content"]);
        for step in case["steps"].as_array().unwrap() {
            let home = Home::new();
            fs::write(home.path().join(owner_secrets::FILE_NAME), &content).unwrap();
            let what = format!("{:?}, {}", String::from_utf8_lossy(&content), step["op"]);
            run_step(&home, step, &what);
            if case["readable"] == false {
                assert!(
                    step["result"]["refused"]
                        .as_str()
                        .unwrap()
                        .contains(owner_secrets::FILE_NAME),
                    "{what}"
                );
                assert_eq!(
                    fs::read(home.path().join(owner_secrets::FILE_NAME)).unwrap(),
                    content,
                    "{what}"
                );
            }
        }
    }
    let home = Home::new();
    fs::create_dir(home.path().join(owner_secrets::FILE_NAME)).unwrap();
    for step in cases()["owner"]["folder"].as_array().unwrap() {
        run_step(&home, step, "a folder where the file should be");
    }
}

// ---------- configured secrets ----------

#[test]
fn configured_secrets_match_python() {
    let fixture = &cases()["secrets"];
    let config = fixture["config"].as_object().unwrap();
    let specs = specs_from_config(Some(config));
    let shown: Vec<Value> = specs
        .iter()
        .map(|s| json!({"key": s.secret.key, "allowed_origins": s.allowed_origins, "account_hint": s.account_hint}))
        .collect();
    same(&json!(shown), &fixture["specs"], "specs_from_config");
    assert!(specs_from_config(None).is_empty());
    for case in fixture["origin_allowed"].as_array().unwrap() {
        let spec = specs
            .iter()
            .find(|s| s.secret.key == case["key"].as_str().unwrap())
            .unwrap();
        let got = origin_allowed(case["origin"].as_str().unwrap(), spec)
            .map(Value::from)
            .unwrap_or(json!("ValueError"));
        assert_eq!(got, case["out"], "{case}");
    }
    for case in fixture["store"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let home = Home::new();
        home.script(&case["calls"]);
        let (helper, told) = recording(&home);
        let again = specs.clone();
        let store = KeychainSecretStore::new(move || again.clone(), helper);
        let secret = SecretRef {
            key: case["key"].as_str().unwrap().to_owned(),
        };
        assert_eq!(store.has(&secret), case["has"], "{name}");
        let want = &case["result"];
        if case["reveal"] == true {
            match store.reveal(&secret) {
                Ok(value) => assert_eq!(value.expose(), unb64_text(&want["value_b64"]), "{name}"),
                Err(e) => {
                    assert_eq!(e.to_string(), want["unavailable"], "{name}");
                    assert_eq!(e.reason, want["reason"], "{name}");
                }
            }
        } else {
            assert_eq!(store.present(&secret), want["present"], "{name}");
        }
        home.assert_calls(&case["calls"], name);
        same(&told_json(&told), &case["told"], name);
    }
}

// ---------- secrets.list ----------

#[test]
fn secrets_list_matches_python() {
    for case in cases()["tool"].as_array().unwrap() {
        let home = Home::new();
        home.script(&json!([]));
        fs::write(
            home.path().join(owner_secrets::FILE_NAME),
            unb64(&case["start"]),
        )
        .unwrap();
        let store = mac_owner_secrets(home.path(), Arc::new(Recorded));
        let sites = strings(&case["sites"]);
        let held: Option<BTreeSet<String>> = Some(&case["holding"])
            .filter(|h| !h.is_null())
            .map(|h| strings(h).into_iter().collect());
        let holds = |source: &str| held.as_ref().is_some_and(|held| held.contains(source));
        let holding: Option<&dyn Fn(&str) -> bool> =
            if held.is_some() { Some(&holds) } else { None };
        let got = secrets_list(
            &store,
            case["place"].as_str().unwrap(),
            &|_| sites.clone(),
            holding,
        );
        same(&got.content, &case["content"], &case["place"].to_string());
        assert_eq!(got.is_error, case["is_error"], "{}", case["place"]);
        assert!(home.calls().is_empty(), "a listing never asks the helper");
    }
}
