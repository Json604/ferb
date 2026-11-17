//! APP-777, HAND-580, HAND-644: a task's default profile is the owner's pick (when it browses as the owner there) or
//! the profiles the owner's words named for it; a call that names any other of the owner's profiles goes there too
//! (HAND-644, owner 2026-11-17: the pick is a default, never a fence). Invented profiles; no browser, extension or
//! store is reached.

mod common;

use std::sync::Arc;

use ferb_browser::bridge::{Hub, Ledger};
use ferb_browser::driver::{Driver, Kind, TaskProfiles, Tasks};
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

/// The owner's driver: no name resolves to the owner's pick (HAND-2), "brave/Work" to another paired profile,
/// "iit" and "chrome/Profile 29" to the IIT profile. By tree: "picked" browses as the owner in their pick; "named"
/// was given the IIT profile by the owner's words (no pick applies); "both" the IIT profile and Brave's; "plus" the
/// pick and, added later, Brave's; "owners" is kept to none.
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
        None | Some("") | Some("chrome/Personal") | Some("chrome/Profile 7") => {
            Ok(("chrome".to_string(), "Profile 7".to_string()))
        }
        Some("brave/Work") | Some("brave/Default") => {
            Ok(("brave".to_string(), "Default".to_string()))
        }
        Some("iit") | Some("chrome/Profile 29") => {
            Ok(("chrome".to_string(), "Profile 29".to_string()))
        }
        Some(other) => Err(ferb_browser::Error::refused(format!(
            "no profile called '{other}'"
        ))),
    });
    let p = |b: &str, f: &str| (b.to_string(), f.to_string());
    let tasks = Tasks {
        task_profiles: Arc::new(move |task: &str| match task.split('.').next() {
            Some("picked") => Some(TaskProfiles {
                pick: true,
                named: vec![],
            }),
            Some("named") => Some(TaskProfiles {
                pick: false,
                named: vec![p("chrome", "Profile 29")],
            }),
            Some("both") => Some(TaskProfiles {
                pick: false,
                named: vec![p("chrome", "Profile 29"), p("brave", "Default")],
            }),
            Some("plus") => Some(TaskProfiles {
                pick: true,
                named: vec![p("brave", "Default")],
            }),
            _ => None,
        }),
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

async fn take(d: &Driver, task: &str, tab_ref: &str) -> String {
    match d.attach(tab_ref, task).await {
        Ok(tab) => format!("took {}", tab.tab_id),
        Err(e) => e.message(),
    }
}

/// Not paired in this test: a call that reaches the profile's bridge says so.
fn reached(b: &str, f: &str) -> String {
    format!("{b}/{f} isn't paired with FERB")
}

// decision: APP-777
// decision: HAND-644
#[tokio::test]
async fn a_task_browsing_in_the_owners_pick_opens_another_profile_when_a_call_names_it() {
    let d = router();
    // HAND-644: the pick is the default, not a fence: a named profile goes to its own bridge (none paired here)
    let other = reached("brave", "Default");
    assert_eq!(open(&d, "picked.worker", Some("brave/Work")).await, other);
    // its checker and its plan steps are the same tree
    assert_eq!(
        open(&d, "picked.step.search.verify", Some("brave/Work")).await,
        other
    );
    // the pick, by its name or by no name, goes on to the pick's own bridge
    let to_pick = reached("chrome", "Profile 7");
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
    // the owner's own tab in another profile is reached the same way (browser.use_tab, as browser.open)
    assert_eq!(
        take(&d, "picked.worker", "brave:Default:12").await,
        "that profile isn't one of the owner's paired profiles"
    );
}

/// The live case (task 0abecb83, 2026-11-16): the owner's words named their IIT profile; a saved pick elsewhere
/// doesn't narrow the task. The named profile opens, by name or with none; the pick and any other profile open
/// when a call names them (HAND-644).
// decision: HAND-580
// decision: HAND-644
#[tokio::test]
async fn a_task_whose_words_named_a_profile_browses_there_whatever_the_pick() {
    let d = router();
    let iit = reached("chrome", "Profile 29");
    assert_eq!(
        open(&d, "named.worker", Some("chrome/Profile 29")).await,
        iit
    );
    assert_eq!(open(&d, "named.worker", Some("iit")).await, iit);
    assert_eq!(
        open(&d, "named.worker", None).await,
        iit,
        "no name: the one named"
    );
    // HAND-644: another of the owner's profiles a call names is taken too
    assert_eq!(
        open(&d, "named.worker", Some("chrome/Personal")).await,
        reached("chrome", "Profile 7")
    );
    assert_eq!(
        open(&d, "named.step.s.verify", Some("brave/Work")).await,
        reached("brave", "Default")
    );
    // the owner's tab is taken wherever it is (none paired here: the bridge says so)
    let unpaired = "that profile isn't one of the owner's paired profiles";
    assert_eq!(
        take(&d, "named.worker", "chrome:Profile 29:5").await,
        unpaired
    );
    assert_eq!(
        take(&d, "named.worker", "chrome:Profile 7:5").await,
        unpaired
    );
}

/// Two profiles named ("both"): either opens, and so does a third a call names; with none named and no tab yet, the
/// call is asked to name one.
// decision: HAND-580
#[tokio::test]
async fn a_task_given_two_profiles_uses_either_and_asks_which_when_none_is_named() {
    let d = router();
    assert_eq!(
        open(&d, "both.worker", Some("iit")).await,
        reached("chrome", "Profile 29")
    );
    assert_eq!(
        open(&d, "both.worker", Some("brave/Work")).await,
        reached("brave", "Default")
    );
    assert_eq!(
        open(&d, "both.worker", None).await,
        "this task may browse in chrome/Profile 29 (Google Chrome “Profile 29”), brave/Default (Brave “Default”), the \
         profiles the owner's words named for it: name one with the profile argument (for example \
         profile=chrome/Profile 29); nothing was opened"
    );
    assert_eq!(
        open(&d, "both.worker", Some("chrome/Personal")).await,
        reached("chrome", "Profile 7")
    );
}

/// A pick task the owner later gave another profile ("use my brave too"): both open, the pick by default.
// decision: HAND-580
#[tokio::test]
async fn a_profile_added_to_a_pick_task_opens_beside_the_pick() {
    let d = router();
    assert_eq!(
        open(&d, "plus.worker", None).await,
        reached("chrome", "Profile 7")
    );
    assert_eq!(
        open(&d, "plus.worker", Some("brave/Work")).await,
        reached("brave", "Default")
    );
    assert_eq!(
        open(&d, "plus.worker", Some("iit")).await,
        reached("chrome", "Profile 29")
    );
}

// decision: HAND-2
#[tokio::test]
async fn a_task_kept_to_no_profile_still_names_its_profile() {
    let d = router();
    assert_eq!(
        open(&d, "owners.worker", Some("brave/Work")).await,
        reached("brave", "Default")
    );
    assert_eq!(
        open(&d, "owners.worker", None).await,
        reached("chrome", "Profile 7")
    );
    assert_eq!(
        take(&d, "owners.worker", "brave:Default:3").await,
        "that profile isn't one of the owner's paired profiles"
    );
}
