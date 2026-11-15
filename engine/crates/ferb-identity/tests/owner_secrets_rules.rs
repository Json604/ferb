//! The owner's saved secrets' safety rules on a stand-in Keychain that records every call: deleting goes through the
//! helper's delete and never reads a value (GATE-2), and a saved card detail fills only a field of its own kind
//! (GATE-219). Modelled on tests/test_app_screens.py (the delete) and tests/test_gateway_needs.py (the card kinds);
//! a temporary home only, nothing reaches the real Keychain.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ferb_identity::owner_secrets::{Keychain, OwnerSecrets, Places, FILE_NAME};
use ferb_identity::Secret;
use serde_json::{json, Value};
use tempfile::TempDir;

/// A Keychain in memory, every call recorded as `op account`.
#[derive(Default)]
struct Memory {
    values: Mutex<HashMap<String, String>>,
    calls: Mutex<Vec<String>>,
}

impl Memory {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn values(&self) -> Vec<String> {
        self.values.lock().unwrap().values().cloned().collect()
    }
}

impl Keychain for Memory {
    fn store(&self, account: &str, value: &Secret) -> ferb_identity::Result<()> {
        self.calls.lock().unwrap().push(format!("store {account}"));
        self.values
            .lock()
            .unwrap()
            .insert(account.into(), value.expose().into());
        Ok(())
    }
    fn lookup(&self, account: &str) -> ferb_identity::Result<Option<Secret>> {
        self.calls.lock().unwrap().push(format!("lookup {account}"));
        Ok(self
            .values
            .lock()
            .unwrap()
            .get(account)
            .map(|v| Secret::new(v.clone())))
    }
    fn delete(&self, account: &str) -> ferb_identity::Result<bool> {
        self.calls.lock().unwrap().push(format!("delete {account}"));
        Ok(self.values.lock().unwrap().remove(account).is_some())
    }
}

/// No app is a browser; a place's host is its address's.
struct Web;

impl Places for Web {
    fn is_browser(&self, _: &str) -> bool {
        false
    }
    fn url_host(&self, url: &str) -> Option<String> {
        let rest = url.split_once("://").map(|(_, r)| r)?;
        let host = rest.split(['/', ':', '?', '#']).next().unwrap_or("");
        (!host.is_empty()).then(|| host.to_lowercase())
    }
}

fn store(home: &TempDir, keychain: &Arc<Memory>) -> OwnerSecrets {
    OwnerSecrets::with_keychain(home.path(), keychain.clone(), Arc::new(Web))
}

const SHOP: &str = "https://shop.example";

// decision: GATE-2
#[test]
fn delete_removes_the_keychain_item_through_the_helper_and_never_reads_a_value() {
    let home = TempDir::new().unwrap();
    let keychain = Arc::new(Memory::default());
    let s = store(&home, &keychain);
    s.remember("password", SHOP, &Secret::from("hunter2-canary"), &[], None)
        .unwrap();
    s.remember(
        "permit number",
        SHOP,
        &Secret::from("4111111111111111"),
        &[],
        Some("card_number"),
    )
    .unwrap();
    // the listing names place, name and kind, never a value or a Keychain account
    let listed = json!(s.inventory().unwrap()).to_string();
    for hidden in ["hunter2", "4111", "\u{b7}"] {
        assert!(!listed.contains(hidden), "{hidden} in {listed}");
    }
    assert!(listed.contains("card_number") && listed.contains("permit number"));
    let before = keychain.calls().len();
    let out = s.delete(SHOP, "permit number").unwrap();
    assert_eq!(
        out,
        json!({"place": SHOP, "name": "permit number", "deleted": true})
    );
    // one call reached the Keychain, the helper's delete; nothing was read first
    assert_eq!(
        keychain.calls()[before..],
        [format!("delete {SHOP} \u{b7} permit number")]
    );
    assert_eq!(keychain.values(), vec!["hunter2-canary".to_string()]);
    let left: Vec<Value> = s.inventory().unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0]["name"], "password");
    // deleting it again: nothing is saved under that name, refused, and the Keychain isn't touched
    let again = s.delete(SHOP, "permit number").unwrap_err();
    assert!(again.refusal().is_some(), "{again:?}");
    assert_eq!(keychain.calls().len(), before + 1);
    assert!(keychain.calls().iter().all(|c| !c.starts_with("lookup")));
}

// decision: GATE-2
#[test]
fn an_item_another_entry_still_names_stays_and_an_item_not_found_counts_as_deleted() {
    let home = TempDir::new().unwrap();
    let keychain = Arc::new(Memory::default());
    keychain
        .values
        .lock()
        .unwrap()
        .insert("shared-item".into(), "invented-value".into());
    // two entries naming one Keychain item (as a migrated older file can), and one whose item is already gone
    let file = json!({"version": 2, "records": {SHOP: {
        "username": null, "username_source": null, "sites": [], "kinds": {},
        "entries": {"one": "shared-item", "two": "shared-item", "gone": "missing-item"}}}, "unusable": []});
    std::fs::write(home.path().join(FILE_NAME), file.to_string()).unwrap();
    let s = store(&home, &keychain);
    assert_eq!(s.delete(SHOP, "one").unwrap()["deleted"], true);
    assert!(
        keychain.calls().is_empty(),
        "an item another entry names stays: {:?}",
        keychain.calls()
    );
    assert_eq!(s.delete(SHOP, "two").unwrap()["deleted"], true);
    assert_eq!(keychain.calls(), ["delete shared-item"]);
    assert!(keychain.values().is_empty());
    assert_eq!(s.delete(SHOP, "gone").unwrap()["deleted"], true);
    assert_eq!(
        keychain.calls(),
        ["delete shared-item", "delete missing-item"]
    );
    assert!(
        s.records().unwrap().is_empty(),
        "a record left with nothing goes"
    );
}

// decision: GATE-219
#[test]
fn a_saved_card_detail_fills_only_its_own_kind_of_field() {
    let home = TempDir::new().unwrap();
    let keychain = Arc::new(Memory::default());
    let s = store(&home, &keychain);
    let place = "https://api.razorpay.example";
    s.remember(
        "my card",
        place,
        &Secret::from("4111111111111111"),
        &[],
        Some("card_number"),
    )
    .unwrap();
    s.remember("my cvv", place, &Secret::from("123"), &[], Some("card_csc"))
        .unwrap();
    s.remember("login", place, &Secret::from("pw"), &[], None)
        .unwrap();
    assert_eq!(
        s.reveal("my card", place, Some("card_number"))
            .unwrap()
            .expose(),
        "4111111111111111"
    );
    let reads = keychain
        .calls()
        .iter()
        .filter(|c| c.starts_with("lookup"))
        .count();
    // never into an ordinary field
    let plain = s.reveal("my card", place, None).unwrap_err();
    assert!(
        plain.refusal().is_some_and(|m| m.contains("card number")),
        "{plain:?}"
    );
    // never into another card field, and a password never into a card field
    for (name, kind) in [("my cvv", "card_number"), ("login", "card_csc")] {
        let e = s.reveal(name, place, Some(kind)).unwrap_err();
        assert!(e.refusal().is_some(), "{name} into {kind}: {e:?}");
    }
    // a refused fill reads nothing from the Keychain
    assert_eq!(
        keychain
            .calls()
            .iter()
            .filter(|c| c.starts_with("lookup"))
            .count(),
        reads
    );
    assert_eq!(
        s.listing(place, &[], None).unwrap()["card_details"],
        json!({"my card": "card_number", "my cvv": "card_csc"})
    );
}
