//! Stage 3 S2 (T-1201): every method of the engine's table has a phone class, the classes follow the phone decisions,
//! and the local socket never gives the phone role. `engine/fixtures/phone/classes.json` is the table as data, for the
//! link crate and the phone apps (`UPDATE_GOLDEN=1 cargo test -p ferb-rpc --test phone_classes` writes it again).

mod common;

use std::collections::BTreeSet;
use std::path::PathBuf;

use ferb_rpc::auth::{allowed, Role, METHODS, PHONE_FILES, PHONE_OWN, PHONE_SESSION};
use ferb_rpc::classes::{class, locked, phone_method, Class, Locked, PHONE_METHODS};
use ferb_rpc::wire;
use serde_json::{json, Value};

const GOLDEN: &str = "../../fixtures/phone/classes.json";

fn names(of: impl Fn(&ferb_rpc::classes::PhoneMethod) -> bool) -> BTreeSet<&'static str> {
    PHONE_METHODS
        .iter()
        .filter(|row| of(row))
        .map(|row| row.name)
        .collect()
}

fn set(items: &[&'static str]) -> BTreeSet<&'static str> {
    items.iter().copied().collect()
}

/// A method added to the engine's table without a phone class fails here (and is `never` meanwhile).
// decision: PHN-112
// decision: PHN-465
#[test]
fn every_method_of_the_engine_has_exactly_one_phone_class() {
    let unclassified: Vec<&str> = METHODS
        .iter()
        .map(|m| m.name)
        .filter(|name| phone_method(name).is_none())
        .collect();
    assert!(
        unclassified.is_empty(),
        "methods with no phone class (engine/crates/ferb-rpc/src/classes.rs): {unclassified:?}"
    );
    let stale: Vec<&str> = PHONE_METHODS
        .iter()
        .map(|row| row.name)
        .filter(|name| ferb_rpc::auth::method(name).is_none())
        .collect();
    assert!(
        stale.is_empty(),
        "phone classes for methods the engine no longer has: {stale:?}"
    );
    let mut seen = BTreeSet::new();
    for row in PHONE_METHODS {
        assert!(seen.insert(row.name), "{} is classified twice", row.name);
        assert!(!row.why.trim().is_empty(), "{} has no reason", row.name);
    }
    assert_eq!(PHONE_METHODS.len(), METHODS.len());
    // default-deny: what has no row is never, for the class and for the role
    for name in [
        "no.such.method",
        "",
        "phone.setup",
        "link.unlock",
        "auth.hello",
    ] {
        assert_eq!(class(name), Class::Never, "{name}");
        assert_eq!(locked(name), Locked::Refused, "{name}");
        assert!(!allowed(Role::Phone, name), "{name}");
    }
}

/// The paired phone is the owner's full remote: every method FERB.app has is reachable except those named `never`,
/// each with the reason the phone is told.
// decision: PHN-111
// decision: PHN-117
// decision: PHN-466
// decision: PHN-581
#[test]
fn the_phone_reaches_everything_but_the_never_rows() {
    let never = names(|row| row.class == Class::Never);
    assert_eq!(
        never,
        set(&[
            "about.guide",
            "chat.history.import",
            "chat.history.put",
            "device.report",
            // T-1274: FERB.app's own reports from its insomnia cover and the camera's frames
            "insomnia.camera_frame",
            "insomnia.report",
            "intro.preview",
            "link.answer",
            "link.devices",
            "link.events",
            "link.pair_cancel",
            "link.pair_request",
            "link.pair_wait",
            "notifications.subscribe",
            "phone.audit",
            "phone.enable",
            "phone.link_status",
            "phone.pair_cancel",
            "phone.pair_decide",
            "phone.pair_start",
            "phone.relay_always",
            "phone.relay_set",
            "phone.relay_test",
            "phone.rename",
            "phone.upload_folder",
            "setup.uninstall_browser_files",
            "tools.call",
            "tools.list",
        ])
    );
    for m in METHODS {
        assert_eq!(
            allowed(Role::Phone, m.name),
            !never.contains(m.name),
            "{}",
            m.name
        );
    }
    for row in PHONE_METHODS.iter().filter(|row| row.class == Class::Never) {
        assert!(
            row.why.len() >= 20,
            "{}: a reason the phone is told",
            row.name
        );
    }
    // what is not FERB.app's at all (a model worker's tool bridge, the link helper's own calls) is not the phone's
    // either; a phone session's own calls belong to no local role (stage 3)
    for m in METHODS.iter().filter(|m| {
        !m.roles.contains(&Role::Cli)
            && !PHONE_SESSION.contains(&m.name)
            && !PHONE_FILES.contains(&m.name)
            && !PHONE_OWN.contains(&m.name)
    }) {
        assert_eq!(class(m.name), Class::Never, "{}", m.name);
    }
}

/// The five FERB.app-only data writers and notification takers stay off the phone (the owner: "keep them off").
// decision: PHN-134
#[test]
fn the_five_stay_off_the_phone() {
    for name in [
        "notifications.subscribe",
        "intro.preview",
        "about.guide",
        "chat.history.put",
        "chat.history.import",
    ] {
        assert_eq!(class(name), Class::Never, "{name}");
        assert!(!phone_method(name).unwrap().owner_to_confirm, "{name}");
    }
}

/// The six that open a prompt or window on the Mac's screen are callable, signed, and say so.
// decision: PHN-117
#[test]
fn the_six_on_the_mac_s_screen_are_sensitive_and_marked() {
    assert_eq!(
        names(|row| row.on_mac_screen),
        set(&[
            "automation.request",
            "browser.show_handoff",
            "clipboard.request",
            "setup.browser_open",
            "setup.permission_request",
            "ui.click_through",
        ])
    );
    for name in names(|row| row.on_mac_screen) {
        assert_eq!(class(name), Class::Sensitive, "{name}");
    }
}

/// A connection the Mac has not seen unlocked reaches only status, Stop and answers that prove themselves.
// decision: PHN-125
// decision: GATE-615
// decision: APP-983
#[test]
fn a_locked_phone_reaches_only_status_stop_and_self_proving_answers() {
    let mut allowed_locked = set(&["chat.status", "ping", "stop.engage", "task.stop"]);
    // and the phone session's own calls (stage 3), each carrying its own proof where it needs one
    allowed_locked.extend(PHONE_SESSION);
    // and an upload's chunks and end, which ride the unlock its begin needed (stage 3 S3b, PHN-127)
    allowed_locked.extend(["phone.put_chunk", "phone.put_end"]);
    // and the phone's own device record, which gives the phone nothing of FERB's; the daemon takes it while locked only
    // from a connection that proved the phone's device key (T-1214, PHN-126)
    allowed_locked.extend(["phone.device_report"]);
    // and "until I'm back at my Mac": what is in force and its end, which only takes something away (GATE-615)
    allowed_locked.extend(["screen_use.end", "screen_use.status"]);
    // and insomnia mode's end, Lock my Mac and It's fine, from the notification: each only takes something away
    // (APP-980, APP-983)
    allowed_locked.extend(["insomnia.end", "insomnia.lock", "insomnia.dismiss"]);
    assert_eq!(names(|row| row.locked == Locked::Allowed), allowed_locked);
    assert_eq!(
        names(|row| row.locked == Locked::Answer),
        set(&["chat.answer", "chat.approve", "chat.correct"])
    );
    for row in PHONE_METHODS {
        if row.locked != Locked::Refused {
            assert_eq!(row.class, Class::Plain, "{}", row.name);
        }
        assert_eq!(locked(row.name), row.locked, "{}", row.name);
    }
}

/// The stored-data viewer is private (the unlock); deleting a turn and letting the AI see a saved identifier are
/// sensitive (the fingerprint).
// decision: PHN-402
// decision: PHN-466
// decision: PHN-467
#[test]
fn viewing_needs_the_unlock_and_deleting_or_unmasking_the_fingerprint() {
    for name in [
        "data.tables",
        "data.rows",
        "data.row",
        "data.cell",
        "data.search",
        "data.context",
    ] {
        assert_eq!(class(name), Class::Private, "{name}");
    }
    assert_eq!(class("data.delete"), Class::Sensitive);
    assert_eq!(class("secrets.unmask"), Class::Sensitive);
    assert_eq!(class("secrets.mask"), Class::Sensitive);
    assert_eq!(class("projects.list"), Class::Private);
    assert_eq!(class("projects.remove"), Class::Sensitive);
    // the owner answered both (PHN-467): no row waits for his word
    assert!(names(|row| row.owner_to_confirm).is_empty());
    assert_eq!(class("setup.uninstall_browser_files"), Class::Never);
    assert_eq!(class("device.report"), Class::Never);
}

/// The live preview, the saved-secrets list and settings changes from the phone; the preview behind the unlock.
// decision: PHN-404
// decision: PHN-454
#[test]
fn preview_saved_secrets_and_settings_are_the_phone_s() {
    assert_eq!(class("tasks.preview"), Class::Private);
    assert_eq!(locked("tasks.preview"), Locked::Refused);
    assert_eq!(class("secrets.saved"), Class::Private);
    assert_eq!(class("settings.get"), Class::Private);
    assert_eq!(class("settings.set"), Class::Sensitive);
}

/// The phone's chat, fetched like from a server: its reads show the conversation, its mark shows nothing.
// decision: ENG-14
#[test]
fn the_conversation_s_reads_are_private_and_its_mark_plain() {
    for name in [
        "chat.entries.changes",
        "chat.entries.days",
        "chat.entries.get",
        "chat.entries.page",
        "chat.entries.watch",
    ] {
        assert_eq!(class(name), Class::Private, "{name}");
    }
    assert_eq!(class("chat.entries.mark"), Class::Plain);
    // the owner's words and answers: the phone is his surface; a path on the Mac or the test tag never ride along
    for name in ["chat.send", "chat.approve", "chat.answer", "chat.correct"] {
        assert!(allowed(Role::Phone, name), "{name}");
    }
    assert_eq!(
        phone_method("chat.send").unwrap().refused_params,
        &["input_paths", "test_run"]
    );
}

/// The link's own state (its identity key, the device list, the relay and its limits) is the Mac owner's side: no
/// phone method writes it but the removal PHN-419 gives a second phone (sensitive: its fingerprint), and the list it
/// needs for that (private). The helper's own calls, a pairing's Yes, renaming and the audit are never a phone's.
// decision: PHN-106
// decision: PHN-419
#[test]
fn no_phone_method_reaches_the_link_s_own_state() {
    for row in PHONE_METHODS {
        if row.name.starts_with("link.") {
            assert_eq!(row.class, Class::Never, "{}", row.name);
        }
    }
    for name in [
        "phone.setup",
        "link.limits",
        "link.devices",
        "link.pair_request",
        "phone.rename",
        "phone.pair_decide",
        "phone.audit",
    ] {
        assert!(!allowed(Role::Phone, name), "{name}");
    }
    assert_eq!(class("phone.list"), Class::Private);
    assert_eq!(class("phone.remove"), Class::Sensitive);
    for name in PHONE_SESSION {
        assert_eq!(class(name), Class::Plain, "{name}");
        assert_eq!(locked(name), Locked::Allowed, "{name}");
    }
}

/// `Role::Phone` comes only from the phone link: a local hello can't name it, by the parser or on the real socket,
/// and the local roles' tables are untouched by it.
// decision: GATE-612
// decision: PHN-112
#[tokio::test]
async fn the_local_socket_never_gives_the_phone_role() {
    assert!(!Role::ALL.contains(&Role::Phone));
    assert_eq!(Role::from_wire("phone"), None);
    assert_eq!(Role::Phone.as_str(), "phone");
    let mut params = wire::Params::new();
    params.insert("role".into(), json!("phone"));
    assert_eq!(
        wire::parse_hello(&params).err(),
        Some("Input should be 'cli', 'mcp_shim' or 'native_host'")
    );
    for m in METHODS {
        assert!(!m.roles.contains(&Role::Phone), "{}", m.name);
    }

    let dir = tempfile::tempdir().unwrap();
    let (server, _) = common::new_server(dir.path());
    server.start().await.unwrap();
    let mut raw = common::Raw::connect(server.path()).await;
    let refused = raw
        .call("auth.hello", json!({"role": "phone"}))
        .await
        .unwrap_err();
    assert_eq!(
        refused,
        (
            wire::INVALID_REQUEST,
            "Input should be 'cli', 'mcp_shim' or 'native_host'".to_owned()
        )
    );
    drop(raw);
    server.close().await;
}

/// The table as data: the file the link crate and the phone apps compare against.
// decision: PHN-112
#[test]
fn the_classes_file_is_the_table() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(GOLDEN);
    let mut table = ferb_rpc::classes::as_json();
    table.as_object_mut().unwrap().insert(
        "_note".into(),
        json!(
            "Generated by engine/crates/ferb-rpc/tests/phone_classes.rs (UPDATE_GOLDEN=1) from \
             engine/crates/ferb-rpc/src/classes.rs, the one authority. Not Python-recorded."
        ),
    );
    let text = format!("{}\n", serde_json::to_string_pretty(&table).unwrap());
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
    }
    let kept =
        std::fs::read_to_string(&path).expect("the classes file (UPDATE_GOLDEN=1 writes it)");
    assert!(
        kept == text,
        "the classes differ from {GOLDEN}; UPDATE_GOLDEN=1 writes them again, then review the diff"
    );
    let kept: Value = serde_json::from_str(&kept).unwrap();
    let total: u64 = Class::ALL
        .iter()
        .map(|c| kept["counts"][c.as_str()].as_u64().unwrap())
        .sum();
    assert_eq!(total as usize, METHODS.len());
}
