//! The owner's personal identifiers in the saved-secret store (GATE-601): kept in one record for any site
//! (GATE-603), sealed from a chat message once (GATE-604) and shown to the AI only on the owner's word (GATE-602).
//! A memory Keychain and a plain host reader stand in for the helper and the browser's parser; values are invented.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use ferb_identity::owner_secrets::{Keychain, OwnerSecrets, Places, ANY_SITE, IDENTIFIER};
use ferb_identity::Secret;
use serde_json::json;
use tempfile::TempDir;

#[derive(Default)]
struct Memory(Mutex<BTreeMap<String, String>>);

impl Keychain for Memory {
    fn store(&self, account: &str, value: &Secret) -> ferb_identity::Result<()> {
        self.0
            .lock()
            .unwrap()
            .insert(account.to_owned(), value.expose().to_owned());
        Ok(())
    }
    fn lookup(&self, account: &str) -> ferb_identity::Result<Option<Secret>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(account)
            .cloned()
            .map(Secret::new))
    }
    fn delete(&self, account: &str) -> ferb_identity::Result<bool> {
        Ok(self.0.lock().unwrap().remove(account).is_some())
    }
}

/// A host as a browser reads it, for the invented addresses here only.
struct Hosts;

impl Places for Hosts {
    fn is_browser(&self, bundle_id: &str) -> bool {
        bundle_id == "com.google.Chrome"
    }
    fn url_host(&self, url: &str) -> Option<String> {
        let rest = url.split_once("://")?.1;
        let host = rest
            .split(['/', ':', '?', '#'])
            .next()?
            .to_ascii_lowercase();
        (!host.is_empty() && !host.contains(' ')).then_some(host)
    }
}

fn store(home: &TempDir, memory: &Arc<Memory>) -> OwnerSecrets {
    OwnerSecrets::with_keychain(home.path(), memory.clone(), Arc::new(Hosts))
}

const PAN: &str = "ABCPE1234F";

// decision: GATE-603
// decision: GATE-605
#[test]
fn a_saved_identifier_fills_on_any_site_and_a_password_keeps_its_site() {
    let (home, memory) = (TempDir::new().unwrap(), Arc::new(Memory::default()));
    let s = store(&home, &memory);
    s.remember_identifier("PAN", &Secret::from(PAN)).unwrap();
    s.remember(
        "password",
        "https://bank.example",
        &Secret::from("invented-pw"),
        &[],
        None,
    )
    .unwrap();
    for target in ["https://tax.example", "https://other.example"] {
        let (value, any_site) = s.reveal_entry("PAN", target, None).unwrap();
        assert_eq!((value.expose(), any_site), (PAN, true), "{target}");
        assert!(s.fills_from_any_site("PAN", target).unwrap());
    }
    // GATE-605: an app gets only its own entries (the sink rule guarding an ID on a site has no app form)
    assert!(s.reveal("PAN", "app:com.example.Forms", None).is_err());
    assert!(!s
        .fills_from_any_site("PAN", "app:com.example.Forms")
        .unwrap());
    assert!(s
        .listing("app:com.example.Forms", &[], None)
        .unwrap()
        .get("identifiers")
        .is_none());
    // a site's own entry is bound to that site: not from any site
    let (_, any_site) = s
        .reveal_entry("password", "https://bank.example", None)
        .unwrap();
    assert!(!any_site);
    assert!(!s
        .fills_from_any_site("password", "https://bank.example")
        .unwrap());
    assert_eq!(
        s.reveal("password", "https://bank.example", None)
            .unwrap()
            .expose(),
        "invented-pw"
    );
    assert!(s.reveal("password", "https://tax.example", None).is_err());
    // never into a card field (GATE-206, GATE-219), never for a whole browser
    assert!(s
        .reveal("PAN", "https://tax.example", Some("card_number"))
        .is_err());
    assert!(s.reveal("PAN", "app:com.google.Chrome", None).is_err());
    // a site's own entry of that name wins
    s.remember(
        "PAN",
        "https://tax.example",
        &Secret::from("site-own"),
        &[],
        None,
    )
    .unwrap();
    assert_eq!(
        s.reveal("PAN", "https://tax.example", None)
            .unwrap()
            .expose(),
        "site-own"
    );
    assert!(!s.fills_from_any_site("PAN", "https://tax.example").unwrap());
    // what secrets.list shows for any place: the names, never a value
    let listing = s.listing("https://other.example", &[], None).unwrap();
    assert_eq!(listing["identifiers"], json!(["PAN"]));
    assert!(!listing.to_string().contains(PAN));
    // the record is never linked to a site
    assert!(listing.get("linked").is_none());
    let inventory = s.inventory().unwrap();
    assert!(inventory.contains(
        &json!({"place": ANY_SITE, "name": "PAN", "kind": IDENTIFIER, "unusable": false, "shown": false})
    ));
}

// decision: GATE-604
#[test]
fn sealing_names_by_kind_and_reuses_the_same_value() {
    let (home, memory) = (TempDir::new().unwrap(), Arc::new(Memory::default()));
    let s = store(&home, &memory);
    assert_eq!(s.seal_identifier("PAN", &Secret::from(PAN)).unwrap(), "PAN");
    // the same value however written: the same entry
    assert_eq!(
        s.seal_identifier("PAN", &Secret::from("abcpe 1234 f"))
            .unwrap(),
        "PAN"
    );
    // another value of the kind: a new name, the first untouched
    assert_eq!(
        s.seal_identifier("PAN", &Secret::from("ABCPE1234G"))
            .unwrap(),
        "PAN 2"
    );
    assert_eq!(
        s.reveal("PAN", "https://x.example", None).unwrap().expose(),
        PAN
    );
    assert_eq!(memory.0.lock().unwrap().len(), 2);
}

// decision: GATE-602
#[test]
fn only_a_saved_identifier_can_be_shown_and_a_removal_or_new_value_ends_it() {
    let (home, memory) = (TempDir::new().unwrap(), Arc::new(Memory::default()));
    let s = store(&home, &memory);
    s.remember_identifier("PAN", &Secret::from(PAN)).unwrap();
    s.remember(
        "password",
        "https://bank.example",
        &Secret::from("invented-pw"),
        &[],
        None,
    )
    .unwrap();
    assert!(
        s.set_shown("password", true).is_err(),
        "a password is never shown"
    );
    assert!(s.set_shown("nothing", true).is_err());
    assert_eq!(
        s.set_shown("PAN", true).unwrap(),
        json!({"name": "PAN", "shown": true})
    );
    let values: Vec<String> = s
        .shown_values()
        .unwrap()
        .iter()
        .map(|v| v.expose().to_owned())
        .collect();
    assert_eq!(values, vec![PAN]);
    // the handle file never holds the value
    let file = std::fs::read_to_string(s.path()).unwrap();
    assert!(
        !file.contains(PAN) && file.contains("\"shown\": [\"PAN\"]"),
        "{file}"
    );
    s.set_shown("PAN", false).unwrap();
    assert!(s.shown_values().unwrap().is_empty());
    // a new value under the name is not the value the owner let show
    s.set_shown("PAN", true).unwrap();
    s.remember_identifier("PAN", &Secret::from("ABCPE1234G"))
        .unwrap();
    assert!(s.shown_values().unwrap().is_empty());
    // deleting the entry ends its grant and leaves no record behind
    s.set_shown("PAN", true).unwrap();
    s.delete(ANY_SITE, "PAN").unwrap();
    assert!(s.shown_values().unwrap().is_empty());
    assert!(s.records().unwrap().get(ANY_SITE).is_none());
}
