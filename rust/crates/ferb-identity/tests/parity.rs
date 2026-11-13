//! The identity port against the Python code (decision: ENG-3): `rust/fixtures/identity/make.py`
//! ran `ferb/identity` on invented data; these tests make the same calls and must produce the
//! same answers and refusals.
//!
//! Nothing here touches the login Keychain, a helper or `~/.ferb`. What goes through the signed
//! helper (its invocations, the owner-secrets flows, the configured secrets, `secrets.list`) is
//! compared in `ferb-platform-macos/tests/identity.rs`, with the Mac's real launcher.

mod common;

use std::{
    collections::BTreeSet,
    fs,
    process::Command,
    sync::{Arc, Mutex},
};

use common::{cases, fixtures, same, strings, unb64, unb64_text, Recorded};
use ferb_identity::{
    mailbox::{parse_feed, FeedError, GmailFeedMailbox, FEED_URL},
    origin::{normalize_origin, NFKC_DELIMITERS},
    owner_secrets::{
        self, card_mismatch, login_place, same_site, site_host, typed_site, OwnerSecrets,
    },
    py, seal, secret_tools, secrets, seed, Secret,
};
use serde_json::{json, Value};
use tempfile::TempDir;

// ---------- Python's own behavior ----------

#[test]
fn json_is_written_as_python_writes_it() {
    for case in cases()["pyjson"].as_array().unwrap() {
        assert_eq!(py::dumps(&case["value"]), unb64_text(&case["dumps"]));
    }
}

#[test]
fn the_netloc_table_is_pythons() {
    let want: Vec<u32> = cases()["nfkc_delimiters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_u64().unwrap() as u32)
        .collect();
    let got: Vec<u32> = NFKC_DELIMITERS.iter().map(|c| *c as u32).collect();
    assert_eq!(
        got, want,
        "regenerate origin.rs's NFKC_DELIMITERS from make.py"
    );
}

#[test]
fn constants_are_pythons() {
    let want = &cases()["constants"];
    assert_eq!(owner_secrets::SERVICE.as_str(), want["owner_service"]);
    assert_eq!(secrets::SERVICE.as_str(), want["ferb_service"]);
    assert_eq!(owner_secrets::VERSION, want["version"]);
    assert_eq!(owner_secrets::WHOLE_BROWSER, want["whole_browser"]);
    let mut kinds = owner_secrets::CARD_KINDS.to_vec();
    kinds.sort();
    assert_eq!(json!(kinds), want["card_kinds"]);
    assert_eq!(std::str::from_utf8(seal::INFO).unwrap(), want["seal_info"]);
    assert_eq!(FEED_URL, want["feed_url"]);
    assert_eq!(secret_tools::NAME, want["tool_name"]);
    assert_eq!(secret_tools::DESCRIPTION, want["tool_description"]);
}

// ---------- origins and places ----------

#[test]
fn origins_and_login_places_read_as_python_reads_them() {
    let places = &cases()["places"];
    for case in places["origins"].as_array().unwrap() {
        let text = case["in"].as_str().unwrap();
        assert_eq!(
            normalize_origin(text)
                .ok()
                .map(Value::from)
                .unwrap_or(Value::Null),
            case["normalize_origin"],
            "{text:?}"
        );
        assert_eq!(login_place(text), case["login_place"], "{text:?}");
    }
    for case in places["site_host"].as_array().unwrap() {
        assert_eq!(
            site_host(&Recorded, case["in"].as_str().unwrap()),
            case["out"],
            "{case}"
        );
    }
    for case in places["same_site"].as_array().unwrap() {
        assert_eq!(
            same_site(case["a"].as_str().unwrap(), case["b"].as_str().unwrap()),
            case["out"],
            "{case}"
        );
    }
    for case in places["card_mismatch"].as_array().unwrap() {
        let got = card_mismatch(case["saved"].as_str(), case["field"].as_str());
        assert_eq!(
            got.map(Value::from).unwrap_or(Value::Null),
            case["out"],
            "{case}"
        );
    }
    for case in places["typed_site"].as_array().unwrap() {
        let words: BTreeSet<String> = strings(&case["tokens"])
            .into_iter()
            .chain(strings(&case["hosts"]))
            .collect();
        assert_eq!(
            typed_site(case["host"].as_str().unwrap(), &|text| words.contains(text)),
            case["out"],
            "{case}"
        );
    }
}

// ---------- owner secrets ----------

/// Two stores on one home, as the daemon's worker threads make them: both names are kept.
#[test]
fn concurrent_saves_keep_both_names() {
    struct Memory(Mutex<Vec<String>>);
    impl ferb_identity::Keychain for Memory {
        fn store(&self, account: &str, _: &Secret) -> ferb_identity::Result<()> {
            self.0.lock().unwrap().push(account.to_owned());
            Ok(())
        }
        fn lookup(&self, _: &str) -> ferb_identity::Result<Option<Secret>> {
            Ok(None)
        }
        fn delete(&self, _: &str) -> ferb_identity::Result<bool> {
            Ok(false)
        }
    }
    let home = TempDir::new().unwrap();
    let memory = Arc::new(Memory(Mutex::new(Vec::new())));
    std::thread::scope(|scope| {
        for i in 0..8 {
            let (memory, path) = (Arc::clone(&memory), home.path());
            scope.spawn(move || {
                OwnerSecrets::with_keychain(path, memory, Arc::new(Recorded))
                    .remember(
                        &i.to_string(),
                        "https://x.example",
                        &Secret::from("invented"),
                        &[],
                        None,
                    )
                    .unwrap();
            });
        }
    });
    let records = OwnerSecrets::with_keychain(home.path(), memory, Arc::new(Recorded))
        .records()
        .unwrap();
    assert_eq!(
        records["https://x.example"]["entries"]
            .as_object()
            .unwrap()
            .len(),
        8
    );
}

// ---------- seals ----------

#[test]
fn seals_match_python() {
    let fixture = &cases()["seals"];
    for case in fixture["seal_key"].as_array().unwrap() {
        assert_eq!(
            seal::seal_key(&unb64(&case["pairing"])).to_vec(),
            unb64(&case["key"])
        );
    }
    let key = unb64(&fixture["key"]);
    for case in fixture["sealed"].as_array().unwrap() {
        let (command, origin) = (
            case["command"].as_str().unwrap(),
            case["origin"].as_str().unwrap(),
        );
        assert_eq!(seal::aad(command, origin), unb64(&case["aad"]));
        let python = seal::Sealed {
            nonce: case["nonce"].as_str().unwrap().into(),
            ct: case["ct"].as_str().unwrap().into(),
        };
        let value = unb64_text(&case["value"]);
        assert_eq!(
            seal::unseal(&key, &python, command, origin)
                .unwrap()
                .expose(),
            value
        );
        let ours = seal::seal(&key, &Secret::new(value.clone()), command, origin).unwrap();
        assert_ne!(ours.nonce, python.nonce, "a fresh nonce for every seal");
        assert_eq!(
            seal::unseal(&key, &ours, command, origin).unwrap().expose(),
            value
        );
        // bound to its command and its origin, and to its key
        assert_eq!(
            seal::unseal(&key, &ours, "another", origin).unwrap_err(),
            seal::SealError::Seal
        );
        assert_eq!(
            seal::unseal(&key, &ours, command, "https://attacker.example").unwrap_err(),
            seal::SealError::Seal
        );
        assert_eq!(
            seal::unseal(&[7u8; 32], &ours, command, origin).unwrap_err(),
            seal::SealError::Seal
        );
        assert_eq!(
            seal::unseal(&key[..16], &ours, command, origin).unwrap_err(),
            seal::SealError::Key
        );
    }
}

/// Needs `uv` and the Python build: a seal made here opens in Python's `seal.unseal`.
#[test]
#[ignore]
fn a_rust_seal_opens_in_python() {
    let key = seal::seal_key(b"invented pairing key");
    let sealed = seal::seal(
        &key[..],
        &Secret::from("invented fill \u{e9}"),
        "cmd-9",
        "https://login.example",
    )
    .unwrap();
    let repo = fixtures().join("../../..");
    let out = Command::new("uv")
        .args([
            "run",
            "python",
            "rust/fixtures/identity/make.py",
            "unseal",
            &py::b64encode(&key[..]),
            &sealed.nonce,
            &sealed.ct,
            "cmd-9",
            "https://login.example",
        ])
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        py::b64decode(String::from_utf8(out.stdout).unwrap().trim()).unwrap(),
        "invented fill \u{e9}".as_bytes()
    );
}

// ---------- Chrome profiles ----------

fn profile_json(p: &seed::ChromeProfile) -> Value {
    json!([p.folder, p.name, p.accounts])
}

#[test]
fn chrome_profiles_match_python() {
    let fixture = &cases()["seed"];
    let data = TempDir::new().unwrap();
    fs::write(
        data.path().join("Local State"),
        fixture["state"].as_str().unwrap(),
    )
    .unwrap();
    for (folder, text) in fixture["preferences"].as_object().unwrap() {
        fs::create_dir(data.path().join(folder)).unwrap();
        fs::write(
            data.path().join(folder).join("Preferences"),
            text.as_str().unwrap(),
        )
        .unwrap();
    }
    let found = seed::profiles(data.path()).unwrap();
    same(
        &json!(found.iter().map(profile_json).collect::<Vec<_>>()),
        &fixture["profiles"],
        "profiles",
    );
    for op in fixture["ops"].as_array().unwrap() {
        let (address, name) = (op["address"].as_str().unwrap(), op["name"].as_str());
        let got = match op["op"].as_str().unwrap() {
            "choose" => seed::choose(&found, address, name),
            _ => seed::verify(address, name.unwrap(), data.path()),
        };
        let got = match got {
            Ok(p) => json!({"ok": profile_json(&p)}),
            Err(e) => json!({"seed_error": e.to_string()}),
        };
        same(&got, &op["result"], &op.to_string());
    }
    for case in fixture["broken"].as_array().unwrap() {
        let data = TempDir::new().unwrap();
        if !case["state"].is_null() {
            fs::write(data.path().join("Local State"), unb64(&case["state"])).unwrap();
        }
        let got = match seed::profiles(data.path()) {
            Ok(found) => json!({"ok": found.iter().map(profile_json).collect::<Vec<_>>()}),
            Err(e) => json!({"seed_error": e.to_string()}),
        };
        same(&got, &case["result"], &case.to_string());
    }
}

// ---------- the unread-mail inventory ----------

#[test]
fn the_mail_inventory_matches_python() {
    for (n, case) in cases()["mailbox"].as_array().unwrap().iter().enumerate() {
        let feed = unb64_text(&case["feed"]);
        let got = match parse_feed(&feed) {
            Ok((title, messages)) => {
                assert!(
                    !format!("{messages:?}").contains("482913"),
                    "a message prints its subject"
                );
                let messages: Vec<Value> = messages
                    .iter()
                    .map(|m| json!({"id": m.id, "sender": m.sender, "subject": py::b64encode(m.subject.expose().as_bytes()),
                                    "summary": py::b64encode(m.summary.expose().as_bytes()), "issued": m.issued}))
                    .collect();
                json!({"title": title, "messages": messages})
            }
            Err(FeedError::Value) => json!({"error": "ValueError"}),
            Err(FeedError::Parse) => json!({"error": "ParseError"}),
        };
        same(&got, &case["parsed"], &format!("feed {n}"));

        let mut mailbox = GmailFeedMailbox::new("FERB@example.com", FEED_URL);
        assert_eq!(mailbox.signed_in(&feed), case["signed_in"], "feed {n}");
        for (round, key) in ["candidates", "candidates_after_m1_used"]
            .into_iter()
            .enumerate()
        {
            if round == 1 {
                mailbox.used.insert("tag:m1".into());
            }
            let mut shown = mailbox.candidates(&feed);
            let text = shown.to_string();
            let mut tokens = Vec::new();
            for (i, candidate) in shown["candidates"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .enumerate()
            {
                let token = candidate["candidate"].as_str().unwrap().to_owned();
                assert_eq!(token.len(), 24, "secrets.token_urlsafe(18)");
                candidate["candidate"] = json!(format!("T{i}"));
                tokens.push(token);
            }
            same(&shown, &case[key]["shown"], &format!("feed {n}, {key}"));
            let codes = case[key]["codes"].as_array().unwrap();
            assert_eq!(
                tokens.iter().collect::<BTreeSet<_>>().len(),
                codes.len(),
                "feed {n}: one token per code"
            );
            for (token, code) in tokens.iter().zip(codes) {
                let code = unb64_text(code);
                assert!(
                    !text.contains(&code) || code.len() == 4 && text.contains("2026"),
                    "feed {n}: the inventory shows a code"
                );
                let taken = mailbox.take(token).unwrap();
                assert_eq!(taken.code.expose(), code, "feed {n}");
                assert!(!format!("{taken:?}").contains(&code));
                assert!(mailbox.take(token).is_none(), "a token is taken once");
            }
            assert!(mailbox.take("another inventory's token").is_none());
        }
    }
}

/// The candidates' tokens come from the random source given (a replayed run's), and a source that
/// gives nothing makes the inventory unavailable, as Python's `token_urlsafe` raising would.
#[test]
fn candidate_tokens_come_from_the_injected_random_source() {
    use ferb_identity::mailbox::TokenRandom;
    struct Fixed(std::sync::Mutex<u8>, bool);
    impl TokenRandom for Fixed {
        fn fill(&self, buf: &mut [u8]) -> bool {
            let mut n = self.0.lock().unwrap();
            *n += 1;
            buf.fill(*n);
            self.1
        }
    }
    let feed = "<feed xmlns=\"http://purl.org/atom/ns#\" version=\"0.3\"><title>Gmail - Inbox for ferb@example.test</title>\
<entry><title>Code</title><summary>Your code is 482913 or 1234</summary><id>tag:m1</id>\
<issued>2026-01-01T09:00:00Z</issued><author><name>A</name><email>a@example.test</email></author></entry></feed>";
    let mut mailbox = GmailFeedMailbox::with_random(
        "ferb@example.test",
        FEED_URL,
        std::sync::Arc::new(Fixed(std::sync::Mutex::new(0), true)),
    );
    let shown = mailbox.candidates(feed);
    let tokens: Vec<&str> = shown["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["candidate"].as_str().unwrap())
        .collect();
    assert_eq!(
        tokens,
        [
            ferb_identity::py::b64encode_urlsafe_nopad(&[1; 18]),
            ferb_identity::py::b64encode_urlsafe_nopad(&[2; 18]),
        ]
    );
    assert_eq!(mailbox.take(tokens[0]).unwrap().code.expose(), "482913");
    let mut broken = GmailFeedMailbox::with_random(
        "ferb@example.test",
        FEED_URL,
        std::sync::Arc::new(Fixed(std::sync::Mutex::new(0), false)),
    );
    assert_eq!(
        broken.candidates(feed),
        json!({"available": false, "reason": "feed_unavailable", "candidates": []})
    );
}
