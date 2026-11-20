//! What an answer to FERB's own questions can be and how long it lasts, as one table the gateway asks from and FERB
//! looks up (decision: PRM-355, PRM-356; the file question's third answer: GATE-593).
//!
//! - [`choices`] is what `Gateway::ask_confirmation` offers: the choices of every question FERB's checks ask come from
//!   here, so the lookup can't list a choice the gateway doesn't offer or miss one it does.
//! - [`lookup`] is `about.read section=answers`: computed when it's read, never stored. Each kind of question with its
//!   card's words built by the same wording the card uses (`ferb_policy::confirm::card`, on example facts), the
//!   choices from [`choices`] with their button words, and how long each lasts: a Yes as the card itself says it
//!   (the card's own sentence about what Yes covers), the other choices from [`Choice::lasts`] (an exhaustive match).
//! - [`Question`] lists the kinds; a test fails when a rule the policy or the gateway can ask has no kind here.
//!
//! What this can't keep current by itself: a new wording branch inside an existing rule (it is listed under the rule's
//! example), and the hand-written lines [`TYPED`], [`CANNOT`] and [`STANDING`].

use ferb_policy::confirm::{self, Apps, AppsUnread, Need, RunningApp, Values};
use serde_json::{json, Map, Value};

use crate::file_grants;
use crate::words::{
    ALLOW_WINDOW, ALLOW_WINDOW_LABEL, ALLOW_WINDOW_NOTE, NO, RETRY_LABELS, RETRY_NO, RETRY_YES,
    STANDING_RULES, YES,
};

/// One answer FERB's own check can take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Yes,
    /// The file question's "Send to every site in this task" (GATE-593).
    EverySite,
    No,
    /// The routine questions' "don't ask for 30 min" (GATE-207).
    AllowWindow,
}

impl Choice {
    /// The option recorded for it (what the app's button number and exact typed words pick).
    pub fn option(self) -> &'static str {
        match self {
            Choice::Yes => YES,
            Choice::EverySite => file_grants::EVERY_SITE,
            Choice::No => NO,
            Choice::AllowWindow => ALLOW_WINDOW,
        }
    }

    /// How long it lasts. A Yes is as each question's card says ([`yes_covers`]).
    pub fn lasts(self) -> String {
        match self {
            Choice::Yes => "as the question's card says (yes_covers)".into(),
            Choice::EverySite => file_grants::note(1),
            Choice::No => "The action is refused for the rest of this task; after a No to sending to a site, FERB's \
                           network filter also refuses that site for the task."
                .into(),
            Choice::AllowWindow => format!(
                "For {} minutes, across tasks, routine questions are answered Yes without asking. {ALLOW_WINDOW_NOTE}",
                (confirm::ALLOW_WINDOW_S / 60.0).round() as i64
            ),
        }
    }
}

/// The choices FERB's check offers for these needs, in the card's order: Yes, then "Send to every site in this task"
/// when every need is the file question, then No, then the 30-minute answer when every need is routine.
pub fn choices(needs: &[Need]) -> Vec<Choice> {
    let mut out = vec![Choice::Yes];
    if !needs.is_empty() && needs.iter().all(file_grants::is_file_need) {
        out.push(Choice::EverySite);
    }
    out.push(Choice::No);
    if needs.iter().all(confirm::routine) {
        out.push(Choice::AllowWindow);
    }
    out
}

/// [`choices`] as the question's options.
pub fn options(needs: &[Need]) -> Vec<String> {
    choices(needs)
        .into_iter()
        .map(|c| c.option().to_string())
        .collect()
}

/// How a typed answer works on FERB's own checks (GATE-510, GATE-512).
pub const TYPED: &str = "FERB's checks are answered by the owner's click on a choice, or by the owner's own typed \
words when they are exactly a choice's words or its button's words. Other words typed on the card go to the task as \
the owner's correction and the action doesn't run; other words in chat apply nothing, and FERB asks which choice it \
is. Nobody else's words count: not a page's, a file's, a message's or the AI's.";

/// What no answer can set.
pub const CANNOT: &str = "No answer sets a condition code can't check (\"only job sites\"), a Yes for every file or \
every site beyond the choices above, a Yes that outlasts what its choice says, or \"never ask again\". Those are \
not applied: FERB says so and offers the choices that exist.";

/// What a standing permission is (D30, ORCH-503, GATE-221).
pub const STANDING: &str = "A big job's plan can state standing send permissions: sends to sites or people the \
owner typed in the request (each one must be in the owner's own words), for that job only. They cover only the \
questions marked standing_permission_can_cover, and only for those exact sites or people.";

/// Rules that are recorded but never asked by themselves: the wider key a terminal Yes records (GATE-586), and the
/// carrier of a site's earlier No (`destination_key`, refused without asking).
pub const NOT_ASKED: [&str; 2] = [confirm::TERMINAL_APP, confirm::SINK_DESTINATION];

/// The kinds of question FERB's checks ask, by the rule that asks them (`ferb_policy::confirm` and the gateway).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Question {
    PrivateUrl,
    FileSend,
    FormSend,
    NewDestination,
    OutsideTyped,
    OutsideRecipient,
    OutsideAmount,
    /// GATE-620: several outside values a step lists before typing them, one question.
    OutsideList,
    NewRecipient,
    UnseenRecipient,
    Pay,
    AccountSecurity,
    DeletePermanent,
    BulkDelete,
    BulkSend,
    TerminalInput,
    ReadLift,
    WriteLift,
    AdminLift,
    ServiceLift,
    ForegroundScript,
    Foreground,
    BrowserPage,
    Pair,
    TlsOff,
    /// ORCH-676: the file about to be sent isn't the one the owner named.
    NotNamed,
    /// HAND-645: a step that would use the keyboard and pointer while the Mac's screen is locked: do it when he's back.
    LockedScreen,
}

impl Question {
    pub const ALL: [Question; 27] = [
        Question::PrivateUrl,
        Question::FileSend,
        Question::FormSend,
        Question::NewDestination,
        Question::OutsideTyped,
        Question::OutsideRecipient,
        Question::OutsideAmount,
        Question::OutsideList,
        Question::NewRecipient,
        Question::UnseenRecipient,
        Question::Pay,
        Question::AccountSecurity,
        Question::DeletePermanent,
        Question::BulkDelete,
        Question::BulkSend,
        Question::TerminalInput,
        Question::ReadLift,
        Question::WriteLift,
        Question::AdminLift,
        Question::ServiceLift,
        Question::ForegroundScript,
        Question::Foreground,
        Question::BrowserPage,
        Question::Pair,
        Question::TlsOff,
        Question::NotNamed,
        Question::LockedScreen,
    ];

    /// The rule that asks it and example facts (placeholders in angle brackets), as the card is built from.
    pub fn example(self) -> Need {
        let (rule, values): (&str, &[(&str, &str)]) = match self {
            Question::PrivateUrl => (
                "sink:private_url",
                &[("destination", "<site>"), ("value", "<your value>")],
            ),
            Question::FileSend => (
                "sink:new_destination",
                &[
                    ("destination", "<site>"),
                    ("files", "<your file>"),
                    ("how", "upload"),
                ],
            ),
            Question::FormSend => (
                "sink:new_destination",
                &[
                    ("destination", "<site>"),
                    ("form", "1"),
                    ("values", "<your value>"),
                ],
            ),
            Question::NewDestination => ("sink:new_destination", &[("destination", "<site>")]),
            Question::OutsideTyped => (
                "sink:outside_value",
                &[
                    ("what", "typed"),
                    ("value", "<value>"),
                    ("places", "<page>"),
                ],
            ),
            Question::OutsideRecipient => (
                "sink:outside_value",
                &[
                    ("what", "recipient"),
                    ("value", "<address>"),
                    ("places", "<page>"),
                ],
            ),
            Question::OutsideAmount => (
                "sink:outside_value",
                &[
                    ("what", "amount"),
                    ("value", "<amount>"),
                    ("places", "<page>"),
                ],
            ),
            Question::OutsideList => (
                confirm::OUTSIDE_LIST,
                &[
                    ("count", "<n>"),
                    ("noun", "addresses"),
                    ("values", "<address>, <address>"),
                    ("lines", "<address> — <page>\n<address> — <page>"),
                    ("origin", "<page>"),
                    ("from", "<page>"),
                    ("site", "<site>"),
                ],
            ),
            Question::NewRecipient => ("floor:new_recipient", &[("recipients", "<person>")]),
            Question::UnseenRecipient => ("floor:new_recipient", &[("place", "<app>")]),
            Question::Pay => (
                confirm::ALWAYS_PAY,
                &[("amount", "<amount>"), ("merchant", "<merchant>")],
            ),
            Question::AccountSecurity => (
                confirm::ALWAYS_ACCOUNT_SECURITY,
                &[("account", "<account>"), ("site", "<site>")],
            ),
            Question::DeletePermanent => ("floor:delete_permanent", &[("path", "<path>")]),
            Question::BulkDelete => (
                confirm::BULK,
                &[
                    ("what", "delete"),
                    ("path", "<folder>"),
                    ("files", "<n> files"),
                    ("folders", "<n> folders"),
                    ("bytes", "<size>"),
                ],
            ),
            Question::BulkSend => (
                confirm::BULK,
                &[("what", "send"), ("count", "<n>"), ("sample", "<people>")],
            ),
            Question::TerminalInput => (
                confirm::TERMINAL_INPUT,
                &[
                    ("app", "<terminal app>"),
                    ("text", "<command>"),
                    ("enter", "1"),
                    ("wide", "1"),
                ],
            ),
            Question::ReadLift => ("escape:<rule>", &[("lift", "read"), ("path", "<folder>")]),
            Question::WriteLift => ("escape:<rule>", &[("lift", "write"), ("path", "<path>")]),
            Question::AdminLift => (
                "escape:<rule>",
                &[("lift", "admin"), ("command", "<command>")],
            ),
            Question::ServiceLift => (
                "escape:<rule>",
                &[("lift", "service"), ("command", "<command>")],
            ),
            Question::ForegroundScript => (
                confirm::FOREGROUND,
                &[
                    ("app", "<browser>"),
                    ("script", "1"),
                    ("script_text", "<script>"),
                ],
            ),
            Question::Foreground => (confirm::FOREGROUND, &[("app", "<app>")]),
            Question::LockedScreen => (confirm::LOCKED_SCREEN, &[("app", "<app>")]),
            Question::BrowserPage => (
                confirm::BROWSER_PAGE,
                &[("app", "<browser>"), ("page", "chrome://settings")],
            ),
            Question::Pair => (
                confirm::PAIR,
                &[("browser", "<browser>"), ("name", "<profile>")],
            ),
            Question::TlsOff => (confirm::TLS_OFF, &[("command", "<command>")]),
            Question::NotNamed => (
                ferb_policy::named::NOT_NAMED,
                &[
                    ("thing", "file"),
                    ("name", "<file>"),
                    ("path", "<path>"),
                    ("site", "<site>"),
                    ("case", "elsewhere"),
                    (
                        "text",
                        "the file attached on <site> isn't <file> from your Mac",
                    ),
                ],
            ),
        };
        let mut v = Values::new();
        for (k, val) in values {
            v.set(k, val);
        }
        let mut n = Need::new(rule, confirm::key(&[json!(rule)]), v);
        n.always = matches!(self, Question::Pay | Question::AccountSecurity);
        if self == Question::OutsideRecipient {
            // an address's Yes makes it known (SQ6), as `decide` records it
            n.destinations = vec![("email".into(), "<address>".into())];
        }
        n
    }
}

/// No app is running: the example cards name the placeholders as they are.
struct NoApps;
impl Apps for NoApps {
    fn candidates(&self, _name: &str) -> Result<Vec<RunningApp>, AppsUnread> {
        Ok(Vec::new())
    }
    fn installed_bundle(&self, _name: &str) -> Result<Option<String>, AppsUnread> {
        Ok(None)
    }
    fn is_browser_app(&self, _app: &str) -> bool {
        false
    }
}

/// The card's own sentences about what a Yes covers; for a card that has none, what the gateway keeps by its need's
/// flags (a Yes spent on one run, else the same action for the rest of the task).
pub fn yes_covers(n: &Need, detail: &str) -> String {
    let said: Vec<&str> = detail
        .split_inclusive(". ")
        .map(str::trim)
        .filter(|s| s.contains("Yes"))
        .collect();
    if !said.is_empty() {
        return said.join(" ");
    }
    if n.always {
        "A Yes covers only this one run of the action.".into()
    } else {
        "A Yes covers this same action for the rest of this task.".into()
    }
}

/// `about.read section=answers`: every kind of question FERB's checks ask, its choices and how long each lasts, how a
/// typed answer counts, what no answer sets, and standing permissions. Computed now from the tables above.
pub fn lookup() -> Value {
    let mut questions: Vec<Value> = Vec::new();
    let mut alike: Vec<Choice> = Vec::new();
    for q in Question::ALL {
        let need = q.example();
        let card = confirm::card(std::slice::from_ref(&need), "", &NoApps).unwrap_or_default();
        let mut labels: Vec<String> = if card.yes.is_empty() {
            Vec::new()
        } else {
            vec![
                card.yes.clone(),
                if card.no.is_empty() {
                    NO.into()
                } else {
                    card.no.clone()
                },
            ]
        };
        let chosen = choices(std::slice::from_ref(&need));
        let mut note = String::new();
        if chosen.contains(&Choice::EverySite) {
            file_grants::card(&mut labels, &mut note, std::slice::from_ref(&need));
        }
        if !labels.is_empty() && chosen.contains(&Choice::AllowWindow) {
            labels.push(ALLOW_WINDOW_LABEL.into());
        }
        let detail = card.detail.clone();
        let rows: Vec<Value> = chosen
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let button = labels
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| c.option().to_string());
                // what a Yes covers is the card's own; the other choices last alike everywhere (listed once below)
                match c {
                    Choice::Yes => {
                        json!({"button": button, "answer": c.option(), "lasts": yes_covers(&need, &detail)})
                    }
                    Choice::EverySite => {
                        json!({"button": button, "answer": c.option(), "lasts": c.lasts()})
                    }
                    Choice::No | Choice::AllowWindow => {
                        if !alike.contains(c) {
                            alike.push(*c);
                        }
                        json!({"button": button, "answer": c.option()})
                    }
                }
            })
            .collect();
        let mut row = Map::new();
        row.insert("rule".into(), json!(need.rule));
        row.insert("asks".into(), json!(card.question));
        row.insert("choices".into(), Value::Array(rows));
        row.insert(
            "standing_permission_can_cover".into(),
            json!(STANDING_RULES.contains(&need.rule.as_str())),
        );
        questions.push(Value::Object(row));
    }
    let retry: Vec<Value> = [(RETRY_YES, RETRY_LABELS[0]), (RETRY_NO, RETRY_LABELS[1])]
        .iter()
        .map(|(o, l)| json!({"button": l, "answer": o, "lasts": "this one retry of an action whose outcome is unknown"}))
        .collect();
    let alike: Vec<Value> = alike
        .iter()
        .map(|c| json!({"answer": c.option(), "lasts": c.lasts()}))
        .collect();
    json!({
        "confirmation_questions": questions,
        "the_other_choices_last": alike,
        "retry_question": {"choices": retry},
        "typed_answers": TYPED,
        "no_answer_sets": CANNOT,
        "standing_permissions": STANDING,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every rule string the policy and the gateway write (`"floor:…"`, `"sink:…"`, `"escape:…"` and the policy's rule
    /// constants), from their sources.
    fn rules_in_sources() -> std::collections::BTreeSet<String> {
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut found = std::collections::BTreeSet::new();
        let rx = fancy_regex::Regex::new(r#""((?:floor|sink):[a-z_]+|escape:)"#).unwrap();
        for dir in [here.join("src"), here.join("../ferb-policy/src")] {
            for e in std::fs::read_dir(&dir).unwrap() {
                let p = e.unwrap().path();
                if p.extension().is_some_and(|x| x == "rs") && !p.ends_with("answers.rs") {
                    let text = std::fs::read_to_string(&p).unwrap();
                    for m in rx.captures_iter(&text) {
                        found.insert(m.unwrap()[1].to_string());
                    }
                }
            }
        }
        found
    }

    // The lookup lists every kind of question the gateway can raise: a rule added to the policy or the gateway
    // without a kind here fails this test.
    // decision: PRM-355
    // decision: PRM-356
    #[test]
    fn every_rule_ferb_can_ask_is_in_the_lookup() {
        let listed: Vec<String> = lookup()["confirmation_questions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|q| q["rule"].as_str().unwrap().to_string())
            .collect();
        let rules = rules_in_sources();
        assert!(rules.len() >= 15, "the scan found the rules: {rules:?}");
        for rule in rules {
            let known = listed
                .iter()
                .any(|l| *l == rule || (rule == "escape:" && l.starts_with("escape:")));
            assert!(
                known || NOT_ASKED.contains(&rule.as_str()),
                "{rule} can be asked but answers::Question has no kind for it"
            );
        }
    }

    // Each kind's card builds from its example, and every choice the gateway offers it is listed with its button and
    // how long it lasts.
    // decision: PRM-355
    #[test]
    fn each_kind_lists_the_choices_the_gateway_offers_with_their_lifetimes() {
        let looked = lookup();
        println!("{}", ferb_py::json::dumps_utf8(&looked));
        let rows = looked["confirmation_questions"].as_array().unwrap();
        assert_eq!(rows.len(), Question::ALL.len());
        for (q, row) in Question::ALL.iter().zip(rows) {
            let need = q.example();
            assert!(
                confirm::card(std::slice::from_ref(&need), "", &NoApps).is_ok(),
                "{q:?}: its card builds"
            );
            let answers: Vec<&str> = row["choices"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["answer"].as_str().unwrap())
                .collect();
            assert_eq!(answers, options(std::slice::from_ref(&need)), "{q:?}");
            for c in row["choices"].as_array().unwrap() {
                assert!(!c["button"].as_str().unwrap().is_empty(), "{q:?}: {c}");
                let lasts = c.get("lasts").cloned().or_else(|| {
                    looked["the_other_choices_last"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|o| o["answer"] == c["answer"])
                        .map(|o| o["lasts"].clone())
                });
                assert!(
                    lasts.is_some_and(|l| !l.as_str().unwrap().is_empty()),
                    "{q:?}: how long {c} lasts"
                );
            }
            assert!(!row["asks"].as_str().unwrap().is_empty(), "{q:?}");
        }
    }

    // decision: GATE-593
    // decision: GATE-595
    #[test]
    fn the_file_question_offers_three_choices_in_order_and_others_keep_theirs() {
        let file = Question::FileSend.example();
        assert_eq!(
            options(std::slice::from_ref(&file)),
            vec![YES, file_grants::EVERY_SITE, NO]
        );
        let form = Question::FormSend.example();
        assert_eq!(options(&[file.clone(), form.clone()]), vec![YES, NO]);
        assert_eq!(options(&[form]), vec![YES, NO]);
        assert_eq!(
            options(&[Question::Pair.example()]),
            vec![YES, NO, ALLOW_WINDOW],
            "routine questions keep the 30-minute answer"
        );
    }
}
