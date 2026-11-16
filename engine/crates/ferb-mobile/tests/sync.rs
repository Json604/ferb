//! Sync over `chat.entries.*` by cursor (ENG-10, ENG-14) against a fake Mac built from the fixtures: the newest page
//! first, older pages on demand, catching up after being offline, `reset_required` and `cursor_ahead` read again,
//! `behind` resumes, a cut body is read whole, duplicates and late revisions change nothing, and the public relay's
//! refusal of older pages is a typed state retried once when the path improves (PHN-444), never in a loop.

mod common;

use common::mac::FakeMac;
use common::phone::{online, Phone, OFFLINE};
use common::{conversations, longest, scenarios};
use ferb_mobile::entry::ChatEntry;
use ferb_mobile::{fold, Change, LinkPath, OlderState};
use serde_json::json;
use std::sync::Arc;

fn whole(mac: &FakeMac) -> fold::Transcript {
    fold::transcript(&mac.newest(), 0)
}

fn shown(m: &ferb_mobile::Mobile, c: &str) -> fold::Transcript {
    let v = m.transcript(c).unwrap();
    fold::Transcript {
        items: v.items,
        questions: v.questions,
    }
}

/// Every one-conversation scenario: the first sync shows the newest page; older pages, two entries each, fill the
/// rest; the end equals FERB.app's fold of everything.
// decision: ENG-10
// decision: PHN-408
// decision: ENG-14
// decision: PHN-475
#[test]
fn the_newest_page_first_then_older_pages_on_demand_reach_the_whole_chat() {
    let mut checked = 0;
    for (file, name, entries) in scenarios() {
        if conversations(&entries).len() != 1 || entries.is_empty() {
            continue;
        }
        let mac = FakeMac::with("C1", &entries);
        let mut phone = Phone::new();
        phone.config.page_limit = 2;
        let m = phone.open();
        m.connect(Arc::new(mac.clone())).unwrap();
        m.link_event(&online("direct"));
        assert_eq!(phone.jobs.run(), 1, "coming online starts one sync");
        assert_eq!(m.current_conversation().unwrap().as_deref(), Some("C1"));
        let mut older = m.transcript("C1").unwrap().older;
        let mut pages = 0;
        while older == OlderState::Available {
            older = m.load_older("C1").unwrap();
            pages += 1;
            assert!(pages < 50, "{file} {name}: paging ends");
        }
        assert_eq!(older, OlderState::Complete);
        assert_eq!(shown(&m, "C1"), whole(&mac), "{file} {name}");
        assert!(m.link_state().watching);
        checked += 1;
    }
    assert!(checked > 100);
}

/// Offline while the Mac goes on (new entries, revisions, a tombstone); back online, the changes after the cursor
/// bring the phone to the Mac's chat; then the watch keeps it there live.
// decision: ENG-10
// decision: ENG-14
// decision: PHN-408
// decision: PHN-475
#[test]
fn catching_up_after_offline_then_watching_live() {
    let (name, entries) = longest();
    let half = entries.len() / 2;
    let mac = FakeMac::with("C1", &entries[..half]);
    let phone = Phone::new();
    let m = phone.open();
    m.connect(Arc::new(mac.clone())).unwrap();
    m.link_event(&online("direct"));
    phone.jobs.run();
    m.link_event(OFFLINE);
    assert_eq!(m.link_state().path, LinkPath::Offline);
    assert!(!m.link_state().watching);
    // the Mac goes on while the phone is away
    mac.lock().online = false;
    for e in &entries[half..] {
        mac.commit(e.clone());
    }
    let mut tomb = entries[0].clone();
    tomb.rev += 10;
    tomb.deleted = true;
    tomb.body = Default::default();
    mac.commit(tomb);
    mac.lock().online = true;
    m.link_event(&online("own_relay"));
    phone.jobs.run();
    assert_eq!(shown(&m, "C1"), whole(&mac), "{name}: caught up");
    assert!(mac
        .calls_of("chat.entries.changes")
        .iter()
        .all(|p| p["epoch"] == "E1"));
    // live: a new revision on the watch
    let mut revised = entries[entries.len() - 1].clone();
    revised.rev += 20;
    revised
        .body
        .insert("text".into(), json!("revised on the Mac"));
    mac.commit(revised);
    assert_eq!(shown(&m, "C1"), whole(&mac), "{name}: live");
    assert!(phone.heard.0.lock().unwrap().contains(&Change::Transcript {
        conversation: "C1".into()
    }));
}

/// A cursor of another epoch (a new store on the Mac) and one past the head both read again from the newest page.
// decision: ENG-14
// decision: PHN-475
#[test]
fn reset_required_and_cursor_ahead_read_again() {
    let (_, entries) = longest();
    let mac = FakeMac::with("C1", &entries);
    let phone = Phone::new();
    let m = phone.open();
    m.connect(Arc::new(mac.clone())).unwrap();
    m.link_event(&online("direct"));
    phone.jobs.run();
    // a new store on the Mac: another epoch, other entries
    {
        let mut st = mac.lock();
        st.epoch = "E2".into();
        st.entries.clear();
        st.head = 0;
        st.sink = None;
    }
    mac.commit(entries[0].clone());
    m.sync().unwrap();
    assert_eq!(
        shown(&m, "C1"),
        whole(&mac),
        "reset_required: what was held is dropped"
    );
    // a cursor past the head (the Mac's log went back): read again
    {
        let mut st = mac.lock();
        st.head = 0;
        st.sink = None;
    }
    let before = mac.calls_of("chat.entries.page").len();
    m.link_event(OFFLINE);
    m.link_event(&online("direct"));
    phone.jobs.run();
    assert!(
        mac.calls_of("chat.entries.page").len() > before + 1,
        "cursor_ahead: the newest page again"
    );
    assert_eq!(shown(&m, "C1"), whole(&mac));
}

/// `behind` ends the watch; the phone reads the changes from its cursor and watches again (once).
// decision: ENG-14
// decision: PHN-475
#[test]
fn behind_reads_the_changes_and_watches_again() {
    let (_, entries) = longest();
    let half = entries.len() / 2;
    let mac = FakeMac::with("C1", &entries[..half]);
    let phone = Phone::new();
    let m = phone.open();
    m.connect(Arc::new(mac.clone())).unwrap();
    m.link_event(&online("direct"));
    phone.jobs.run();
    assert_eq!(mac.lock().watches, 1);
    mac.behind();
    for e in &entries[half..] {
        mac.commit(e.clone()); // no watch: not delivered
    }
    assert_eq!(phone.jobs.pending(), 1, "one resync, no loop");
    phone.jobs.run();
    assert_eq!(mac.lock().watches, 2);
    assert_eq!(shown(&m, "C1"), whole(&mac));
}

/// An entry a reply cut (`body_cut`) is never folded cut: the phone reads it whole, piece by piece.
// decision: ENG-14
// decision: PHN-475
#[test]
fn a_cut_body_is_read_whole() {
    let (_, entries) = longest();
    let mac = FakeMac::with("C1", &entries);
    mac.lock().cut_over = Some(5);
    let mut phone = Phone::new();
    phone.config.piece_chars = 4;
    let m = phone.open();
    m.connect(Arc::new(mac.clone())).unwrap();
    m.link_event(&online("direct"));
    phone.jobs.run();
    assert!(!mac.calls_of("chat.entries.get").is_empty());
    assert_eq!(shown(&m, "C1"), whole(&mac));
    // a cut one on the watch, too
    let mut long = entries[entries.len() - 1].clone();
    long.rev += 3;
    long.body.insert(
        "text".into(),
        json!("a long answer that the reply had to cut"),
    );
    mac.commit(long);
    phone.jobs.run();
    assert_eq!(shown(&m, "C1"), whole(&mac));
}

/// The same changes again, and older revisions late, change nothing.
// decision: ENG-10
// decision: PHN-475
#[test]
fn duplicate_and_out_of_order_delivery_change_nothing() {
    let (_, entries) = longest();
    let mac = FakeMac::with("C1", &entries);
    let phone = Phone::new();
    let m = phone.open();
    m.connect(Arc::new(mac.clone())).unwrap();
    m.link_event(&online("direct"));
    phone.jobs.run();
    let before = shown(&m, "C1");
    let sink = mac.lock().sink.clone().unwrap();
    for e in mac.newest().iter().rev() {
        sink.frame(json!({"type": "change", "entry": e.to_json()}).to_string());
        let mut old = e.clone();
        old.rev -= 1;
        old.seq -= 1;
        old.body.insert("text".into(), json!("stale"));
        sink.frame(json!({"type": "change", "entry": old.to_json()}).to_string());
    }
    assert_eq!(shown(&m, "C1"), before);
}

/// On the public relay an older page is refused before anything is asked (PHN-444): a typed state with the words
/// to show. When the link says the path carries it, it is fetched once, by itself.
// decision: PHN-444
// decision: PHN-410
// decision: PHN-461
// decision: PHN-475
#[test]
fn older_pages_on_the_public_relay_wait_for_a_better_path_without_a_loop() {
    let (_, entries) = longest();
    let mac = FakeMac::with("C1", &entries);
    mac.lock().public_relay = true;
    let mut phone = Phone::new();
    phone.config.page_limit = 2;
    let m = phone.open();
    m.connect(Arc::new(mac.clone())).unwrap();
    m.link_event(&online("shared_relay"));
    phone.jobs.run();
    assert!(!m.link_state().bulk_allowed);
    assert_eq!(m.transcript("C1").unwrap().older, OlderState::Available);
    let older = m.load_older("C1").unwrap();
    assert!(matches!(older, OlderState::NeedsBetterPath { .. }));
    assert!(matches!(
        m.load_older("C1").unwrap(),
        OlderState::NeedsBetterPath { .. }
    ));
    let asked = mac
        .calls_of("chat.entries.page")
        .iter()
        .filter(|p| p.get("before_pos").is_some())
        .count();
    assert_eq!(asked, 0, "nothing asked of the Mac");
    assert_eq!(
        phone.jobs.pending(),
        0,
        "no retry by itself on the same path"
    );
    // the path improves
    mac.lock().public_relay = false;
    m.link_event(r#"{"type":"bulk","allowed":true,"tier":"direct"}"#);
    assert_eq!(phone.jobs.pending(), 1, "offered again once");
    phone.jobs.run();
    assert_eq!(
        mac.calls_of("chat.entries.page")
            .iter()
            .filter(|p| p.get("before_pos").is_some())
            .count(),
        1
    );
    assert!(m.transcript("C1").unwrap().older != OlderState::Unknown);
    m.link_event(r#"{"type":"bulk","allowed":true,"tier":"direct"}"#);
    assert_eq!(phone.jobs.pending(), 0, "a second event retries nothing");
}

/// The watch's live frames show over the transcript and go when the link does (nothing live after a restart).
// decision: ENG-8
// decision: PHN-475
#[test]
fn live_frames_from_the_watch_show_and_go_with_the_link() {
    let entries: Vec<ChatEntry> = ChatEntry::list(&json!([{
        "id": "p1", "conversation_id": "C1", "pos": 4294967296u64, "seq": 1, "rev": 1, "kind": "message",
        "state": "working", "open": true, "request_id": "r1", "created_at": "2026-01-01T09:00:00+00:00",
        "updated_at": "2026-01-01T09:00:00+00:00", "day": "2026-01-01",
        "body": {"text": "look it up", "attachments": [], "hidden": []}, "deleted": false}]));
    let mac = FakeMac::with("C1", &entries);
    let phone = Phone::new();
    let m = phone.open();
    m.connect(Arc::new(mac.clone())).unwrap();
    m.link_event(&online("direct"));
    phone.jobs.run();
    assert!(m.transcript("C1").unwrap().live[0].typing);
    mac.live(
        "r1",
        json!({"type": "progress", "planning": true, "text": "Reading the page"}),
    );
    assert_eq!(
        m.transcript("C1").unwrap().live[0]
            .line
            .as_ref()
            .map(|l| l.text.clone()),
        Some("Reading the page".into())
    );
    assert!(phone.heard.0.lock().unwrap().contains(&Change::Live {
        conversation: "C1".into()
    }));
    m.link_event(OFFLINE);
    assert!(
        m.transcript("C1").unwrap().live[0].line.is_none(),
        "nothing live after the link went"
    );
}

/// The fold's zone decides whether a later answer said after midnight quotes its turn.
// decision: APP-914
// decision: PHN-473
#[test]
fn a_later_answer_on_another_day_quotes_its_turn() {
    let e = |id: &str, pos: i64, kind: &str, at: &str, body: serde_json::Value| {
        json!({"id": id, "conversation_id": "C1", "pos": pos, "seq": pos, "rev": 1, "kind": kind,
               "state": "done", "open": false, "request_id": "r1", "answers": if kind == "answer" { json!("m1") } else { json!(null) },
               "placement": if kind == "answer" { json!("inline") } else { json!(null) },
               "created_at": at, "updated_at": at, "day": "", "body": body, "deleted": false})
    };
    let entries = ChatEntry::list(&json!([
        e(
            "m1",
            1,
            "message",
            "2026-01-01T23:00:00+00:00",
            json!({"text": "hi"})
        ),
        e(
            "a1",
            2,
            "answer",
            "2026-01-01T23:10:00+00:00",
            json!({"text": "Hello", "status": "done"})
        ),
        e(
            "a2",
            3,
            "answer",
            "2026-01-02T00:30:00+00:00",
            json!({"text": "More", "status": "done", "follows": "a1"})
        ),
    ]));
    let quotes = |tz: i32| match &fold::transcript(&entries, tz).items[1] {
        fold::Item::Reply(r) => r.quotes_request,
        other => panic!("{other:?}"),
    };
    assert!(quotes(0), "said the next day in UTC: quoted");
    assert!(
        !quotes(-3600),
        "the same day an hour west: right under its turn"
    );
}
