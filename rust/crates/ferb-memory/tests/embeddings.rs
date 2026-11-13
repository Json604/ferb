//! The tokenizer and the static embeddings against what Python produced (`rust/fixtures/memory/make.py`:
//! tokenizer.json, embeddings.json; ENG-3). Token ids, float64 bit patterns, int8 vectors, sign bits,
//! similarities and every refusal must be identical.

mod common;

use std::fs;

use common::*;
use ferb_memory::embeddings::{self, Embedder, StaticEmbedder};
use ferb_memory::tokenizer::WordPiece;
use ferb_memory::Error;
use serde_json::Value;

// decision: MEM-25
#[test]
fn the_tokenizer_gives_pythons_ids() {
    let fx = fixture("tokenizer.json");
    let mut total = 0;
    let mut differ = Vec::new();
    for config in fx["configs"].as_array().unwrap() {
        let w = WordPiece::from_spec(&config["spec"]).unwrap();
        for case in config["cases"].as_array().unwrap() {
            total += 1;
            let want: Vec<i64> = case["ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_i64().unwrap())
                .collect();
            let got = w.ids(s(&case["text"]));
            if got != want {
                differ.push(format!(
                    "{}: {:?}\n  norm {:?}\n  got {got:?}\n want {want:?}",
                    config["name"],
                    s(&case["text"]),
                    w.normalize(s(&case["text"]))
                ));
            }
        }
    }
    assert!(total > 5000, "{total} cases");
    assert!(
        differ.is_empty(),
        "{} of {total} differ:\n{}",
        differ.len(),
        differ[..differ.len().min(5)].join("\n")
    );
}

fn tiny() -> (tempfile::TempDir, StaticEmbedder, Value) {
    let fx = fixture("embeddings.json");
    let dir = tempfile::tempdir().unwrap();
    let root = write_model(&dir.path().join("tiny"), &fx["model"]["files"]);
    let e = StaticEmbedder::open(&root, &fx["model"]["manifest"]).unwrap();
    (dir, e, fx)
}

// decision: MEM-25
#[test]
fn a_text_is_pythons_normalized_mean_bit_for_bit() {
    let (_dir, e, fx) = tiny();
    assert_eq!(
        e.name(),
        format!("tiny@{}:12", &s(&fx["model"]["manifest"]["revision"])[..12])
    );
    let mut n = 0;
    for case in fx["cases"].as_array().unwrap() {
        let text = s(&case["text"]);
        let floats = e.floats(text);
        let want = case["floats"].as_array();
        match (floats, want) {
            (None, None) => assert!(case["floats"].is_null()),
            (Some(f), Some(w)) => {
                let got: Vec<String> = f.iter().map(|x| bits(*x)).collect();
                let want: Vec<String> = w.iter().map(|x| s(x).to_owned()).collect();
                assert_eq!(got, want, "floats of {text:?}");
            }
            (got, want) => panic!("{text:?}: {got:?} vs {want:?}"),
        }
        match e.encode(text) {
            None => assert!(case["int8"].is_null(), "{text:?}"),
            Some(v) => {
                assert_eq!(v, i8s(&case["int8"]), "int8 of {text:?}");
                assert_eq!(hex(&embeddings::sign_bits(&v)), s(&case["bits"]));
            }
        }
        assert_eq!(embeddings::text_hash(text), s(&case["hash"]));
        n += 1;
    }
    assert!(n > 500);
    for pair in fx["pairs"].as_array().unwrap() {
        let got = embeddings::similarity(&i8s(&pair["a"]), &i8s(&pair["b"]));
        assert_eq!(bits(got), s(&pair["similarity"]));
    }
}

// decision: MEM-25
#[test]
fn quantize_and_sign_bits_match() {
    let fx = fixture("embeddings.json");
    for case in fx["quantize"].as_array().unwrap() {
        let values: Vec<f64> = case["values"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| unbits(s(v)))
            .collect();
        match embeddings::quantize(&values) {
            Ok(v) => assert_eq!(v, i8s(&case["int8"]), "{values:?}"),
            Err(e) => {
                assert_eq!(e.kind, s(&case["raises"]), "{values:?}");
                assert_eq!(e.message, s(&case["message"]));
            }
        }
    }
    for case in fx["sign_bits"].as_array().unwrap() {
        assert_eq!(
            hex(&embeddings::sign_bits(&i8s(&case["vector"]))),
            s(&case["bits"])
        );
    }
}

fn message(e: &Error) -> String {
    match e {
        Error::Refused(m) => m.clone(),
        other => format!("{other:?}"),
    }
}

// decision: MEM-25, MEM-26
#[test]
fn the_manifest_and_loading_refuse_as_python_does() {
    let fx = fixture("embeddings.json");
    for case in fx["manifests"].as_array().unwrap() {
        let got = embeddings::manifest_ok(&case["manifest"]);
        if case["ok"] == Value::Bool(true) {
            assert!(got.is_ok(), "{:?}: {got:?}", case["manifest"]);
        } else {
            assert_eq!(
                message(&got.unwrap_err()),
                s(&case["message"]),
                "{}",
                case["manifest"]
            );
        }
    }
    assert_eq!(embeddings::model(), fx["pinned"]);
    embeddings::manifest_ok(&embeddings::model()).unwrap();
    assert_eq!(
        embeddings::default_root(std::path::Path::new("/FX/home")).to_string_lossy(),
        s(&fx["default_root"])
    );
    for case in fx["loads"].as_array().unwrap() {
        let dir = tempfile::tempdir().unwrap();
        let root = write_model(&dir.path().join("m"), &case["files"]);
        if let Some(link) = case["link"].as_str() {
            std::os::unix::fs::symlink(root.join("config.json"), root.join(link)).unwrap();
        }
        let got = StaticEmbedder::open(&root, &case["manifest"]);
        if case["ok"] == Value::Bool(true) {
            assert!(got.is_ok(), "{}: {:?}", case["name"], got.err());
        } else {
            assert_eq!(
                message(&got.err().unwrap()),
                s(&case["message"]),
                "{}",
                case["name"]
            );
        }
        drop(fs::remove_dir_all(dir.path()));
    }
}

/// The real potion-base-8M files from a local build folder (`rust/fixtures/memory/real_model.json` holds what
/// Python made of invented texts with them). Run with `-- --ignored` when the build cache has the files.
// decision: MEM-25
#[test]
#[ignore]
fn the_real_model_gives_pythons_ids_and_vectors() {
    let fx = fixture("real_model.json");
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let root = std::env::var_os("FERB_EMBEDDING_ROOT")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            fs::read_dir(repo.join("build"))
                .ok()?
                .flatten()
                .find_map(|d| {
                    let p = d
                        .path()
                        .join(".work/models/embeddings")
                        .join(s(&fx["model"]));
                    p.is_dir().then_some(p)
                })
        })
        .expect("the real model files (set FERB_EMBEDDING_ROOT)");
    let e = StaticEmbedder::open(&root, &embeddings::model()).unwrap();
    for case in fx["cases"].as_array().unwrap() {
        let text = s(&case["text"]);
        let want: Vec<i64> = case["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_i64().unwrap())
            .collect();
        assert_eq!(
            e.tokenizer
                .ids(ferb_memory::pystr::prefix(text, embeddings::EMBED_CHARS)),
            want,
            "{text:?}"
        );
        let floats = e.floats(text);
        match &case["floats_sha256"] {
            Value::Null => assert!(floats.is_none()),
            Value::String(h) => {
                let bytes: Vec<u8> = floats
                    .unwrap()
                    .iter()
                    .flat_map(|x| x.to_le_bytes())
                    .collect();
                use sha2::Digest;
                assert_eq!(&hex(&sha2::Sha256::digest(&bytes)), h, "{text:?}");
            }
            _ => unreachable!(),
        }
        match e.encode(text) {
            None => assert!(case["int8"].is_null()),
            Some(v) => assert_eq!(v, i8s(&case["int8"])),
        }
    }
}
