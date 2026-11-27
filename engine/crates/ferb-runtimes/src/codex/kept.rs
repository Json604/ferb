//! T-1343 (kb/plans/front-door.md, "Keeping the runtime ready"): the Codex app-server kept across messages, one
//! message at a time.
//!
//! A kept program is today's program (the same folder, sign-in copy, command line and item gate) with three
//! differences. Its token file starts blank and holds each message's token only while that message runs (written
//! before its thread is resumed, blanked once its turn ended); its tool bridge reads the file at each request
//! (`mcp_bridge::TOKEN_WAIT_ENV`). Each message takes the measured recipe: after the message `thread/unsubscribe`,
//! before the next the template rewritten and `thread/resume` with the same cache thread id, so the next message
//! carries nothing of the last one and still reads the provider's cache. And a turn's answer waits for the stderr
//! reader to have checked each of its model calls' request traces instead of killing the program to read them to
//! the end ([`super::stream::KEPT_TRACE_WAIT_S`]; past that wait the turn ends as today's).
//!
//! A message's program goes back to the keeper only when its turn ended in Codex (completed, or interrupted and
//! completed), nothing outside FERB was seen, its thread was unsubscribed and its token blanked; anything else ends
//! it with today's code (killed and reaped, a refreshed sign-in published, its folder removed).

use std::any::Any;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_core::interfaces::BoxFuture;
use ferb_core::timing::Steps;
use ferb_core::types::{ModelRequest, RunHandle, Task};
use serde_json::{json, Map, Value};
use tokio::time::Instant;

use super::stream::{
    ensure_version, events_body, new_run, program_body, timer_now, traced, turn_body, CodexRun,
    KeptLink,
};
use super::{check_input, path_str, CodexRuntime, Mode};
use crate::error::{Raised, RuntimeError};
use crate::gen::EventStream;
use crate::keep::{Back, Keepable, Ready, IDLE_S};
use crate::run::{RunControl, Started};

/// How long a kept program's tool bridge waits for its message's token: longer than a program is ever kept idle.
pub const BRIDGE_WAIT_S: f64 = IDLE_S + 60.0;

/// The longest a program made ready ahead may take to start (its call's own limit applies when it has one).
pub const PREPARE_LIMIT_S: f64 = 120.0;

/// A Codex app-server ready for a message.
pub struct CodexReady {
    run: Arc<CodexRun>,
    /// The start steps of its first message's program (files, spawn, initialize), told once.
    prepared: Mutex<Option<Map<String, Value>>>,
}

impl Ready for CodexReady {
    fn settings(&self) -> &str {
        self.run.kept_settings()
    }

    fn alive(&self) -> bool {
        self.run.save_refresh();
        self.run.kept_alive()
    }

    fn end(self: Arc<Self>) -> BoxFuture<'static, ()> {
        Box::pin(async move { self.run.end_program().await })
    }

    fn as_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }
}

/// One message's hold on a kept program: its control while it runs. Once the message ended (and the program
/// moved on to another), its cancel does nothing to the program.
pub struct CodexLease {
    run: Arc<CodexRun>,
    generation: u64,
    done: tokio::sync::Mutex<bool>,
    /// RUN-477: the usage Codex reported for this message that no event carried, kept when the message ended.
    leftover: Mutex<Vec<Map<String, Value>>>,
}

impl CodexLease {
    /// The message ends (its events ended, or the caller cancelled it): its turn is interrupted when it still runs
    /// (a plain-mode run, as a cancel does today), and the program goes back to the keeper only when it can take
    /// another message; otherwise it ends as today's run does.
    pub(super) async fn release(&self, cancelling: bool) {
        let mut done = self.done.lock().await;
        if *done {
            return;
        }
        *done = true;
        let run = &self.run;
        if run.generation() != self.generation {
            return;
        }
        if cancelling && run.rt().plain() {
            run.interrupt().await;
        }
        run.end_events();
        let ended = run.turn_ended() && run.kept_alive() && traced(run).await;
        *self.leftover.lock().unwrap_or_else(|e| e.into_inner()) =
            RunControl::unreported_usage(run.as_ref());
        let kept = ended && run.unsubscribe().await.is_ok() && run.put_token("").is_ok();
        match run.kept_back() {
            Some(back) if kept => {
                run.save_refresh();
                run.next_generation();
                back(Arc::new(CodexReady {
                    run: run.clone(),
                    prepared: Mutex::new(None),
                }));
            }
            _ => run.end_program().await,
        }
    }
}

impl RunControl for CodexLease {
    fn steer<'a>(&'a self, _message: &'a str) -> BoxFuture<'a, Result<(), RuntimeError>> {
        // a kept program runs supervisor turns only, which never take steering
        Box::pin(async {
            Err(RuntimeError::unsupported(
                "Only active Codex worker turns accept steering",
            ))
        })
    }

    fn cancel(&self) -> BoxFuture<'_, ()> {
        Box::pin(self.release(true))
    }

    fn unreported_usage(&self) -> Vec<Map<String, Value>> {
        std::mem::take(&mut *self.leftover.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

fn settings_of(settings: &str) -> Value {
    ferb_py::json::loads(settings).unwrap_or(Value::Null)
}

impl Keepable for CodexRuntime {
    /// A supervisor run (the front door, the planner): everything on its app-server's command line and in its
    /// environment. A worker-mode run (a worker, the checker) is never kept: it starts as today.
    fn keep_settings(&self, _request: Option<&ModelRequest>, has_token: bool) -> Option<String> {
        if self.mode != Mode::Supervisor {
            return None;
        }
        let environ: Vec<(&String, &String)> = self.host.environ.iter().collect();
        Some(ferb_py::json::dumps(&json!({
            "runtime": "codex", "binary": path_str(&self.binary), "home": path_str(&self.ferb_home),
            "model": self.model, "effort": self.sent_effort(), "shim": self.shim_cmd, "tools": has_token,
            "temp": path_str(&self.host.temp_root), "environ": environ,
        })))
    }

    fn prepare<'a>(
        &'a self,
        settings: &'a str,
        back: Back,
    ) -> BoxFuture<'a, Result<Arc<dyn Ready>, RuntimeError>> {
        Box::pin(async move {
            let has_token = settings_of(settings)["tools"] == json!(true);
            let mut rt = self.clone();
            rt.keep_wait_s = Some(BRIDGE_WAIT_S);
            ensure_version(&rt).await?;
            let run = new_run(
                &rt,
                "",
                None,
                Some(KeptLink {
                    settings: settings.to_string(),
                    back,
                }),
            )?;
            let mut steps = Steps::new(timer_now(&rt));
            let limit = Duration::from_secs_f64(rt.timeout_s.unwrap_or(PREPARE_LIMIT_S).max(0.0));
            let body = program_body(&run, &rt, has_token.then_some(""), &mut steps);
            let out = match tokio::time::timeout(limit, body).await {
                Ok(r) => r,
                Err(_) => Err(RuntimeError::Raised(Raised::timeout())),
            };
            if let Err(e) = out {
                run.end_program().await;
                return Err(e);
            }
            Ok(Arc::new(CodexReady {
                run,
                prepared: Mutex::new(Some(steps.steps.clone())),
            }) as Arc<dyn Ready>)
        })
    }

    fn take<'a>(
        &'a self,
        ready: Arc<dyn Ready>,
        back: Back,
        task: &'a Task,
        request: &'a ModelRequest,
        run_token: &'a str,
    ) -> BoxFuture<'a, Result<Started, RuntimeError>> {
        Box::pin(async move {
            let Ok(ready) = ready.clone().as_any().downcast::<CodexReady>() else {
                back(ready);
                return Err(RuntimeError::protocol("not a Codex program"));
            };
            let mut steps = Steps::new(timer_now(self));
            if let Err(e) = check_input(&request.context()) {
                back(ready);
                return Err(e.into());
            }
            if let Err(e) = ensure_version(self).await {
                back(ready);
                return Err(e);
            }
            let version = steps.mark("version");
            let run = ready.run.clone();
            if run.has_token_file() == run_token.is_empty() {
                // the program was made for calls with (or without) tools; this one is the other kind
                back(ready);
                return Err(RuntimeError::protocol(
                    "the kept Codex program does not fit this call",
                ));
            }
            let first = run.generation() == 0;
            let deadline = self
                .timeout_s
                .map(|s| Instant::now() + Duration::from_secs_f64(s.max(0.0)));
            let body = async {
                run.arm(self, &task.id, deadline, run_token).await?;
                turn_body(&run, self, task, request, &mut steps).await
            };
            let out = match deadline {
                Some(d) => match tokio::time::timeout_at(d, body).await {
                    Ok(r) => r,
                    Err(_) => Err(RuntimeError::Raised(Raised::timeout())),
                },
                None => body.await,
            };
            if let Err(e) = out {
                run.end_program().await;
                return Err(e);
            }
            let mut start_steps = Map::new();
            start_steps.insert("version".into(), json!(version));
            for (k, v) in steps.steps.clone() {
                start_steps.insert(k, v);
            }
            let mut state = Map::new();
            state.insert("app_server".into(), json!(true));
            state.insert("start_steps".into(), Value::Object(start_steps));
            state.insert("context_role".into(), json!(run.context_role_now()));
            if let Some(t) = run.token_path() {
                state.insert("token_file".into(), json!(path_str(&t)));
            }
            // T-1343: the program was ready (it took a message before, or was made ahead) or made for this call
            state.insert("kept".into(), json!(if first { "new" } else { "ready" }));
            if let Some(p) = ready
                .prepared
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                state.insert("prepare_steps".into(), Value::Object(p));
            }
            let handle = RunHandle {
                id: run.run_id_now(),
                runtime: "codex".into(),
                task_id: task.id.clone(),
                state,
            };
            let lease = Arc::new(CodexLease {
                run: run.clone(),
                generation: run.generation(),
                done: tokio::sync::Mutex::new(false),
                leftover: Mutex::new(Vec::new()),
            });
            let control: Arc<dyn RunControl> = lease.clone();
            let events = EventStream::spawn(move |y| events_body(run, Some(lease), y));
            Ok(Started {
                handle,
                events,
                control,
            })
        })
    }
}
