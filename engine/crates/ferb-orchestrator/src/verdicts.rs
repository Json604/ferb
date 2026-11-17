//! ORCH-665: one condition's verdict from code's fact about it and the checker's result, in a fixed order where each
//! row is a mechanical rule and no words are read (rule 4): the owner's word (applied after, ORCH-613); the Mac's state
//! now; "FERB did nothing in this step that could have done it"; a receipt showing FERB acted on something else; then
//! the checker. The worker never decides a verdict. Pure: the supervisor gathers the inputs.
//!
//! decision: ORCH-664, ORCH-665, ORCH-667, ORCH-668, ORCH-673, ORCH-309

use serde_json::{json, Value};

use crate::verify::{
    CodeFact, ConditionResult, ACTION_CHECKS, BY_CHECKER, BY_MAC, BY_RECORD, DIFFERENT, MET,
    NOT_MET,
};

/// The basis of the "nothing done" fact (ORCH-666): the step made no acting call and changed no file.
pub const NOTHING_DONE: &str =
    "FERB pressed, typed and ran nothing in this step that could have done it, and changed no file";
/// ORCH-667: what was acted on when no file left the Mac but the check saw one on the page.
pub const PAGE_FILE: &str = "a file chosen on the page itself, not one from your Mac";

/// What the supervisor knows about the step a condition belongs to.
#[derive(Debug, Clone, Default)]
pub struct Step {
    /// The condition's check kind ("" or "none": no check named).
    pub kind: String,
    /// The checker's `applies` (ORCH-587): false when the condition's "if" didn't happen; it stands as the checker said.
    pub applies: bool,
    /// Whether the step made an acting call or changed a file (None: FERB can't tell).
    pub acted: Option<bool>,
    /// The places of this step's actions whose outcome the ledger records as unknown (never retried unasked).
    pub unknown: Vec<String>,
}

/// A settled condition and, when code's fact and the checker said different things, the record of it (the journal's
/// `task.verdict_disagreement`, ORCH-673).
#[derive(Debug, Clone, PartialEq)]
pub struct Settled {
    pub result: ConditionResult,
    pub disagreement: Option<Value>,
}

fn by_of(r: &ConditionResult) -> &'static str {
    if r.path == "verifier" {
        BY_CHECKER
    } else {
        ""
    }
}

fn code_fact(c: &ConditionResult) -> CodeFact {
    CodeFact {
        by: c.by.clone(),
        verdict: c.verdict_of().to_string(),
        basis: c.basis.clone(),
    }
}

/// The checker's result as the verdict, with code's fact kept beside it.
fn checker_with(checker: &ConditionResult, code: Option<CodeFact>) -> ConditionResult {
    let mut out = checker.clone();
    out.verdict = checker.verdict_of().to_string();
    out.by = by_of(checker).into();
    out.basis = checker.evidence.clone();
    out.code = code;
    out
}

fn disagreement(
    kind: &str,
    code: &CodeFact,
    checker: &ConditionResult,
    out: &ConditionResult,
) -> Value {
    json!({"index": out.index, "condition": out.text, "kind": kind,
           "code": {"by": code.by, "verdict": code.verdict, "basis": code.basis},
           "checker": {"met": checker.met, "path": checker.path, "evidence": checker.evidence},
           "verdict": out.verdict_of(), "by": out.by})
}

/// ORCH-665's order for one condition. `code` is code's fact (verify's `Checks::run`), `checker` the checker's result
/// for the same index (path `verifier`, or `unchecked` when the check couldn't run).
pub fn settle(step: &Step, code: Option<&ConditionResult>, checker: &ConditionResult) -> Settled {
    let plain = |r: ConditionResult| Settled {
        result: r,
        disagreement: None,
    };
    if !step.applies {
        return plain(checker.clone());
    }
    let kind = step.kind.as_str();
    let action = ACTION_CHECKS.contains(&kind);
    let judged = checker.path == "verifier" && checker.met.is_some();
    // 1. the Mac's own state now beats every model, both ways
    if let Some(c) = code.filter(|c| c.by == BY_MAC && c.met.is_some()) {
        let out = c.clone();
        let differs = judged && checker.met != c.met;
        return Settled {
            disagreement: differs.then(|| disagreement(kind, &code_fact(c), checker, &out)),
            result: out,
        };
    }
    // 2. nothing was done that could have done it: "FERB did it" is false; an outcome the checker still sees holds
    // (ORCH-309: done is about the outcome), but a file on the page when no file left the Mac is another file
    let no_bytes = code.filter(|c| c.by == BY_RECORD && c.verdict == NOT_MET);
    let none_done = action
        && code.is_some()
        && kind != "downloaded"
        && step.acted == Some(false)
        && step.unknown.is_empty();
    if none_done || no_bytes.is_some() {
        let basis = no_bytes
            .map(|c| c.basis.clone())
            .unwrap_or_else(|| NOTHING_DONE.to_string());
        let fact = CodeFact {
            by: BY_RECORD.into(),
            verdict: NOT_MET.into(),
            basis: basis.clone(),
        };
        if checker.met == Some(true) && checker.path == "verifier" {
            let out = if kind == "uploaded" {
                let mut r = ConditionResult::fact(
                    checker.index,
                    &checker.text,
                    DIFFERENT,
                    BY_RECORD,
                    &basis,
                );
                r.evidence = format!(
                    "{basis}; the check saw a file on the page: {}",
                    checker.evidence
                );
                r.asked = code.map(|c| c.asked.clone()).unwrap_or_default();
                r.acted_on = PAGE_FILE.into();
                r
            } else {
                checker_with(checker, Some(fact.clone()))
            };
            return Settled {
                disagreement: Some(disagreement(kind, &fact, checker, &out)),
                result: out,
            };
        }
        let mut out =
            ConditionResult::fact(checker.index, &checker.text, NOT_MET, BY_RECORD, &basis);
        out.asked = code.map(|c| c.asked.clone()).unwrap_or_default();
        return plain(out);
    }
    // 3. a receipt shows FERB acted on something else: the checker can't turn it into met
    if let Some(c) = code.filter(|c| c.verdict == DIFFERENT) {
        let out = c.clone();
        return Settled {
            disagreement: (checker.met == Some(true) && checker.path == "verifier")
                .then(|| disagreement(kind, &code_fact(c), checker, &out)),
            result: out,
        };
    }
    // 4. the checker decides, with code's fact kept beside its verdict
    match code.filter(|c| !c.basis.is_empty()) {
        Some(c) => plain(checker_with(checker, Some(code_fact(c)))),
        None => plain(checker.clone()),
    }
}

/// Whether a verdict ends the work as met.
pub fn is_met(r: &ConditionResult) -> bool {
    r.verdict_of() == MET
}
