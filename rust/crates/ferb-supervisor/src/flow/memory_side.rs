//! The memory and forget side entries of a turn (supervisor.py:3344 `_side_entries`' remember and forget parts):
//! each remembered item quote-checked against the owner's words (`_owner_said`, the message that holds the quote,
//! `_quote_source`), what it is about (`_remember_about`), where a login name it carries may be saved (`_turn_sites`,
//! `_sites_named_elsewhere`), saved through the memory port, which answers with code's receipt, line and card or why
//! it refused; a forget asks the owner first (the memory port's `forget_request`), never before their Yes (I2).
//! Also their part of `_side_quote_problems` (the one repair before the reply stands, ORCH-520, ORCH-522).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use ferb_orchestrator::models::{Remember, SupervisorOutcome};
use ferb_policy::small::blocked;
use ferb_sanitize::origin;
use serde_json::{json, Value};

use super::{Failure, Flow, Inner, Turn};
use crate::NotPorted;

/// supervisor.py:447 `_Said`: an owner message as a quote check reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Said {
    pub request_id: String,
    pub entry_id: String,
    pub words: String,
}

/// What the memory side entries did: events (in order), code's lines, cards, refusals to tell.
#[derive(Default)]
pub(crate) struct MemorySide {
    pub events: Vec<Value>,
    /// code's lines: what was saved, and each forget's line, in the order they were made
    pub saved: Vec<String>,
    /// The places in `saved` of lines FERB wrote whole (a forget's question): its own words are never tagged as the
    /// task's (GATE-581's kind; fixed against Python, which tagged "FERB deletes:").
    pub own: Vec<usize>,
    pub cards: Vec<Value>,
    pub unsaved: Vec<(String, String)>,
    pub stopped: Vec<(String, String)>,
    pub receipts: Vec<String>,
}

/// `Remember.model_dump()`, field order kept.
pub fn dump_remember(item: &Remember) -> Value {
    let int = |s: &str| serde_json::from_str::<Value>(s).unwrap_or(Value::Null);
    json!({"kind": item.kind, "subject": item.subject, "key": item.key, "content": item.content,
           "quote": item.quote, "login_origin": item.login_origin,
           "until": item.until.as_ref().map(|u| json!({"quote": u.quote, "day": u.day, "date": u.date,
               "in_days": u.in_days.as_ref().map(|n| int(&n.0))})),
           "replaces": item.replaces, "review_in_days": int(&item.review_in_days.0)})
}

/// owner_secrets.py:86 `site_host`: a web place's or a task site's host as a browser reads it; "" for an app.
fn site_host(place_or_site: &str) -> String {
    let text = ferb_orchestrator::py::strip(place_or_site);
    if text.is_empty() || text.starts_with("app:") {
        return String::new();
    }
    let url = if text.contains("://") {
        text.to_string()
    } else {
        format!("https://{text}")
    };
    ferb_policy::urls::parse(&url)
        .map(|p| p.host)
        .map(|h| h.trim_end_matches('.').to_string())
        .unwrap_or_default()
}

/// owner_secrets.py:104 `typed_site`: whether the owner typed `host`'s site (a whole token or a URL's host of their
/// words).
fn typed_site(host: &str, typed: &ferb_policy::provenance::Text) -> bool {
    let bare = host.strip_prefix("www.").unwrap_or(host);
    if bare.is_empty() {
        return false;
    }
    let www = format!("www.{bare}");
    [bare, www.as_str()]
        .iter()
        .any(|h| typed.tokens.contains(*h) || typed.hosts.contains(*h))
}

impl Inner {
    /// supervisor.py:1384 `_owner_words_in_chat` and :1399 `_said_in_chat`, with each message's request and entry.
    pub(crate) fn said_in_chat_full(&self, turn: &Arc<Mutex<Turn>>) -> Vec<Said> {
        let (first, lead, rid, session, current) = {
            let t = turn.lock().unwrap();
            (
                if t.typed_lines.is_empty() {
                    t.typed.clone()
                } else {
                    t.typed_lines.clone()
                },
                t.entries.first().map(|e| e.id.clone()).unwrap_or_default(),
                t.request_id.clone(),
                t.session.clone(),
                t.request_ids(),
            )
        };
        let mut said: Vec<Option<Said>> = Vec::new();
        for entry in self.of_kind("chat.message", "session_id", &session, Some("owner")) {
            if current.contains(&entry.request_id) {
                continue;
            }
            let held = origin::split(Some(&entry.origin));
            said.push(
                if !held.contains(origin::OWNER)
                    || held.contains(&origin::external("unlabeled", ""))
                {
                    None
                } else {
                    Some(Said {
                        request_id: entry.request_id.clone(),
                        entry_id: entry.id.clone(),
                        words: ferb_sanitize::untrusted::typed_lines_of(&entry.text("text")),
                    })
                },
            );
        }
        let recent: Vec<Said> = said
            .iter()
            .rev()
            .take(crate::words::RECENT_EXCHANGES)
            .flatten()
            .cloned()
            .collect();
        std::iter::once(Said {
            request_id: rid,
            entry_id: lead,
            words: first,
        })
        .chain(recent)
        .collect()
    }

    /// supervisor.py:1414 `_owner_said` (with `clause`, a forget's rule): the owner's words holding `quote`; this turn's
    /// names the message among its own that holds it (`_quote_source`).
    pub(crate) fn owner_said_full(
        &self,
        turn: &Arc<Mutex<Turn>>,
        quote: &str,
        clause: bool,
    ) -> Option<Said> {
        let said = self.said_in_chat_full(turn);
        if ferb_learning::text::quoted(&said[0].words, quote, clause) {
            return Some(Said {
                request_id: said[0].request_id.clone(),
                entry_id: self.quote_source(turn, quote),
                words: said[0].words.clone(),
            });
        }
        said.into_iter()
            .skip(1)
            .find(|s| ferb_learning::text::quoted(&s.words, quote, clause))
    }

    /// supervisor.py:3321 `_quote_source`: the message whose words hold a quote: this turn's messages first, then the
    /// earlier ones its question continued, newest first; the turn's lead message when none holds it alone.
    pub(crate) fn quote_source(&self, turn: &Arc<Mutex<Turn>>, quote: &str) -> String {
        let t = turn.lock().unwrap();
        for entry in t.entries.iter().chain(t.carried.iter().rev()) {
            let text = without_wrapped(&entry.text("text"));
            if ferb_learning::text::quoted(&text, quote, false) {
                return entry.id.clone();
            }
        }
        t.entries.first().map(|e| e.id.clone()).unwrap_or_default()
    }

    /// supervisor.py:3330 `_remember_about` (ORCH-312): what a remembered quote is about, from the task: the turn's
    /// new task's goal, else the goal of the task it controls; "" for none.
    pub(crate) fn remember_about(&self, out: &SupervisorOutcome, session: &str) -> String {
        if let Some(task) = &out.task {
            if !ferb_orchestrator::py::strip(&task.goal).is_empty() {
                return task.goal.clone();
            }
        }
        if let Some(control) = &out.control {
            let live = self.live().tree_of.get(&control.task_id).cloned();
            if let Some(tree) = live {
                return tree.spec().goal;
            }
            if let Ok(Some(saved)) = self.continuity(|c| c.load(&control.task_id, session)) {
                return saved.spec.goal;
            }
        }
        String::new()
    }

    /// supervisor.py:3278 `_turn_sites` (L36 §5): the hosts of the candidate sites the owner typed, where a login name
    /// they give may be saved.
    pub(crate) fn turn_sites(
        &self,
        out: &SupervisorOutcome,
        turn: &Arc<Mutex<Turn>>,
        named: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        let (typed, session, answered) = {
            let t = turn.lock().unwrap();
            (t.typed.clone(), t.session.clone(), t.answered_tree.clone())
        };
        let mut candidates: Vec<String> = out
            .task
            .as_ref()
            .map(|t| t.targets.sites.clone())
            .unwrap_or_default();
        candidates.extend(out.remember.iter().filter_map(|i| i.login_origin.clone()));
        let mut words = vec![typed];
        let mut trees: Vec<(String, BTreeSet<String>)> = Vec::new();
        if let Some(c) = &out.control {
            trees.push((c.task_id.clone(), named.clone()));
        }
        if let Some(a) = answered {
            trees.push((a, BTreeSet::new()));
        }
        for (tree_id, before) in trees {
            if let Ok(Some(saved)) = self.continuity(|c| c.load(&tree_id, &session)) {
                let live = self.live().tree_of.get(&tree_id).cloned();
                let sites = match live {
                    Some(t) => t.spec().targets.sites,
                    None => saved.spec.targets.sites.clone(),
                };
                candidates.extend(sites);
                candidates.extend(before);
                words.push(saved.words.clone());
            }
        }
        let owner = ferb_policy::provenance::prepare(&words.join("\n"));
        candidates
            .iter()
            .map(|s| site_host(s))
            .filter(|h| !h.is_empty() && typed_site(h, &owner))
            .collect()
    }

    /// supervisor.py:3303 `_sites_named_elsewhere`: the candidate sites the owner typed only for a task of another
    /// chat.
    pub(crate) fn sites_named_elsewhere(
        &self,
        out: &SupervisorOutcome,
        turn: &Arc<Mutex<Turn>>,
    ) -> BTreeSet<String> {
        let (session, answered) = {
            let t = turn.lock().unwrap();
            (t.session.clone(), t.answered_tree.clone())
        };
        let mut found = BTreeSet::new();
        let ids: Vec<String> = out
            .control
            .as_ref()
            .map(|c| c.task_id.clone())
            .into_iter()
            .chain(answered)
            .filter(|t| !t.is_empty())
            .collect();
        for tree_id in ids {
            let other = self
                .continuity(|c| c.conversation_of(&tree_id))
                .ok()
                .flatten();
            let Some(other) = other.filter(|o| *o != session) else {
                continue;
            };
            let Ok(Some(saved)) = self.continuity(|c| c.load(&tree_id, &other)) else {
                continue;
            };
            let live = self.live().tree_of.get(&tree_id).cloned();
            let mut candidates = match live {
                Some(t) => t.spec().targets.sites,
                None => saved.spec.targets.sites.clone(),
            };
            candidates.extend(out.remember.iter().filter_map(|i| i.login_origin.clone()));
            let owner = ferb_policy::provenance::prepare(&saved.words);
            found.extend(
                candidates
                    .iter()
                    .map(|s| site_host(s))
                    .filter(|h| !h.is_empty() && typed_site(h, &owner)),
            );
        }
        found
    }

    /// The remember and forget parts of supervisor.py:3344 `_side_entries`.
    pub(crate) async fn memory_side(
        &self,
        turn: &Arc<Mutex<Turn>>,
        out: &SupervisorOutcome,
        named: &BTreeSet<String>,
    ) -> Flow<MemorySide> {
        let mut side = MemorySide::default();
        if out.remember.is_empty() && out.forget.is_empty() {
            return Ok(side);
        }
        let (session, rid) = {
            let t = turn.lock().unwrap();
            (t.session.clone(), t.request_id.clone())
        };
        let recent = blocked::NOT_IN_RECENT;
        let (sites, elsewhere, about) = if out.remember.is_empty() {
            (BTreeSet::new(), BTreeSet::new(), String::new())
        } else {
            let sites = self.turn_sites(out, turn, named);
            let elsewhere: BTreeSet<String> = self
                .sites_named_elsewhere(out, turn)
                .difference(&sites)
                .cloned()
                .collect();
            (sites, elsewhere, self.remember_about(out, &session))
        };
        let sites: Vec<String> = sites.into_iter().collect();
        let elsewhere: Vec<String> = elsewhere.into_iter().collect();
        for item in &out.remember {
            // one at a time, so a refused one is known by name, with code's reason
            let (words, source) = match self.owner_said_full(turn, &item.quote, false) {
                Some(held) => (held.words, held.entry_id),
                None => (
                    turn.lock().unwrap().typed_lines.clone(),
                    self.quote_source(turn, &item.quote),
                ),
            };
            let items = vec![dump_remember(item)];
            let answers = self
                .deps
                .daemon
                .remember(&items, &words, &source, &sites, &elsewhere, &about)
                .await
                .ok_or_else(|| {
                    Failure::from(NotPorted("Supervisor._side_entries (no memory port)"))
                })?;
            let saved_now: Vec<&Value> = answers
                .iter()
                .filter(|r| r.get("receipt").is_some())
                .collect();
            side.receipts
                .extend(saved_now.iter().map(|r| text(&r["receipt"])));
            side.saved
                .extend(saved_now.iter().map(|r| text(&r["line"])));
            side.cards.extend(
                saved_now
                    .iter()
                    .filter_map(|r| r.get("card").filter(|c| c.is_object()).cloned()),
            );
            let guard = answers
                .iter()
                .find(|r| r.get("guard").is_some_and(ferb_py::value::truthy));
            if saved_now.is_empty() {
                if let Some(guard) = guard {
                    let g = text(&guard["guard"]);
                    let why = if g == "quote not in the owner's words" {
                        recent.to_string()
                    } else {
                        ferb_learning::text::capture_told(&g)
                            .unwrap_or_default()
                            .to_string()
                    };
                    side.stopped.push((
                        "memory".into(),
                        blocked::blocked(
                            blocked::MEMORY,
                            &why,
                            &format!("{} isn't saved", crate::lines::quoted(&[&item.content])),
                            "tell me again in your own words and I'll save it",
                        ),
                    ));
                } else {
                    let refused = answers
                        .iter()
                        .find_map(|r| {
                            r.get("refused")
                                .filter(|v| ferb_py::value::truthy(v))
                                .map(text)
                        })
                        .unwrap_or_else(|| "FERB's memory store gave no reason".into());
                    side.unsaved.push((item.content.clone(), refused));
                }
            }
        }
        turn.lock().unwrap().intake_receipts = side.receipts.clone();
        for receipt in &side.receipts {
            side.events.push(json!({"type": "memory", "text": receipt}));
        }
        // nothing goes before the owner's Yes to a question naming it (I2)
        for wanted in &out.forget {
            let line = if self.owner_said_full(turn, &wanted.quote, true).is_some() {
                self.deps
                    .daemon
                    .forget_memory(&wanted.item_id, &rid)
                    .ok_or_else(|| {
                        Failure::from(NotPorted("Supervisor._side_entries (no memory port)"))
                    })?
            } else {
                blocked::blocked(
                    blocked::FORGET,
                    recent,
                    "I didn't ask about deleting anything",
                    "say which memory to forget in a sentence of your own",
                )
            };
            side.events.push(json!({"type": "memory", "text": line}));
            side.own.push(side.saved.len());
            side.saved.push(line);
        }
        Ok(side)
    }

    /// The remember and forget parts of supervisor.py:1439 `_side_quote_problems`.
    pub(crate) fn memory_quote_problems(
        &self,
        turn: &Arc<Mutex<Turn>>,
        out: &SupervisorOutcome,
    ) -> Vec<String> {
        let mut problems = Vec::new();
        let tell = |what: &str, quote: &str| {
            format!(
                "{what}: its quote isn't the owner's exact words from this message or their recent messages in this \
                 chat, so code would refuse it: {}",
                ferb_py::json::dumps_utf8(&json!(quote))
            )
        };
        let repr = |id: &str| ferb_orchestrator::py::repr_str(id);
        for item in &out.remember {
            match self.owner_said_full(turn, &item.quote, false) {
                None => problems.push(tell(&format!("remember {}", repr(&item.key)), &item.quote)),
                Some(held) => {
                    if !ferb_learning::text::capture(&held.words, &item.content, &item.quote)
                        .refused
                        .is_empty()
                    {
                        problems.push(format!(
                            "remember {}: its quote doesn't say what content would save, so code would refuse it",
                            repr(&item.key)
                        ));
                    }
                }
            }
        }
        for wanted in &out.forget {
            if self.owner_said_full(turn, &wanted.quote, true).is_none() {
                problems.push(tell(
                    &format!(
                        "forget {} (a whole sentence or clause)",
                        repr(&wanted.item_id)
                    ),
                    &wanted.quote,
                ));
            }
        }
        problems
    }
}

fn text(v: &Value) -> String {
    ferb_py::value::py_str(v)
}

/// `WRAPPED.sub(" ", text)` (untrusted.py: `<untrusted-data id="HEX">.*?</untrusted-data id="HEX">`, dot matching
/// newlines): each part FERB wrapped as data becomes one space.
pub fn without_wrapped(text: &str) -> String {
    const OPEN: &str = "<untrusted-data id=\"";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'scan: while let Some(at) = rest.find(OPEN) {
        let after = &rest[at + OPEN.len()..];
        let hex_len = after
            .bytes()
            .take_while(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
            .count();
        if hex_len > 0 && after[hex_len..].starts_with("\">") {
            let close = format!("</untrusted-data id=\"{}\">", &after[..hex_len]);
            let body = &after[hex_len + 2..];
            if let Some(end) = body.find(&close) {
                out.push_str(&rest[..at]);
                out.push(' ');
                rest = &body[end + close.len()..];
                continue 'scan;
            }
        }
        // not a whole wrapper here: keep its first character and look again after it
        let step = at + rest[at..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&rest[..step]);
        rest = &rest[step..];
    }
    out.push_str(rest);
    out
}
