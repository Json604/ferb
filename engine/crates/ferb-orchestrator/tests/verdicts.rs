//! ORCH-665's order, row by row, as a table: the Mac's state, then "FERB did nothing that could have done it", then a
//! receipt showing another target, then the checker. And ORCH-673's record of each disagreement.

#![cfg(feature = "store")]

use ferb_orchestrator::verdicts::{settle, Step, NOTHING_DONE, PAGE_FILE};
use ferb_orchestrator::verify::{
    ConditionResult, BY_MAC, BY_RECEIPT, BY_RECORD, CANT_TELL, DIFFERENT, MET, NOT_MET,
};

fn checker(met: Option<bool>) -> ConditionResult {
    ConditionResult::new(
        0,
        "The refund is claimed.",
        met,
        "verifier",
        "a fresh page read",
    )
}

fn fact(verdict: &str, by: &str) -> ConditionResult {
    ConditionResult::fact(0, "The refund is claimed.", verdict, by, "code's basis")
}

fn step(kind: &str, acted: Option<bool>) -> Step {
    Step {
        kind: kind.into(),
        applies: true,
        acted,
        unknown: Vec::new(),
    }
}

/// (kind, acted, code's fact, the checker's met) -> (met, verdict, by, a disagreement recorded)
// decision: ORCH-665, ORCH-664, ORCH-667, ORCH-673, ORCH-309
#[test]
fn the_order_of_who_decides() {
    type Row = (
        &'static str,
        Option<bool>,
        Option<(&'static str, &'static str)>,
        Option<bool>,
        (Option<bool>, &'static str, &'static str, bool),
    );
    let rows: Vec<Row> = vec![
        // the Mac's state beats the checker both ways
        (
            "trashed",
            Some(true),
            Some((NOT_MET, BY_MAC)),
            Some(true),
            (Some(false), NOT_MET, BY_MAC, true),
        ),
        (
            "deleted",
            Some(true),
            Some((MET, BY_MAC)),
            Some(false),
            (Some(true), MET, BY_MAC, true),
        ),
        (
            "deleted",
            Some(true),
            Some((MET, BY_MAC)),
            None,
            (Some(true), MET, BY_MAC, false),
        ),
        // nothing done: the checker's met is the outcome (already done, F), else not met
        (
            "submitted",
            Some(false),
            Some((CANT_TELL, BY_RECEIPT)),
            Some(true),
            (Some(true), MET, "checker", true),
        ),
        (
            "submitted",
            Some(false),
            Some((CANT_TELL, BY_RECEIPT)),
            None,
            (Some(false), NOT_MET, BY_RECORD, false),
        ),
        (
            "sent",
            Some(false),
            Some((CANT_TELL, BY_RECEIPT)),
            Some(false),
            (Some(false), NOT_MET, BY_RECORD, false),
        ),
        // no file left the Mac but the page shows one: another file
        (
            "uploaded",
            Some(true),
            Some((NOT_MET, BY_RECORD)),
            Some(true),
            (Some(false), DIFFERENT, BY_RECORD, true),
        ),
        (
            "uploaded",
            Some(true),
            Some((NOT_MET, BY_RECORD)),
            Some(false),
            (Some(false), NOT_MET, BY_RECORD, false),
        ),
        // a receipt on another target: the checker can't make it met
        (
            "sent",
            Some(true),
            Some((DIFFERENT, BY_RECEIPT)),
            Some(true),
            (Some(false), DIFFERENT, BY_RECEIPT, true),
        ),
        // otherwise the checker decides, with code's fact beside it
        (
            "submitted",
            Some(true),
            Some((CANT_TELL, BY_RECEIPT)),
            Some(true),
            (Some(true), MET, "checker", false),
        ),
        (
            "submitted",
            Some(true),
            Some((CANT_TELL, BY_RECEIPT)),
            Some(false),
            (Some(false), NOT_MET, "checker", false),
        ),
        (
            "paid",
            Some(true),
            Some((CANT_TELL, BY_RECEIPT)),
            None,
            (None, CANT_TELL, "checker", false),
        ),
        // a download that wasn't made is never "nothing done" (the file may have been there already)
        (
            "downloaded",
            Some(false),
            Some((CANT_TELL, BY_MAC)),
            Some(true),
            (Some(true), MET, "checker", false),
        ),
        // no code fact at all (no store): the checker's result as it was
        ("submitted", Some(false), None, None, (None, "", "", false)),
    ];
    for (kind, acted, code, met, (want_met, want_verdict, want_by, disagrees)) in rows {
        let code = code.map(|(v, b)| fact(v, b));
        let s = settle(&step(kind, acted), code.as_ref(), &checker(met));
        let row = format!("{kind} acted={acted:?} code={code:?} checker={met:?}");
        assert_eq!(s.result.met, want_met, "{row}: {:?}", s.result);
        assert_eq!(s.result.verdict, want_verdict, "{row}: {:?}", s.result);
        assert_eq!(s.result.by, want_by, "{row}: {:?}", s.result);
        assert_eq!(
            s.disagreement.is_some(),
            disagrees,
            "{row}: {:?}",
            s.disagreement
        );
    }
}

/// "Already done" keeps code's fact beside the checker's met, so the reply can say FERB didn't need to act; the Drive
/// case says what was acted on.
// decision: ORCH-665, ORCH-667, ORCH-672
#[test]
fn already_done_and_another_file_keep_their_facts() {
    let s = settle(
        &step("submitted", Some(false)),
        Some(&fact(CANT_TELL, BY_RECEIPT)),
        &checker(Some(true)),
    );
    let code = s.result.code.expect("code's fact kept");
    assert_eq!(
        (code.by.as_str(), code.verdict.as_str(), code.basis.as_str()),
        (BY_RECORD, NOT_MET, NOTHING_DONE)
    );
    let mut no_bytes = fact(NOT_MET, BY_RECORD);
    no_bytes.asked = "/Users/me/Desktop/report.pdf".into();
    let s = settle(
        &step("uploaded", Some(true)),
        Some(&no_bytes),
        &checker(Some(true)),
    );
    assert_eq!(s.result.asked, "/Users/me/Desktop/report.pdf");
    assert_eq!(s.result.acted_on, PAGE_FILE);
    let d = s.disagreement.unwrap();
    assert_eq!(d["code"]["by"], "record");
    assert_eq!(d["checker"]["met"], true);
    assert_eq!(d["verdict"], "different");
}

/// An action whose outcome the ledger doesn't know is never "nothing done"; a condition whose "if" didn't happen
/// stands as the checker said (ORCH-587's applies).
// decision: ORCH-665
#[test]
fn unknown_outcomes_and_conditions_that_didnt_apply() {
    let mut st = step("paid", Some(false));
    st.unknown = vec!["shop.example".into()];
    let s = settle(&st, Some(&fact(CANT_TELL, BY_RECEIPT)), &checker(None));
    assert_eq!(s.result.met, None, "{:?}", s.result);
    let mut st = step("trashed", Some(true));
    st.applies = false;
    let s = settle(&st, Some(&fact(NOT_MET, BY_MAC)), &checker(Some(true)));
    assert_eq!(s.result.met, Some(true));
    assert_eq!(s.result.path, "verifier");
}

/// The stored form: a plain checker result keeps its recorded bytes; a settled one carries verdict, by and basis, and
/// reads back the same.
// decision: ORCH-665
#[test]
fn the_added_fields_are_stored_only_when_set_and_read_back() {
    let plain = checker(Some(true));
    assert_eq!(
        plain
            .as_dict()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        ["index", "condition", "met", "path", "evidence"]
    );
    let s = settle(
        &step("submitted", Some(false)),
        Some(&fact(CANT_TELL, BY_RECEIPT)),
        &checker(Some(true)),
    );
    let stored = s.result.as_dict();
    let back = ConditionResult::new(
        0,
        "The refund is claimed.",
        Some(true),
        "verifier",
        "a fresh page read",
    )
    .with_stored(&stored);
    assert_eq!(back, s.result);
}
