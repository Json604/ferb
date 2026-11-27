//! T-1343 (kb/plans/front-door.md, "Keeping the runtime ready"): Claude Code started and initialised ahead of its
//! message, used for that one message and ended after it by today's code.
//!
//! A second message into the same Claude Code would carry the first one's history, so a program takes one message.
//! What is made ahead is everything before the message: the run id and its relay registration, the blank token file in
//! its own owner-only folder, the program itself and its `initialize`. The message writes its token into the file
//! (replaced in one step) and sends its frame; the run is then today's [`super::ClaudeRun`]. Its tool bridge reads the
//! token file at each request and holds its `initialize` answer until the token is there
//! (`mcp_bridge::TOKEN_WAIT_ENV`); Claude Code is told to wait as long for it (MCP_TIMEOUT).

use std::any::Any;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_core::interfaces::BoxFuture;
use ferb_core::timing::{CallTimer, Steps};
use ferb_core::types::{ModelRequest, RunHandle, Task};
use serde_json::{json, Map, Value};
use tokio::time::Instant;

use super::transport::Client;
use super::{
    first_frame, remove_token, tail_after_marker, typed_now, write_token_file, ClaudeRun,
    ClaudeRuntime, Mode, State, SIGNED_IN,
};
use crate::error::{Raised, RuntimeError};
use crate::gen::EventStream;
use crate::keep::{Back, Keepable, Ready, IDLE_S};
use crate::run::{RunControl, Started};

/// How long a ready program's tool bridge (and Claude Code, for it) waits for its message: longer than a program is
/// ever kept idle.
pub const BRIDGE_WAIT_S: f64 = IDLE_S + 60.0;

/// The longest a program made ahead may take to start (its call's own limit applies when it has one).
pub const PREPARE_LIMIT_S: f64 = 120.0;

/// FERB's sign-in marker as it was when the program started (a new sign-in or a sign-out ends the program).
fn signed_in(rt: &ClaudeRuntime) -> Option<(u64, Option<std::time::SystemTime>)> {
    let m = std::fs::metadata(rt.ferb_home.join("claude-home").join(SIGNED_IN)).ok()?;
    Some((m.len(), m.modified().ok()))
}

/// A Claude Code started and initialised, waiting for its one message.
pub struct ClaudeReady {
    settings: String,
    run_id: String,
    token_file: Option<PathBuf>,
    client: Arc<Client>,
    typed: String,
    signin: Option<(u64, Option<std::time::SystemTime>)>,
    rt: ClaudeRuntime,
    prepared: Map<String, Value>,
    used: Mutex<bool>,
}

impl ClaudeReady {
    async fn close(&self) {
        self.client.disconnect().await;
        self.rt.proxy.forget(&self.run_id);
        remove_token(self.token_file.as_ref());
    }
}

impl Ready for ClaudeReady {
    fn settings(&self) -> &str {
        &self.settings
    }

    fn alive(&self) -> bool {
        !*self.used.lock().unwrap_or_else(|e| e.into_inner())
            && self.client.alive()
            && signed_in(&self.rt) == self.signin
    }

    fn end(self: Arc<Self>) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            let used = std::mem::replace(
                &mut *self.used.lock().unwrap_or_else(|e| e.into_inner()),
                true,
            );
            if !used {
                self.close().await;
            }
        })
    }

    fn as_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }
}

fn settings_of(settings: &str) -> Value {
    ferb_py::json::loads(settings).unwrap_or(Value::Null)
}

impl Keepable for ClaudeRuntime {
    /// A supervisor run with FERB's bundled Claude Code: everything on its command line and in its environment,
    /// with how its answer form goes and whether its request has a tail after FERB's marker (RUN-476's switch; a
    /// program made ahead assumes one, as every front-door request has). A worker-mode run starts as today.
    fn keep_settings(&self, request: Option<&ModelRequest>, has_token: bool) -> Option<String> {
        if self.mode != Mode::Supervisor {
            return None;
        }
        let cli = self.cli.as_ref()?;
        let environ: Vec<(&String, &String)> = self.host.environ.iter().collect();
        Some(ferb_py::json::dumps(&json!({
            "runtime": "claude", "cli": cli.to_string_lossy(), "home": self.ferb_home.to_string_lossy(),
            "model": self.model, "effort": self.effort, "shim": self.shim_cmd, "tools": has_token,
            "allowed": self.allowed, "schema": self.shape.as_ref().map(|s| s.wire.clone()),
            "typed": typed_now(self), "tail": request.map(tail_after_marker).unwrap_or(true),
            "temp": self.host.temp_root.to_string_lossy(), "environ": environ,
        })))
    }

    fn prepare<'a>(
        &'a self,
        settings: &'a str,
        _back: Back,
    ) -> BoxFuture<'a, Result<Arc<dyn Ready>, RuntimeError>> {
        Box::pin(async move {
            let s = settings_of(settings);
            let has_token = s["tools"] == json!(true);
            let typed = s["typed"].as_str().unwrap_or_default().to_string();
            let tail = s["tail"] != json!(false);
            let mut rt = self.clone();
            rt.keep_wait_s = Some(BRIDGE_WAIT_S);
            let mut steps = Steps::new(rt.now());
            let run_id = format!("run-{}", (rt.host.token_hex)(6));
            let token_file = if has_token {
                Some(write_token_file(&rt.host.temp_root, "")?)
            } else {
                None
            };
            let signin = signed_in(&rt);
            let (cli, options) =
                match rt.options(&run_id, token_file.as_deref(), typed == "stated", tail) {
                    Ok(o) => o,
                    Err(e) => {
                        rt.proxy.forget(&run_id);
                        remove_token(token_file.as_ref());
                        return Err(e);
                    }
                };
            steps.mark("options");
            let limit = Duration::from_secs_f64(rt.limit_s.unwrap_or(PREPARE_LIMIT_S).max(0.0));
            let connect = Client::connect(
                rt.host.spawner.as_ref(),
                &cli,
                &rt.host.environ,
                &options,
                rt.host.urandom4.clone(),
            );
            let client = match tokio::time::timeout(limit, connect).await {
                Ok(Ok(c)) => Arc::new(c),
                failed => {
                    rt.proxy.forget(&run_id);
                    remove_token(token_file.as_ref());
                    return Err(match failed {
                        Ok(Err(e)) => e,
                        _ => RuntimeError::Raised(Raised::timeout()),
                    });
                }
            };
            steps.mark("connect");
            Ok(Arc::new(ClaudeReady {
                settings: settings.to_string(),
                run_id,
                token_file,
                client,
                typed,
                signin,
                rt,
                prepared: steps.steps.clone(),
                used: Mutex::new(false),
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
            let Ok(ready) = ready.clone().as_any().downcast::<ClaudeReady>() else {
                back(ready);
                return Err(RuntimeError::protocol("not a Claude Code program"));
            };
            if ready.token_file.is_some() == run_token.is_empty() {
                back(ready);
                return Err(RuntimeError::protocol(
                    "the ready Claude Code does not fit this call",
                ));
            }
            // used once: whatever happens next, it never takes another message
            *ready.used.lock().unwrap_or_else(|e| e.into_inner()) = true;
            let mut steps = Steps::new(self.now());
            let deadline = self
                .limit_s
                .map(|s| Instant::now() + Duration::from_secs_f64(s.max(0.0)));
            let mut timer = CallTimer::new(self.now(), "runtime");
            let body = async {
                if let Some(path) = &ready.token_file {
                    let next = path.with_file_name(".run-token.next");
                    put_private(&next, run_token.as_bytes())?;
                    std::fs::rename(&next, path)?;
                }
                timer.begin();
                ready
                    .client
                    .query(vec![first_frame(
                        &self.request_as_sent(request, &ready.typed),
                    )])
                    .await
            };
            let out = match deadline {
                Some(d) => match tokio::time::timeout_at(d, body).await {
                    Ok(r) => r,
                    Err(_) => Err(RuntimeError::Raised(Raised::timeout())),
                },
                None => body.await,
            };
            if let Err(e) = out {
                ready.close().await;
                return Err(e);
            }
            steps.mark("query");
            let mut state = Map::new();
            state.insert("start_steps".into(), Value::Object(steps.steps.clone()));
            state.insert("typed".into(), json!(ready.typed));
            if let Some(t) = &ready.token_file {
                state.insert("token_file".into(), json!(t.to_string_lossy()));
            }
            state.insert("kept".into(), json!("ready"));
            state.insert(
                "prepare_steps".into(),
                Value::Object(ready.prepared.clone()),
            );
            let run = Arc::new(ClaudeRun {
                rt: self.clone(),
                id: ready.run_id.clone(),
                request: request.clone(),
                deadline,
                st: Mutex::new(State::new(
                    Some(ready.client.clone()),
                    ready.token_file.clone(),
                    timer,
                    ready.typed.clone(),
                )),
                input_lock: tokio::sync::Mutex::new(()),
            });
            let handle = RunHandle {
                id: ready.run_id.clone(),
                runtime: "claude".into(),
                task_id: task.id.clone(),
                state,
            };
            let control: Arc<dyn RunControl> = run.clone();
            let events = EventStream::spawn(move |y| super::events_body(run, y));
            Ok(Started {
                handle,
                events,
                control,
            })
        })
    }

    fn once(&self) -> bool {
        true
    }
}

/// An owner-only file, truncated.
fn put_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(data)
}
