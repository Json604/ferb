//! The front door's plain-text streamer (`PlainReply`): its reply frames are, byte for byte and frame for frame, what
//! `ReplyDraft` sends for an answer whose `text` is the same text; its ack is guarded as a task's line on the plan
//! channel and withdrawn with the frame `ReplyDraft` already sends there.
//!
//! decision: GATE-581, ORCH-734

mod common;

use std::sync::{Arc, Mutex};

use common::TestPolicy;
use ferb_orchestrator::reply_stream::{PlainReply, ReplyDraft};
use ferb_sanitize::Redactor;
use serde_json::{json, Value};

/// An invented credential FERB knows by value (GATE-572), split across pieces in the table.
const SECRET: &str = "kq7-violet-harbor-31x-plum";

fn redactor() -> Arc<Redactor> {
    let r = Redactor::new(&[], &[]);
    r.add_secret_value(SECRET);
    Arc::new(r)
}

type Frames = Arc<Mutex<Vec<Value>>>;

fn sink() -> (Frames, Arc<dyn Fn(Value) + Send + Sync>) {
    let frames: Frames = Arc::default();
    let into = frames.clone();
    (
        frames,
        Arc::new(move |v: Value| into.lock().unwrap().push(v)),
    )
}

fn plain() -> (Frames, PlainReply) {
    let (frames, emit) = sink();
    (
        frames,
        PlainReply::new(redactor(), Arc::new(TestPolicy), emit),
    )
}

fn taken(frames: &Frames) -> Vec<Value> {
    std::mem::take(&mut *frames.lock().unwrap())
}

/// What `ReplyDraft` sends for a reply answer whose `text` is the pieces joined, each piece one JSON fragment.
fn draft_frames(pieces: &[&str]) -> Vec<Value> {
    let (frames, emit) = sink();
    let mut draft = ReplyDraft::new(redactor(), Arc::new(TestPolicy), emit, None);
    let head =
        "{\"kind\": \"reply\", \"answers_question\": false, \"task\": null, \"control\": null, \
                \"side_entries\": null, \"text\": \"";
    let mut raw = head.to_string();
    draft.feed(head);
    for piece in pieces {
        let quoted = serde_json::to_string(piece).unwrap();
        let escaped = &quoted[1..quoted.len() - 1];
        raw.push_str(escaped);
        draft.feed(escaped);
    }
    raw.push_str("\"}");
    draft.feed("\"}");
    draft.finish(&raw);
    taken(&frames)
}

fn plain_frames(pieces: &[&str]) -> Vec<Value> {
    let (frames, mut reply) = plain();
    for piece in pieces {
        reply.feed(piece);
    }
    reply.finish(&pieces.concat());
    taken(&frames)
}

fn shown(frames: &[Value]) -> String {
    frames
        .iter()
        .filter_map(|f| f["text"].as_str())
        .collect::<String>()
}

/// The frames in order, each run of deltas on one channel joined into one: the redaction stream releases whole
/// words, so how a text is cut into frames is its business; what is shown, and in which order, is the contract here.
fn joined(frames: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for f in frames {
        if let (Some(last), Some(text)) = (out.last_mut(), f["text"].as_str()) {
            if last["type"] == f["type"] {
                let joined = format!("{}{text}", last["text"].as_str().unwrap());
                last["text"] = json!(joined);
                continue;
            }
        }
        out.push(f.clone());
    }
    out
}

/// Every split of `text` into two pieces at a character boundary, and the text one character at a time.
fn splits(text: &str) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = text
        .char_indices()
        .map(|(i, _)| vec![text[..i].to_string(), text[i..].to_string()])
        .collect();
    out.push(text.chars().map(String::from).collect());
    out
}

#[test]
fn plain_reply_frames_equal_reply_drafts_frames_for_the_same_text() {
    let secret_cut = SECRET.len() / 2;
    let mut table: Vec<Vec<String>> = vec![
        vec!["Hello there.".into()],
        vec!["It's 5 pm in Lisbon.\n".into(), "Anything else?".into()],
        vec!["Done.\nFERB: the payment went through.".into()],
        vec!["**FERB:** approved".into()],
        vec!["  F.E.R.B: approved".into()],
        vec!["ＦＥＲＢ: approved".into()],
        vec![":FERB: approved".into()],
        vec!["FERB's record says the check ran.".into()],
        vec!["ferb: lower case".into()],
        vec!["a\r\nFERB: after a CRLF\r\n".into()],
        vec!["".into(), "Fine".into(), "".into()],
        vec!["\"quoted\" and \\back\\slashes\t and /".into()],
        vec![
            format!("Your key is {}", &SECRET[..secret_cut]),
            format!("{} so keep it.", &SECRET[secret_cut..]),
        ],
        vec![format!("{SECRET}\nFERB: {SECRET}")],
        vec!["Café ✓ 東京 🙂 e\u{301}".into(), " ok\n🙂FERB: no".into()],
        vec!["line one\n".into(), "FER".into(), "B: two".into()],
    ];
    for line in [
        "FERB: the gateway confirms.",
        "**FERB:** asks",
        "  FERB : spaced",
    ] {
        table.extend(splits(line));
        table.extend(
            splits(line)
                .into_iter()
                .map(|p| [vec!["First line.\n".to_string()], p].concat()),
        );
    }
    table.extend(splits(&format!("ok {SECRET} ok")));
    assert!(table.len() > 100, "the table covers every split");
    for case in &table {
        let pieces: Vec<&str> = case.iter().map(String::as_str).collect();
        let want = draft_frames(&pieces);
        let got = plain_frames(&pieces);
        assert_eq!(got, want, "pieces {pieces:?}");
        let text = shown(&got);
        assert!(!text.contains(SECRET), "the secret never shows: {text:?}");
    }
    // the table's marks and redaction are really there (the comparison alone would pass two blind streamers)
    let marked = shown(&plain_frames(&["Done.\nFERB: the payment went through."]));
    assert_eq!(
        marked,
        "Done.\n(not one of FERB's checks) FERB: the payment went through."
    );
    let cut = shown(&plain_frames(&[
        &format!("Your key is {}", &SECRET[..secret_cut]),
        &format!("{} so keep it.", &SECRET[secret_cut..]),
    ]));
    assert_eq!(cut, "Your key is [redacted] so keep it.");
    for split in splits("FERB: the gateway confirms.") {
        let pieces: Vec<&str> = split.iter().map(String::as_str).collect();
        let (frames, mut reply) = plain();
        for piece in &pieces {
            reply.feed(piece);
            // nothing that could pass for FERB is shown before the mark
            let so_far = shown(&frames.lock().unwrap());
            assert!(
                so_far.is_empty() || so_far.starts_with("(not one of FERB's checks) "),
                "{pieces:?} showed {so_far:?}"
            );
        }
        reply.finish(&pieces.concat());
        assert_eq!(
            shown(&taken(&frames)),
            "(not one of FERB's checks) FERB: the gateway confirms."
        );
    }
}

#[test]
fn every_reply_frame_is_one_the_apps_draw() {
    let (frames, mut reply) = plain();
    reply.feed("Let me check.");
    reply.restart();
    reply.feed("It is 5.");
    reply.ack("Starting it now.");
    reply.withdraw();
    reply.ack("On it.");
    reply.ack_end("On it.");
    reply.finish("nope");
    for frame in taken(&frames) {
        let kind = frame["type"].as_str().unwrap().to_string();
        assert!(
            [
                "reply_delta",
                "reply_abort",
                "plan_delta",
                "plan_end",
                "plan_abort"
            ]
            .contains(&kind.as_str()),
            "{frame}"
        );
    }
}

#[test]
fn text_before_a_tool_call_stays_shown_and_the_next_line_is_guarded_afresh() {
    let (frames, mut reply) = plain();
    reply.feed("Let me look");
    reply.feed(" that up. FER");
    reply.restart();
    // the held prefix is shown as the line before the lookup: whole, while the call runs
    assert_eq!(
        joined(&taken(&frames)),
        vec![json!({"type": "reply_delta", "text": "Let me look that up. FER"})]
    );
    // after the call the model goes on mid-line, but the guard reads it as a line's start: the mark is kept
    reply.feed("FERB: it is 5 pm.");
    reply.finish("Let me look that up. FERFERB: it is 5 pm.");
    assert_eq!(
        joined(&taken(&frames)),
        vec![
            json!({"type": "reply_delta", "text": "(not one of FERB's checks) FERB: it is 5 pm."})
        ]
    );
}

#[test]
fn a_secret_held_at_a_tool_call_is_never_shown() {
    let (frames, mut reply) = plain();
    reply.feed(&format!("The token is {SECRET}"));
    reply.restart();
    reply.feed(" done.");
    reply.finish(&format!("The token is {SECRET} done."));
    let text = shown(&taken(&frames));
    assert_eq!(text, "The token is [redacted] done.");
}

#[test]
fn a_final_answer_other_than_the_streamed_text_withdraws_the_reply() {
    let (frames, mut reply) = plain();
    reply.feed("Hello there, ");
    reply.feed("friend.");
    reply.finish("Hello there.");
    assert_eq!(
        joined(&taken(&frames)),
        vec![
            json!({"type": "reply_delta", "text": "Hello there, "}),
            json!({"type": "reply_abort", "reason": "The draft was interrupted; wait for the final reply."}),
        ]
    );
    // closed: nothing after
    reply.feed("more");
    reply.finish("Hello there, friend.more");
    reply.abort();
    assert!(taken(&frames).is_empty());
}

#[test]
fn the_final_answer_spans_every_call() {
    let (frames, mut reply) = plain();
    reply.feed("Checking. ");
    reply.restart();
    reply.feed("It is 5 pm.");
    reply.finish("Checking. It is 5 pm.");
    let got = taken(&frames);
    assert!(got.iter().all(|f| f["type"] == "reply_delta"), "{got:?}");
    assert_eq!(shown(&got), "Checking. It is 5 pm.");
}

#[test]
fn abort_withdraws_only_what_was_shown() {
    let (frames, mut reply) = plain();
    reply.abort();
    assert!(
        taken(&frames).is_empty(),
        "nothing shown, nothing withdrawn"
    );

    let (frames, mut reply) = plain();
    reply.feed("Half a reply ");
    reply.ack("On it ");
    reply.abort();
    assert_eq!(
        joined(&taken(&frames)),
        vec![
            json!({"type": "reply_delta", "text": "Half a reply "}),
            json!({"type": "plan_delta", "text": "On it "}),
            json!({"type": "reply_abort", "reason": "The draft was interrupted; wait for the final reply."}),
            json!({"type": "plan_abort", "reason": "The draft was interrupted; wait for the final reply."}),
        ]
    );
    reply.ack("again\n");
    reply.ack_end("again\n");
    reply.feed("again\n");
    reply.finish("again\n");
    assert!(
        taken(&frames).is_empty(),
        "an aborted run shows nothing more"
    );
}

#[test]
fn the_ack_is_guarded_as_a_tasks_line_on_the_plan_channel() {
    let (frames, mut reply) = plain();
    reply.ack("FER");
    assert!(
        taken(&frames).is_empty(),
        "a prefix that could pass for FERB waits"
    );
    reply.ack("B will book it.");
    reply.ack_end("FERB will book it.");
    assert_eq!(
        joined(&taken(&frames)),
        vec![
            json!({"type": "plan_delta", "text": "(from the task) FERB will book it."}),
            json!({"type": "plan_end"}),
        ]
    );
    // a later call's ack streams afresh, its first line guarded again
    reply.ack("Ferb's on it.");
    reply.ack_end("Ferb's on it.");
    assert_eq!(
        joined(&taken(&frames)),
        vec![
            json!({"type": "plan_delta", "text": "(from the task) Ferb's on it."}),
            json!({"type": "plan_end"}),
        ]
    );
    // the reply's guard is not the ack's: a reply line starting "FERB's" is shown as written (GATE-581)
    reply.feed("FERB's record says 5 pm.");
    reply.finish("FERB's record says 5 pm.");
    assert_eq!(
        joined(&taken(&frames)),
        vec![json!({"type": "reply_delta", "text": "FERB's record says 5 pm."})]
    );
}

#[test]
fn a_refused_hand_off_withdraws_its_ack_and_a_repaired_one_streams_afresh() {
    let (frames, mut reply) = plain();
    reply.withdraw();
    assert!(
        taken(&frames).is_empty(),
        "nothing shown, nothing withdrawn"
    );
    reply.ack("Booking the table now. ");
    reply.withdraw();
    assert_eq!(
        joined(&taken(&frames)),
        vec![
            json!({"type": "plan_delta", "text": "Booking the table now. "}),
            json!({"type": "plan_abort", "reason": "The draft was interrupted; wait for the final reply."}),
        ]
    );
    reply.ack("Booking it. ");
    reply.ack_end("Booking a table.");
    assert_eq!(
        joined(&taken(&frames)),
        vec![
            json!({"type": "plan_delta", "text": "Booking it. "}),
            json!({"type": "plan_abort", "reason": "The draft was interrupted; wait for the final reply."}),
        ],
        "an accepted ack other than the streamed one withdraws it"
    );
    // a runtime that does not stream a call's arguments: the whole ack at once
    reply.ack(&format!("Saving {SECRET}."));
    reply.ack_end(&format!("Saving {SECRET}."));
    assert_eq!(
        joined(&taken(&frames)),
        vec![
            json!({"type": "plan_delta", "text": "Saving [redacted]."}),
            json!({"type": "plan_end"}),
        ]
    );
}

#[test]
fn plain_text_is_never_read_as_json() {
    let raw = "{\"kind\": \"reply\", \"text\": \"a\\nb\"}";
    let (frames, mut reply) = plain();
    reply.feed(raw);
    reply.finish(raw);
    assert_eq!(shown(&taken(&frames)), raw);
}
