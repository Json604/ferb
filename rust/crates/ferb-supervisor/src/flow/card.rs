//! Several asks in one message and their card (ORCH-319, ORCH-532, ORCH-535, ORCH-543): one card per multi-part
//! turn of a task, a row per part (ferb-orchestrator's `parts::Card`), each answer posted once when its step's check
//! accepts it, a row whose step can never run settled at once, and the task's end an index of the rows with code's
//! count ("N of M done"). supervisor.py:4713 `_card_payload`, :4767 `_part_event`, :4851 `card_state`, :4910
//! `_part_of`, :5374 `_post_part`, :5410 `_said`, :5415 `_couldnt_do`, :5430 `_close_rows`, :5453 `_never_ran`,
//! :7495 `_final_card`.

use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use ferb_core::types::Outcome;
use ferb_orchestrator::models::{Part, Step};
use ferb_orchestrator::parts::{composed, Card, STOPPED as STOPPED_ROW};
use ferb_orchestrator::plans::DONE;
use ferb_orchestrator::ports::{Journal, JournalEntry};
use serde_json::{json, Map, Value};

use super::tree::step_task;
use super::{Inner, Tree};
use crate::words::{
    COULDNT_DO, NOT_ACCEPTED, REPORT_SHORT_CHARS, STOPPED_BY_YOU, UNCONFIRMED_LINE,
};

/// A card and the part events it emitted that are not yet told (its `emit` only queues them: Python's `emit` reads
/// the card's count while the card is still posting, which here waits until the card's lock is let go).
pub struct TaskCard {
    cell: Mutex<CardCell>,
    events: Arc<Mutex<Vec<Value>>>,
}

/// `parts::Card` holds its journal as an `Rc` (a single-threaded handle), so it is not `Send` by itself.
pub struct CardCell(Card);

// SAFETY: the only `Rc` inside a `Card` is its `journal`, made by `Inner::new_card` for that one card and never cloned
// (`Card` has no `Clone` and no method hands the `Rc` out), so its count is touched only through the card itself;
// the card is only ever reached through the `Mutex` in `TaskCard`, one thread at a time. Moving the whole card to
// another thread therefore moves every handle to the `Rc`'s allocation with it. Its other fields (`Arc` closures that
// are `Send + Sync`, strings, maps) are `Send`.
unsafe impl Send for CardCell {}

impl TaskCard {
    pub fn lock(&self) -> MutexGuard<'_, CardCell> {
        self.cell.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl std::ops::Deref for CardCell {
    type Target = Card;
    fn deref(&self) -> &Card {
        &self.0
    }
}

impl std::ops::DerefMut for CardCell {
    fn deref_mut(&mut self) -> &mut Card {
        &mut self.0
    }
}

/// FERB's journal as `parts::Card` reads and writes it.
struct CardJournal(Weak<Inner>);

impl Journal for CardJournal {
    fn append(&self, kind: &str, data: Value, origin: Option<&str>) -> JournalEntry {
        let inner = self.0.upgrade().expect("the supervisor");
        let e = inner.journal(kind, data, origin);
        JournalEntry {
            id: e.id,
            at: e.at,
            kind: e.kind,
            data: e.data,
            origin: e.origin,
        }
    }

    fn of_kind(&self, kind: &str, field: &str, equals: &str) -> Vec<JournalEntry> {
        let inner = self.0.upgrade().expect("the supervisor");
        inner
            .of_kind(kind, field, equals, None)
            .into_iter()
            .map(|e| JournalEntry {
                id: e.id,
                at: e.at,
                kind: e.kind,
                data: e.data,
                origin: e.origin,
            })
            .collect()
    }
}

impl Inner {
    /// supervisor.py:4552: a multi-part task's card (its rows' events go on the task's line and the bus).
    pub(crate) fn new_card(
        &self,
        tree_id: &str,
        request_id: &str,
        parts: Vec<Part>,
    ) -> Arc<TaskCard> {
        let events: Arc<Mutex<Vec<Value>>> = Arc::default();
        let queue = events.clone();
        let me = self.me.clone();
        let made_for = tree_id.to_string();
        let clock = self.deps.clock.clone();
        let card = Card::new(
            Rc::new(CardJournal(self.me.clone())),
            tree_id,
            request_id,
            parts,
            Arc::new(move |event: Value| queue.lock().unwrap().push(event)),
            Arc::new(move || {
                me.upgrade()
                    .map(|i| i.task_made(&made_for))
                    .unwrap_or_default()
            }),
            // parts.py `_next_seq` reads the wall clock: FERB's clock here
            Arc::new(move || {
                let now = ferb_clock::py::utc(&clock.now());
                ferb_py::time::DateTime::from_isoformat(&ferb_clock::py::iso(&now)).expect("a time")
            }),
        );
        Arc::new(TaskCard {
            cell: Mutex::new(CardCell(card)),
            events,
        })
    }

    /// The card of a tree's root, if the task asks several things.
    pub(crate) fn card_of(&self, tree_id: &str) -> Option<Arc<TaskCard>> {
        self.root(tree_id).and_then(|r| r.lock().card.clone())
    }

    /// One change to a card, then the part events it made, told (`_part_event`).
    pub(crate) fn with_card<T>(
        &self,
        tree_id: &str,
        card: &Arc<TaskCard>,
        f: impl FnOnce(&mut Card) -> T,
    ) -> T {
        let out = {
            let mut cell = card.lock();
            f(&mut cell)
        };
        let events: Vec<Value> = std::mem::take(&mut *card.events.lock().unwrap());
        for event in events {
            if let Value::Object(event) = event {
                self.part_event(tree_id, card, event);
            }
        }
        out
    }

    /// supervisor.py:4767 `_part_event`: a row's change, on the task's line and the bus (`task.part`), with the
    /// card's count after the change.
    fn part_event(&self, tree_id: &str, card: &Arc<TaskCard>, mut event: Map<String, Value>) {
        if let Some(step) = event
            .get("step")
            .filter(|v| ferb_py::value::truthy(v))
            .map(ferb_py::value::py_str)
        {
            event.insert("step_task_id".into(), json!(step_task(tree_id, &step)));
        }
        let (done, total) = card.lock().tally();
        event.insert("done".into(), json!(done));
        event.insert("total".into(), json!(total));
        self.tell_tree(tree_id, event.clone());
        self.deps.daemon.publish("task.part", Value::Object(event));
    }

    /// supervisor.py:4713 `_card_payload`: the card as the app draws it (APP-300).
    pub(crate) fn card_payload(
        &self,
        tree: &Arc<Tree>,
        card: &Arc<TaskCard>,
    ) -> Map<String, Value> {
        let (done, total, rows, request_id, stopped) = {
            let c = card.lock();
            let (done, total) = c.tally();
            (done, total, c.rows(), c.request_id.clone(), c.stopped())
        };
        let rows: Vec<Value> = rows
            .into_iter()
            .map(|mut row| {
                if let Some(step) = row
                    .get("step")
                    .filter(|v| ferb_py::value::truthy(v))
                    .map(ferb_py::value::py_str)
                {
                    row.insert("step_task_id".into(), json!(step_task(&tree.id, &step)));
                }
                Value::Object(row)
            })
            .collect();
        let mut out = Map::new();
        out.insert("request_id".into(), json!(request_id));
        out.insert("rows".into(), Value::Array(rows));
        out.insert("done".into(), json!(done));
        out.insert("total".into(), json!(total));
        if stopped > 0 {
            out.insert("stopped".into(), json!(stopped));
        }
        out.insert("steps".into(), Value::Object(self.step_payload(tree)));
        out.insert("plan".into(), Value::Array(self.plan_lines(&tree.id)));
        out
    }

    /// supervisor.py:4851 `card_state`: a running multi-part task's card now (chat.status).
    pub fn card_state(&self, tree_id: &str) -> Option<Value> {
        let tree = self.live().tree_of.get(tree_id).cloned()?;
        let card = tree.lock().card.clone()?;
        Some(Value::Object(self.card_payload(&tree, &card)))
    }

    /// supervisor.py:4910 `_part_of`: the row a step of this tree answers, when the tree is a multi-part task's.
    pub(crate) fn part_of(&self, tree_id: &str, step_id: &str) -> Option<(Arc<TaskCard>, Part)> {
        let card = self.card_of(tree_id)?;
        let part = card.lock().part_of(step_id).cloned()?;
        Some((card, part))
    }

    /// supervisor.py:5410 `_said`: model-written words as the owner reads them (redacted, D33's mark).
    pub(crate) fn said(&self, text: &str) -> String {
        let t = ferb_orchestrator::py::strip(text);
        if t.is_empty() {
            return String::new();
        }
        ferb_policy::confirm::from_task_lines(&self.redact_text(t))
    }

    /// supervisor.py:5374 `_post_part`: the row this step answers fills once (its checked answer and at most one
    /// "Couldn't confirm" line), or says it couldn't be done; a stop posts nothing. Then the corrections the step's
    /// worker reported update rows already posted.
    pub(crate) fn post_part(
        &self,
        tree: &Arc<Tree>,
        step_id: &str,
        member: Option<&Arc<Tree>>,
        status: &str,
        output: &Map<String, Value>,
    ) {
        let card = tree.lock().card.clone();
        let Some(card) = card else {
            return;
        };
        if self.stopped(tree) || self.live().suspending {
            return;
        }
        let part = card.lock().part_of(step_id).cloned();
        let task_id = member.map(|m| m.worker.id.clone()).unwrap_or_default();
        let done = DONE.contains(&status);
        if let Some(part) = &part {
            let declined = match member {
                Some(m) => m.lock().declined,
                None => output.get("declined").is_some_and(ferb_py::value::truthy),
            };
            if done && declined {
                let said = match member {
                    Some(m) => m.lock().owner_answer.clone(),
                    None => output.get("answer").map(text_of).unwrap_or_default(),
                };
                let said = if said.is_empty() {
                    ferb_orchestrator::answer_marks::DECLINED_LINE.to_string()
                } else {
                    said
                };
                let row = self.said(&said);
                self.with_card(&tree.id, &card, |c| {
                    c.settle(part, STOPPED_ROW, &row, "", false, &task_id)
                });
            } else if done {
                let checked = member.and_then(|m| {
                    let out = m.lock().outcome.clone()?;
                    Some(self.checked_answer(m, &out))
                });
                let (text, note, verified) = match checked {
                    Some(c) => c,
                    None => {
                        let text = output
                            .get("answer")
                            .filter(|v| ferb_py::value::truthy(v))
                            .or_else(|| output.get("summary").filter(|v| ferb_py::value::truthy(v)))
                            .map(text_of)
                            .unwrap_or_default();
                        let note = output
                            .get("note")
                            .filter(|v| ferb_py::value::truthy(v))
                            .map(text_of)
                            .unwrap_or_default();
                        let verified = output.get("verified").is_some_and(ferb_py::value::truthy);
                        (text, note, verified)
                    }
                };
                let mut shown = ferb_orchestrator::py::strip(&text).to_string();
                if !note.is_empty() && !verified {
                    shown.push_str(&format!("\n\n{UNCONFIRMED_LINE}"));
                }
                let row = self.said(&shown);
                self.with_card(&tree.id, &card, |c| {
                    c.settle(part, "done", &row, "", verified, &task_id)
                });
            } else if ["failed", "blocked", "needs_user"].contains(&status) {
                let row = self.said(&self.couldnt_do(member, output));
                self.with_card(&tree.id, &card, |c| {
                    c.settle(part, "failed", &row, "", false, &task_id)
                });
            }
        }
        if done {
            if let Some(m) = member {
                let updates = m.lock().part_updates.clone();
                for update in updates {
                    let id = update.get("part").map(text_of).unwrap_or_default();
                    let target = card.lock().parts.iter().find(|p| p.id == id).cloned();
                    let text = ferb_orchestrator::py::strip(
                        &update.get("text").map(text_of).unwrap_or_default(),
                    )
                    .to_string();
                    let why = ferb_orchestrator::py::strip(
                        &update.get("why").map(text_of).unwrap_or_default(),
                    )
                    .to_string();
                    if let Some(target) = target.filter(|t| t.direct.is_none() && !text.is_empty())
                    {
                        let (text, why) = (self.said(&text), self.said(&why));
                        self.with_card(&tree.id, &card, |c| {
                            c.update(&target, &text, &why, &task_id)
                        });
                    }
                }
            }
        }
    }

    /// supervisor.py:5415 `_couldnt_do`: a failed row's line, "Couldn't do:" and why in the owner's terms.
    pub(crate) fn couldnt_do(
        &self,
        member: Option<&Arc<Tree>>,
        output: &Map<String, Value>,
    ) -> String {
        let question = output
            .get("question")
            .filter(|v| ferb_py::value::truthy(v))
            .map(text_of)
            .unwrap_or_default();
        let mut why = String::new();
        if let Some(m) = member {
            let (stop_reason, reason) = {
                let st = m.lock();
                (st.stop_reason.clone(), st.reason.clone())
            };
            if !stop_reason.is_empty() || !reason.is_empty() {
                let outcomes = self.results(m).1;
                let given = if stop_reason.is_empty() {
                    &reason
                } else {
                    &stop_reason
                };
                let (lead, own) = self.told_failure(m, &outcomes, given);
                let reason = ferb_orchestrator::py::strip(&reason).to_string();
                why = if !lead.is_empty() {
                    lead
                } else if !reason.is_empty() {
                    reason
                } else {
                    own
                };
            }
        }
        if why.is_empty() {
            why = if !question.is_empty() {
                format!("I need your answer: {question}")
            } else {
                output
                    .get("summary")
                    .filter(|v| ferb_py::value::truthy(v))
                    .map(text_of)
                    .unwrap_or_else(|| NOT_ACCEPTED.to_string())
            };
        }
        format!(
            "{COULDNT_DO} {}",
            ferb_orchestrator::py::strip(&crate::lines::sentence(&why))
        )
    }

    /// supervisor.py:5430 `_close_rows`: when the plan run ends (not by a stop or a shutdown), a row that never filled
    /// says why: its step's saved answer (finished before a restart), its reason, or the step it needed.
    pub(crate) fn close_rows(
        &self,
        tree: &Arc<Tree>,
        states: &[(String, ferb_orchestrator::plan_store::StepState)],
    ) {
        let card = tree.lock().card.clone();
        let Some(card) = card else {
            return;
        };
        if self.stopped(tree) || self.live().suspending {
            return;
        }
        let steps: Vec<Step> = self
            .plans(|p| p.load(&tree.id))
            .map(|l| l.plan.steps)
            .unwrap_or_default();
        let unposted = card.lock().unposted();
        for part in unposted {
            let key = part.answer_step.clone().unwrap_or_else(|| "None".into());
            let state = states
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, s)| s.clone());
            let status = state
                .as_ref()
                .map(|s| s.status.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "pending".into());
            let mut output = state
                .as_ref()
                .and_then(|s| s.output.as_object().cloned())
                .unwrap_or_default();
            if DONE.contains(&status.as_str()) {
                self.post_part(tree, &key, None, &status, &output);
                continue;
            }
            if !["failed", "blocked", "needs_user", "cancelled"].contains(&status.as_str()) {
                output = never_ran(&key, &steps, states);
            }
            let row = self.said(&self.couldnt_do(None, &output));
            self.with_card(&tree.id, &card, |c| {
                c.settle(&part, "failed", &row, "", false, "")
            });
        }
    }

    /// The row of a step that can never run (`_step_state`'s `wont_run`, ORCH-532): settled at once as couldn't-do.
    pub(crate) fn settle_wont_run(&self, root: &Arc<Tree>, step_id: &str) {
        let Some((card, part)) = self.part_of(&root.id, step_id) else {
            return;
        };
        if self.stopped(root) || !card.lock().unposted().iter().any(|p| p.id == part.id) {
            return;
        }
        let states = self.plans(|p| p.steps(&root.id)).unwrap_or_default();
        let steps: Vec<Step> = self
            .plans(|p| p.load(&root.id))
            .map(|l| l.plan.steps)
            .unwrap_or_default();
        let row = self.said(&self.couldnt_do(None, &never_ran(step_id, &steps, &states)));
        self.with_card(&root.id, &card, |c| {
            c.settle(&part, "failed", &row, "", false, "")
        });
    }

    /// supervisor.py:7495 `_final_card`: a multi-part task's end. Code adds the done count and its fixed lines; a
    /// stop or a failure of the whole task says so above them. The answer kept for the record is an index of the rows
    /// (`parts.composed`); `card` carries the footer for the app.
    pub(crate) fn final_card(&self, tree: &Arc<Tree>, card: &Arc<TaskCard>) -> Map<String, Value> {
        let out = tree.lock().outcome.clone().unwrap_or_else(|| Outcome {
            task_id: tree.worker.id.clone(),
            status: ferb_core::types::OutcomeStatus::Failed,
            summary: "No report was received.".into(),
            artifacts: Vec::new(),
            evidence: Vec::new(),
        });
        let sites = self.deps.daemon.sites_contacted(&tree.id);
        let lines = self.code_lines(tree, &sites);
        let mut artifacts = out.artifacts.clone();
        artifacts.sort();
        let cards = self.result_cards(tree, &artifacts, &sites);
        let (ended, records, parts, line, answer_index) = {
            let c = card.lock();
            (
                c.ended().to_string(),
                c.records(),
                c.parts.clone(),
                c.line(),
                composed(&c, &lines, &artifacts, REPORT_SHORT_CHARS),
            )
        };
        let rows: Vec<&Part> = parts
            .iter()
            .filter(|p| {
                p.direct.is_none()
                    && records
                        .get(&p.id)
                        .and_then(|r| r.get("state"))
                        .and_then(Value::as_str)
                        != Some(STOPPED_ROW)
            })
            .collect();
        let (stop, stop_reason, runtime_switch, resources) = {
            let st = tree.lock();
            (
                st.stop.clone(),
                st.stop_reason.clone(),
                st.runtime_switch.clone(),
                st.resources.clone(),
            )
        };
        let verified = stop.is_empty()
            && ended == "done"
            && rows.iter().all(|p| {
                records
                    .get(&p.id)
                    .and_then(|r| r.get("verified"))
                    .is_some_and(ferb_py::value::truthy)
            });
        let mut headline = String::new();
        let status: String;
        if stop == STOPPED_BY_YOU {
            status = self.stop_status(tree);
            headline = "Stopped, as you asked.".into();
        } else if !stop.is_empty() {
            status = self.stop_status(tree);
            headline = format!("I couldn't finish that: {stop}");
        } else if ended == "done" {
            status = if verified { "done" } else { "done_unverified" }.into();
        } else {
            status = ended.clone();
        }
        let mut answer = answer_index;
        if !headline.is_empty() {
            answer = format!("{headline}\n{answer}");
        }
        let answer = ferb_policy::confirm::from_task_lines(&answer);
        let summary = if headline.is_empty() {
            line.clone()
        } else {
            headline.clone()
        };
        let mut payload = self.card_payload(tree, card);
        payload.insert("line".into(), json!(line));
        payload.insert("lines".into(), json!(lines));
        if !headline.is_empty() {
            payload.insert("headline".into(), json!(headline));
        }
        let mut final_ = Map::new();
        final_.insert("type".into(), json!("outcome"));
        final_.insert("task_id".into(), json!(tree.id));
        final_.insert("status".into(), json!(status));
        final_.insert("summary".into(), json!(summary));
        final_.insert("answer".into(), json!(answer));
        final_.insert("verified".into(), json!(verified));
        final_.insert("verification".into(), json!([]));
        final_.insert(
            "subtasks".into(),
            json!({"worker": {"status": out.status.as_str(), "summary": out.summary}}),
        );
        final_.insert("artifacts".into(), json!(artifacts));
        final_.insert("resources".into(), Value::Array(resources));
        final_.insert("stop_reason".into(), json!(stop_reason));
        final_.insert("card".into(), Value::Object(payload));
        if let Some(sw) = runtime_switch.filter(|_| status == "failed") {
            final_.insert("runtime_switch".into(), sw);
        }
        if !cards.is_empty() {
            final_.insert("cards".into(), Value::Array(cards));
        }
        final_.extend(self.told_in(tree));
        final_
    }

    /// The step outcome of a multi-part task's plan run (`_execute_plan`): the rows are the answer.
    pub(crate) fn card_outcome(
        &self,
        tree: &Arc<Tree>,
        card: &Arc<TaskCard>,
        artifacts: Vec<String>,
        evidence: Vec<String>,
    ) -> Outcome {
        if self.stopped(tree) {
            let stop = tree.lock().stop.clone();
            let why = if stop.is_empty() {
                STOPPED_BY_YOU.to_string()
            } else {
                stop
            };
            return super::tree::outcome(&tree.worker.id, "cancelled", &why, artifacts, evidence);
        }
        let (ended, line) = {
            let c = card.lock();
            (c.ended().to_string(), c.line())
        };
        super::tree::outcome(&tree.worker.id, &ended, &line, artifacts, evidence)
    }

    /// supervisor.py:6932: what the owner already has, as of this attempt (a multi-part task's step's context).
    pub(crate) fn posted_answers(&self, tree_id: &str) -> Vec<Value> {
        let Some(card) = self.card_of(tree_id) else {
            return Vec::new();
        };
        let records = card.lock().records();
        // `records()` is a dict in Python: its order is the order each part was first recorded
        let mut ordered: Vec<(String, Map<String, Value>)> = records.into_iter().collect();
        let order = self.first_recorded(tree_id);
        ordered.sort_by_key(|(k, _)| order.iter().position(|o| o == k).unwrap_or(usize::MAX));
        ordered
            .into_iter()
            .filter(|(_, row)| {
                !row.get("direct").is_some_and(ferb_py::value::truthy)
                    && matches!(
                        row.get("state").and_then(Value::as_str),
                        Some("done") | Some("updated")
                    )
            })
            .map(|(_, row)| {
                let mut out = Map::new();
                for k in ["part", "label", "text"] {
                    if let Some(v) = row.get(k) {
                        out.insert(k.into(), v.clone());
                    }
                }
                Value::Object(out)
            })
            .collect()
    }

    /// The parts of a task in the order their first `chat.part` row was written.
    fn first_recorded(&self, tree_id: &str) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        for e in self.of_kind(ferb_orchestrator::parts::KIND, "task_id", tree_id, None) {
            let part = e.text("part");
            if !seen.contains(&part) {
                seen.push(part);
            }
        }
        seen
    }
}

/// supervisor.py:5453 `_never_ran`: why a step never ran: the step it needed that didn't finish (and its reason),
/// else the plan's end.
pub(crate) fn never_ran(
    key: &str,
    steps: &[Step],
    states: &[(String, ferb_orchestrator::plan_store::StepState)],
) -> Map<String, Value> {
    let state = |id: &str| states.iter().find(|(k, _)| k == id).map(|(_, s)| s);
    let ended: Vec<String> = steps
        .iter()
        .find(|s| s.id == key)
        .map(|s| s.depends_on.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|d| !state(d).is_some_and(|s| DONE.contains(&s.status.as_str())))
        .collect();
    let why = ended
        .iter()
        .filter_map(|d| state(d))
        .map(|s| s.output.get("summary").map(text_of).unwrap_or_default())
        .find(|w| !w.is_empty())
        .unwrap_or_default();
    let goal = ended
        .first()
        .and_then(|d| steps.iter().find(|s| &s.id == d))
        .map(|s| s.goal.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    let summary = if goal.is_empty() {
        "the plan stopped before it ran".to_string()
    } else {
        format!(
            "a step it needed (\u{201c}{}\u{201d}) didn't finish{}",
            ferb_orchestrator::lines::shorten(&goal, 120, true),
            if why.is_empty() {
                String::new()
            } else {
                format!(": {why}")
            }
        )
    };
    let mut out = Map::new();
    out.insert("summary".into(), json!(summary));
    out
}

fn text_of(v: &Value) -> String {
    ferb_py::value::py_str(v)
}

/// supervisor.py:304 `_overlaps`: two quotes share some of the owner's words in `words` (a part of one, quotes of
/// several messages joined by blank lines, sits where a part of the other does).
fn overlaps(words: &str, quote: &str, other: &str) -> bool {
    static BLANK: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let blank = BLANK.get_or_init(|| regex::Regex::new(r"\n[ \t]*\n").expect("a pattern"));
    let spans = |q: &str| -> Vec<(usize, usize)> {
        blank
            .split(q)
            .filter(|p| !ferb_orchestrator::py::strip(p).is_empty())
            .filter_map(|p| ferb_learning::text::owner_span_at(words, p))
            .collect()
    };
    let (mine, theirs) = (spans(quote), spans(other));
    mine.iter()
        .any(|(a, b)| theirs.iter().any(|(c, d)| a < d && c < b))
}

impl Inner {
    /// supervisor.py:1485 `_unbacked_parts` (T-1066): a part the planner says this turn's own entries answer (direct
    /// "") must be one an entry of this turn stands for: a memory, reminder, schedule or schedule change entry whose
    /// quote is (part of) the same words of the owner's as the part's quote.
    pub(crate) fn unbacked_parts(
        &self,
        turn: &Arc<std::sync::Mutex<super::Turn>>,
        out: &ferb_orchestrator::models::SupervisorOutcome,
    ) -> Vec<String> {
        let Some(task) = &out.task else {
            return Vec::new();
        };
        if !task.parts.iter().any(|p| p.direct.as_deref() == Some("")) {
            return Vec::new();
        }
        let quotes: Vec<&str> = out
            .remember
            .iter()
            .map(|e| e.quote.as_str())
            .chain(out.forget.iter().map(|e| e.quote.as_str()))
            .chain(out.timers.iter().map(|e| e.quote.as_str()))
            .chain(out.schedules.iter().map(|e| e.quote.as_str()))
            .chain(out.schedule_changes.iter().map(|e| e.quote.as_str()))
            .chain(out.reminder_cancels.iter().map(|e| e.quote.as_str()))
            .collect();
        let said: Vec<String> = self
            .said_in_chat_full(turn)
            .into_iter()
            .map(|s| s.words)
            .collect();
        task.parts
            .iter()
            .filter(|part| {
                part.direct.as_deref() == Some("")
                    && !said
                        .iter()
                        .any(|w| quotes.iter().any(|q| overlaps(w, &part.quote, q)))
            })
            .map(|part| {
                format!(
                    "part {}: direct \"\" says an entry of this turn answers it, but no remember, timers, schedules \
                     or schedule_changes entry quotes its words. Scheduling and reminders happen only through those \
                     entries (a worker can't make or change a schedule): add the entry, or answer the part with a \
                     step.",
                    part.id
                )
            })
            .collect()
    }
}
