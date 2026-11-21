//! Pairing that converges, the extension's half (HAND-566, extension 0.19.12): the extension's own JavaScript under
//! node with fake Chrome APIs and a fake clock (tests/extension/harness.js), never a real browser. Each scenario opens
//! the worker's link to FERB (a fake native port that records what was posted) and plays FERB's answers on it.
//!
//! Live 2026-11-16: a fresh FERB install over browser profiles whose extension kept the previous install's key. FERB
//! refused the older key's hellos; the extension kept presenting that key (46,331 messages on one connection) and
//! nothing told it the key was dead, so only a pairing replaced it.

use std::path::PathBuf;
use std::process::Command;

use serde_json::{json, Value};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// node, from PATH or where Homebrew and the installer put it; `None` when this Mac has none.
fn node() -> Option<PathBuf> {
    let mut places: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join("node")).collect())
        .unwrap_or_default();
    places.push("/opt/homebrew/bin/node".into());
    places.push("/usr/local/bin/node".into());
    places.into_iter().find(|p| p.is_file())
}

/// Helpers every scenario shares: the stored state, the links opened, what each posted, FERB's lines.
const WORLD: &str = r#"
  const OLD = btoa("o".repeat(32)), NEW = btoa("n".repeat(32));
  const stored = async () => (await chrome.storage.local.get("ferb")).ferb;
  const status = async () => ((await chrome.storage.session.get("ferbConnection")).ferbConnection || {}).state;
  const link = (i = fake.ports.length - 1) => fake.ports[i];
  const posted = (i) => link(i).posted.map((m) => ({ type: m.type, seq: m.seq ?? null, signed: !!m.mac,
                                                     nonce: m.type === "hello" ? JSON.parse(m.body_json).nonce : null }));
  const daemon = async (m, i) => { for (const f of link(i).onMessage.listeners) await f(m); await settle(); };
  const hostExits = async (i) => { for (const f of link(i).onDisconnect.listeners) f(); await settle(); };
  const lastHello = (i) => posted(i).filter((m) => m.type === "hello").pop();
"#;

fn worker(scenario: &str) -> Option<Value> {
    let Some(node) = node() else {
        eprintln!("SKIPPED: no node on this Mac, so the extension's JavaScript wasn't run");
        return None;
    };
    let out = Command::new(node)
        .arg(root().join("engine/crates/ferb-browser/tests/extension/harness.js"))
        .arg(root().join("extension"))
        .arg("worker")
        .arg(format!("{WORLD}\n{scenario}"))
        .output()
        .expect("node runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(serde_json::from_slice(&out.stdout).expect("the scenario's JSON"))
}

/// FERB refuses a hello signed with a key that isn't its own for this install and names that hello (`stale_key`):
/// the extension drops exactly that key (its install id stays), says it isn't paired, says hello once more unsigned,
/// and stops presenting the dead key; nothing else is posted and no new link is opened.
// decision: HAND-566
#[test]
fn a_refusal_naming_a_hello_signed_with_an_older_key_drops_that_key() {
    let Some(got) = worker(
        r#"
        await chrome.storage.local.set({ ferb: { installId: "I1", key: OLD, seq: 169686 } });
        await ensurePort();
        await settle();
        const first = lastHello();
        await daemon({ type: "refused", reason: "extension pairing failed (bad_mac); pair this profile again",
                       stale_key: { nonce: first.nonce, seq: first.seq, reason: "bad_mac" } });
        await advance(120000);
        const st = await stored();
        return { first, after: posted().slice(1), installId: st.installId, key: st.key, seq: st.seq,
                 status: await status(), links: fake.ports.length, refused };
        "#,
    ) else {
        return;
    };
    assert_eq!(got["first"]["signed"], true);
    assert_eq!(got["first"]["seq"], 169_687);
    assert_eq!(got["installId"], "I1");
    assert_eq!(got["key"], Value::Null);
    assert_eq!(got["seq"], 0);
    assert_eq!(got["status"], "pairing_required");
    assert_eq!(got["refused"], Value::Null);
    assert_eq!(got["links"], 1);
    let after = got["after"].as_array().unwrap();
    assert_eq!(after.len(), 1, "{after:?}");
    assert_eq!(after[0]["type"], "hello");
    assert_eq!(after[0]["signed"], false);
}

/// The owner's race: the extension posted a hello with its older key, then the pairing page's token; FERB paired it
/// (`paired` with the new key), then refused the older-key hello. That refusal names a key the extension no longer
/// holds, so the new key stays and its hello is the next one posted, on the same link.
// decision: HAND-566
#[test]
fn a_refusal_of_an_older_hello_after_a_pairing_keeps_the_new_key() {
    let Some(got) = worker(
        r#"
        await chrome.storage.local.set({ ferb: { installId: "I1", key: OLD, seq: 500 } });
        await ensurePort();
        await settle();
        const old = lastHello();
        for (const f of chrome.runtime.onMessage.listeners) f({ pairToken: "tok" }, {}, () => {});
        await settle();
        await daemon({ type: "paired", key: NEW });
        await daemon({ type: "refused", reason: "extension pairing failed (bad_mac); pair this profile again",
                       stale_key: { nonce: old.nonce, seq: old.seq, reason: "bad_mac" } });
        await advance(1000);
        const st = await stored();
        return { posted: posted(), key: st.key === NEW, links: fake.ports.length, status: await status() };
        "#,
    ) else {
        return;
    };
    assert_eq!(got["key"], true, "{got}");
    assert_eq!(got["links"], 1);
    let types: Vec<&str> = got["posted"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["type"].as_str().unwrap())
        .collect();
    assert_eq!(types, ["hello", "pair", "hello"], "{got}");
    // the new key's hello starts its counter afresh
    assert_eq!(got["posted"][2]["seq"], 1);
    assert_ne!(got["status"], "pairing_required");
}

/// HAND-538's gap: a token handed over while the link to FERB is down (the host had exited) used to wait in the outbox,
/// which only a welcome flushes, and an unpaired copy never gets one. Now the token opens a link at once (the owner's
/// click: no backoff) and the pair goes right after that link's hello.
// decision: HAND-566, HAND-538
#[test]
fn a_token_handed_over_while_the_link_is_down_is_sent_when_a_link_is_up() {
    let Some(got) = worker(
        r#"
        await chrome.storage.local.set({ ferb: { installId: "I1", key: null, seq: 0 } });
        await ensurePort();
        await settle();
        await hostExits(0);                       // FERB wasn't up yet
        for (const f of chrome.runtime.onMessage.listeners) f({ pairToken: "tok" }, {}, () => {});
        await settle();
        return { links: fake.ports.length, second: posted(1).map((m) => m.type), outbox: outbox.length };
        "#,
    ) else {
        return;
    };
    assert_eq!(got["links"], 2, "{got}");
    assert_eq!(got["second"], json!(["hello", "pair"]), "{got}");
    assert_eq!(got["outbox"], 0);
}

/// After FERB's end of the link closes, what the worker sends (a tab that loaded, an event) waits for the backoff
/// instead of starting a native host each time; the scheduled retry opens the next link.
// decision: HAND-566
#[test]
fn a_closed_link_is_retried_on_the_backoff_not_on_every_message() {
    let Some(got) = worker(
        r#"
        await chrome.storage.local.set({ ferb: { installId: "I1", key: OLD, seq: 1 } });
        await ensurePort();
        await settle();
        await hostExits(0);
        fake.tabs.set(5, { id: 5, windowId: 1, url: "https://a.example/", active: true });
        for (let i = 0; i < 20; i++) {
          for (const f of chrome.tabs.onUpdated.listeners) f(5, { status: "complete" }, fake.tabs.get(5));
          await advance(40);
        }
        const during = fake.ports.length;
        await advance(1000);
        return { during, after: fake.ports.length };
        "#,
    ) else {
        return;
    };
    assert_eq!(got["during"], 1, "{got}");
    assert_eq!(got["after"], 2, "{got}");
}

/// A link FERB never answered is closed by the handshake's timeout and cleared at once (Chrome tells only the other
/// end of a close): before, the worker kept that dead port and posted every later message into it.
// decision: HAND-566
#[test]
fn a_link_closed_by_the_handshake_timeout_is_cleared_and_retried() {
    let Some(got) = worker(
        r#"
        await chrome.storage.local.set({ ferb: { installId: "I1", key: null, seq: 0 } });
        await ensurePort();
        await settle();
        await advance(15000);
        const cleared = port === null && link(0).closed;
        await advance(1000);
        return { cleared, links: fake.ports.length, status: await status() };
        "#,
    ) else {
        return;
    };
    assert_eq!(got["cleared"], true, "{got}");
    assert_eq!(got["links"], 2, "{got}");
}

/// A replay refusal of a hello signed with FERB's own key says the counter fell behind the one FERB last took: the
/// counter moves past it and the hello is said again; the key stays.
// decision: HAND-566
#[test]
fn a_counter_that_fell_behind_moves_past_the_one_ferb_took() {
    let Some(got) = worker(
        r#"
        await chrome.storage.local.set({ ferb: { installId: "I1", key: NEW, seq: 3 } });
        await ensurePort();
        await settle();
        const h = lastHello();
        await daemon({ type: "refused", reason: "extension pairing failed (replay); pair this profile again",
                       stale_key: { nonce: h.nonce, seq: h.seq, reason: "replay", accepted: 900 } });
        await settle();
        const st = await stored();
        return { key: st.key === NEW, hellos: posted().filter((m) => m.type === "hello").map((m) => m.seq) };
        "#,
    ) else {
        return;
    };
    assert_eq!(got["key"], true);
    assert_eq!(got["hellos"], json!([4, 901]), "{got}");
}

/// Many hellos asked for at once (alarms, reconnects) wait as one: never a pile of them behind the state lock.
// decision: HAND-566
#[test]
fn hellos_asked_for_at_once_are_said_once() {
    let Some(got) = worker(
        r#"
        await chrome.storage.local.set({ ferb: { installId: "I1", key: OLD, seq: 1 } });
        await ensurePort();
        await settle();
        const before = posted().length;
        await Promise.all(Array.from({ length: 200 }, () => announceHello()));
        await settle();
        return { more: posted().length - before };
        "#,
    ) else {
        return;
    };
    assert_eq!(got["more"], 1, "{got}");
}

/// 0.19.12 adds only optional handling (`stale_key` on a refusal, which an older FERB never sends), so the protocol
/// number and FERB's minimum stay: a FERB without it and an extension without it keep working together. Later
/// versions (0.19.13, HAND-649) keep it.
// decision: HAND-566
#[test]
fn extension_0_19_12_keeps_protocol_21_and_the_minimum() {
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("extension/manifest.json")).unwrap(),
    )
    .unwrap();
    let version = manifest["version"].as_str().unwrap();
    assert!(
        ferb_browser::protocol::version_ok(version, "0.19.12").unwrap(),
        "{version}"
    );
    let protocol = std::fs::read_to_string(root().join("extension/protocol.js")).unwrap();
    assert!(protocol.contains(&format!(
        "const FERB_PROTOCOL = {};",
        ferb_browser::protocol::PROTOCOL_VERSION
    )));
    assert_eq!(ferb_browser::protocol::MIN_EXTENSION_VERSION, "0.19.0");
}
