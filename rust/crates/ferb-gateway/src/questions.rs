//! Pending questions and exact answers (`ferb/daemon/questions.py`). Must not treat ambiguity or silence as
//! consent.
//!
//! In memory only, as in Python (stage 1, PLAN part 1): a question has no lifetime and stays pending while its task
//! waits; it closes when answered or when its task's request is forgotten. A restart loses every pending question
//! (the planner asks again; `restart_with_open_question` in the daemon recordings).

use std::collections::BTreeSet;

use serde_json::{Map, Value};

/// FERB's own consequential checks, by capability: only an option's own words answer them (GATE-510, ORCH-536/537).
pub const OWN_CHECKS: &[&str] = &[
    "ferb.confirm",
    "ferb.retry",
    "conversation.approve",
    "memory.forget",
    "downloads.delete",
    "browsing.choose",
];

pub const SECRET_KINDS: &[&str] = &["owner_login_code", "owner_admin_password", "owner_secret"];

/// A question's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Pending,
    Granted,
    Denied,
    Used,
    Superseded,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Pending => "pending",
            State::Granted => "granted",
            State::Denied => "denied",
            State::Used => "used",
            State::Superseded => "superseded",
        }
    }
}

/// `Approval` (`questions.py:22`).
#[derive(Debug, Clone, PartialEq)]
pub struct Approval {
    pub id: String,
    pub capability: String,
    pub reason: String,
    pub request_id: String,
    pub kind: String,
    pub state: State,
    pub options: Vec<String>,
    pub choice: Option<String>,
    /// The typed answer. Held only until the waiting call takes it (a hidden input's is cleared at once).
    pub answer: Option<String>,
    pub summary: String,
    pub from_task: bool,
    pub why: String,
    pub detail: String,
    pub exact: String,
    pub taken: bool,
    pub key: String,
    pub task_id: String,
    pub about: Vec<String>,
    pub part: String,
    pub runtime_switch: Option<Map<String, Value>>,
    pub hidden: Vec<String>,
    pub options_only: bool,
    pub corrects: bool,
    pub labels: Vec<String>,
    pub note: String,
    pub title: String,
    pub secret_labels: Vec<String>,
    pub site: String,
    pub picture: String,
}

impl Approval {
    fn new(id: String, capability: &str, reason: &str, request_id: &str) -> Approval {
        Approval {
            id,
            capability: capability.to_string(),
            reason: reason.to_string(),
            request_id: request_id.to_string(),
            kind: "owner_question".into(),
            state: State::Pending,
            options: Vec::new(),
            choice: None,
            answer: None,
            summary: String::new(),
            from_task: false,
            why: String::new(),
            detail: String::new(),
            exact: String::new(),
            taken: false,
            key: String::new(),
            task_id: String::new(),
            about: Vec::new(),
            part: String::new(),
            runtime_switch: None,
            hidden: Vec::new(),
            options_only: false,
            corrects: false,
            labels: Vec::new(),
            note: String::new(),
            title: String::new(),
            secret_labels: Vec::new(),
            site: String::new(),
            picture: String::new(),
        }
    }

    /// `option_for(text)`: the option (1-based) a typed answer picks: its exact words or its button's words in any
    /// case; else, for a task's own question, its number. FERB's own checks and check-ins take only words.
    pub fn option_for(&self, text: &str) -> Option<usize> {
        let said = casefold(text);
        let mut matches: BTreeSet<usize> = BTreeSet::new();
        for (i, o) in self.options.iter().enumerate() {
            if casefold(py_strip(o)) == said {
                matches.insert(i + 1);
            }
        }
        for (i, label) in self.labels.iter().take(self.options.len()).enumerate() {
            let l = py_strip(label);
            if !l.is_empty() && casefold(l) == said {
                matches.insert(i + 1);
            }
        }
        if matches.len() == 1 {
            return matches.into_iter().next();
        }
        if self.options_only || OWN_CHECKS.contains(&self.capability.as_str()) {
            return None;
        }
        if !text.is_empty() && text.chars().all(ferb_py::unicode::is_decimal) {
            let n: usize = text
                .chars()
                .map(char_decimal)
                .try_fold(0usize, |acc, d| acc.checked_mul(10)?.checked_add(d?))?;
            if (1..=self.options.len()).contains(&n) {
                return Some(n);
            }
        }
        None
    }
}

/// A decimal digit's value (any script's, as `int()` reads it): its place in its run of decimal digits, each run a
/// multiple of ten starting at a zero.
fn char_decimal(c: char) -> Option<usize> {
    if !ferb_py::unicode::is_decimal(c) {
        return None;
    }
    let cp = c as u32;
    let mut start = cp;
    while start > 0 && char::from_u32(start - 1).is_some_and(ferb_py::unicode::is_decimal) {
        start -= 1;
    }
    Some(((cp - start) % 10) as usize)
}

/// Python's `str.casefold()` (approximated by full lowercase plus the sharp-s fold).
fn casefold(s: &str) -> String {
    s.to_lowercase().replace('ß', "ss")
}

fn py_strip(s: &str) -> &str {
    crate::validators::py_strip(s)
}

/// `header(ap)`: the card's header by structure (GATE-566).
pub fn header(ap: &Approval) -> String {
    if !ap.title.is_empty() {
        return ap.title.clone();
    }
    if SECRET_KINDS.contains(&ap.kind.as_str()) {
        return "Private — only the page gets this".into();
    }
    if ap.capability == "ferb.confirm" || ap.capability == "conversation.approve" {
        return "Check before FERB goes on".into();
    }
    if ap.capability == "ferb.retry" {
        return "Check before FERB retries".into();
    }
    if ap.from_task {
        return "Your task has a question".into();
    }
    if ap.options_only || ap.runtime_switch.is_some() {
        return "Your task needs a decision".into();
    }
    String::new()
}

/// `card_parts(ap)` (GATE-513, GATE-566, GATE-579).
pub fn card_parts(ap: &Approval) -> Map<String, Value> {
    let mut m = Map::new();
    let s = |v: &str| Value::String(v.to_string());
    if !ap.detail.is_empty() {
        m.insert("detail".into(), s(&ap.detail));
    }
    if !ap.exact.is_empty() {
        m.insert("exact".into(), s(&ap.exact));
    }
    if ap.picture == "shown" {
        m.insert("picture".into(), Value::Bool(true));
    } else if !ap.picture.is_empty() {
        m.insert("picture_problem".into(), s(&ap.picture));
    }
    let title = header(ap);
    if !title.is_empty() {
        m.insert("title".into(), s(&title));
    }
    if ap.labels.iter().any(|l| !l.is_empty()) {
        m.insert(
            "labels".into(),
            Value::Array(ap.labels.iter().map(|l| s(l)).collect()),
        );
    }
    if !ap.note.is_empty() {
        m.insert("note".into(), s(&ap.note));
    }
    for (k, v) in ["field_label", "decline_label", "send_label"]
        .iter()
        .zip(ap.secret_labels.iter())
    {
        m.insert((*k).into(), s(v));
    }
    m
}

/// `card_flags(ap)`.
pub fn card_flags(ap: &Approval) -> Map<String, Value> {
    let mut m = Map::new();
    if ap.options_only {
        m.insert("options_only".into(), Value::Bool(true));
    }
    if ap.corrects {
        m.insert("corrects".into(), Value::Bool(true));
    }
    m
}

/// Why `resolve` refused an answer (Python's `KeyError` / `ValueError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    Unknown(String),
    Invalid(String),
}

/// What asks a question: the action's capability, task and request (`ActionRequest`'s fields `question` reads).
#[derive(Debug, Clone)]
pub struct Asker<'a> {
    pub capability: &'a str,
    pub task_id: &'a str,
    pub request_id: &'a str,
}

/// `Approvals` (`questions.py:121`): every question still kept, oldest first, and the requests nobody is at chat for.
#[derive(Default)]
pub struct Approvals {
    items: Vec<Approval>,
    pub unattended: BTreeSet<String>,
    ids: Option<Box<IdFn>>,
}

type IdFn = dyn FnMut() -> String + Send + Sync;

impl std::fmt::Debug for Approvals {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Approvals")
            .field("items", &self.items)
            .field("unattended", &self.unattended)
            .finish()
    }
}

impl Approvals {
    /// `ids`: a question's new id (Python: `"ap-" + secrets.token_hex(6)`).
    pub fn new(ids: impl FnMut() -> String + Send + Sync + 'static) -> Approvals {
        Approvals {
            items: Vec::new(),
            unattended: BTreeSet::new(),
            ids: Some(Box::new(ids)),
        }
    }

    fn new_id(&mut self) -> String {
        match self.ids.as_mut() {
            Some(f) => f(),
            None => {
                let mut b = [0u8; 6];
                getrandom_fill(&mut b);
                format!("ap-{}", hex::encode(b))
            }
        }
    }

    /// `question(action, question, options, ident)`: `ident` keeps the id of a question asked again after a restart.
    pub fn question(
        &mut self,
        asker: &Asker<'_>,
        question: &str,
        options: &[String],
        ident: Option<&str>,
    ) -> &mut Approval {
        let id = ident.map(str::to_string).unwrap_or_else(|| self.new_id());
        let mut ap = Approval::new(id.clone(), asker.capability, question, asker.request_id);
        ap.options = options.to_vec();
        ap.task_id = asker.task_id.to_string();
        self.items.retain(|a| a.id != id);
        self.items.push(ap);
        self.items.last_mut().expect("just pushed")
    }

    /// `resolve(id, granted, choice, text)`.
    pub fn resolve(
        &mut self,
        approval_id: &str,
        granted: bool,
        choice: Option<usize>,
        text: Option<&str>,
    ) -> Result<&Approval, ResolveError> {
        let ap = self
            .items
            .iter_mut()
            .find(|a| a.id == approval_id)
            .ok_or_else(|| ResolveError::Unknown(approval_id.to_string()))?;
        if ap.state != State::Pending {
            return Err(ResolveError::Invalid(format!(
                "question {approval_id} is {}",
                ap.state.as_str()
            )));
        }
        let text: Option<String> = if ap.kind == "owner_admin_password" || ap.kind == "owner_secret"
        {
            if text.is_some_and(|t| t.contains(['\r', '\n', '\0'])) {
                return Err(ResolveError::Invalid(
                    "the hidden password must be one line".into(),
                ));
            }
            text.filter(|t| !t.is_empty()).map(str::to_string)
        } else {
            let t = py_strip(text.unwrap_or(""));
            (!t.is_empty()).then(|| t.to_string())
        };
        let mut choice = choice;
        if let Some(t) = &text {
            if choice.is_none() && !ap.options.is_empty() {
                choice = ap.option_for(t);
            }
        }
        if granted {
            if let Some(c) = choice {
                if !(1..=ap.options.len()).contains(&c) {
                    return Err(ResolveError::Invalid(format!(
                        "pick one of 1–{}",
                        ap.options.len()
                    )));
                }
                ap.choice = Some(ap.options[c - 1].clone());
            }
            ap.answer = text;
        }
        ap.state = if granted {
            State::Granted
        } else {
            State::Denied
        };
        Ok(ap)
    }

    /// `supersede(id)`: close a pending question nobody can use any more (L3); not a No.
    pub fn supersede(&mut self, approval_id: &str) -> Option<&Approval> {
        let ap = self.items.iter_mut().find(|a| a.id == approval_id)?;
        if ap.state != State::Pending {
            return None;
        }
        ap.state = State::Superseded;
        Some(ap)
    }

    pub fn pending(&self) -> Vec<&Approval> {
        self.items
            .iter()
            .filter(|a| a.state == State::Pending)
            .collect()
    }

    pub fn get(&self, approval_id: &str) -> Option<&Approval> {
        self.items.iter().find(|a| a.id == approval_id)
    }

    pub fn get_mut(&mut self, approval_id: &str) -> Option<&mut Approval> {
        self.items.iter_mut().find(|a| a.id == approval_id)
    }

    /// `of_request(id)`: every question of this request still kept, oldest first.
    pub fn of_request(&self, request_id: &str) -> Vec<&Approval> {
        self.items
            .iter()
            .filter(|a| a.request_id == request_id)
            .collect()
    }

    /// `forget_request(id)`: its questions go (the task ended).
    pub fn forget_request(&mut self, request_id: &str) {
        self.unattended.remove(request_id);
        self.items.retain(|a| a.request_id != request_id);
    }
}

fn getrandom_fill(b: &mut [u8]) {
    if getrandom::fill(b).is_err() {
        b.fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asked(capability: &str, options: &[&str]) -> (Approvals, String) {
        let mut n = 0;
        let mut a = Approvals::new(move || {
            n += 1;
            format!("ap-{n}")
        });
        let opts: Vec<String> = options.iter().map(|s| s.to_string()).collect();
        let id = a
            .question(
                &Asker {
                    capability,
                    task_id: "t1",
                    request_id: "r1",
                },
                "Pay?",
                &opts,
                None,
            )
            .id
            .clone();
        (a, id)
    }

    // decision: GATE-512
    #[test]
    fn ferbs_own_checks_take_only_an_options_words() {
        let (mut a, id) = asked("ferb.confirm", &["Yes", "No"]);
        assert_eq!(
            a.get(&id).unwrap().option_for("1"),
            None,
            "a typed 1 is words, never a Yes"
        );
        assert_eq!(
            a.get(&id).unwrap().option_for(" yes "),
            None,
            "only after the owner's text is stripped"
        );
        assert_eq!(
            a.resolve(&id, true, None, Some(" YES "))
                .unwrap()
                .choice
                .as_deref(),
            Some("Yes")
        );
        let (b, id2) = asked("owner.ask", &["a", "b"]);
        assert_eq!(
            b.get(&id2).unwrap().option_for("2"),
            Some(2),
            "a task's own question takes a number"
        );
    }

    #[test]
    fn an_answered_question_is_not_answered_again_and_superseded_is_not_a_no() {
        let (mut a, id) = asked("ferb.confirm", &["Yes", "No"]);
        assert_eq!(a.supersede(&id).unwrap().state, State::Superseded);
        assert_eq!(
            a.resolve(&id, true, Some(1), None).unwrap_err(),
            ResolveError::Invalid(format!("question {id} is superseded"))
        );
        assert!(a.supersede(&id).is_none());
        assert_eq!(
            a.resolve("nope", true, None, None).unwrap_err(),
            ResolveError::Unknown("nope".into())
        );
        a.forget_request("r1");
        assert!(a.get(&id).is_none());
    }

    #[test]
    fn a_hidden_password_must_be_one_line() {
        let (mut a, id) = asked("shell.admin", &[]);
        a.get_mut(&id).unwrap().kind = "owner_admin_password".into();
        assert_eq!(
            a.resolve(&id, true, None, Some("pa\nss")).unwrap_err(),
            ResolveError::Invalid("the hidden password must be one line".into())
        );
        assert_eq!(
            a.resolve(&id, true, None, Some(" pass "))
                .unwrap()
                .answer
                .as_deref(),
            Some(" pass ")
        );
    }

    #[test]
    fn a_cards_header_comes_from_structure() {
        let (a, id) = asked("ferb.retry", &["Yes", "No"]);
        assert_eq!(header(a.get(&id).unwrap()), "Check before FERB retries");
        let mut ap = a.get(&id).unwrap().clone();
        ap.labels = vec!["Try again".into(), "Don't retry".into()];
        ap.picture = NOT.into();
        let parts = card_parts(&ap);
        assert_eq!(
            ferb_py::json::dumps(&parts),
            r#"{"picture_problem": "x", "title": "Check before FERB retries", "labels": ["Try again", "Don't retry"]}"#
        );
    }

    const NOT: &str = "x";
}
