//! The owner's coding-agent sessions (kb HAND-651..666) in this program: ferb-daemon's `sessions` area over the Mac's process
//! table and host adapters (`ferb_platform_macos::sessions`, only with the Mac's own parts on; a test daemon reads no
//! process and types nowhere), the gateway's own questions (the agents' permission prompts and questions, the Yes
//! that turns the feature on), the supervisor's chat (a hand-over is the owner's message), the bus and the journal.
//! The generic foreground path is the Mac hands' own foreground typing (HAND-519), after code has proven the focused
//! terminal is the session's (HAND-660).

use std::path::PathBuf;
use std::sync::Arc;

use ferb_core::interfaces::{BoxFuture, Clock, IdSource};
use ferb_daemon::sessions::{Ask, AskFn, Foreground, Mac, Seams, Sessions, KEY, SECTION};
use ferb_gateway::gateway::Gateway;
use ferb_rpc::Server;
use ferb_sessions::host::{Proc, Table};
use serde_json::{json, Value};

use crate::basics::{Events, Journal};

/// What the area takes from the composition.
pub struct Parts {
    pub journal: Arc<Journal>,
    pub events: Arc<Events>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdSource>,
    pub config: Arc<ferb_core::config::Config>,
    pub redactor: Arc<ferb_sanitize::redact::Redactor>,
    pub gateway: Arc<Gateway>,
    pub supervisor: ferb_supervisor::flow::Supervisor,
    pub notifier: Arc<ferb_daemon::areas_b::notify::Notifier>,
    pub tools: super::tools::WeakParts,
    pub user_home: PathBuf,
    pub app_bundle: Option<PathBuf>,
    /// The Mac's own parts: the real process table and input. Off (a test daemon): none.
    pub native: bool,
}

/// FERB's `ferb` program, which Claude Code's hooks run: FERB.app's `Contents/Resources/bin/ferb`, else the one beside
/// this program (a checkout's daemon).
pub fn hook_program(app_bundle: Option<&std::path::Path>) -> String {
    if let Some(app) = app_bundle {
        let p = app.join(ferb_device::install::BUNDLE_CLI);
        if p.is_file() {
            return p.to_string_lossy().into_owned();
        }
    }
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("ferb")))
        .filter(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A daemon without the Mac's own parts sees no process and types nowhere (TEST-508).
struct AbsentMac;

impl Mac for AbsentMac {
    fn processes(&self) -> Table {
        Table::new()
    }
    fn cwd_of(&self, _pid: i32) -> String {
        String::new()
    }
    fn tmux_clients(&self, _server: i32) -> Vec<(i32, String)> {
        Vec::new()
    }
    fn app(&self, _bundle: &str) -> (String, String) {
        (String::new(), String::new())
    }
    fn claude_agents(&self, _exe: Option<&str>) -> Option<String> {
        None
    }
    fn terminal_type(&self, _tty: &str, _text: &str) -> Result<(), String> {
        Err("this FERB runs without the Mac's own parts".into())
    }
    fn terminal_open(&self, _command: &str) -> Result<(), String> {
        Err("this FERB runs without the Mac's own parts".into())
    }
    fn tmux_send(&self, _: i32, _: &str, _: &str, _: &str, _: bool) -> Result<(), String> {
        Err("this FERB runs without the Mac's own parts".into())
    }
    fn foreground(&self, _f: Foreground) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async { Err("this FERB runs without the Mac's own parts".to_string()) })
    }
}

/// The real Mac.
#[cfg(target_os = "macos")]
struct NativeMac {
    home: PathBuf,
    tools: super::tools::WeakParts,
}

#[cfg(target_os = "macos")]
impl NativeMac {
    fn tmux_program(&self, server: i32) -> String {
        ferb_platform_macos::sessions::exe_of(server)
    }
}

#[cfg(target_os = "macos")]
impl Mac for NativeMac {
    fn processes(&self) -> Table {
        ferb_platform_macos::sessions::processes()
            .into_iter()
            .map(|(pid, p)| {
                (
                    pid,
                    Proc {
                        pid: p.pid,
                        ppid: p.ppid,
                        pgid: p.pgid,
                        tpgid: p.tpgid,
                        tty: p.tty,
                        exe: p.exe,
                    },
                )
            })
            .collect()
    }
    fn cwd_of(&self, pid: i32) -> String {
        ferb_platform_macos::sessions::cwd_of(pid)
    }
    fn tmux_clients(&self, server: i32) -> Vec<(i32, String)> {
        let program = self.tmux_program(server);
        if program.is_empty() {
            return Vec::new();
        }
        ferb_platform_macos::sessions::tmux_clients(&program, &self.home, server)
    }
    fn app(&self, bundle: &str) -> (String, String) {
        ferb_platform_macos::sessions::app_of(bundle)
    }
    fn claude_agents(&self, exe: Option<&str>) -> Option<String> {
        ferb_platform_macos::sessions::claude_agents(&self.home, exe)
    }
    fn terminal_type(&self, tty: &str, text: &str) -> Result<(), String> {
        ferb_platform_macos::sessions::terminal_type(&self.home, tty, text)
    }
    fn terminal_open(&self, command: &str) -> Result<(), String> {
        ferb_platform_macos::sessions::terminal_open(&self.home, command)
    }
    fn tmux_send(
        &self,
        server: i32,
        tty: &str,
        text: &str,
        key: &str,
        enter: bool,
    ) -> Result<(), String> {
        let program = self.tmux_program(server);
        if program.is_empty() {
            return Err("tmux's server isn't there any more".into());
        }
        ferb_platform_macos::sessions::tmux_send(
            &program, &self.home, server, tty, text, key, enter,
        )
    }
    fn foreground(&self, f: Foreground) -> BoxFuture<'static, Result<(), String>> {
        let driver = self
            .tools
            .upgrade()
            .and_then(|p| p.mac().map(|m| m.tools.driver.clone()));
        Box::pin(async move {
            let Some(driver) = driver else {
                return Err("FERB's Mac hands aren't running".to_string());
            };
            foreground(driver, f).await
        })
    }
}

/// HAND-660: the generic foreground path. The host comes to the front; the focused terminal must show the session's
/// title (or, before it has one, its folder) in its own or its window's words; then the Mac hands' foreground typing
/// types the whole text into it as a terminal input the owner said Yes to (HAND-519, GATE-585's grant), Return
/// follows, and the app that was in front comes back.
#[cfg(target_os = "macos")]
async fn foreground(
    driver: Arc<ferb_mac_hands::driver::MacDriver>,
    f: Foreground,
) -> Result<(), String> {
    use ferb_core::types::{
        ComputerAction, ComputerActionKind, Mechanism, TerminalGrant, TerminalInput,
    };
    let before = driver.apps.frontmost();
    let (d, bundle, pid) = (driver.clone(), f.bundle_id.clone(), f.pid);
    tokio::task::spawn_blocking(move || d.apps.activate(&bundle, Some(pid)))
        .await
        .map_err(|e| e.to_string())?;
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let restore = |driver: &Arc<ferb_mac_hands::driver::MacDriver>| {
        if let Some(b) = &before {
            if b.pid != f.pid {
                let _ = driver.apps.activate(&b.bundle_id, Some(b.pid));
            }
        }
    };
    // the proof: the session's own words around the focused element
    let d = driver.clone();
    let words = tokio::task::spawn_blocking(move || {
        let el = d.ax.focused_element(pid)?;
        let chain = d.ax.surface_facts(&el).unwrap_or_default();
        Some(
            chain
                .iter()
                .flat_map(|m| m.values())
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
                .to_lowercase(),
        )
    })
    .await
    .map_err(|e| e.to_string())?;
    let proven = match (&words, f.proof.first()) {
        (Some(w), Some(p)) => w.contains(&p.to_lowercase()),
        _ => false,
    };
    if !proven {
        restore(&driver);
        return Err(format!(
            "FERB couldn't tell that the terminal in front in {} is this session's, so it typed nothing. Click into the session's terminal once, then send again.",
            if f.app.is_empty() { "its app" } else { &f.app }
        ));
    }
    let confirmation = format!("sessions:{}:{}", f.bundle_id, f.pid);
    let grant_for =
        |text: Option<String>, keys: Option<String>, enter: bool, covered: bool| TerminalGrant {
            input: TerminalInput {
                app: f.app.clone(),
                bundle_id: f.bundle_id.clone(),
                text,
                keys,
                enter,
                covered_by: covered.then(|| confirmation.clone()),
                ..TerminalInput::default()
            },
            confirmation_id: confirmation.clone(),
        };
    let action = |kind, text: &str| ComputerAction {
        kind,
        element: None,
        text: Some(text.to_owned()),
        select: Value::Null,
        direction: None,
        amount: None,
        window: None,
        allow_foreground: true,
        mechanism: Some(Mechanism::Foreground),
        name: None,
    };
    let out = if !f.key.is_empty() {
        let g = grant_for(None, Some(f.key.clone()), false, false);
        driver
            .act(
                &f.bundle_id,
                &action(ComputerActionKind::Key, &f.key),
                "sessions",
                None,
                Some(&g),
            )
            .await
    } else {
        let g = grant_for(Some(f.text.clone()), None, f.enter, false);
        let typed = driver
            .act(
                &f.bundle_id,
                &action(ComputerActionKind::Type, &f.text),
                "sessions",
                None,
                Some(&g),
            )
            .await;
        match typed {
            Ok(r) if r.ok && f.enter => {
                let g = grant_for(None, Some("return".into()), false, true);
                driver
                    .act(
                        &f.bundle_id,
                        &action(ComputerActionKind::Key, "return"),
                        "sessions",
                        None,
                        Some(&g),
                    )
                    .await
            }
            other => other,
        }
    };
    restore(&driver);
    match out {
        Ok(r) if r.ok => Ok(()),
        Ok(r) => Err(r.detail),
        Err(e) => Err(e.to_string()),
    }
}

/// The own question's answer, through the gateway's one store (GATE-613's pattern): set aside if the area drops it.
fn ask_with(gw: Arc<Gateway>) -> AskFn {
    Arc::new(move |a: Ask| {
        let ap = gw.ask_own(&ferb_gateway::gateway::OwnQuestion {
            capability: a.capability.clone(),
            reason: format!("{}\n{}", a.summary, a.detail),
            options: a.options.clone(),
            request_id: String::new(),
            summary: Some(a.summary.clone()),
            detail: Some(a.detail.clone()),
            labels: a.options.clone(),
            task_id: String::new(),
            options_only: true,
            approval_id: None,
            part: String::new(),
            runtime_switch: None,
            from_task: false,
            corrects: false,
            exact: a.detail.clone(),
            event: ferb_gateway::gateway::OwnEvent::Publish,
        });
        let id = ap.id.clone();
        let gw = gw.clone();
        let answer: BoxFuture<'static, Option<String>> = Box::pin(async move {
            let ap = gw
                .await_own(&id, &ferb_gateway::gateway::Cancel::new())
                .await
                .ok()
                .flatten()?;
            match ap.state {
                ferb_gateway::questions::State::Granted | ferb_gateway::questions::State::Used => {
                    ap.choice
                }
                _ => None,
            }
        });
        (ap.id, answer)
    })
}

/// Builds the area and registers `sessions.*` on `server`.
pub fn build(server: &Server, p: Parts) -> Sessions {
    let mac: Arc<dyn Mac> = native_mac(&p);
    let config = p.config.clone();
    let config2 = p.config.clone();
    let journal = p.journal.clone();
    let journal2 = p.journal.clone();
    let events = p.events.clone();
    let redactor = p.redactor.clone();
    let gw = p.gateway.clone();
    let sup = p.supervisor.clone();
    let clock = p.clock.clone();
    let clock2 = p.clock.clone();
    let ids = p.ids.clone();
    let notifier = p.notifier.clone();
    let runtime = tokio::runtime::Handle::try_current().ok();
    let seams = Seams {
        mac,
        home: p.user_home.clone(),
        hook_program: if p.native {
            hook_program(p.app_bundle.as_deref())
        } else {
            String::new()
        },
        own_pid: std::process::id() as i32,
        enabled: Arc::new(move || match config.get(SECTION, KEY) {
            Ok(Some(Value::Bool(b))) => Some(b),
            _ => None,
        }),
        set_enabled: Arc::new(move |on: bool| {
            config2
                .set(SECTION, KEY, &json!(on))
                .map_err(|e| e.to_string())
        }),
        setup_done: Arc::new(move || !journal2.of_kind("setup.finished", "", "").is_empty()),
        redact: Arc::new(move |t: &str| redactor.text(t)),
        publish: Arc::new(move |topic: &str, data: Value| {
            events.publish(topic, crate::basics::object(data))
        }),
        journal: Arc::new(move |kind: &str, data: Value| journal.note(kind, data)),
        ask: ask_with(gw.clone()),
        close_ask: Arc::new(move |id: &str| {
            gw.close_own(id);
        }),
        hand_over: Arc::new(move |text: String, client: Option<String>| {
            let mut params = serde_json::Map::new();
            params.insert("text".into(), json!(text));
            if let Some(c) = client {
                params.insert("client_id".into(), json!(c));
            }
            let mut rx = sup.chat_send_params(&params).map_err(|r| r.0)?;
            // the turn's own events reach every client through the chat's entries; nobody reads these
            if let Some(rt) = tokio::runtime::Handle::try_current()
                .ok()
                .or_else(|| runtime.clone())
            {
                rt.spawn(async move { while rx.recv().await.is_some() {} });
            }
            Ok(())
        }),
        tell: Arc::new(move |ident: &str, text: &str| {
            let (n, ident, text) = (notifier.clone(), ident.to_owned(), text.to_owned());
            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                rt.spawn(async move {
                    let _ = n
                        .tell_owner(
                            &ident,
                            &[],
                            &text,
                            ferb_sanitize::origin::FERB,
                            "",
                            "",
                            &[],
                            &Value::Null,
                            None,
                        )
                        .await;
                });
            }
        }),
        now: Arc::new(move || ferb_core::when::isoformat(&clock.now())),
        mono: Arc::new(move || clock2.monotonic()),
        new_id: Arc::new(move || ids.new_id()),
        version: super::VERSION.to_owned(),
    };
    let s = Sessions::new(seams);
    ferb_daemon::sessions::register(&s, server);
    s
}

#[cfg(target_os = "macos")]
fn native_mac(p: &Parts) -> Arc<dyn Mac> {
    if p.native {
        return Arc::new(NativeMac {
            home: p.user_home.clone(),
            tools: p.tools.clone(),
        });
    }
    Arc::new(AbsentMac)
}

#[cfg(not(target_os = "macos"))]
fn native_mac(_p: &Parts) -> Arc<dyn Mac> {
    Arc::new(AbsentMac)
}

/// How often the area looks (the process table, the transcripts' new lines; never a model).
pub const LOOK_MS: u64 = 2000;

/// The area's loop, its question once setup is done, and what ends a control or a hand-over.
pub fn start_loops(d: &Arc<super::Daemon>, s: &Sessions) {
    // decision: GATE-622 — a phone's control ends with its lock; a hand-over ends with its task
    {
        let s = s.clone();
        d.events.events.on(
            Some(&["phone.locked", "task.finished"]),
            Arc::new(move |topic: &str, data: serde_json::Map<String, Value>| {
                let text = |k: &str| data.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
                match topic {
                    "phone.locked" => {
                        s.release_control(
                            None,
                            &ferb_daemon::sessions::Caller::Phone(text("phone")),
                        );
                    }
                    _ => s.task_ended(&text("task_id")),
                }
            }),
        );
    }
    let s = s.clone();
    d.spawn_loop("sessions", move |_| {
        let s = s.clone();
        async move {
            loop {
                let one = s.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    one.look();
                    one.offer();
                })
                .await;
                tokio::time::sleep(std::time::Duration::from_millis(LOOK_MS)).await;
            }
        }
    });
}

/// PHN-797: one frame of a session's window for its Watch view (the `tasks.preview` answer with `session`): the
/// host app's window that shows the session (its title naming the session or its folder, else the app's front
/// window), captured as the live preview captures (APP-922: as it is, memory only); the caller reads its bytes for
/// FERB.app or the paired phone (PHN-454). The fields go before the image, as a task preview's do.
#[cfg(target_os = "macos")]
pub async fn watch_frame(
    parts: &super::tools::Parts,
    id: &str,
) -> Result<
    (
        serde_json::Map<String, Value>,
        ferb_mac_hands::point::OwnerOnlyFrame,
    ),
    String,
> {
    let area = parts
        .sessions()
        .ok_or("FERB doesn't see your coding sessions.")?;
    let s = area.session(id).ok_or("That session has ended.")?;
    let host = s
        .host
        .clone()
        .filter(|h| h.pid > 0 && !h.bundle_id.is_empty())
        .ok_or("No app on your Mac shows this session.")?;
    let driver = parts
        .mac()
        .map(|m| m.tools.driver.clone())
        .ok_or("FERB's Mac hands aren't running.")?;
    let folder = ferb_daemon::sessions::folder_name(&s.cwd);
    let words: Vec<String> = [s.title.clone(), folder.clone()]
        .into_iter()
        .filter(|w| !w.is_empty())
        .collect();
    let (d, pid) = (driver.clone(), host.pid);
    let window = tokio::task::spawn_blocking(move || {
        words
            .iter()
            .find_map(|w| d.shots.front_window(pid, None, Some(w)))
            .or_else(|| d.shots.front_window(pid, None, None))
    })
    .await
    .map_err(|e| e.to_string())?
    .ok_or("Its window isn't open on your Mac.")?;
    let (_target, capture) = driver
        .owner_preview_source(&host.bundle_id, Some(&window.id.to_string()), None, None)
        .map_err(|e| e.to_string())?;
    let (frame, _) = capture().await.map_err(|e| e.to_string())?;
    let mut fields = serde_json::Map::new();
    fields.insert(
        "seq".into(),
        json!(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)),
    );
    fields.insert("state".into(), json!(s.status.as_str()));
    let title = if s.title.is_empty() {
        folder
    } else {
        s.title.clone()
    };
    fields.insert(
        "line".into(),
        json!(format!("{} · {title}", s.agent.name())),
    );
    fields.insert("fresh".into(), json!(true));
    Ok((fields, frame))
}
