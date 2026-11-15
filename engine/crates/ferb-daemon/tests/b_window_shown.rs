//! A browser window that came up during a task is told to the owner (owner, HAND-545: notify, never wait), in one
//! line per task and site written from the facts alone (HAND-546), as a quiet notice: published `attention.notice`,
//! journaled `chat.notice`, kept in the notices; never a banner while a FERB client shows notices. Invented facts and
//! sites, a temporary home.

mod b_support;

use std::sync::{Arc, Mutex};

use b_support::*;
use ferb_daemon::areas_b::{banners, notify};
use ferb_rpc::BoxFuture;
use serde_json::{json, Map, Value};

fn store(home: &Home) -> notify::SharedDb {
    let db = ferb_store::Db::open(home.path().join("ferb.db")).unwrap();
    db.migrate().unwrap();
    Arc::new(Mutex::new(db))
}

struct Desk;

impl notify::Desk for Desk {
    fn osascript(&self, _script: &str) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async { Err("no banner in this test".into()) })
    }
    fn away(&self) -> bool {
        false
    }
}

/// A connected FERB client shows every notice.
struct Shown;

impl notify::Listeners for Shown {
    fn client_shows(&self, _topic: &str) -> bool {
        true
    }
}

fn fact(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap()
}

/// The line for each kind of fact, as the owner reads it.
// decision: HAND-546
#[test]
fn each_fact_reads_as_one_plain_line_from_structure_alone() {
    let line = |v: Value, site: &str| notify::window_shown_line(&fact(v), site);
    let site = "app.example.edu";
    let cases = [
        (
            json!({"cause": "popup_window", "window": "popup", "put_back": true}),
            "A pop-up window opened for a moment on app.example.edu; I minimized it.",
        ),
        (
            json!({"cause": "popup_window", "window": "popup", "focused": true, "put_back": true}),
            "A pop-up window opened for a moment on app.example.edu and took your keyboard focus; I minimized it.",
        ),
        (
            json!({"cause": "popup_window", "window": "popup", "put_back": false}),
            "A pop-up window opened on app.example.edu; I couldn't minimize it.",
        ),
        (
            json!({"cause": "new_tab", "window": "ferb", "put_back": true}),
            "My browser window came up for a moment when app.example.edu opened a new tab; I minimized it.",
        ),
        (
            json!({"cause": "dialog", "window": "ferb", "focused": true, "put_back": true}),
            "My browser window came up for a moment and took your keyboard focus when app.example.edu showed a \
             dialog; I minimized it.",
        ),
        (
            json!({"cause": "dialog", "window": "popup", "put_back": true}),
            "A pop-up window came up for a moment when app.example.edu showed a dialog; I minimized it.",
        ),
        (
            json!({"cause": "new_tab", "window": "owner", "focused": true}),
            "Your browser window came to the front and took your keyboard focus when app.example.edu opened a new \
             tab; I left it as it was.",
        ),
        (
            json!({"cause": "front_app", "app": "Google Chrome"}),
            "Google Chrome came to the front while I was working on app.example.edu.",
        ),
    ];
    for (f, want) in cases {
        assert_eq!(line(f.clone(), site).as_deref(), Some(want), "{f}");
    }
    // without a site
    assert_eq!(
        line(
            json!({"cause": "popup_window", "window": "popup", "put_back": true}),
            ""
        )
        .as_deref(),
        Some("A pop-up window opened for a moment; I minimized it.")
    );
    assert_eq!(
        line(
            json!({"cause": "new_tab", "window": "ferb", "put_back": true}),
            ""
        )
        .as_deref(),
        Some(
            "My browser window came up for a moment when a page opened a new tab; I minimized it."
        )
    );
    // a request the extension refused brought nothing forward; an unknown kind says nothing
    assert_eq!(
        line(json!({"cause": "focus", "prevented": true}), site),
        None
    );
    assert_eq!(
        line(json!({"cause": "made_up", "window": "ferb"}), site),
        None
    );
    // a window's purpose is never claimed: the type is the only fact
    let said = line(
        json!({"cause": "popup_window", "window": "popup", "put_back": true}),
        "accounts.example.com",
    )
    .unwrap();
    assert!(!said.to_lowercase().contains("sign"), "{said}");
}

/// An app that came to the front during one of FERB's calls: the Mac's facts (HAND-549: what FERB did in it, whether it
/// put the owner's app back) and a browser call's own browser (HAND-560), each read from its fields only.
// decision: HAND-560
#[test]
fn an_app_that_came_forward_reads_with_what_ferb_did_and_whether_it_put_yours_back() {
    let line = |v: Value, site: &str| notify::window_shown_line(&fact(v), site).unwrap();
    let mac = |action: &str, put_back: bool, why: Option<&str>| {
        let mut f = json!({"cause": "front_app", "app": "WhatsApp", "put_back": put_back, "previous": "Notes",
                           "input": "raw", "action": action});
        if let Some(w) = why {
            f["why"] = json!(w);
        }
        line(f, "")
    };
    assert_eq!(
        mac("click", true, None),
        "WhatsApp came to the front when I clicked in it; I put Notes back."
    );
    assert_eq!(
        mac("type", true, None),
        "WhatsApp came to the front when I typed in it; I put Notes back."
    );
    assert_eq!(
        mac("scroll", false, None),
        "WhatsApp came to the front when I scrolled in it; I left it in front."
    );
    assert_eq!(
        mac("use", false, Some("another app is in front now")),
        "WhatsApp came to the front while I was working in it; I couldn't put Notes back: another app is in front now."
    );
    assert_eq!(
        mac("drag", false, Some("it has quit")),
        "WhatsApp came to the front when I dragged in it; I couldn't put Notes back: it has quit."
    );
    // a browser call's own browser
    assert_eq!(
        line(
            json!({"cause": "front_app", "app": "Google Chrome", "put_back": true, "previous": "Brave Browser"}),
            "app.example.edu"
        ),
        "Google Chrome came to the front while I was working on app.example.edu; I put Brave Browser back."
    );
    assert_eq!(
        line(
            json!({"cause": "front_app", "app": "Google Chrome", "put_back": false, "previous": "Brave Browser",
                   "why": "another app is in front now"}),
            "app.example.edu"
        ),
        "Google Chrome came to the front while I was working on app.example.edu; I couldn't put Brave Browser back: \
         another app is in front now."
    );
}

/// A Mac app's line (no site) is once per task and app: another app of the same task gets its own.
// decision: HAND-560
#[tokio::test(flavor = "multi_thread")]
async fn a_mac_apps_line_is_once_per_task_and_app() {
    let home = Home::new("");
    let db = store(&home);
    let notifier = notify::Notifier {
        host: home.host.clone(),
        posters: banners::Posters::default(),
        desk: Arc::new(Desk),
        listeners: Arc::new(Shown),
        notices: Some(db.clone()),
        held: Mutex::new(vec![]),
        uid: Arc::new(|| "0001".into()),
    };
    let came = |app: &str| {
        json!({"task_id": "tree-2.1", "site": "",
               "shown": [{"cause": "front_app", "app": app, "put_back": true, "previous": "Notes",
                          "input": "raw", "action": "click"}]})
    };
    let mut told = vec![];
    for app in ["WhatsApp", "WhatsApp", "Mail"] {
        told.push(
            notifier
                .tell_window_shown(came(app).as_object().unwrap())
                .await
                .unwrap(),
        );
    }
    assert_eq!(
        told,
        vec![
            Some((
                "window-shown-tree-2-app:WhatsApp".to_string(),
                "WhatsApp came to the front when I clicked in it; I put Notes back.".to_string()
            )),
            None,
            Some((
                "window-shown-tree-2-app:Mail".to_string(),
                "Mail came to the front when I clicked in it; I put Notes back.".to_string()
            )),
        ]
    );
}

/// Once per task tree and site, as a notice in chat (no banner while a client shows notices), whatever step of the
/// tree it came from; another site of the same task, or another task, gets its own line.
// decision: HAND-545
// decision: HAND-546
#[tokio::test(flavor = "multi_thread")]
async fn the_owner_is_told_once_per_task_and_site_as_a_quiet_notice() {
    let home = Home::new("");
    let db = store(&home);
    let notifier = notify::Notifier {
        host: home.host.clone(),
        posters: banners::Posters::default(),
        desk: Arc::new(Desk),
        listeners: Arc::new(Shown),
        notices: Some(db.clone()),
        held: Mutex::new(vec![]),
        uid: Arc::new(|| "0001".into()),
    };
    let popup = json!({"task_id": "tree-1.2", "tab": "t1", "site": "app.example.edu",
                       "shown": [{"cause": "focus", "prevented": true},
                                 {"cause": "popup_window", "window": "popup", "put_back": true}]});
    let told = notifier
        .tell_window_shown(popup.as_object().unwrap())
        .await
        .unwrap();
    assert_eq!(
        told,
        Some((
            "window-shown-tree-1-app.example.edu".to_string(),
            "A pop-up window opened for a moment on app.example.edu; I minimized it.".to_string()
        ))
    );
    // the same task and site again (another step, the call's own browser at the front): nothing more
    let again = json!({"task_id": "tree-1", "site": "app.example.edu",
                       "shown": [{"cause": "front_app", "app": "Google Chrome"}]});
    assert_eq!(
        notifier
            .tell_window_shown(again.as_object().unwrap())
            .await
            .unwrap(),
        None
    );
    // only a refused request: nothing to tell
    let refused = json!({"task_id": "tree-1", "site": "other.example",
                         "shown": [{"cause": "fullscreen", "prevented": true}]});
    assert_eq!(
        notifier
            .tell_window_shown(refused.as_object().unwrap())
            .await
            .unwrap(),
        None
    );
    // another site of the task
    let other = json!({"task_id": "tree-1", "site": "other.example",
                       "shown": [{"cause": "front_app", "app": "Google Chrome"}]});
    assert!(notifier
        .tell_window_shown(other.as_object().unwrap())
        .await
        .unwrap()
        .is_some());
    let notices: Vec<Value> = home
        .events()
        .into_iter()
        .filter(|e| e[0] == "attention.notice")
        .map(|e| e[1].clone())
        .collect();
    assert_eq!(notices.len(), 2, "{notices:?}");
    assert_eq!(
        notices[0]["id"],
        json!("window-shown-tree-1-app.example.edu")
    );
    assert_eq!(
        notices[1]["text"],
        json!("Google Chrome came to the front while I was working on other.example.")
    );
    assert!(notices.iter().all(|n| n.get("banner").is_none()));
    // kept with the notices, tied to the task
    let refs: String = db
        .lock()
        .unwrap()
        .conn()
        .query_row(
            "SELECT source_refs_json FROM attention_notices WHERE id = 'window-shown-tree-1-app.example.edu'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(refs, r#"["task:tree-1"]"#);
    let journaled = home
        .journal_rows()
        .iter()
        .filter(|r| r[0] == "chat.notice")
        .count();
    assert_eq!(journaled, 2);
}
