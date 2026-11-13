//! Every browser tool against the frozen catalog (rust/fixtures/catalog/tools.json, made by the
//! Python registry): its name, MCP name, description (the fill tools' with FERB's saved secrets,
//! browser.downloads' with the download limit), scopes, identities and the facts the gateway reads,
//! in the registry's order. The input schemas are the catalog's bytes, served by the gateway.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use ferb_browser::bridge::{Hub, Ledger};
use ferb_browser::driver::{Driver, Kind, Secrets, Tasks};
use ferb_browser::pairing::{InstallRow, InstallStore, PairingKey, Pairings, StoreError};
use ferb_browser::rules::{BrowserApps, BrowserInfo};
use ferb_browser::tools::{Host, Policy, Tools, FERB_BROWSER, OWNER_BROWSER};
use ferb_core::interfaces::{BoxFuture, Clock};
use ferb_core::types::{Identity, TabRef, ToolContext};
use ferb_core::when::DateTime;
use ferb_identity::secrets::{SecretRef, SecretSpec};
use ferb_identity::Secret;
use serde_json::{json, Map, Value};

use common::Counter;

struct Empty;

impl InstallStore for Empty {
    fn row(&self, _: &str) -> Result<Option<InstallRow>, StoreError> {
        Ok(None)
    }
    fn install_for(&self, _: &str, _: &str) -> Result<Option<String>, StoreError> {
        Ok(None)
    }
    fn installs(&self) -> Result<Vec<String>, StoreError> {
        Ok(vec![])
    }
    fn key(&self, _: &str) -> Result<Option<PairingKey>, StoreError> {
        Ok(None)
    }
    fn key_and_seq(&self, _: &str) -> Result<Option<(PairingKey, i64)>, StoreError> {
        Ok(None)
    }
    fn version(&self, _: &str) -> Result<Option<String>, StoreError> {
        Ok(None)
    }
    fn set_version(&self, _: &str, _: &str) -> Result<(), StoreError> {
        Ok(())
    }
    fn set_last_seq(&self, _: &str, _: i64) -> Result<(), StoreError> {
        Ok(())
    }
    fn unpair(&self, _: &str, _: &str) -> Result<bool, StoreError> {
        Ok(false)
    }
    fn pair(&self, _: &str, _: &InstallRow, _: &PairingKey, _: &str) -> Result<(), StoreError> {
        Ok(())
    }
}

struct Clocked;

impl Clock for Clocked {
    fn now(&self) -> DateTime {
        ferb_core::when::parse("2026-01-01T09:00:00+00:00").unwrap()
    }
    fn monotonic(&self) -> f64 {
        0.0
    }
    fn sleep_until(&self, _: DateTime) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

struct NoLedger;

impl Ledger for NoLedger {
    fn state(&self, _: &str) -> Option<String> {
        None
    }
    fn reconcile(&self, _: &str, _: &str, _: Vec<String>) {}
}

struct Apps;

impl BrowserApps for Apps {
    fn known(&self, _: &str) -> bool {
        true
    }
    fn browser(&self, _: &str) -> Option<BrowserInfo> {
        None
    }
}

struct Saved(Vec<SecretSpec>);

impl Secrets for Saved {
    fn specs(&self) -> Vec<SecretSpec> {
        self.0.clone()
    }
    fn has(&self, s: &SecretRef) -> bool {
        self.0.iter().any(|x| x.secret == *s)
    }
    fn spec(&self, s: &SecretRef) -> Option<SecretSpec> {
        self.0.iter().find(|x| x.secret == *s).cloned()
    }
    fn reveal(&self, _: &SecretRef) -> BoxFuture<'_, Result<Secret, String>> {
        Box::pin(async { Err(String::new()) })
    }
}

struct NoPolicy;

impl Policy for NoPolicy {
    fn parse(&self, _: &str) -> Option<(String, String)> {
        None
    }
    fn typed_destination(&self, _: &str) -> Option<String> {
        None
    }
}

struct NoHost;

impl Host for NoHost {
    fn pair(
        &self,
        _: &str,
        _: &str,
    ) -> Option<BoxFuture<'_, ferb_browser::Result<Map<String, Value>>>> {
        None
    }
    fn handoff(
        &self,
        _: TabRef,
        _: String,
        _: ToolContext,
    ) -> Option<BoxFuture<'_, ferb_browser::Result<Map<String, Value>>>> {
        None
    }
    fn resolve(&self, p: &str, _: &ToolContext, _: bool) -> ferb_browser::Result<String> {
        Ok(p.into())
    }
    fn owner_path(&self, p: &str, _: &str) -> String {
        p.into()
    }
    fn read(&self, _: &str) -> ferb_browser::Result<Vec<u8>> {
        Ok(vec![])
    }
    fn new_file(&self, _: &str, _: &[u8], _: &ToolContext) -> ferb_browser::Result<()> {
        Ok(())
    }
    fn display_of(&self, _: ferb_browser::mask::Rect) -> Option<i64> {
        None
    }
    fn recordings(&self) -> Option<Arc<dyn ferb_browser::tools::Recordings>> {
        None
    }
}

fn catalog() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/catalog/tools.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn tools(secrets: Vec<SecretSpec>) -> (Tools, Tools) {
    let random = Arc::new(Counter::new());
    let pairings = Arc::new(Pairings::new(
        Arc::new(Empty),
        Arc::new(Clocked),
        random.clone(),
    ));
    let hub = Hub::new(
        pairings,
        Arc::new(NoLedger),
        Arc::new(|_: &str, _: Map<String, Value>| {}),
        Arc::new(|| "dev-1".to_string()),
        random.clone(),
        Arc::new(Apps),
        Arc::new(|_| None),
    );
    let owner = Driver::new(
        hub.clone(),
        Kind::Owner {
            choose: Arc::new(|_| Err(ferb_browser::Error::refused("none"))),
            input: None,
        },
        Tasks::default(),
        None,
        random.clone(),
    );
    let ferb = Driver::new(
        hub,
        Kind::Ferb {
            profile: Arc::new(|| None),
            secrets: Arc::new(Saved(secrets)),
        },
        Tasks::default(),
        None,
        random,
    );
    let make = |family, driver| Tools {
        family,
        driver,
        policy: Arc::new(NoPolicy),
        host: Arc::new(NoHost),
    };
    (make(OWNER_BROWSER, owner), make(FERB_BROWSER, ferb))
}

// decisions: HAND-402, HAND-436, GATE-202, L31, L33
#[test]
fn every_browser_tool_is_the_catalogs() {
    let c = catalog();
    let (owner, ferb) = tools(vec![]);
    let listed: Vec<&Value> = c["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["name"].as_str().unwrap().contains("browser."))
        .collect();
    let mut names = vec![];
    for family in [&owner, &ferb] {
        for v in family.verbs() {
            let name = family.name(v);
            let t = listed
                .iter()
                .find(|t| t["name"] == name.as_str())
                .unwrap_or_else(|| panic!("{name} in the catalog"));
            assert_eq!(t["mcp_name"], name.replace('.', "_"), "{name}");
            assert_eq!(t["description"], family.description(v), "{name}");
            assert_eq!(t["scopes"], json!(family.scopes(v)), "{name}");
            let who = if family.family.identity == Identity::Ferb {
                "ferb"
            } else {
                "owner"
            };
            assert_eq!(t["identities"], json!([who]), "{name}");
            assert_eq!(t["facts"], Value::Object(family.facts(v)), "{name}");
            assert_eq!(
                t["acts"],
                json!(v.risk > 1 || v.generic_input),
                "{name}: acts"
            );
            names.push(name);
        }
    }
    // the registry's order: each family's tools, as the catalog lists them
    let order: Vec<&str> = listed.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, order);
    // descriptions that fill in what is saved now
    let spec = SecretSpec {
        secret: SecretRef {
            key: "ferb/example/password".into(),
        },
        allowed_origins: vec![
            "https://accounts.example.test".into(),
            "https://example.test".into(),
        ],
        account_hint: "ferb@example.test".into(),
    };
    let (_, ferb) = tools(vec![spec]);
    for (name, want) in c["dynamic_descriptions"]["with_one_invented_secret"]
        .as_object()
        .unwrap()
    {
        let v = ferb_browser::tools::verb(name.split_once('.').unwrap().1).unwrap();
        assert_eq!(ferb.description(v), want.as_str().unwrap(), "{name}");
    }
}

/// The model validators' words (pydantic's `model_validator`s on browser.pdf, browser.media and
/// browser.act), as Python's raise them.
#[test]
fn the_tools_own_checks() {
    use ferb_browser::tools::validate;
    assert_eq!(
        validate("pdf", &json!({"path": "a.pdf"})).unwrap(),
        "give exactly one of tab or html_path"
    );
    assert_eq!(
        validate("pdf", &json!({"path": "a.PDF", "tab": "t1"})),
        None
    );
    assert_eq!(
        validate("pdf", &json!({"path": "a.png", "tab": "t1"})).unwrap(),
        "path must end in .pdf"
    );
    assert_eq!(
        validate("media", &json!({"tab": "t1", "action": "seek"})).unwrap(),
        "seek needs `to` (seconds); only seek takes it"
    );
    assert_eq!(
        validate(
            "media",
            &json!({"tab": "t1", "action": "play", "index": 0, "media_id": "m1-aaaaaa"})
        )
        .unwrap(),
        "name the element by index or by media_id, one of them"
    );
    assert_eq!(
        validate(
            "act",
            &json!({"tab": "t1", "element": "s1e1", "action": "type"})
        )
        .unwrap(),
        "type requires explicit text; no default key or replacement value"
    );
    assert_eq!(
        validate(
            "act",
            &json!({"tab": "t1", "element": "s1e1", "action": "click", "mode": "insert"})
        )
        .unwrap(),
        "mode is for type only"
    );
    assert_eq!(
        validate(
            "act",
            &json!({"tab": "t1", "element": "s1e1", "action": "hover", "modifiers": ["cmd"]})
        )
        .unwrap(),
        "modifiers go with a real click, dblclick or rightclick only (not page_mode)"
    );
    assert_eq!(
        validate(
            "act",
            &json!({"tab": "t1", "element": "s1e1", "action": "press", "text": ""})
        )
        .unwrap(),
        "press requires explicit text naming a key"
    );
}

/// The owner's hidden input as an older implementor wrote it: only the task-keyed calls.
struct OldInput(std::sync::Mutex<Vec<String>>);

impl ferb_browser::driver::OwnerInput for OldInput {
    fn ask_code(&self, task: &str, intent: &str, origin: &str) -> BoxFuture<'_, Option<Secret>> {
        self.0
            .lock()
            .unwrap()
            .push(format!("code {task} {intent} {origin}"));
        Box::pin(async { None })
    }
    fn ask_secret(
        &self,
        task: &str,
        intent: &str,
        origin: &str,
        _ask: ferb_browser::driver::SecretAsk,
    ) -> BoxFuture<'_, Option<Secret>> {
        self.0
            .lock()
            .unwrap()
            .push(format!("secret {task} {intent} {origin}"));
        Box::pin(async { None })
    }
}

#[test]
fn both_families_are_built_by_one_constructor_each_and_old_inputs_still_answer() {
    use ferb_browser::driver::OwnerInput;
    use ferb_browser::tools::FamilyParts;
    let (owner, _) = tools(vec![]);
    let parts = || FamilyParts {
        hub: owner.driver.hub.clone(),
        tasks: Tasks::default(),
        wake: None,
        random: Arc::new(Counter::new()),
        policy: Arc::new(NoPolicy),
        host: Arc::new(NoHost),
    };
    let o = Tools::owner_family(
        parts(),
        Arc::new(|_| Err(ferb_browser::Error::refused("none"))),
        None,
    );
    assert_eq!(o.family.prefix, OWNER_BROWSER.prefix);
    assert_eq!(o.driver.identity, Identity::Owner);
    let f = Tools::ferb_family(parts(), Arc::new(|| None), Arc::new(Saved(vec![])));
    assert_eq!(f.family.prefix, FERB_BROWSER.prefix);
    assert_eq!(f.driver.identity, Identity::Ferb);
    let names: Vec<String> = f.verbs().iter().map(|v| f.name(v)).collect();
    assert_eq!(names.len(), ferb_browser::tools::FERB_VERBS.len());
    assert!(names.iter().all(|n| n.starts_with("ferb_browser.")));

    // the context-carrying asks forward to the old ones by default
    let input = OldInput(std::sync::Mutex::new(Vec::new()));
    let ctx = ToolContext {
        task_id: "t-1".into(),
        worker_id: "w-1".into(),
        request_id: "r-1".into(),
        identity: Identity::Owner,
        scopes: Default::default(),
        device_id: "dev-1".into(),
        span_id: None,
        workspace: "/tmp/invented".into(),
        owner_roots: vec![],
        untrusted_roots: vec![],
        intent_id: None,
        observe_only: false,
        probe: false,
        path_closed: None,
        escape: None,
        terminal: None,
        folders: vec![],
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    assert!(rt
        .block_on(input.ask_code_for(&ctx, "i-1", "https://example.test"))
        .is_none());
    assert!(rt
        .block_on(input.ask_secret_for(&ctx, "i-2", "https://example.test", Default::default()))
        .is_none());
    assert_eq!(
        *input.0.lock().unwrap(),
        [
            "code t-1 i-1 https://example.test",
            "secret t-1 i-2 https://example.test"
        ]
    );
}
