//! The owner's "no rabbit holes" rule as code applies it (ORCH-679..681), and its invariant (ORCH-680): from the check's
//! labels and what the step already did, code only ever keeps the checker's own choice or puts the question to the
//! owner in place of a retry or of a gapped accept refused because the worker reported not done. Never a fail, a stop
//! or any other outcome, and a missing or malformed label changes nothing.

use ferb_context::pyvalue::loads;
use ferb_orchestrator::rabbit_holes::{
    owner_step_stands, question, rule, Checked, Kind, Reached, Rule, Unmet, ASK_TAIL,
};
use ferb_orchestrator::validate::{checker_outcome_validate, dump_checker_outcome};

/// Every next a check can give, and two it can't.
const NEXTS: [&str; 8] = [
    "accept",
    "retry",
    "ask",
    "accept_unconfirmed",
    "accept_with_gaps",
    "fail",
    "",
    "keep digging",
];

fn conditions() -> Vec<Unmet> {
    let kinds: Vec<Option<Kind>> = std::iter::once(None)
        .chain(Kind::ALL.iter().copied().map(Some))
        .collect();
    let reached: Vec<Option<Reached>> = std::iter::once(None)
        .chain(Reached::ALL.iter().copied().map(Some))
        .collect();
    let mut out = Vec::new();
    for k in &kinds {
        for r in &reached {
            for retried in [false, true] {
                for proved_false in [false, true] {
                    out.push(Unmet {
                        kind: *k,
                        reached: *r,
                        retried,
                        proved_false,
                    });
                }
            }
        }
    }
    out
}

/// ORCH-680: every label combination (none, one or two unmet conditions, each kind or none, each reach or none, retried
/// or not, proved false or not), every next, a refused gapped accept or not, progress given or not, the owner's "keep
/// trying" or not: the outcome is the checker's own choice or the question, and the question only in place of a retry
/// or a gapped accept refused because the worker reported not done, with progress to ask with, never after the owner
/// said keep trying and never about a condition code's own facts proved false.
// decision: ORCH-680, ORCH-679, ORCH-681
#[test]
fn the_rule_only_ever_keeps_the_checkers_choice_or_asks() {
    let one = conditions();
    let mut sets: Vec<Vec<Unmet>> = vec![vec![]];
    sets.extend(one.iter().map(|u| vec![*u]));
    for a in &one {
        for b in &one {
            sets.push(vec![*a, *b]);
        }
    }
    let mut asked = 0usize;
    let mut kept = 0usize;
    for next in NEXTS {
        for refused_not_done in [false, true] {
            for said in [false, true] {
                for (owner_kept, owner_step, stuck) in
                    (0..8).map(|b| (b & 1 == 1, b & 2 == 2, b & 4 == 4))
                {
                    let checked = Checked {
                        next,
                        refused_not_done,
                        said,
                        owner_kept,
                        owner_step,
                        stuck,
                    };
                    for unmet in &sets {
                        match rule(&checked, unmet) {
                            Rule::AsChecked => kept += 1,
                            Rule::Ask => {
                                asked += 1;
                                let gapped = next == "accept_with_gaps" && refused_not_done;
                                assert!(next == "retry" || gapped, "{next}: {unmet:?}");
                                // ORCH-691: the guard asks after a "keep trying" too
                                assert!(said && (!owner_kept || stuck), "{checked:?}");
                                assert!(!unmet.is_empty(), "{checked:?}");
                                assert!(
                                    unmet.iter().all(|u| !u.proved_false),
                                    "{checked:?} {unmet:?}"
                                );
                                let owners = owner_step
                                    || stuck
                                    || unmet.iter().any(|u| u.kind == Some(Kind::Owner));
                                if !gapped && !owners {
                                    // a retry is asked about only on whole labels and a dig already retried
                                    assert!(
                                        unmet
                                            .iter()
                                            .all(|u| u.kind.is_some() && u.reached.is_some()),
                                        "{unmet:?}"
                                    );
                                    assert!(
                                        unmet.iter().any(|u| u.retried
                                            && u.kind.is_some_and(Kind::digs)
                                            && u.reached >= Some(Reached::Some)),
                                        "{unmet:?}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(asked > 0 && kept > 0);
}

fn unmet(kind: Kind, reached: Reached, retried: bool) -> Unmet {
    Unmet {
        kind: Some(kind),
        reached: Some(reached),
        retried,
        proved_false: false,
    }
}

const RETRY: Checked = Checked {
    next: "retry",
    refused_not_done: false,
    said: true,
    owner_kept: false,
    owner_step: false,
    stuck: false,
};

/// ORCH-679: the first retry is the checker's; after it a guess, a search or a stuck block with some of it reached is
/// asked about; a fix or nothing reached is the checker's retry; a fix on another condition doesn't hold the question
/// back. Named change (ORCH-690, owner 2026-11-22): an owner's input is asked about at once, no longer the
/// checker's retry.
// decision: ORCH-679, ORCH-690
#[test]
fn the_rule_on_the_measured_shapes() {
    use Kind::*;
    use Reached::*;
    assert_eq!(rule(&RETRY, &[unmet(Search, Most, false)]), Rule::AsChecked);
    for k in [Guess, Search, Stuck] {
        assert_eq!(rule(&RETRY, &[unmet(k, Some, true)]), Rule::Ask, "{k:?}");
        assert_eq!(rule(&RETRY, &[unmet(k, None, true)]), Rule::AsChecked);
    }
    assert_eq!(rule(&RETRY, &[unmet(Fix, Most, true)]), Rule::AsChecked);
    assert_eq!(rule(&RETRY, &[unmet(Owner, Most, true)]), Rule::Ask);
    assert_eq!(rule(&RETRY, &[unmet(Owner, None, false)]), Rule::Ask);
    assert_eq!(
        rule(&RETRY, &[unmet(Search, Some, true), unmet(Fix, Some, true)]),
        Rule::Ask
    );
    let kept = Checked {
        owner_kept: true,
        ..RETRY
    };
    assert_eq!(rule(&kept, &[unmet(Search, Most, true)]), Rule::AsChecked);
    let gapped = Checked {
        next: "accept_with_gaps",
        refused_not_done: true,
        ..RETRY
    };
    assert_eq!(rule(&gapped, &[unmet(Fix, None, false)]), Rule::Ask);
}

/// ORCH-679: the question is the check's two sentences, each ending with its stop, then FERB's fixed words.
// decision: ORCH-679
#[test]
fn the_question_is_the_checks_sentences_then_the_fixed_words() {
    let q = question("I found three addresses", "The fourth isn't confirmed.");
    assert!(q.starts_with("I found three addresses. The fourth isn't confirmed. "));
    assert!(q.ends_with(ASK_TAIL));
    assert_eq!(
        question("", "Nothing loaded."),
        format!("Nothing loaded. {ASK_TAIL}")
    );
}

/// ORCH-680: a check whose labels are malformed is still a valid check (the labels are dropped or kept as written, and
/// an unknown one is no label); a check without them dumps as before.
// decision: ORCH-680
#[test]
fn a_malformed_label_never_fails_the_check() {
    let base = r#"{"results": [{"index": 0, "met": null, "applies": true, "evidence": "e", "how": null, LEFT}],
        "next": "retry", "findings": [], "missing": "m", "instructions": "i", "worker_type": null, "question": "",
        "reason": "", "gaps": [], PROGRESS}"#;
    let make = |left: &str, progress: &str| {
        loads(&base.replace("LEFT", left).replace("PROGRESS", progress)).unwrap()
    };
    let bad = checker_outcome_validate(&make(r#""left": "search""#, r#""progress": 7"#)).unwrap();
    assert!(bad.results[0].left.is_none() && bad.progress.is_none());
    let odd = checker_outcome_validate(&make(
        r#""left": {"kind": "dig", "reached": 3}"#,
        r#""progress": {"found": "f"}"#,
    ))
    .unwrap();
    let left = odd.results[0].left.as_ref().unwrap();
    assert_eq!(Kind::of(&left.kind), None);
    assert_eq!(Reached::of(&left.reached), None);
    assert_eq!(odd.progress.as_ref().unwrap().missing, "");
    let good = checker_outcome_validate(&make(
        r#""left": {"kind": "search", "reached": "most"}"#,
        r#""progress": {"found": "f", "missing": "m"}"#,
    ))
    .unwrap();
    assert_eq!(
        Kind::of(&good.results[0].left.as_ref().unwrap().kind),
        Some(Kind::Search)
    );
    let none = checker_outcome_validate(&make(r#""left": null"#, r#""progress": null"#)).unwrap();
    let dumped = dump_checker_outcome(&none).to_value().to_string();
    assert!(
        !dumped.contains("left") && !dumped.contains("progress"),
        "{dumped}"
    );
}

/// ORCH-690 (owner, 2026-11-22: "Yes, always ask me"): a tool's owner_step fact standing in the attempt makes
/// the check's retry the question, whatever the check labelled (task a843aa54's check said `fix`, none reached).
/// ORCH-691 (owner, 2026-11-22: "8 failed actions or 4 min"): an attempt the guard stopped asks at once, also after
/// an earlier "keep trying"; a check that accepts, asks or fails keeps its own choice.
// decision: ORCH-690, ORCH-691
#[test]
fn the_owners_step_and_a_stopped_attempt_are_asked_about_at_once() {
    use Kind::*;
    use Reached::*;
    let fix = [unmet(Fix, None, false)];
    assert_eq!(rule(&RETRY, &fix), Rule::AsChecked);
    let owner_step = Checked {
        owner_step: true,
        ..RETRY
    };
    assert_eq!(rule(&owner_step, &fix), Rule::Ask);
    let stuck = Checked {
        stuck: true,
        ..RETRY
    };
    assert_eq!(rule(&stuck, &fix), Rule::Ask);
    let kept = Checked {
        owner_kept: true,
        ..RETRY
    };
    assert_eq!(
        rule(
            &Checked {
                stuck: true,
                ..kept
            },
            &fix
        ),
        Rule::Ask
    );
    assert_eq!(
        rule(
            &Checked {
                owner_step: true,
                ..kept
            },
            &fix
        ),
        Rule::AsChecked
    );
    for next in ["accept", "accept_unconfirmed", "ask", "fail"] {
        let c = Checked {
            next,
            owner_step: true,
            stuck: true,
            ..RETRY
        };
        assert_eq!(rule(&c, &fix), Rule::AsChecked, "{next}");
    }
    let proved = [Unmet {
        proved_false: true,
        ..unmet(Fix, Most, true)
    }];
    assert_eq!(rule(&stuck, &proved), Rule::AsChecked);
}

/// ORCH-690: the latest call of each tool in the attempt decides whether its owner_step fact stands.
// decision: ORCH-690
#[test]
fn an_owner_step_stands_until_the_same_tool_says_otherwise() {
    assert!(!owner_step_stands([]));
    assert!(owner_step_stands([
        ("browser.pair", true),
        ("mac.menu", false)
    ]));
    assert!(!owner_step_stands([
        ("browser.pair", true),
        ("browser.pair", false)
    ]));
    assert!(owner_step_stands([
        ("browser.pair", false),
        ("browser.pair", true)
    ]));
}
