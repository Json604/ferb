//! SENSE-432 (front door step 3d; owner, 2026-11-26: "oh, remove it from phone msg"): a message sent from a paired phone
//! carries the Mac's front window (its app and title) but no picture of the Mac's screen, and its window block says
//! plainly why; a message from FERB.app on the Mac keeps its picture, also when it is read together with one from the
//! phone. Scripted on the recorded plain_reply scenario through the real supervisor with stand-in models and a
//! stand-in capture at send (temp store, injected ids and time; no screen is read).

mod common;
mod support_control;

use ferb_supervisor::flow::FrontSeen;
use ferb_supervisor::words::PHONE_NO_PICTURE;
use serde_json::{json, Value};
use support_control::{run_with, world_with, World};

const WINDOW: &str = "INVENTED-WINDOW-TITLE";

fn reply(text: &str) -> Value {
    let said = json!({"kind": "reply", "answers_question": false, "task": null, "control": null,
                      "side_entries": null, "text": text})
    .to_string();
    json!({"key": "supervisor", "events": [
        {"kind": "usage", "data": {"input_tokens": 10, "output_tokens": 5}},
        {"kind": "final", "data": {"text": said}}]})
}

fn world() -> World {
    let w = world_with("plain_reply", |calls| calls.clear());
    ferb_store::phones::add(
        &w.db.lock().unwrap(),
        &ferb_store::phones::NewPhone {
            endpoint: "e0e0e0".into(),
            name: "Invented Phone".into(),
            app: "android".into(),
            ..Default::default()
        },
        "2026-11-17T22:00:00+05:30",
        5,
    )
    .unwrap()
    .unwrap();
    *w.daemon.capture.lock().unwrap() = Some(FrontSeen {
        facts:
            json!({"app": "TextEdit", "bundle_id": "com.apple.TextEdit", "window_title": WINDOW})
                .as_object()
                .unwrap()
                .clone(),
        image: Some(ferb_core::types::ModelImage::new(
            vec![1, 2, 3],
            "evidence:shot-1",
        )),
        origin: "external:unlabeled".into(),
    });
    w
}

/// Sends each (client, text) on the chat `s1` at once, one after another before any reply; every planner call made
/// (its parts, how many images it was given).
fn send(w: &mut World, messages: &[(&str, &str)]) -> Vec<(Vec<String>, u64)> {
    {
        // a turn read again with a later message reads the recorded one's environment and facts too
        let mut q = w.daemon.supervise.lock().unwrap();
        let last = q.last().cloned().unwrap();
        q.push(last);
    }
    let _ = run_with(w, "plain_reply", |steps| {
        steps.retain(|st| st["do"] != "stream" && st["do"] != "stream_end");
        let at = steps.iter().position(|st| st["do"] == "close").unwrap();
        let mut ours =
            vec![json!({"do": "calls_from_here", "calls": [reply("Done."), reply("Done.")]})];
        for (n, (client, text)) in messages.iter().enumerate() {
            let sent = json!({"jsonrpc": "2.0", "id": n + 2, "method": "chat.send",
                              "params": {"session_id": "s1", "text": text, "client_id": client}});
            ours.push(json!({"do": "stream", "conn": format!("c{n}"), "lines": [{"send": sent.to_string()}]}));
        }
        for n in 0..messages.len() {
            ours.push(json!({"do": "stream_end", "conn": format!("c{n}"), "lines": []}));
        }
        for (i, st) in ours.into_iter().enumerate() {
            steps.insert(at + i, st);
        }
    });
    let started = w.daemon.calls.lock().unwrap().started.clone();
    started
        .iter()
        .map(|c| {
            let parts = c["parts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p.as_str().unwrap_or_default().to_string())
                .collect::<Vec<String>>();
            (parts, c["images"].as_u64().unwrap())
        })
        .filter(|(p, _)| p.iter().any(|t| t.starts_with("Layer 5 — conversation:")))
        .collect()
}

fn windows(parts: &[String]) -> Vec<&String> {
    parts.iter().filter(|p| p.contains(WINDOW)).collect()
}

// decision: SENSE-432
#[test]
fn a_message_from_the_phone_carries_the_window_but_no_picture() {
    let mut w = world();
    let calls = send(&mut w, &[("phone:e0e0e0", "what's on my screen?")]);
    let (parts, images) = calls.last().expect("a planner call");
    assert_eq!(*images, 0, "no picture of the Mac's screen");
    let shown = windows(parts);
    assert_eq!(shown.len(), 1, "{parts:#?}");
    assert!(
        shown[0].contains("TextEdit"),
        "the window's app and title go: {}",
        shown[0]
    );
    assert!(
        shown[0].contains(&format!("withheld: {PHONE_NO_PICTURE}")),
        "and why there is no picture: {}",
        shown[0]
    );
}

// decision: SENSE-432
#[test]
fn a_message_from_the_mac_keeps_its_picture() {
    let mut w = world();
    let calls = send(&mut w, &[("mac-app-install-1", "what's on my screen?")]);
    let (parts, images) = calls.last().expect("a planner call");
    assert_eq!(*images, 1);
    assert!(windows(parts)[0].contains("attached to this request as an image"));
}

// decision: SENSE-432
#[test]
fn messages_read_together_keep_the_mac_ones_picture() {
    let mut w = world();
    let calls = send(
        &mut w,
        &[
            ("mac-app-install-1", "look at this window"),
            ("phone:e0e0e0", "and tell me what it says"),
        ],
    );
    let (parts, images) = calls.last().expect("a planner call");
    // read together (the second message's row says so), one window, the Mac message's picture
    let together = {
        let db = w.db.lock().unwrap();
        let n: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM journal WHERE kind = 'chat.message' AND json_extract(data_json, '$.read_with') IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        n
    };
    assert_eq!(together, 1, "{parts:#?}");
    assert_eq!(*images, 1, "{parts:#?}");
    let shown = windows(parts);
    assert_eq!(shown.len(), 1, "{parts:#?}");
    assert!(
        shown[0].contains("attached to this request as an image"),
        "{}",
        shown[0]
    );
}
