//! The live lines code writes while a task runs (worker.py:315-362): a tool call's line (L8), built from the tool
//! and its target, never from the worker's words (`tool_progress`); a call waiting on the task's own command says
//! which and for how long (ORCH-628); the tree's live line and the bus get each one (APP-893).

use ferb_context::pyvalue::{self, Py};
use ferb_orchestrator::progress;
use serde_json::{Map, Value};

use crate::errors::Error;
use crate::ports::Grant;
use crate::worker::Worker;

impl Worker {
    /// worker.py:315 `tool_progress`: a worker (or the final checker) started a tool call (the gateway's
    /// `tool_started`). A line that can't be built is journaled (`progress.failed`) and the call goes on. A call
    /// after an outage is the connection back: that line comes first (fix wave 7). An error is what Python would
    /// have raised into the gateway's observer (the redactor failing on the last-step record, or on the
    /// reconnection line).
    pub fn tool_progress(
        &self,
        grant: &Grant,
        name: &str,
        args: &Map<String, Value>,
        kind: &str,
        site: Option<&str>,
    ) -> Result<(), Error> {
        let host = self.host();
        if let Some(notice) = self.runs().connection(&grant.worker_id) {
            notice.back(host)?;
        }
        let Some(tree) = self.runs().tree_of(&grant.task_id) else {
            return Ok(()); // no running task to show it on
        };
        let built = (|| -> Result<(Value, Option<String>), Error> {
            let app = if name.starts_with("mac.") {
                let installed = host.apps()?;
                progress::app_name(args, site, &installed, &|| host.running_apps())
            } else {
                None
            };
            let line = match self.waiting_line(name, args, &grant.task_id)? {
                Some(line) => line,
                None => progress::describe(host.policy(), name, args, kind, site, app.as_deref()),
            };
            Ok((host.redact(&Value::String(line))?, app))
        })();
        let (text, app) = match built {
            Ok(built) => built,
            Err(e) => {
                host.journal(
                    "progress.failed",
                    serde_json::json!({"task_id": grant.task_id, "tool": name, "error": e.class()}),
                );
                return Ok(());
            }
        };
        // ORCH-595: the planner's task state names the tree's last tool step, as this line says it
        let mut step = Map::new();
        step.insert("tool".into(), Value::String(name.to_string()));
        step.insert("line".into(), text.clone());
        if let Some(site) = site.filter(|s| !s.is_empty()) {
            step.insert("site".into(), Value::String(site.to_string()));
        }
        if let Some(app) = app.filter(|a| !a.is_empty()) {
            step.insert("app".into(), Value::String(app));
        }
        let local = host.clock().zone().local(&host.clock().now());
        step.insert(
            "at".into(),
            Value::String(local.format("%H:%M").to_string()),
        );
        let step = host.redact(&Value::Object(step))?;
        let tree_id = tree.id();
        self.runs().set_last_step(&tree_id, step);
        // the step's own task id, so each of a card's parallel rows shows its own line (ORCH-531)
        self.progress_line(&tree_id, &tree.worker_task_id(), text);
        Ok(())
    }

    /// worker.py:339 `_waiting_line` (ORCH-628): a call that waits on or reads one of the task's own commands says
    /// which command and how long it has run; None for any other call, or a command that isn't this task's.
    fn waiting_line(
        &self,
        name: &str,
        args: &Map<String, Value>,
        task_id: &str,
    ) -> Result<Option<String>, Error> {
        if progress::waits_on_command(name).is_none() {
            return Ok(None);
        }
        let Some(Value::String(id)) = args.get("id") else {
            return Ok(None);
        };
        let Some(proc_) = self.host().shell_process(id)? else {
            return Ok(None);
        };
        let owner = proc_
            .get("owner_task")
            .filter(|v| ferb_py::value::truthy(v))
            .map(|v| pyvalue::to_str(&Py::from_value(v)))
            .unwrap_or_default();
        if owner != task_id {
            return Ok(None);
        }
        let started = match proc_.get("started").filter(|v| ferb_py::value::truthy(v)) {
            None => 0.0,
            Some(v) => py_float(v)?,
        };
        let now = timestamp(&self.host().clock().now());
        let minutes = ((now - started).max(0.0) / 60.0).floor() as i64;
        let command = proc_
            .get("command")
            .filter(|v| ferb_py::value::truthy(v))
            .map(|v| pyvalue::to_str(&Py::from_value(v)))
            .unwrap_or_default();
        Ok(Some(progress::waiting_for(name, &command, minutes)))
    }

    /// worker.py:351 `_progress_line` (APP-893): a task's plain live line, on the tree's stream while it runs and
    /// on the bus (`task.progress`) for FERB.app.
    pub fn progress_line(&self, tree_id: &str, step_task_id: &str, text: Value) {
        let host = self.host();
        let mut line = Map::new();
        line.insert(
            "step_task_id".into(),
            Value::String(step_task_id.to_string()),
        );
        line.insert("text".into(), text);
        line.insert(
            "started_at".into(),
            Value::String(ferb_clock::py::isoformat(
                &host.clock().now(),
                ferb_clock::py::Spec::Auto,
            )),
        );
        let mut event = Map::new();
        event.insert("type".into(), Value::String("progress".into()));
        event.extend(line.clone());
        self.tell_tree(tree_id, event);
        let mut data = Map::new();
        data.insert("task_id".into(), Value::String(tree_id.to_string()));
        data.extend(line);
        host.publish("task.progress", data);
    }

    /// worker.py:359 `_tell_tree`: an event on a tree's live line (its run's stream, L8), while the tree runs.
    pub fn tell_tree(&self, tree_id: &str, event: Map<String, Value>) {
        if let Some(live) = self.runs().live_sender(tree_id) {
            let mut event = event;
            event.insert("task_id".into(), Value::String(tree_id.to_string()));
            let _ = live.send(event);
        }
    }
}

/// `dt.timestamp()` of an aware time: whole microseconds over 10**6.
pub(crate) fn timestamp(t: &ferb_clock::py::Time) -> f64 {
    let us = t.timestamp() as i128 * 1_000_000 + i128::from(t.timestamp_subsec_micros());
    us as f64 / 1e6
}

/// Python's `float(x)` of a decoded JSON value.
fn py_float(v: &Value) -> Result<f64, Error> {
    match v {
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Value::Number(n) => Ok(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => {
            let t = ferb_py::value::strip(s);
            let lower = t.to_ascii_lowercase();
            let ok = !t.is_empty()
                && !t.contains('_')
                && (t.chars().all(|c| c.is_ascii_digit() || "+-.eE".contains(c))
                    || matches!(
                        lower.trim_start_matches(['+', '-']),
                        "inf" | "infinity" | "nan"
                    ));
            match t.parse::<f64>() {
                Ok(f) if ok => Ok(f),
                _ => Err(Error::value(format!(
                    "could not convert string to float: {}",
                    pyvalue::repr(&Py::Str(s.clone()))
                ))),
            }
        }
        other => Err(Error::other(
            "TypeError",
            format!(
                "float() argument must be a string or a real number, not '{}'",
                Py::from_value(other).type_of()
            ),
        )),
    }
}
