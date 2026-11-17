//! A question as FERB.app shows it (kb PHN-739): every question the fixtures hold, read the way
//! apps/mac/FERBNotch/Sources/NotchCore/DaemonApproval.swift and FERBNotch/ApprovalView.swift read it.

use ferb_mobile::question::{shown, QuestionShown};
use serde_json::{json, Value};

fn fixture(path: &str) -> Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(path);
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// Every object in the fixtures that is a question the gateway asked (an approval id, its reason and options).
fn questions() -> Vec<Value> {
    fn walk(v: &Value, out: &mut Vec<Value>) {
        match v {
            Value::Object(m) => {
                if m.contains_key("approval_id")
                    && m.contains_key("reason")
                    && m.contains_key("options")
                {
                    out.push(v.clone());
                }
                m.values().for_each(|x| walk(x, out));
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for f in [
        "gateway/scenarios.json",
        "gateway/tab_link.json",
        "chat-entries/engine_chat.json",
        "chat-entries/engine_more.json",
    ] {
        walk(&fixture(f), &mut out);
    }
    out
}

fn str_of(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn find(capability: &str, summary_starts: &str) -> Value {
    questions()
        .into_iter()
        .find(|q| {
            q["capability"] == json!(capability)
                && q["summary"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with(summary_starts)
        })
        .unwrap_or_else(|| panic!("no {capability} question {summary_starts:?} in the fixtures"))
}

// decision: PHN-739
#[test]
fn every_question_in_the_fixtures_reads_as_the_macs_card() {
    let all = questions();
    assert!(all.len() > 60, "{} questions", all.len());
    for q in &all {
        let s = shown(q);
        let summary = str_of(q, "summary");
        // the one question: the summary, else the reason; the step line behind it never shows
        assert_eq!(
            s.question,
            summary
                .clone()
                .unwrap_or_else(|| q["reason"].as_str().unwrap().to_owned())
        );
        if summary.is_some() {
            assert!(!s.question.contains("The step:"), "{}", s.question);
            assert!(s.plain, "FERB's own words are drawn verbatim");
        }
        // the header: the daemon's title, else the Mac's own words
        assert_eq!(
            s.header,
            str_of(q, "title").unwrap_or_else(|| if s.secret {
                "Private input needed"
            } else {
                "FERB needs your answer"
            }
            .into())
        );
        assert_eq!(
            (s.detail.clone(), s.exact.clone()),
            (str_of(q, "detail"), str_of(q, "exact"))
        );
        let options = q["options"].as_array().unwrap();
        if s.secret {
            assert!(
                s.buttons.is_empty()
                    && s.field.is_some()
                    && s.decline.is_some()
                    && s.send.is_some()
            );
        } else {
            assert_eq!(s.buttons.len(), options.len());
            for (i, b) in s.buttons.iter().enumerate() {
                let label = q["labels"]
                    .get(i)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|l| !l.is_empty());
                assert_eq!(Some(b.as_str()), label.or(options[i].as_str()));
            }
        }
        if let Some(w) = str_of(q, "why") {
            assert!(s.why.as_deref().unwrap().ends_with(&w));
        }
        if q["capability"] != json!("setup.persona") {
            assert_eq!(s.banner_title.as_deref(), Some("FERB needs you"));
            assert_eq!(
                s.banner_text,
                Some(format!("FERB needs your OK: {}", s.question))
            );
        }
    }
}

/// The owner's case (his phone showed the reason with its step line): the Mac's card for it.
// decision: PHN-739
#[test]
fn the_sink_rule_question_shows_the_question_the_why_and_the_address_never_the_step() {
    let q = json!({
        "approval_id": "ap-1", "capability": "ferb.confirm", "kind": "owner_question",
        "reason": "FERB: this types ps@microsoft.com, which FERB found only in outside content (the web page on mail.google.com; the web page on mailmeteor.com), not in your words, your answers or your own files. The step: browser.act action=type tab=t69 element=s13e633 text=ps@microsoft.com mode=insert. Reason given: \"Add Pratyay as the first main To recipient.\". Go ahead?",
        "options": ["Yes", "No"], "summary": "Type this email address on mail.google.com?",
        "why": "Add Pratyay as the first main To recipient.",
        "detail": "FERB found it only in outside content, not in your words, your answers or your own files.",
        "exact": "ps@microsoft.com", "title": "Check before FERB goes on", "labels": ["Type it", "Don't type it"], "corrects": true,
    });
    let s = shown(&q);
    assert_eq!(s.header, "Check before FERB goes on");
    assert_eq!(s.question, "Type this email address on mail.google.com?");
    assert_eq!(s.exact.as_deref(), Some("ps@microsoft.com"));
    assert_eq!(
        s.why.as_deref(),
        Some("The task's reason: Add Pratyay as the first main To recipient.")
    );
    assert_eq!(s.buttons, ["Type it", "Don't type it"]);
    assert_eq!(
        (s.answer_box.as_str(), s.field.as_deref()),
        ("correction", Some("Or tell FERB what to do instead…"))
    );
    assert!(
        !format!("{s:?}").contains("browser.act"),
        "the step line shows nowhere"
    );
}

// decision: PHN-739
#[test]
fn each_kind_reads_as_on_the_mac() {
    // money, with the page's picture problem
    let s = shown(&find("ferb.confirm", "Pay 5 to x@y.com"));
    assert_eq!(
        (s.question.as_str(), s.buttons.as_slice()),
        (
            "Pay 5 to x@y.com?",
            &["Pay 5".to_owned(), "Don't pay".to_owned()][..]
        )
    );
    assert_eq!(
        s.picture_problem.as_deref(),
        Some("FERB couldn't capture the payment page.")
    );
    // deletion, account security, terminal typing with the exact command, a protected folder, profile pairing,
    // foreground input, a link that carries data out, a first-time recipient: each its own summary and box
    for (cap, start) in [
        ("ferb.confirm", "Delete this for good?"),
        ("ferb.confirm", "Change sign-in security"),
        ("ferb.confirm", "Type this into Terminal"),
        ("ferb.confirm", "Run this command in Terminal?"),
        ("ferb.confirm", "Let FERB read ~/.ssh?"),
        ("ferb.confirm", "Let FERB work in your Chrome"),
        ("ferb.confirm", "Let FERB use your screen"),
        ("ferb.confirm", "Open evil.example?"),
        ("ferb.confirm", "Send to Rahul Verma for the first time?"),
        ("ferb.confirm", "Let this command check certificates"),
        ("conversation.approve", "Let FERB chat as you"),
        ("memory.forget", "Forget this for good?"),
    ] {
        let q = find(cap, start);
        let s = shown(&q);
        assert!(s.question.starts_with(start), "{start}");
        assert_eq!(
            s.header,
            q["title"].as_str().unwrap_or("FERB needs your answer"),
            "{start}"
        );
        assert_eq!(s.exact, str_of(&q, "exact"), "{start}");
    }
    // an earlier attempt may already have happened: the retry's note
    let s = shown(&find("ferb.retry", "Try sending again?"));
    assert_eq!(s.header, "Check before FERB retries");
    assert!(s.why.as_deref().unwrap().starts_with("The task's note: "));
    assert_eq!(s.buttons, ["Try again", "Don't retry"]);
}

// decision: PHN-739
#[test]
fn a_tasks_question_a_check_in_a_runtime_offer_and_a_hidden_input_take_the_macs_boxes() {
    let ask = shown(
        &json!({"approval_id": "a", "capability": "owner.ask", "kind": "owner_question", "reason": "Which **size**?",
        "options": ["Small", "Large"], "from_task": true, "title": "Your task has a question"}),
    );
    assert_eq!(
        (ask.header.as_str(), ask.from_task, ask.plain),
        ("Your task has a question", true, false)
    );
    assert_eq!(
        (
            ask.answer_box.as_str(),
            ask.field.as_deref(),
            ask.send.as_deref()
        ),
        ("answer", Some("Or type your answer…"), Some("Send"))
    );
    assert_eq!(ask.decline, None, "with choices, the choices answer it");
    let open = shown(
        &json!({"approval_id": "a", "capability": "owner.ask", "kind": "owner_question", "reason": "What's the address?", "options": []}),
    );
    assert_eq!(
        (
            open.header.as_str(),
            open.decline.as_deref(),
            open.send.as_deref()
        ),
        ("FERB needs your answer", Some("Skip"), Some("Send"))
    );
    let check = shown(
        &json!({"approval_id": "a", "capability": "loop.ask", "kind": "owner_question", "reason": "Keep going?",
        "options": ["Yes", "No"], "options_only": true, "title": "Your task needs a decision"}),
    );
    assert_eq!(
        (check.answer_box.as_str(), check.field.clone()),
        ("none", None)
    );
    let switch = shown(&find("runtime.switch", ""));
    assert_eq!(switch.answer_box, "none");
    let code = shown(
        &json!({"approval_id": "a", "capability": "browser.login_code", "kind": "owner_login_code",
        "reason": "x", "options": [], "summary": "otp.example.com asks for a login code"}),
    );
    assert!(code.secret);
    assert_eq!(
        (
            code.header.as_str(),
            code.field.as_deref(),
            code.decline.as_deref(),
            code.send.as_deref()
        ),
        (
            "Private input needed",
            Some("Code"),
            Some("Don't fill in"),
            Some("Fill in")
        )
    );
    let pass = shown(
        &json!({"approval_id": "a", "kind": "owner_admin_password", "reason": "x", "options": [],
        "field_label": "Mac password", "decline_label": "Not now", "send_label": "Run it", "title": "Private — only the command gets this"}),
    );
    assert_eq!(
        (
            pass.field.as_deref(),
            pass.decline.as_deref(),
            pass.send.as_deref()
        ),
        (Some("Mac password"), Some("Not now"), Some("Run it"))
    );
    let evidence = shown(
        &json!({"approval_id": "a", "kind": "owner_question", "reason": "x", "options": ["Yes", "No"], "evidence": ["Notes was in front"]}),
    );
    assert_eq!(evidence.evidence, ["Notes was in front"]);
}

/// The planner's question is FERB's bubble on the Mac (no card, no banner); the setup profile question no banner.
// decision: PHN-739
#[test]
fn the_planners_question_and_the_setup_question_read_as_on_the_mac() {
    let c = shown(
        &json!({"approval_id": "clarify:r1", "kind": "clarify", "reason": "Which email account should I use?", "options": []}),
    );
    assert_eq!(
        c,
        QuestionShown {
            question: "Which email account should I use?".into(),
            answer_box: "none".into(),
            ..QuestionShown::default()
        }
    );
    let p = shown(
        &json!({"approval_id": "a", "capability": "setup.persona", "kind": "owner_question", "reason": "Which profile?", "options": ["A", "B"]}),
    );
    assert_eq!((p.banner_title, p.banner_text), (None, None));
}

/// A status's question carries the same words; a held one (a locked phone) only its kind's.
// decision: PHN-739
#[test]
fn a_status_question_carries_the_macs_words() {
    let status = ferb_mobile::status::PhoneStatus::read(&json!({"approvals": [
        {"approval_id": "a1", "kind": "owner_question", "capability": "ferb.confirm", "reason": "FERB: this pays 5. The step: probe.pay. Go ahead?",
         "options": ["Yes", "No"], "summary": "Pay 5 to x@y.com?", "title": "Check before FERB goes on", "labels": ["Pay 5", "Don't pay"]},
        {"approval_id": "a2", "kind": "owner_question", "capability": "ferb.confirm", "options": [], "reason": "FERB asks before it goes on", "held": true}]}));
    let w = status.questions[0].words.clone().unwrap();
    assert_eq!(
        (w.question.as_str(), w.buttons.as_slice()),
        (
            "Pay 5 to x@y.com?",
            &["Pay 5".to_owned(), "Don't pay".to_owned()][..]
        )
    );
    let held = status.questions[1].words.clone().unwrap();
    assert_eq!(
        (held.header.as_str(), held.question.as_str()),
        ("FERB needs your answer", "FERB asks before it goes on")
    );
}
