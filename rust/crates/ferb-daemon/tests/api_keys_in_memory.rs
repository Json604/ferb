//! A provider's API key is read through the signed helper once per daemon run and kept in memory until a store or
//! delete changes it; presence checks never read a value (GATE-544, for GATE-543). Modelled on
//! tests/test_api_keys.py; the helper and `security` are stand-ins that record every call, nothing reaches the
//! Keychain.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ferb_daemon::areas_b::runtimes::keys::{HelperError, KeyCache, KeyHelper, Keys, Security};
use ferb_identity::Secret;

/// The helper's items in memory; every call recorded as `op account`.
#[derive(Default)]
struct Helper {
    items: Mutex<HashMap<String, String>>,
    calls: Mutex<Vec<String>>,
}

impl Helper {
    fn lookups(&self) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.starts_with("lookup"))
            .count()
    }
}

impl KeyHelper for Helper {
    fn store(&self, account: &str, value: &Secret) -> Result<(), HelperError> {
        self.calls.lock().unwrap().push(format!("store {account}"));
        self.items
            .lock()
            .unwrap()
            .insert(account.into(), value.expose().into());
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

/// `security` over the helper's items (a presence check finds them, as the real one does) and no older items; every
/// run recorded.
struct Sec {
    helper: Arc<Helper>,
    runs: Mutex<Vec<Vec<String>>>,
}

impl Security for Sec {
    fn run(&self, args: &[&str]) -> (i32, Secret) {
        self.runs
            .lock()
            .unwrap()
            .push(args.iter().map(|a| a.to_string()).collect());
        let account = args
            .iter()
            .position(|a| *a == "-a")
            .and_then(|i| args.get(i + 1))
            .copied()
            .unwrap_or("");
        let found = self.helper.items.lock().unwrap().contains_key(account);
        match args.first() {
            Some(&"find-generic-password") if found && !args.contains(&"-w") => {
                (0, Secret::new(String::new()))
            }
            _ => (44, Secret::new(String::new())),
        }
    }
}

/// One daemon run's key store: its own memory, the same Keychain.
fn run(helper: &Arc<Helper>, security: &Arc<Sec>) -> Keys {
    Keys {
        helper: helper.clone(),
        security: security.clone(),
        cache: Arc::new(KeyCache::default()),
    }
}

// decision: GATE-543, GATE-544
#[test]
fn a_key_is_read_through_the_helper_once_a_run_and_kept_in_memory() {
    let helper = Arc::new(Helper::default());
    let security = Arc::new(Sec {
        helper: helper.clone(),
        runs: Mutex::new(vec![]),
    });
    let mut problems = vec![];
    assert!(run(&helper, &security)
        .store_key("openai", &Secret::from("sk-test-123"), &mut problems)
        .unwrap());
    assert!(problems.is_empty());

    // a new run: the first use reads the helper's item, every later use is from memory
    let keys = run(&helper, &security);
    let before = helper.lookups();
    for _ in 0..3 {
        let key = keys.read_key("openai", &mut problems).unwrap().unwrap();
        assert_eq!(key.expose(), "sk-test-123");
    }
    assert_eq!(
        helper.lookups(),
        before + 1,
        "one helper read per provider per run"
    );
    assert!(keys.read_key("anthropic", &mut problems).unwrap().is_none());

    // presence checks never read a value: no helper read, and `security` never asked for one (-w)
    let lookups = helper.lookups();
    assert!(keys.has_api_key("openai") && !keys.has_api_key("google"));
    assert_eq!(helper.lookups(), lookups);
    assert!(security
        .runs
        .lock()
        .unwrap()
        .iter()
        .all(|args| !args.iter().any(|a| a == "-w")));

    // a store changes the kept key without another read; a delete forgets it
    assert!(keys
        .store_key("openai", &Secret::from("sk-test-456"), &mut problems)
        .unwrap());
    let lookups = helper.lookups();
    assert_eq!(
        keys.read_key("openai", &mut problems)
            .unwrap()
            .unwrap()
            .expose(),
        "sk-test-456"
    );
    assert_eq!(helper.lookups(), lookups);
    assert!(keys.delete_key("openai", &mut problems).unwrap());
    assert!(keys.read_key("openai", &mut problems).unwrap().is_none());
    assert!(!keys.has_api_key("openai"));
}
