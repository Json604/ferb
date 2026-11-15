//! APP-777: a task that browses as the owner because FERB has no profile of its own opens tabs only in the
//! profile the owner picked for FERB's web work. A profile the worker names that resolves elsewhere is refused
//! before anything opens; a task the planner gave the owner's identity itself keeps naming any paired profile
//! (HAND-2). Invented profiles; no browser, extension or store is reached.

mod common;

use std::sync::Arc;

use ferb_browser::bridge::{Hub, Ledger};
use ferb_browser::driver::{Driver, Kind, Tasks};
use ferb_browser::pairing::{InstallRow, InstallStore, PairingKey, Pairings, StoreError};
use ferb_browser::rules::{BrowserApps, BrowserInfo};
use ferb_core::interfaces::{BoxFuture, Clock};
use ferb_core::when::DateTime;
use serde_json::{Map, Value};

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

/// The owner's driver: no name resolves to the owner's pick (HAND-2), "brave/Work" to another paired profile.
/// Tasks of tree "picked" browse as the owner in their pick; tree "owners" is a task the planner gave the
/// owner's identity.
fn router() -> Arc<Driver> {
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
    let choose = Arc::new(|named: Option<&str>| match named {
        None | Some("") | Some("chrome/Personal") => {
            Ok(("chrome".to_string(), "Profile 7".to_string()))
        }
        Some("brave/Work") => Ok(("brave".to_string(), "Default".to_string())),
        Some(other) => Err(ferb_browser::Error::refused(format!(
            "no profile called '{other}'"
        ))),
    });
    let tasks = Tasks {
        browses_as_owner: Arc::new(|task: &str| task.split('.').next() == Some("picked")),
        ..Tasks::default()
    };
    Driver::new(
        hub,
        Kind::Owner {
            choose,
            input: None,
        },
        tasks,
        None,
        random,
    )
}

async fn open(d: &Driver, task: &str, profile: Option<&str>) -> String {
    match d
        .open_for("https://example.com/", task, profile, None, false)
        .await
    {
        Ok(tab) => format!("opened {}", tab.tab_id),
        Err(e) => e.message(),
    }
}

// decision: APP-777
#[tokio::test]
async fn a_task_browsing_in_the_owners_pick_never_opens_in_another_profile() {
    let d = router();
    // Fixed against Python (ext_bridge.py:3016 Router.open_for took the named profile): refused, nothing opened.
    let refused = open(&d, "picked.worker", Some("brave/Work")).await;
    assert_eq!(
        refused,
        "brave/Default isn't the profile this task browses in: FERB has no browser profile of its own, and the \
         owner picked chrome/Profile 7 for FERB's web work, so this task's tabs open there. Leave profile out (or \
         name chrome/Profile 7); nothing was opened"
    );
    // its checker and its plan steps are the same tree
    assert_eq!(
        open(&d, "picked.step.search.verify", Some("brave/Work")).await,
        refused
    );
    // the pick, by its name or by no name, goes on to the pick's own bridge (none paired in this test)
    let to_pick = "chrome/Profile 7 isn't paired with FERB";
    assert_eq!(
        open(&d, "picked.worker", Some("chrome/Personal")).await,
        to_pick
    );
    assert_eq!(open(&d, "picked.worker", None).await, to_pick);
    assert_eq!(open(&d, "picked.worker", Some("")).await, to_pick);
    // a name that resolves to nothing keeps the resolver's own words
    assert_eq!(
        open(&d, "picked.worker", Some("nowhere")).await,
        "no profile called 'nowhere'"
    );
}

// decision: HAND-2
#[tokio::test]
async fn a_task_the_planner_gave_the_owner_still_names_its_profile() {
    let d = router();
    assert_eq!(
        open(&d, "owners.worker", Some("brave/Work")).await,
        "brave/Default isn't paired with FERB"
    );
    assert_eq!(
        open(&d, "owners.worker", None).await,
        "chrome/Profile 7 isn't paired with FERB"
    );
}
