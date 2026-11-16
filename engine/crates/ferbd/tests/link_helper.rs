//! Stage 3 S4 (T-1204): the phone link's helper as the composed `ferbd daemon` runs it, with a stand-in helper (a
//! small shell script, never the real program and never the Keychain: the test daemon keeps the link's one item in
//! memory). Off on a fresh install, nothing runs; FERB.app's `phone.enable` starts it with the keys on its standard
//! input and nothing secret in its arguments; off again, its input closes and it ends; one that ends by itself is
//! journaled `daemon.loop_failed` and started again.

mod support;

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use support::*;

fn call(run: &Run, method: &str, params: Value) -> Value {
    let mut c = Raw::connect(&run.socket()).unwrap();
    c.hello("cli").unwrap();
    c.send(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string());
    serde_json::from_str(&c.recv().expect("an answer")).unwrap()
}

fn wait(secs: u64, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    f()
}

/// A stand-in helper: writes its arguments and the first line of its input to `dir`, then waits for its input to end
/// (`stay`) or ends at once.
fn helper(dir: &Path, stay: bool) -> std::path::PathBuf {
    let script = dir.join(if stay { "helper-stays" } else { "helper-ends" });
    let out = dir.join("seen");
    std::fs::create_dir_all(&out).unwrap();
    let body = format!(
        "#!/bin/sh\nn=$(ls {o} | wc -l | tr -d ' ')\necho $$ > {o}/pid-$n\necho \"$@\" > {o}/args-$n\nIFS= read -r keys\necho \"$keys\" > {o}/keys-$n\n{rest}\n",
        o = out.display(),
        rest = if stay {
            format!("cat > /dev/null\necho ended > {}/ended-$n", out.display())
        } else {
            "exit 3".to_string()
        }
    );
    std::fs::write(&script, body).unwrap();
    let mut p = std::fs::metadata(&script).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut p, 0o755);
    std::fs::set_permissions(&script, p).unwrap();
    script
}

fn seen(dir: &Path, prefix: &str) -> Vec<String> {
    let mut files: Vec<_> = std::fs::read_dir(dir.join("seen"))
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    files.sort();
    files
        .iter()
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(prefix))
        .map(|p| std::fs::read_to_string(p).unwrap_or_default())
        .collect()
}

// decision: PHN-488
// decision: PHN-487
#[test]
fn the_helper_runs_while_the_phone_feature_is_on_with_its_keys_on_its_input() {
    let tmp = root();
    let dir = tmp.path().join("h");
    std::fs::create_dir_all(&dir).unwrap();
    let script = helper(&dir, true);
    let mut run = Run::start(
        tmp.path(),
        "01",
        json!({"link_helper": script.to_string_lossy()}),
    );
    // off on a fresh install: nothing runs
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        seen(&dir, "args").is_empty(),
        "the helper ran with the feature off"
    );
    let st = call(&run, "phone.link_status", json!({}));
    assert_eq!(st["result"]["enabled"], false, "{st}");
    assert_eq!(st["result"]["running"], false, "{st}");
    // FERB.app turns it on: started, with the keys on its input and nothing secret in its arguments
    let on = call(&run, "phone.enable", json!({"on": true}));
    assert_eq!(on["result"]["enabled"], true, "{on}");
    assert!(
        wait(10, || !seen(&dir, "keys").is_empty()),
        "the helper never started"
    );
    let args = seen(&dir, "args")[0].clone();
    assert!(
        args.starts_with("run --daemon ") && args.contains(" --state "),
        "{args}"
    );
    let keys: Value = serde_json::from_str(seen(&dir, "keys")[0].trim()).unwrap();
    let identity = keys["keys"]["identity"].as_str().unwrap().to_string();
    let state = keys["keys"]["state-key"].as_str().unwrap().to_string();
    assert_eq!((identity.len(), state.len()), (44, 44), "{keys}");
    assert!(!args.contains(&identity) && !args.contains(&state));
    // off: its input closes and it ends by itself
    let off = call(&run, "phone.enable", json!({"on": false}));
    assert_eq!(off["result"]["enabled"], false, "{off}");
    assert!(
        wait(10, || !seen(&dir, "ended").is_empty()),
        "the helper did not end"
    );
    // on again: the same keys (made once, kept in the one item)
    call(&run, "phone.enable", json!({"on": true}));
    assert!(wait(10, || seen(&dir, "keys").len() == 2));
    let again: Value = serde_json::from_str(seen(&dir, "keys")[1].trim()).unwrap();
    assert_eq!(again["keys"]["identity"], json!(identity));
    let pid = seen(&dir, "pid").last().unwrap().trim().to_string();
    run.stop();
    // stopped with the daemon
    let alive = || {
        std::process::Command::new("/bin/kill")
            .args(["-0", &pid])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    assert!(wait(10, || !alive()), "the helper outlived the daemon");
}

// decision: PHN-488
#[test]
fn a_helper_that_ends_by_itself_is_journaled_and_started_again() {
    let tmp = root();
    let dir = tmp.path().join("h");
    std::fs::create_dir_all(&dir).unwrap();
    let script = helper(&dir, false);
    let run = Run::start(
        tmp.path(),
        "01",
        json!({"link_helper": script.to_string_lossy()}),
    );
    call(&run, "phone.enable", json!({"on": true}));
    // the stand-ins' pause doesn't wait: a second start follows the first failure
    assert!(
        wait(15, || seen(&dir, "keys").len() >= 2),
        "it was not started again"
    );
    let db = rusqlite::Connection::open(run.home.join("ferb.db")).unwrap();
    db.busy_timeout(Duration::from_secs(5)).unwrap();
    let rows: Vec<String> = db
        .prepare("SELECT data_json FROM journal WHERE kind = 'daemon.loop_failed'")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        rows.iter()
            .any(|r| r.contains("\"phone link\"") && r.contains("ended by itself")),
        "{rows:?}"
    );
}
