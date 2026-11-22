//! The extension's pairing page (`extension/pair.js`) under node with a fake page and fake Chrome APIs, never a real
//! browser. With its one-time token it hands the token to the profile's service worker and closes itself; without one
//! (typed by hand, opened from history) it closes nothing and says what it is (HAND-680, extension 0.19.14).
//!
//! Live 2026-11-22 (task a843aa54…): a worker typed `chrome-extension://…/pair.html` into a profile's only window three
//! times; the page closed itself at once each time, and the window with it.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn node() -> Option<PathBuf> {
    let mut places: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join("node")).collect())
        .unwrap_or_default();
    places.push("/opt/homebrew/bin/node".into());
    places.push("/usr/local/bin/node".into());
    places.into_iter().find(|p| p.is_file())
}

/// Runs pair.js in a fake page at `hash`; what it did: the messages it sent, whether it closed, the text it showed.
const PAGE: &str = r#"
const vm = require("vm"), fs = require("fs");
const [file, hash] = process.argv.slice(1);
const did = { sent: [], closed: 0, shown: "", replaced: [] };
const loaded = [];
const body = { append: (el) => { did.shown += el.textContent; } };
const sandbox = {
  URLSearchParams,
  location: { hash, pathname: "/pair.html" },
  history: { replaceState: (_s, _t, url) => did.replaced.push(url) },
  window: { close: () => { did.closed += 1; } },
  chrome: { runtime: { sendMessage: (m, done) => { did.sent.push(m); done(); } } },
  document: {
    body,
    createElement: () => ({ textContent: "", style: {} }),
    addEventListener: (name, fn) => { if (name === "DOMContentLoaded") loaded.push(fn); },
  },
};
vm.runInNewContext(fs.readFileSync(file, "utf8"), sandbox);
loaded.forEach((fn) => fn());
process.stdout.write(JSON.stringify(did));
"#;

fn open(hash: &str) -> Option<Value> {
    let Some(node) = node() else {
        eprintln!("SKIPPED: no node on this Mac, so the extension's JavaScript wasn't run");
        return None;
    };
    let out = Command::new(node)
        .arg("-e")
        .arg(PAGE)
        .arg(root().join("extension/pair.js"))
        .arg(hash)
        .output()
        .expect("node runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(serde_json::from_slice(&out.stdout).expect("the page's JSON"))
}

// decision: HAND-677, HAND-680
#[test]
fn the_pairing_page_without_its_token_closes_nothing_and_says_what_it_is() {
    let Some(did) = open("") else { return };
    assert_eq!(did["closed"], 0, "{did}");
    assert_eq!(did["sent"], serde_json::json!([]), "{did}");
    let shown = did["shown"].as_str().unwrap();
    assert!(shown.contains("FERB's pairing page"), "{did}");
    let Some(did) = open("#token=abc") else {
        return;
    };
    assert_eq!(
        did["sent"],
        serde_json::json!([{"pairToken": "abc"}]),
        "{did}"
    );
    assert_eq!(did["closed"], 1, "{did}");
    assert_eq!(did["shown"], "", "{did}");
    // the token leaves the address before anything else
    assert_eq!(did["replaced"], serde_json::json!(["/pair.html"]), "{did}");
}
