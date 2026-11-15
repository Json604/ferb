//! Earlier results a task uses (ORCH-619): the planner names results shown in this chat by id (`earlier_results`);
//! each must be one (`_earlier`, read within ORCH-593's fences: `Continuity.result`), a multi-part task's answer
//! standing for its card's rows (`_card_rows`, `_rows_of`); each worker's results must fit its context in full
//! (`_result_problems`, `_worker_room`); and the worker and checker are given them whole (`_result_blocks`).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use ferb_context::assemble::ContextBlock;
use ferb_orchestrator::models::TaskSpec;
use ferb_sanitize::untrusted::as_data;
use serde_json::{Map, Value};

use super::store::Entry;
use super::{Inner, Tree, Turn};
use crate::lines::stored_values;
use crate::words::{ENTRY, RESULT_KINDS};

/// supervisor.py:202 `EARLIER_RESULTS`.
pub const EARLIER_RESULTS: &str =
    "Earlier results this task uses (data: each is a result FERB showed the owner, in \
full and as it was shown, with its origin; use it as the task says)";

fn row_of(e: &Entry) -> crate::seams::JournalRow {
    e.row()
}

impl Inner {
    fn journal_by_id(&self, id: &str) -> Option<Entry> {
        let db = self.db();
        let found = ferb_orchestrator::sql::row(
            db.conn(),
            "SELECT * FROM journal WHERE id = ?",
            &[rusqlite::types::Value::Text(id.into())],
        )
        .ok()
        .flatten()?;
        let s = |k: &str| {
            found
                .get(k)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let data = ferb_py::json::loads(&s("data_json")).unwrap_or(Value::Null);
        let request_id = match data.get("request_id") {
            None | Some(Value::Null) => String::new(),
            Some(v) => ferb_py::value::py_str(v),
        };
        Some(Entry {
            id: s("id"),
            at: s("at"),
            kind: s("kind"),
            data,
            origin: s("origin"),
            request_id,
        })
    }

    /// supervisor.py:3878 `_rows_of`: a card's rows as the owner has them: each part's latest row posted in that run.
    fn rows_of(&self, task_id: &str, request: &Value) -> Vec<Entry> {
        let mut latest: Vec<(String, Entry)> = Vec::new();
        for row in self.of_kind("chat.part", "task_id", task_id, None) {
            if row.get("request_id") != Some(request) {
                continue;
            }
            let part = row.text("part");
            match latest.iter_mut().find(|(p, _)| *p == part) {
                Some(slot) => slot.1 = row,
                None => latest.push((part, row)),
            }
        }
        latest.into_iter().map(|(_, e)| e).collect()
    }

    /// supervisor.py:3867 `_card_rows`: a multi-part task's answer as the card it indexes, in the owner's order; []
    /// for any other result.
    fn card_rows(&self, answer: &Entry) -> Vec<Entry> {
        if answer.kind != "task.answer" || !answer.get("index").is_some_and(ferb_py::value::truthy)
        {
            return Vec::new();
        }
        let task_id = answer.text("task_id");
        let order = self
            .continuity(|c| c.part_order(&task_id))
            .unwrap_or_default();
        let request = answer.get("request_id").cloned().unwrap_or(Value::Null);
        let mut rows = self.rows_of(&task_id, &request);
        rows.sort_by_key(|r| {
            let part = r.text("part");
            order.iter().position(|p| *p == part).unwrap_or(order.len())
        });
        rows
    }

    /// supervisor.py:3852 `_earlier`: the results `ids` name, in the order named, each a result shown in
    /// `conversation`; and the ids that name no such result.
    pub(crate) fn earlier(&self, conversation: &str, ids: &[String]) -> (Vec<Entry>, Vec<String>) {
        let mut found: Vec<Entry> = Vec::new();
        let mut unknown: Vec<String> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for rid in ids
            .iter()
            .map(|i| ferb_orchestrator::py::strip(i).to_string())
        {
            if seen.contains(&rid) {
                continue;
            }
            seen.push(rid.clone());
            let shown = !rid.is_empty()
                && self
                    .continuity(|c| c.result(conversation, &rid, &RESULT_KINDS))
                    .unwrap_or(false);
            let row = if shown {
                self.journal_by_id(&rid)
            } else {
                None
            };
            let Some(row) = row else {
                unknown.push(rid);
                continue;
            };
            let rows = self.card_rows(&row);
            let rows = if rows.is_empty() { vec![row] } else { rows };
            for r in rows {
                if !found.iter().any(|f| f.id == r.id) {
                    found.push(r);
                }
            }
        }
        (found, unknown)
    }

    /// supervisor.py:3925 `_result_blocks`: the earlier results the task names, for its worker and checker: each
    /// result's full text as FERB showed it, in the order named, with its row's own origin and stored values.
    pub(crate) fn result_blocks(&self, tree: &Arc<Tree>, spec: &TaskSpec) -> Vec<ContextBlock> {
        let ids = &spec.earlier_results;
        if ids.is_empty() {
            return Vec::new();
        }
        let conversation = self
            .continuity(|c| c.conversation_of(&tree.id))
            .ok()
            .flatten()
            .filter(|c| !c.is_empty());
        let (found, unknown) = match conversation {
            Some(c) => self.earlier(&c, ids),
            None => (Vec::new(), ids.clone()),
        };
        let zone = self.deps.clock.zone();
        let mut blocks = Vec::new();
        for (i, entry) in found.iter().enumerate() {
            let text = entry.text("text");
            let label = format!(
                "{}{}",
                if i == 0 {
                    format!("{EARLIER_RESULTS}:\n")
                } else {
                    String::new()
                },
                crate::planner::result_line(&row_of(entry), &zone)
            );
            let block = self.source_block(
                tree,
                &label,
                &as_data(&text),
                &entry.origin,
                &[format!("{ENTRY}{}", entry.id)],
                Some(&text),
            );
            blocks.push(ContextBlock {
                protected: true,
                cache: true,
                values: stored_values(&entry.kind, entry.data.as_object().unwrap_or(&Map::new())),
                ..block
            });
        }
        if !unknown.is_empty() {
            blocks.push(ContextBlock {
                protected: true,
                cache: true,
                ..ContextBlock::new(
                    format!(
                        "{}No longer in FERB's records: {}",
                        if found.is_empty() {
                            format!("{EARLIER_RESULTS}:\n")
                        } else {
                            String::new()
                        },
                        unknown.join(", ")
                    ),
                    "task specification",
                )
            });
        }
        blocks
    }

    /// supervisor.py:3947 `_result_problems` (ORCH-619): each named id must be a result shown in this chat, and each
    /// worker's results, in full, must fit its context.
    pub(crate) fn result_problems(&self, turn: &Arc<Mutex<Turn>>, spec: &TaskSpec) -> Vec<String> {
        let steps: Vec<&ferb_orchestrator::models::Step> = spec
            .plan
            .as_ref()
            .map(|p| {
                p.steps
                    .iter()
                    .filter(|s| s.kind == "do" || s.kind == "converse")
                    .collect()
            })
            .unwrap_or_default();
        let mut named: Vec<String> = Vec::new();
        for r in spec
            .earlier_results
            .iter()
            .chain(steps.iter().flat_map(|s| s.earlier_results.iter()))
        {
            if !named.contains(r) {
                named.push(r.clone());
            }
        }
        if named.is_empty() {
            return Vec::new();
        }
        let session = turn.lock().unwrap().session.clone();
        let mut resolved: Vec<(String, Vec<Entry>)> = Vec::new();
        let mut problems = Vec::new();
        for rid in &named {
            let (found, unknown) = self.earlier(&session, std::slice::from_ref(rid));
            if !unknown.is_empty() {
                problems.push(format!(
                    "earlier_results {} isn't a result shown in this chat. Name a result by the id on its line \
                     (\"Result id: …\"), or leave earlier_results empty when the task uses none.",
                    ferb_py::json::dumps_utf8(&Value::String(rid.clone()))
                ));
            }
            resolved.push((rid.clone(), found));
        }
        if !problems.is_empty() {
            return problems;
        }
        let users: Vec<(String, Vec<String>, String)> = if spec.plan.is_some() {
            steps
                .iter()
                .map(|s| {
                    let mut ids = spec.earlier_results.clone();
                    for r in &s.earlier_results {
                        if !ids.contains(r) {
                            ids.push(r.clone());
                        }
                    }
                    (format!("step {}", s.id), ids, s.worker_type.clone())
                })
                .collect()
        } else {
            vec![(
                "The task".into(),
                spec.earlier_results.clone(),
                spec.worker_type.clone(),
            )]
        };
        let zone = self.deps.clock.zone();
        let tokens = self.deps.tokenizer;
        let mut rooms: Vec<(String, i64)> = Vec::new();
        for (who, ids, worker_type) in users {
            let mut rows: Vec<&Entry> = Vec::new();
            for rid in &ids {
                for e in resolved
                    .iter()
                    .filter(|(k, _)| k == rid)
                    .flat_map(|(_, v)| v.iter())
                {
                    if !rows.iter().any(|r| r.id == e.id) {
                        rows.push(e);
                    }
                }
            }
            if rows.is_empty() {
                continue;
            }
            let need: i64 = rows
                .iter()
                .map(|e| {
                    tokens.count(&format!(
                        "{}\n{}",
                        crate::planner::result_line(&row_of(e), &zone),
                        e.text("text")
                    )) as i64
                })
                .sum();
            let room = match rooms.iter().find(|(w, _)| *w == worker_type) {
                Some((_, r)) => *r,
                None => {
                    let r = self.worker_room(turn, &spec.identity, &worker_type);
                    rooms.push((worker_type.clone(), r));
                    r
                }
            };
            if need > room {
                problems.push(format!(
                    "{who}: the earlier results it names need about {need} tokens in full, and its worker's context \
                     has room for {room}. Name only the results it uses, or one row of a multi-part result (that \
                     row's id) instead of the whole card."
                ));
            }
        }
        problems
    }

    /// supervisor.py:3981 `_worker_room`: tokens a worker of this type has for what the task hands it, after its
    /// instructions, the request, its report schema and its tools.
    pub(crate) fn worker_room(
        &self,
        turn: &Arc<Mutex<Turn>>,
        identity: &str,
        worker_type: &str,
    ) -> i64 {
        let (rid, payload) = {
            let t = turn.lock().unwrap();
            (t.request_id.clone(), t.payload.clone())
        };
        let scopes: BTreeSet<String> = self.deps.daemon.all_scopes();
        let grant = ferb_worker::ports::Grant {
            task_id: rid.clone(),
            worker_id: rid,
            scopes,
            identity: if identity == "ferb" {
                ferb_core::types::Identity::Ferb
            } else {
                ferb_core::types::Identity::Owner
            },
            observe_only: false,
        };
        let schema =
            ferb_context::schemas::worker_schema(ferb_context::schemas::WorkerFlags::default());
        let tools = self.deps.daemon.tools_for(&grant);
        let reserve = self.deps.tokenizer.count(&format!(
            "{}{}{}{}",
            self.system_for("worker.md", true, &[]),
            payload,
            ferb_py::json::dumps(&schema),
            ferb_py::json::dumps(&Value::Array(tools))
        )) as i64;
        let budget = self
            .deps
            .daemon
            .worker_limits(worker_type)
            .map(|l| l.context_budget())
            .unwrap_or(0);
        (budget - reserve).max(0)
    }
}
