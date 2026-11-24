//! PHN-828 (owner, 2026-11-24): a phone pairs only with a Mac signed in to the same FERB account, checked by the
//! daemon from the site's signed statement before the owner is asked; and FERB.app's sign-in itself (APP-1138,
//! PHN-830): `account.get`, `account.save`, `account.clear` through a fake Keychain behind FERB's helper seam, the
//! tokens masked and never journaled, and an account change removing the phones of another (or no) account.
//!
//! The statements are signed with the site's test vector (site/tests/fixtures/assertion-vector.json): an invented key,
//! never FERB's. The vector's own assertion is checked byte for byte.

mod phone_support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ferb_daemon::account::{Account, Seams as AccountSeams, KEYCHAIN_ACCOUNT};
use ferb_daemon::areas_b::runtimes::keys::{HelperError, KeyFault, KeyHelper};
use ferb_daemon::phone::account::{AccountCheck, AccountRefused, MacAccount, MacUnreadable};
use ferb_identity::Secret;
use ferb_rpc::phone_gate as code;
use ferb_sanitize::redact::Redactor;
use p256::ecdsa::SigningKey;
use phone_support::*;
use serde_json::{json, Value};

const MISMATCH_WORDS: &str =
    "This phone is signed in as bob@example.com, but FERB on this Mac is signed in as \
                              ada@example.com. Sign in with the same account on both.";
const SIGNED_OUT_WORDS: &str =
    "FERB on this Mac isn't signed in yet. Sign in on the Mac, then scan a new code.";
const UNREADABLE_WORDS: &str =
    "FERB on this Mac couldn't read its sign-in just now. Open FERB on the Mac, then \
                                scan a new code.";
const UNCONFIRMED_WORDS: &str =
    "FERB couldn't confirm which account this phone is signed in to. Check that the phone \
                                 is online, then scan a new code on the Mac.";

fn asked(w: &World) -> usize {
    w.published
        .lock()
        .unwrap()
        .iter()
        .filter(|(k, _)| k == "phone.pair_requested")
        .count()
}

async fn request(w: &World, endpoint: &str, account: Option<String>) -> Reply {
    let mut link = Conn::link(w).await;
    let mut params =
        json!({"endpoint": endpoint, "name": "Pixel", "app": "FERB Android", "code": "123 456"});
    if let Some(a) = account {
        params["account"] = json!(a);
    }
    link.call("link.pair_request", params, None).await
}

/// The site's own vector verifies here, byte for byte, and says whose account it is; a second later than its life it
/// doesn't.
// decision: PHN-828
#[test]
fn the_site_s_vector_verifies() {
    let v = site_vector();
    let check = AccountCheck::new(
        &[(
            v["kid"].as_str().unwrap(),
            v["test_public_key_sec1_uncompressed_b64"].as_str().unwrap(),
        )],
        Arc::new(|| Ok(None)),
    );
    let (assertion, dev, now) = (
        v["assertion"].as_str().unwrap(),
        v["payload"]["dev"].as_str().unwrap(),
        v["now"].as_i64().unwrap(),
    );
    assert_eq!(
        assertion,
        format!(
            "fpa1.{}.{}",
            v["payload_b64url"].as_str().unwrap(),
            v["signature_b64url"].as_str().unwrap()
        )
    );
    let ok = check.verify(Some(assertion), dev, now).unwrap();
    assert_eq!(ok.account_id, v["payload"]["sub"].as_str().unwrap());
    assert_eq!(ok.email, v["payload"]["email"].as_str().unwrap());
    // the SubjectPublicKeyInfo form of the same key is the same key
    let spki = AccountCheck::new(
        &[(
            v["kid"].as_str().unwrap(),
            v["test_public_key_spki_der_b64"].as_str().unwrap(),
        )],
        Arc::new(|| Ok(None)),
    );
    assert!(spki.verify(Some(assertion), dev, now).is_ok());
    let exp = v["payload"]["exp"].as_i64().unwrap();
    assert_eq!(check.verify(Some(assertion), dev, exp), Err("expired"));
    assert_eq!(
        check.verify(Some(assertion), "another-phone", now),
        Err("other_phone")
    );
    // the key pinned under another kid is not this one's
    let other_kid = AccountCheck::new(
        &[(
            "other",
            v["test_public_key_sec1_uncompressed_b64"].as_str().unwrap(),
        )],
        Arc::new(|| Ok(None)),
    );
    assert_eq!(other_kid.verify(Some(assertion), dev, now), Err("kid"));
}

/// A build with no key pinned can't confirm anyone's account: every pairing is refused, whatever the phone sends.
// decision: PHN-828
#[test]
fn no_pinned_key_confirms_nothing() {
    let v = site_vector();
    let none = AccountCheck::new(&[], Arc::new(|| Ok(Some(mac_account()))));
    assert!(!none.can_confirm());
    assert_eq!(
        none.check(
            v["assertion"].as_str(),
            v["payload"]["dev"].as_str().unwrap(),
            v["now"].as_i64().unwrap(),
            Some(&mac_account())
        ),
        Err(AccountRefused::Unconfirmed("no_key"))
    );
}

/// The phone's statement names the Mac's own account: the owner is asked, and the phone pairs on his Yes with the
/// account kept on its row.
// decision: PHN-828
// decision: PHN-102
#[tokio::test]
async fn a_phone_of_the_mac_s_account_is_asked_about_and_pairs() {
    let w = world(None).await;
    pair(&w, "e1", &Keys::new(1), false, false).await;
    assert_eq!(asked(&w), 1);
    let row = {
        let db = w.db.lock().unwrap();
        ferb_store::phones::by_endpoint(db.conn(), "e1")
            .unwrap()
            .unwrap()
    };
    assert_eq!(row.account_id.as_deref(), Some(ACCOUNT_ID));
}

/// Another account: refused with words naming both emails, before the owner is asked; nothing pairs.
// decision: PHN-828
#[tokio::test]
async fn a_phone_of_another_account_is_refused_before_the_owner_is_asked() {
    let w = world(None).await;
    let mut claims = claims(&w, "e1");
    claims["sub"] = json!("11111111-2222-4333-8444-555555555555");
    claims["email"] = json!("bob@example.com");
    let (n, words) = request(&w, "e1", Some(assertion_of(&site_key(), &claims)))
        .await
        .unwrap_err();
    assert_eq!(n, code::ACCOUNT_MISMATCH);
    assert_eq!(words, MISMATCH_WORDS);
    assert_eq!(asked(&w), 0, "the owner is never asked");
    assert!(w.phones.list(&app_session()).unwrap()["pairing"].is_null());
    // the journal and the audit keep the refusal's name, never an email
    let journal = format!("{:?}", w.journal.lock().unwrap());
    assert!(journal.contains("phone.pair_refused") && journal.contains("account_mismatch"));
    assert!(!journal.contains("example.com"), "{journal}");
    let audit = audit_text(&w);
    assert!(
        audit.contains("link.pair_request link account_mismatch"),
        "{audit}"
    );
    assert!(!audit.contains("example.com"));
}

/// Expired, forged, under an unknown key, for another phone, for another audience, too long-lived, or none at all:
/// "couldn't confirm", and the owner is never asked.
// decision: PHN-828
#[tokio::test]
async fn a_statement_that_can_t_be_confirmed_is_refused() {
    let w = world(None).await;
    let good = claims(&w, "e1");
    let now = good["iat"].as_i64().unwrap();
    let with = |k: &str, v: Value| {
        let mut c = good.clone();
        c[k] = v;
        assertion_of(&site_key(), &c)
    };
    let forged = {
        let mut a = assertion(&w, "e1");
        // the same payload, signed by another key
        let other = SigningKey::from_slice(&[9u8; 32]).unwrap();
        let theirs = assertion_of(&other, &good);
        a.truncate(a.rfind('.').unwrap());
        format!("{a}{}", &theirs[theirs.rfind('.').unwrap()..])
    };
    let changed_payload = {
        // a payload changed after signing (another account) under the original signature
        let a = assertion(&w, "e1");
        let sig = &a[a.rfind('.').unwrap()..];
        let mut c = good.clone();
        c["sub"] = json!("someone-else");
        let b = assertion_of(&site_key(), &c);
        format!("{}{sig}", &b[..b.rfind('.').unwrap()])
    };
    let cases: Vec<(&str, Option<String>)> = vec![
        ("missing", None),
        ("empty", Some(String::new())),
        ("not one", Some("fpa1.junk".into())),
        ("expired", Some(with("exp", json!(now - 1)))),
        ("from the future", Some(with("iat", json!(now + 120)))),
        (
            "lives too long",
            Some({
                let mut c = good.clone();
                c["iat"] = json!(now - 10);
                c["exp"] = json!(now + 900);
                assertion_of(&site_key(), &c)
            }),
        ),
        ("forged", Some(forged)),
        ("changed after signing", Some(changed_payload)),
        ("unknown kid", Some(with("kid", json!("someone-elses-key")))),
        ("another phone", Some(with("dev", json!("e2")))),
        ("another audience", Some(with("aud", json!("ferb-login")))),
        ("another version", Some(with("v", json!(2)))),
        ("no account", Some(with("sub", json!("")))),
        (
            "another format",
            Some(assertion(&w, "e1").replacen("fpa1.", "fpa2.", 1)),
        ),
    ];
    for (what, statement) in cases {
        let (n, words) = request(&w, "e1", statement).await.unwrap_err();
        assert_eq!(n, code::ACCOUNT_UNCONFIRMED, "{what}");
        assert_eq!(words, UNCONFIRMED_WORDS, "{what}");
    }
    assert_eq!(asked(&w), 0, "the owner is never asked");
    // and the good one still pairs
    request(&w, "e1", Some(assertion(&w, "e1"))).await.unwrap();
    assert_eq!(asked(&w), 1);
}

/// FERB on the Mac isn't signed in: refused with its own words, the owner never asked.
// decision: PHN-828
#[tokio::test]
async fn a_signed_out_mac_pairs_no_phone() {
    let w = world(None).await;
    *w.mac.lock().unwrap() = None;
    let (n, words) = request(&w, "e1", Some(assertion(&w, "e1")))
        .await
        .unwrap_err();
    assert_eq!(n, code::MAC_SIGNED_OUT);
    assert_eq!(words, SIGNED_OUT_WORDS);
    assert_eq!(asked(&w), 0);
}

/// A Mac whose sign-in can't be read now (the Keychain failing) is never called signed out: its own refusal and
/// words, nothing changed, the owner never asked; pairing and binding alike.
// decision: REL-453, PHN-844
#[tokio::test]
async fn a_mac_whose_sign_in_can_t_be_read_says_so_not_signed_out() {
    use std::sync::atomic::Ordering;
    let w = world(None).await;
    w.unreadable.store(true, Ordering::SeqCst);
    let (n, words) = request(&w, "e1", Some(assertion(&w, "e1")))
        .await
        .unwrap_err();
    assert_eq!(
        (n, words.as_str()),
        (code::ACCOUNT_UNCONFIRMED, UNREADABLE_WORDS)
    );
    assert_eq!(asked(&w), 0);
    let journal = format!("{:?}", w.journal.lock().unwrap());
    assert!(journal.contains("mac_account_unreadable"), "{journal}");
    assert!(audit_text(&w).contains("link.pair_request link mac_account_unreadable"));
    // read again: it pairs as usual
    w.unreadable.store(false, Ordering::SeqCst);
    assert!(request(&w, "e2", Some(assertion(&w, "e2"))).await.is_ok());
    // a build with no key pinned still says it can't confirm anyone
    let none = AccountCheck::new(&[], Arc::new(|| Err(MacUnreadable)));
    assert_eq!(
        none.check_read(None, "e1", 0, &Err(MacUnreadable)),
        Err(AccountRefused::Unconfirmed("no_key"))
    );
}

/// A Hide My Email address in a mismatch is named as its own account, with what to do.
// decision: REL-453, PHN-844
#[tokio::test]
async fn a_mismatch_with_a_hide_my_email_address_says_it_is_a_separate_account() {
    let w = world(None).await;
    let mut claims = claims(&w, "e1");
    claims["sub"] = json!("11111111-2222-4333-8444-555555555555");
    claims["email"] = json!("x7k2p9@privaterelay.appleid.com");
    let (n, words) = request(&w, "e1", Some(assertion_of(&site_key(), &claims)))
        .await
        .unwrap_err();
    assert_eq!(n, code::ACCOUNT_MISMATCH);
    assert_eq!(
        words,
        "This phone is signed in as x7k2p9@privaterelay.appleid.com, but FERB on this Mac is signed in as \
         ada@example.com. Sign in with the same account on both. x7k2p9@privaterelay.appleid.com is a Hide My Email \
         address from Apple, so it's a separate account. To use it on both, sign in with Apple on both, with the same \
         Apple ID."
    );
}

// ---- FERB.app's sign-in -----------------------------------------------------------------------------------------

/// The Keychain behind FERB's signed helper, as a map; every call recorded by account name.
#[derive(Default)]
struct FakeKeychain {
    items: Mutex<HashMap<String, String>>,
    calls: Mutex<Vec<String>>,
}

impl KeyHelper for FakeKeychain {
    fn store(&self, account: &str, value: &Secret) -> Result<(), HelperError> {
        self.calls.lock().unwrap().push(format!("store {account}"));
        self.items
            .lock()
            .unwrap()
            .insert(account.into(), value.expose().to_string());
        Ok(())
    }
    fn lookup(&self, account: &str) -> Result<Option<Secret>, HelperError> {
        self.calls.lock().unwrap().push(format!("lookup {account}"));
        Ok(self
            .items
            .lock()
            .unwrap()
            .get(account)
            .map(|v| Secret::new(v.clone())))
    }
    fn delete(&self, account: &str) -> Result<bool, HelperError> {
        self.calls.lock().unwrap().push(format!("delete {account}"));
        Ok(self.items.lock().unwrap().remove(account).is_some())
    }
}

struct Area {
    account: Account,
    keychain: Arc<FakeKeychain>,
    redactor: Arc<Redactor>,
    journal: Arc<Mutex<Vec<(String, Value)>>>,
}

fn area(keychain: Arc<FakeKeychain>) -> Area {
    let redactor = Arc::new(Redactor::new(&[], &[]));
    let journal = Arc::new(Mutex::new(Vec::new()));
    let account = Account::new(AccountSeams {
        helper: keychain.clone(),
        redactor: redactor.clone(),
        journal: {
            let j = journal.clone();
            Arc::new(move |k: &str, v: Value| j.lock().unwrap().push((k.into(), v)))
        },
    });
    Area {
        account,
        keychain,
        redactor,
        journal,
    }
}

const ACCESS: &str = "fa1_AccessTokenInventedForThisTestOnly_0123456789abcd";
const REFRESH: &str = "fr1_RefreshTokenInventedForThisTestOnly_0123456789abc";

fn sign_in(id: &str, email: &str) -> serde_json::Map<String, Value> {
    json!({"account": {"id": id, "email": email, "name": "Ada", "providers": ["google"]},
           "tokens": {"access_token": ACCESS, "access_expires_at": 1_800_003_600, "refresh_token": REFRESH}})
    .as_object()
    .unwrap()
    .clone()
}

/// Saved, read back, cleared: one Keychain item (service "ferb", account "ferb.account") through the helper seam; the
/// tokens come back only in `account.get`, are masked by the redactor, and never reach the journal. A new run reads the
/// item and masks the tokens on that first read.
// decision: APP-1138
// decision: PHN-830
#[test]
fn the_sign_in_round_trips_through_the_keychain_and_its_tokens_stay_secret() {
    let a = area(Arc::new(FakeKeychain::default()));
    let out = a.account.get().unwrap();
    assert_eq!(
        Value::Object(out),
        json!({"signed_in": false, "account": null, "tokens": null})
    );
    assert_eq!(a.account.mac_account(), None);

    let saved = a.account.save(&sign_in(ACCOUNT_ID, ACCOUNT_EMAIL)).unwrap();
    assert_eq!(
        Value::Object(saved),
        json!({"saved": true, "removed_phones": []})
    );
    let items = a.keychain.items.lock().unwrap().clone();
    assert_eq!(items.keys().collect::<Vec<_>>(), vec![KEYCHAIN_ACCOUNT]);
    let got = a.account.get().unwrap();
    assert_eq!(got["signed_in"], true);
    assert_eq!(
        got["account"],
        json!({"id": ACCOUNT_ID, "email": ACCOUNT_EMAIL, "name": "Ada", "providers": ["google"]})
    );
    assert_eq!(
        got["tokens"],
        json!({"access_token": ACCESS, "access_expires_at": 1_800_003_600, "refresh_token": REFRESH})
    );
    assert_eq!(
        a.account.mac_account(),
        Some(MacAccount {
            id: ACCOUNT_ID.into(),
            email: ACCOUNT_EMAIL.into()
        })
    );
    for t in [ACCESS, REFRESH] {
        assert!(!a.redactor.text(&format!("the token {t} here")).contains(t));
    }
    // a new run: read once from the Keychain, masked on that read
    let b = area(a.keychain.clone());
    assert!(b.redactor.text(ACCESS).contains(ACCESS), "not known yet");
    assert_eq!(b.account.get().unwrap()["signed_in"], true);
    for t in [ACCESS, REFRESH] {
        assert!(!b.redactor.text(&format!("the token {t} here")).contains(t));
    }
    let reads = |k: &FakeKeychain| {
        k.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.starts_with("lookup"))
            .count()
    };
    let before = reads(&b.keychain);
    b.account.get().unwrap();
    b.account.mac_account();
    assert_eq!(reads(&b.keychain), before, "read once per run");

    let cleared = a.account.clear().unwrap();
    assert_eq!(Value::Object(cleared), json!({"cleared": true}));
    assert!(a.keychain.items.lock().unwrap().is_empty());
    assert_eq!(a.account.get().unwrap()["signed_in"], false);
    assert_eq!(a.account.mac_account(), None);
    // clearing again is fine
    a.account.clear().unwrap();

    // the journal: structural facts only
    let journal = a.journal.lock().unwrap().clone();
    assert_eq!(
        journal,
        vec![
            (
                "account.saved".to_string(),
                json!({"provider_count": 1, "changed": true})
            ),
            ("account.cleared".to_string(), json!({})),
            ("account.cleared".to_string(), json!({})),
        ]
    );
    let text = format!("{journal:?}");
    for never in [ACCESS, REFRESH, ACCOUNT_EMAIL, "fa1_", "fr1_"] {
        assert!(!text.contains(never), "{never}");
    }
    // every Keychain call was for the one item
    assert!(a
        .keychain
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|c| c.ends_with(KEYCHAIN_ACCOUNT)));
}

/// The Keychain failing is told apart from signed out (REL-453): `mac_account_read` is `Err`, then reads again.
// decision: REL-453, PHN-844
#[test]
fn a_keychain_that_can_t_be_read_is_not_signed_out() {
    #[derive(Default)]
    struct Failing(std::sync::atomic::AtomicBool);
    impl KeyHelper for Failing {
        fn store(&self, _: &str, _: &Secret) -> Result<(), HelperError> {
            Ok(())
        }
        fn lookup(&self, _: &str) -> Result<Option<Secret>, HelperError> {
            if self.0.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(None);
            }
            Err(HelperError::Fault(KeyFault {
                class: "OSError".into(),
                message: "the helper couldn't run".into(),
            }))
        }
        fn delete(&self, _: &str) -> Result<bool, HelperError> {
            Ok(false)
        }
    }
    let helper = Arc::new(Failing::default());
    let account = Account::new(AccountSeams {
        helper: helper.clone(),
        redactor: Arc::new(Redactor::new(&[], &[])),
        journal: Arc::new(|_: &str, _: Value| {}),
    });
    assert_eq!(account.mac_account_read(), Err(MacUnreadable));
    assert_eq!(account.mac_account(), None);
    helper.0.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        account.mac_account_read(),
        Ok(None),
        "tried again, and signed out"
    );
}

/// A name is never a reason to refuse a sign-in: up to 200 characters (not bytes) kept whole, a longer one cut to 200
/// characters, one that isn't text dropped; the site keeps the same 200 characters.
// decision: REL-453, PHN-844
#[test]
fn a_name_never_stops_a_sign_in() {
    for (given, kept) in [
        (json!("é".repeat(200)), json!("é".repeat(200))),
        (json!("😀".repeat(250)), json!("😀".repeat(200))),
        (json!("x".repeat(201)), json!("x".repeat(200))),
        (json!(7), Value::Null),
        (Value::Null, Value::Null),
    ] {
        let a = area(Arc::new(FakeKeychain::default()));
        let mut p = sign_in(ACCOUNT_ID, ACCOUNT_EMAIL);
        p["account"]["name"] = given.clone();
        assert!(a.account.save(&p).is_ok(), "{given}");
        assert_eq!(a.account.get().unwrap()["account"]["name"], kept);
    }
}

/// What isn't a sign-in is refused in plain words, and nothing is stored.
// decision: APP-1138
#[test]
fn a_malformed_sign_in_is_refused_and_nothing_is_stored() {
    let a = area(Arc::new(FakeKeychain::default()));
    let mut cases: Vec<serde_json::Map<String, Value>> = Vec::new();
    for (path, bad) in [
        (("account", "id"), json!("")),
        (("account", "id"), json!("x".repeat(65))),
        (("account", "id"), json!(7)),
        (("account", "email"), json!("a".repeat(321))),
        (("account", "email"), Value::Null),
        (("account", "providers"), json!("google")),
        (("tokens", "access_token"), json!("t".repeat(513))),
        (("tokens", "refresh_token"), json!(null)),
        (("tokens", "access_token"), json!(["a"])),
    ] {
        let mut p = sign_in(ACCOUNT_ID, ACCOUNT_EMAIL);
        p[path.0][path.1] = bad;
        cases.push(p);
    }
    let mut no_tokens = sign_in(ACCOUNT_ID, ACCOUNT_EMAIL);
    no_tokens.remove("tokens");
    cases.push(no_tokens);
    cases.push(serde_json::Map::new());
    for p in cases {
        let e = a.account.save(&p).unwrap_err();
        assert!(!e.message.is_empty(), "{p:?}");
        assert!(!e.message.contains(ACCESS) && !e.message.contains(REFRESH));
    }
    assert!(a.keychain.items.lock().unwrap().is_empty());
    assert!(a.journal.lock().unwrap().is_empty());
    // the longest values taken are taken
    let mut long = sign_in(&"i".repeat(64), &"e".repeat(320));
    long["tokens"]["access_token"] = json!("t".repeat(512));
    a.account.save(&long).unwrap();
}

/// Signing in to another account removes every phone bound to the old one through `phone.remove`'s own path (`by`
/// "account"); the reply names them. A phone paired before accounts (none recorded) survives every save. The same
/// account again keeps its phones, and signing out removes none.
// decision: PHN-828
// decision: APP-1138
#[tokio::test]
async fn another_account_removes_the_old_account_s_phones_and_the_same_one_keeps_them() {
    let w = world(None).await;
    let a = area(Arc::new(FakeKeychain::default()));
    a.account.set_phones(w.phones.clone());
    legacy(&w, "old", &Keys::new(9));
    // the Mac signs in: the phone paired before accounts stays
    let first = a.account.save(&sign_in(ACCOUNT_ID, ACCOUNT_EMAIL)).unwrap();
    assert_eq!(first["removed_phones"], json!([]));
    pair(&w, "e1", &Keys::new(1), false, false).await;
    pair(&w, "e2", &Keys::new(2), false, false).await;
    // the same account again (a token refresh): nothing goes
    let same = a.account.save(&sign_in(ACCOUNT_ID, ACCOUNT_EMAIL)).unwrap();
    assert_eq!(same["removed_phones"], json!([]));
    assert_eq!(live(&w), vec!["old", "e1", "e2"]);
    // signing out removes nothing
    a.account.clear().unwrap();
    assert_eq!(live(&w), vec!["old", "e1", "e2"]);
    // another account: the two bound to the old one go, as phone.remove removes one; the unbound one stays
    let other = a
        .account
        .save(&sign_in(OTHER_ID, "bob@example.com"))
        .unwrap();
    assert_eq!(
        other["removed_phones"],
        json!([{"id": "e1", "name": "Phone e1"}, {"id": "e2", "name": "Phone e2"}])
    );
    assert_eq!(live(&w), vec!["old"]);
    let all = {
        let db = w.db.lock().unwrap();
        ferb_store::phones::all(db.conn()).unwrap()
    };
    assert!(all
        .iter()
        .filter(|p| p.endpoint != "old")
        .all(|p| p.removed_by.as_deref() == Some("account")));
    let removed: Vec<String> = w
        .published
        .lock()
        .unwrap()
        .iter()
        .filter(|(k, _)| k == "phone.removed")
        .map(|(_, v)| v["phone"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(removed, vec!["e1", "e2"]);
    let journal = format!("{:?}", w.journal.lock().unwrap());
    assert!(journal.contains(r#""by": String("account")"#), "{journal}");
    // the removed phone is refused at once
    let refused = Conn::open(&w, json!({"role": "phone", "device": "e1", "session": "s"}))
        .await
        .err()
        .unwrap();
    assert_eq!(refused.0, code::UNKNOWN_PHONE);
    // and the account area's own journal keeps no email or token
    let text = format!("{:?}", a.journal.lock().unwrap());
    assert!(!text.contains("example.com") && !text.contains(ACCESS));
}

const OTHER_ID: &str = "99999999-8888-4777-8666-555555555555";

/// A phone paired before accounts, as migration 0068 left it (no account recorded), with real keys.
fn legacy(w: &World, endpoint: &str, keys: &Keys) {
    let db = w.db.lock().unwrap();
    ferb_store::phones::add(
        &db,
        &ferb_store::phones::NewPhone {
            endpoint: endpoint.into(),
            name: "Old phone".into(),
            app: "FERB".into(),
            unlock_pub: Some(spki(&keys.lock)),
            device_pub: Some(spki(&keys.device)),
            strong_pub: None,
            account_id: None,
        },
        "2026-11-01T10:00:00+05:30",
        5,
    )
    .unwrap()
    .unwrap();
}

fn account_of(w: &World, endpoint: &str) -> Option<String> {
    let db = w.db.lock().unwrap();
    ferb_store::phones::by_endpoint(db.conn(), endpoint)
        .unwrap()
        .and_then(|p| p.account_id)
}

/// A phone paired before accounts binds itself when it signs in: the Mac's account binds it (again: no change); a
/// statement that can't be confirmed or a signed-out Mac change nothing; and the bound phone then survives the Mac's
/// saves of the same account.
// decision: PHN-828
#[tokio::test]
async fn a_phone_paired_before_accounts_binds_to_the_mac_s_account() {
    let w = world(None).await;
    let keys = Keys::new(9);
    legacy(&w, "old", &keys);
    let mut c = Conn::phone(&w, "old", "s").await;
    // nothing proves it but the site's signature: allowed while the app is locked
    let (n, words) = c
        .call(
            "phone.bind_account",
            json!({"assertion": "fpa1.junk"}),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(
        (n, words.as_str()),
        (code::ACCOUNT_UNCONFIRMED, UNCONFIRMED_WORDS)
    );
    // a statement for another phone's endpoint is no proof for this one
    let (n, _) = c
        .call(
            "phone.bind_account",
            json!({"assertion": assertion(&w, "e1")}),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(n, code::ACCOUNT_UNCONFIRMED);
    *w.mac.lock().unwrap() = None;
    let (n, words) = c
        .call(
            "phone.bind_account",
            json!({"assertion": assertion(&w, "old")}),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(
        (n, words.as_str()),
        (code::MAC_SIGNED_OUT, SIGNED_OUT_WORDS)
    );
    assert_eq!(account_of(&w, "old"), None, "nothing changed");
    *w.mac.lock().unwrap() = Some(mac_account());
    let bound = c
        .ok(
            "phone.bind_account",
            json!({"assertion": assertion(&w, "old")}),
        )
        .await;
    assert_eq!(bound, json!({"bound": true, "changed": true}));
    assert_eq!(account_of(&w, "old").as_deref(), Some(ACCOUNT_ID));
    let again = c
        .ok(
            "phone.bind_account",
            json!({"assertion": assertion(&w, "old")}),
        )
        .await;
    assert_eq!(again, json!({"bound": true, "changed": false}));
    let journal = format!("{:?}", w.journal.lock().unwrap());
    assert!(journal.contains("phone.account_bound") && journal.contains("phone.bind_refused"));
    assert!(!journal.contains("example.com"), "{journal}");
    // bound: the same account's save keeps it
    let a = area(Arc::new(FakeKeychain::default()));
    a.account.set_phones(w.phones.clone());
    a.account.save(&sign_in(ACCOUNT_ID, ACCOUNT_EMAIL)).unwrap();
    assert_eq!(live(&w), vec!["old"]);
    // and the phone goes on as before
    c.present(&keys).await;
    c.unlock(&keys).await;
    c.ok("tasks.list", json!({})).await;
}

/// A phone paired before accounts that signs in to another account than the Mac's is removed, as `phone.remove`
/// removes one (`by` "account"), and told so in the mismatch's words.
// decision: PHN-828
#[tokio::test]
async fn a_phone_paired_before_accounts_of_another_account_is_removed() {
    let w = world(None).await;
    legacy(&w, "old", &Keys::new(9));
    let mut c = Conn::phone(&w, "old", "s").await;
    let mut claims = claims(&w, "old");
    claims["sub"] = json!(OTHER_ID);
    claims["email"] = json!("bob@example.com");
    let (n, words) = c
        .call(
            "phone.bind_account",
            json!({"assertion": assertion_of(&site_key(), &claims)}),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(
        (n, words.as_str()),
        (code::ACCOUNT_MISMATCH, MISMATCH_WORDS)
    );
    assert!(live(&w).is_empty());
    let row = {
        let db = w.db.lock().unwrap();
        ferb_store::phones::all(db.conn()).unwrap().remove(0)
    };
    assert_eq!(row.removed_by.as_deref(), Some("account"));
    assert!(w
        .published
        .lock()
        .unwrap()
        .iter()
        .any(|(k, v)| k == "phone.removed" && v["phone"] == "old"));
    // every later call under it is refused
    let refused = Conn::open(
        &w,
        json!({"role": "phone", "device": "old", "session": "t"}),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(refused.0, code::UNKNOWN_PHONE);
}

/// Only a new pairing and `phone.bind_account` look at accounts: with no key pinned, or the Mac signed out, a phone
/// already paired (bound or not) opens its session, unlocks and calls exactly as before.
// decision: PHN-828
// decision: PHN-124
#[tokio::test]
async fn an_already_paired_phone_works_with_no_key_pinned_or_the_mac_signed_out() {
    for keys_pinned in [false, true] {
        let w = world_with(None, keys_pinned).await;
        let (k1, k2) = (Keys::new(1), Keys::new(2));
        legacy(&w, "old", &k1);
        legacy(&w, "bound", &k2);
        {
            let db = w.db.lock().unwrap();
            ferb_store::phones::bind_account(db.conn(), "bound", ACCOUNT_ID).unwrap();
        }
        *w.mac.lock().unwrap() = None;
        for (endpoint, keys) in [("old", &k1), ("bound", &k2)] {
            let mut c = Conn::phone(&w, endpoint, "s").await;
            c.present(keys).await;
            c.unlock(keys).await;
            c.ok("tasks.list", json!({})).await;
            c.ok("chat.status", json!({})).await;
        }
        // a new pairing is what stops
        let (n, _) = request(&w, "e1", Some(assertion(&w, "e1")))
            .await
            .unwrap_err();
        assert_eq!(
            n,
            if keys_pinned {
                code::MAC_SIGNED_OUT
            } else {
                code::ACCOUNT_UNCONFIRMED
            }
        );
    }
}

/// A pairing waiting for the owner's Yes under the old account ends when the Mac's account changes: his Yes then pairs
/// nothing.
// decision: PHN-828
#[tokio::test]
async fn a_waiting_pairing_of_the_old_account_ends_with_the_change() {
    let w = world(None).await;
    let a = area(Arc::new(FakeKeychain::default()));
    a.account.set_phones(w.phones.clone());
    let asked = request(&w, "e1", Some(assertion(&w, "e1"))).await.unwrap();
    a.account
        .save(&sign_in(OTHER_ID, "bob@example.com"))
        .unwrap();
    let mut yes = ferb_rpc::Params::new();
    yes.insert("yes".into(), json!(true));
    yes.insert("request_id".into(), asked["request_id"].clone());
    assert!(w.phones.pair_decide(&yes).is_err());
    assert!(live(&w).is_empty());
}

fn live(w: &World) -> Vec<String> {
    let db = w.db.lock().unwrap();
    ferb_store::phones::live(db.conn())
        .unwrap()
        .into_iter()
        .map(|p| p.endpoint)
        .collect()
}
