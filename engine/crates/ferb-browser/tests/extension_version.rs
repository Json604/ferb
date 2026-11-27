//! The extension in the repository (`extension`) is one this engine accepts: its manifest's version is at
//! least every minimum the engine names, it speaks the engine's protocol, and its key gives the extension id the
//! engine trusts. Read from the live files, not a fixture: this is what tests/test_release_checks.py checked against
//! the Python constants before the Python code was removed (ENG-16).

mod common;

use std::path::PathBuf;

use ferb_browser::protocol;
use serde_json::Value;
use sha2::{Digest, Sha256};

use common::b64d;

fn extension() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../extension")
}

fn manifest() -> Value {
    let path = extension().join("manifest.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn at_least(version: &str, minimum: &str) -> bool {
    protocol::version_ok(version, minimum).expect("decimal version parts")
}

// decision: REL-2
#[test]
fn the_extension_in_the_repo_is_one_this_ferb_accepts() {
    let m = manifest();
    let version = m["version"]
        .as_str()
        .expect("manifest.json names a version");
    for (name, minimum) in [
        ("MIN_EXTENSION_VERSION", protocol::MIN_EXTENSION_VERSION),
        ("PDF_EXTENSION_VERSION", protocol::PDF_EXTENSION_VERSION),
        ("HTML_EXTENSION_VERSION", protocol::HTML_EXTENSION_VERSION),
        (
            "OWNER_VIEW_EXTENSION_VERSION",
            protocol::OWNER_VIEW_EXTENSION_VERSION,
        ),
        (
            "UPLOAD_BYTES_EXTENSION_VERSION",
            protocol::UPLOAD_BYTES_EXTENSION_VERSION,
        ),
        ("INPUT_EXTENSION_VERSION", protocol::INPUT_EXTENSION_VERSION),
        (
            "OWNER_CLOSE_EXTENSION_VERSION",
            protocol::OWNER_CLOSE_EXTENSION_VERSION,
        ),
    ] {
        assert!(
            at_least(version, minimum),
            "extension/manifest.json is {version}, below {name} {minimum}"
        );
    }
    // the comparison is numeric per part, as the engine's own (0.19.10 is above 0.19.9)
    assert!(at_least("0.19.10", "0.19.9") && !at_least("0.18.99", "0.19.0"));
}

// decision: REL-2
#[test]
fn the_extension_speaks_the_engines_protocol() {
    let path = extension().join("protocol.js");
    let js = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let declared: Vec<i64> = js
        .lines()
        .filter_map(|l| l.trim().strip_prefix("const FERB_PROTOCOL = "))
        .map(|rest| rest.trim_end_matches(';').trim().parse().expect("a number"))
        .collect();
    assert_eq!(
        declared,
        vec![protocol::PROTOCOL_VERSION],
        "protocol.js FERB_PROTOCOL"
    );
}

// decision: REL-2
#[test]
fn the_manifests_key_gives_the_extension_id_the_engine_trusts() {
    // Chrome's id: the first 16 bytes of SHA-256 over the public key's DER, each hex digit 0-f written as a-p
    let key = manifest()["key"]
        .as_str()
        .expect("manifest.json carries the development key")
        .to_owned();
    let digest = Sha256::digest(b64d(&key));
    let id: String = digest[..16]
        .iter()
        .flat_map(|b| [b >> 4, b & 0xf])
        .map(|n| (b'a' + n) as char)
        .collect();
    assert_eq!(id, protocol::FERB_EXTENSION_ID);
}
