//! The intro's plan-only preview (ORCH-500): supervisor.py:2422 `Supervisor.preview`. The real planner turn on the
//! request, alone: `_supervise` with no tools and no token, never `_route`, `_start`, `run_tree` or a worker; its
//! turn leaves out the owner's live state (`Turn.preview`). Nothing is kept but one `intro.preview` journal row.
//!
//! decision: ORCH-500

use std::sync::{Arc, Mutex};

use ferb_orchestrator::models::Step;
use ferb_sanitize::origin;
use ferb_sanitize::untrusted::mark;
use serde_json::{json, Value};

use super::turn::{origin_set, Cancel, Repairable};
use super::{Failure, Inner, Supervisor, Turn};

impl Supervisor {
    /// supervisor.py:2422 `preview`: the events the intro's preview streams, in order (`plan`, one `part` per planned
    /// step, `done`, each marked `preview: true`); Err: the planner's failure, as Python raises it to the RPC.
    pub async fn preview_events(
        &self,
        session_id: &str,
        text: &str,
    ) -> Result<Vec<Value>, Failure> {
        self.inner.preview_events(session_id, text).await
    }
}

impl Inner {
    pub(crate) async fn preview_events(
        &self,
        session_id: &str,
        text: &str,
    ) -> Result<Vec<Value>, Failure> {
        let request_id = self.new_id();
        let m = mark(text);
        // the request is FERB's suggestion (or general task), not words the owner typed into a chat
        let turn = Arc::new(Mutex::new(Turn {
            request_id: request_id.clone(),
            session: session_id.to_string(),
            words: m.own_words.clone(),
            payload: m.for_model.clone(),
            own_words: m.own_words.clone(),
            typed: m.own_words.clone(),
            typed_lines: m.typed_lines.clone(),
            for_model: m.for_model.clone(),
            untrusted: !m.untrusted.is_empty(),
            message_origin: origin_set(&[origin::FERB, origin::INFERRED]),
            preview: true,
            ..Turn::default()
        }));
        let cancel = Cancel::default();
        let decided = async {
            // no overall limit: its planner call is stopped only by the silence guard (ORCH-528)
            let environment = self.deps.daemon.environment(&m.own_words).await;
            turn.lock().unwrap().environment = environment;
            match self.supervise_turn(&turn, None, &cancel).await {
                Ok(out) => Ok(out),
                // the schema repair a chat turn gets (`_decide`)
                Err(Repairable::Problems(problems, _)) => self
                    .supervise_turn(&turn, Some(&problems), &cancel)
                    .await
                    .map_err(Repairable::into_failure),
                Err(Repairable::Not(f)) => Err(f),
            }
        }
        .await;
        let out = match decided {
            Ok(out) => out,
            Err(e) => {
                self.journal(
                    "intro.preview",
                    json!({"request_id": request_id, "shown": false, "why": e.class()}),
                    None,
                );
                return Err(e);
            }
        };
        let spec = if out.kind == "task" {
            out.task.clone()
        } else {
            None
        };
        let steps: Vec<Step> = match &spec {
            None => Vec::new(),
            Some(s) => match &s.plan {
                Some(plan) => plan.steps.clone(),
                None => {
                    let mut step = Step::new(ferb_orchestrator::plans::SINGLE, "do");
                    step.goal = s.goal.clone();
                    vec![step]
                }
            },
        };
        self.journal(
            "intro.preview",
            json!({"request_id": request_id, "shown": true, "kind": out.kind, "steps": steps.len()}),
            None,
        );
        let said = self
            .deps
            .policy
            .from_task_lines(&self.redact_text(&out.text));
        let mut events = vec![
            json!({"type": "plan", "preview": true, "request_id": request_id,
            "text": said, "plan_line": said,
            "goal": spec.as_ref().map(|s| self.redact_text(&s.goal)).unwrap_or_default(),
            "identity": spec.as_ref().map(|s| s.identity.clone()).unwrap_or_default(),
            "total": steps.len()}),
        ];
        for step in &steps {
            let first = [
                step.goal.as_str(),
                step.question.as_str(),
                step.until.as_deref().unwrap_or(""),
                step.event.as_deref().unwrap_or(""),
            ]
            .into_iter()
            .find(|t| !t.is_empty())
            .unwrap_or("");
            let label = ferb_orchestrator::py::squeeze(first);
            events.push(
                json!({"type": "part", "preview": true, "request_id": request_id,
                "part": step.id, "step": step.id, "kind": step.kind,
                "label": self.redact_text(&label), "state": "preview",
                "depends_on": step.depends_on}),
            );
        }
        events.push(json!({"type": "done", "preview": true, "request_id": request_id}));
        Ok(events)
    }
}
