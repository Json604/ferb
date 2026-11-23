//! The owner's coding-agent sessions (kb HAND-651..666) in this program: ferb-daemon's `sessions` area over the Mac's process
//! table and host adapters (`ferb_platform_macos::sessions`, only with the Mac's own parts on; a test daemon reads no
//! process and types nowhere), the gateway's own questions (the agents' permission prompts and questions, the Yes
//! that turns the feature on), the supervisor's session driver (Hand to FERB: HAND-707), the bus and the journal.
//! The generic foreground path is the Mac hands' own foreground typing (HAND-519), after code has proven the focused
//! terminal is the session's (HAND-660); the window path (HAND-712) brings the session's window onto the screen and
//! types into it while the owner watches. Nothing here reads another app's text (HAND-711).

use std::path::PathBuf;
use std::sync::Arc;

use ferb_core::interfaces::{BoxFuture, Clock, IdSource};
use ferb_daemon::sessions::{
    Act, Ask, AskFn, Decision, Driver, Foreground, Mac, NotTyped, Seams, Sessions, Step, KEY,
    SECTION,
};
use ferb_gateway::gateway::Gateway;
use ferb_rpc::Server;
#[cfg(target_os = "macos")]
use ferb_sessions::apps::AppWindow;
use ferb_sessions::host::{Proc, Table};
#[cfg(target_os = "macos")]
use ferb_sessions::proof::{proven, terminal_input, Focus, Unproven};
#[cfg(target_os = "macos")]
use ferb_sessions::TerminalChoice;
#[cfg(target_os = "macos")]
type WinInfo = ferb_mac_hands::driver::Window;
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
    fn foreground(&self, _f: Foreground) -> BoxFuture<'static, Result<(), NotTyped>> {
        Box::pin(async {
            Err("this FERB runs without the Mac's own parts"
                .to_string()
                .into())
        })
    }
}

/// The real Mac.
#[cfg(target_os = "macos")]
struct NativeMac {
    home: PathBuf,
    tools: super::tools::WeakParts,
    /// The daemon's clock: a terminal's last activity is written in its offset (HAND-692).
    clock: Arc<dyn Clock>,
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
    fn app_pid(&self, bundle_id: &str, found: i32) -> i32 {
        // HAND-654, PHN-797: the running app's own process owns its windows; the found one when it is that, or when
        // there isn't exactly one to take instead
        let Some(d) = self.driver() else { return found };
        let id = ferb_sanitize::parity::casefold(bundle_id);
        match d.apps.pids_of(&[id.as_str()]).as_slice() {
            pids if pids.contains(&found) => found,
            [one] => *one,
            _ => found,
        }
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
    fn foreground(&self, f: Foreground) -> BoxFuture<'static, Result<(), NotTyped>> {
        let driver = self
            .tools
            .upgrade()
            .and_then(|p| p.mac().map(|m| m.tools.driver.clone()));
        Box::pin(async move {
            let Some(driver) = driver else {
                return Err("FERB's Mac hands aren't running".to_string().into());
            };
            foreground(driver, f).await
        })
    }
    // decision: HAND-711 — the agent and chat apps' open windows, by structure (app and title) only
    fn app_windows(&self, bundles: &[String]) -> Vec<AppWindow> {
        let Some(driver) = self.driver() else {
            return Vec::new();
        };
        app_windows(&driver, bundles)
    }
    // decision: HAND-692 — a terminal's program, activity and window, from the Mac's own records
    fn program_of(&self, pid: i32, exe: &str) -> String {
        ferb_sessions::terminals::program_name(&ferb_platform_macos::sessions::argv0_of(pid), exe)
    }
    fn tty_activity(&self, tty: &str) -> Option<(f64, String)> {
        let at = ferb_platform_macos::sessions::tty_activity(tty)?;
        let offset = *self.clock.now().offset();
        let dt = chrono::DateTime::from_timestamp(at.trunc() as i64, (at.fract() * 1e9) as u32)?
            .with_timezone(&offset);
        Some((at, ferb_core::when::isoformat(&dt)))
    }
    fn window_titles(&self) -> Vec<(i32, i64, String)> {
        ferb_mac_hands::native::windows::NativeWindows
            .listed(true)
            .unwrap_or_default()
            .into_iter()
            .map(|l| l.window)
            // the windows that draw, as the session's window is found (ferb-sessions' `window`, PHN-797)
            .filter(|w| w.normal_layer() && w.alpha > 0.0 && !w.title.trim().is_empty())
            .map(|w| (w.pid, w.id, w.title))
            .collect()
    }
    // decision: HAND-695 — a new window in the folder, in the background, typing nothing
    fn terminal_open_at(&self, bundle_id: &str, folder: &str) -> Result<(), String> {
        ferb_platform_macos::sessions::open_folder_in(&self.home, bundle_id, folder)
    }
}

#[cfg(target_os = "macos")]
impl NativeMac {
    fn driver(&self) -> Option<Arc<ferb_mac_hands::driver::MacDriver>> {
        self.tools
            .upgrade()
            .and_then(|p| p.mac().map(|m| m.tools.driver.clone()))
    }
}

/// HAND-711: the open windows of these apps now, on any desktop: normal windows that draw, with a title, big enough to
/// show something (an app's own strips and bars are left out: ferb-sessions' `apps::pick`).
#[cfg(target_os = "macos")]
pub fn app_windows(
    driver: &Arc<ferb_mac_hands::driver::MacDriver>,
    bundles: &[String],
) -> Vec<AppWindow> {
    let apps: Vec<(String, String, i32)> = driver
        .apps
        .running()
        .into_iter()
        .filter(|a| bundles.contains(&a.bundle_id))
        .map(|a| (a.bundle_id, a.name, a.pid))
        .collect();
    if apps.is_empty() {
        return Vec::new();
    }
    ferb_sessions::apps::pick(&apps, &listed_windows())
}

/// The window server's windows on every desktop, front to back, each with whether it is on the desktop on screen.
#[cfg(target_os = "macos")]
pub fn listed_windows() -> Vec<ferb_sessions::apps::Listed> {
    let list = ferb_mac_hands::native::windows::NativeWindows;
    let here: std::collections::HashSet<i64> = list
        .listed(false)
        .unwrap_or_default()
        .into_iter()
        .map(|l| l.window.id)
        .collect();
    list.listed(true)
        .unwrap_or_default()
        .into_iter()
        .map(|l| {
            let w = l.window;
            ferb_sessions::apps::Listed {
                pid: w.pid,
                id: w.id,
                normal: w.normal_layer(),
                alpha: w.alpha,
                size: (w.bounds.2, w.bounds.3),
                on_screen: here.contains(&w.id),
                title: w.title,
            }
        })
        .collect()
}

/// The window's Accessibility element and its nodes (roles and labels: where its terminals are) (Electron apps build their tree when asked: `expose_tree`),
/// page by page up to CHAT_TREE_NODES.
#[cfg(target_os = "macos")]
fn ax_nodes(
    driver: &Arc<ferb_mac_hands::driver::MacDriver>,
    w: &WinInfo,
) -> Option<(ferb_mac_hands::ax::El, Vec<ferb_mac_hands::ax::AxNode>)> {
    let window = driver.shots.window(w.id)?;
    let _ = driver.ax.expose_tree(w.pid);
    let el = driver
        .ax
        .window_with_frame(w.pid, window.bounds, Some(&w.title), None)?;
    let mut nodes = Vec::new();
    let mut offset = 0;
    loop {
        let tree = driver
            .ax
            .read_tree(w.pid, false, Some(&el), offset, Some(3.0), false);
        nodes.extend(tree.nodes);
        match tree.next_offset {
            Some(n) if n > offset && nodes.len() < TREE_NODES => offset = n,
            _ => break,
        }
    }
    Some((el, nodes))
}

/// The most elements FERB reads of one window when it looks for its terminals (their roles and labels, never text).
pub const TREE_NODES: usize = 20_000;

/// HAND-660: the generic foreground path. The host comes to the front; the focused terminal must show the session's
/// title (or, before it has one, its folder) in its own or its window's words; then the Mac hands' foreground typing
/// types the whole text into it as a terminal input the owner said Yes to (HAND-519, GATE-585's grant), Return
/// follows, and the app that was in front comes back.
#[cfg(target_os = "macos")]
async fn foreground(
    driver: Arc<ferb_mac_hands::driver::MacDriver>,
    f: Foreground,
) -> Result<(), NotTyped> {
    use ferb_core::types::{
        ComputerAction, ComputerActionKind, Mechanism, TerminalGrant, TerminalInput,
    };
    // decision: HAND-712 — the window path: no terminal proof, the window the owner watches
    if f.into_window {
        return window_type(driver, f).await;
    }
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
    // decision: HAND-682 — the proof: a terminal by structure, the session's by its title or by its program and folder;
    // decision: HAND-683 — else the terminal the owner chose, still the same element; else the ones he may choose from
    let chosen_el = {
        let (d, f2) = (driver.clone(), f.clone());
        let found = tokio::task::spawn_blocking(move || terminal_for(&d, &f2))
            .await
            .map_err(|e| e.to_string())?;
        match found {
            Ok(el) => el,
            Err(e) => {
                restore(&driver);
                return Err(e);
            }
        }
    };
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
    // HAND-683: the chosen terminal is still the element in front, right before typing
    if let Some(el) = &chosen_el {
        let still = driver
            .ax
            .focused_element(f.pid)
            .is_some_and(|now| driver.ax.same(&now, el));
        if !still {
            restore(&driver);
            let (d, f2) = (driver.clone(), f.clone());
            let terminals = tokio::task::spawn_blocking(move || terminal_choices(&d, &f2))
                .await
                .unwrap_or_default();
            return Err(NotTyped::Choose {
                why: changed_words(&f.app),
                terminals,
            });
        }
    }
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
        Ok(r) => Err(r.detail.into()),
        Err(e) => Err(e.to_string().into()),
    }
}

/// HAND-712, HAND-713: the session's window (found as Watch finds it) brought onto the screen: back from the Dock when
/// minimized, its app activated and the window raised, which switches to its desktop (Space) when it is on another one.
/// Answers the window as it is now, or why it couldn't.
#[cfg(target_os = "macos")]
pub fn bring_into_view(
    driver: &Arc<ferb_mac_hands::driver::MacDriver>,
    bundle_id: &str,
    app: &str,
    pid: i32,
    proof: &ferb_sessions::proof::Proof,
    chosen: &str,
    known: i64,
) -> Result<WinInfo, String> {
    use ferb_sessions::window::{session_window, Unseen};
    let wins = host_windows(pid);
    let facts: Vec<_> = wins.iter().map(|(_, x)| x.clone()).collect();
    let found = session_window(&facts, proof, chosen, known)
        .ok_or_else(|| Unseen::NoWindow.words(app, ""))?;
    let win = wins
        .into_iter()
        .find(|(w, _)| w.id == found.id)
        .map(|(w, _)| w)
        .ok_or_else(|| Unseen::NoWindow.words(app, ""))?;
    let minimized = driver.ax.window_minimized(pid, win.bounds) == Some(true);
    if found.on_screen && !minimized {
        return Ok(win);
    }
    driver.ax.show_window(pid, win.bounds);
    driver.apps.activate(bundle_id, Some(pid));
    driver.ax.raise_window(pid, win.bounds);
    // the desktop's switch animates: wait until the window is on screen, at most two seconds
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if driver.shots.on_screen(win.id) {
            return Ok(win);
        }
    }
    Err(if minimized {
        Unseen::Minimized.words(app, &found.title)
    } else {
        Unseen::OtherSpace.words(app, &found.title)
    })
}

/// HAND-712: the window path. The session's window onto the screen (HAND-713), its app in front with that window
/// focused, then the words and Return through the Mac hands' foreground typing under the owner's Yes (the session's
/// control, GATE-622). Nothing is typed unless the app in front and its focused window are the session's, checked right
/// before the words and again before Return. The window stays in front: the owner is watching it.
#[cfg(target_os = "macos")]
async fn window_type(
    driver: Arc<ferb_mac_hands::driver::MacDriver>,
    f: Foreground,
) -> Result<(), NotTyped> {
    use ferb_core::types::{
        ComputerAction, ComputerActionKind, Mechanism, TerminalGrant, TerminalInput,
    };
    let app = app_words(&f.app).to_owned();
    let (d, f2) = (driver.clone(), f.clone());
    let win = tokio::task::spawn_blocking(move || {
        let win = bring_into_view(
            &d,
            &f2.bundle_id,
            &f2.app,
            f2.pid,
            &f2.proof,
            &f2.chosen,
            f2.window,
        )?;
        if !d.apps.activate(&f2.bundle_id, Some(f2.pid)) {
            return Err(format!(
                "FERB couldn't bring {} to the front, so it typed nothing.",
                app_words(&f2.app)
            ));
        }
        d.ax.raise_window(f2.pid, win.bounds);
        std::thread::sleep(std::time::Duration::from_millis(400));
        Ok(win)
    })
    .await
    .map_err(|e| e.to_string())??;
    let in_front = |driver: &Arc<ferb_mac_hands::driver::MacDriver>| -> Option<String> {
        match driver.apps.frontmost() {
            Some(front) if front.pid == f.pid => {}
            Some(front) => {
                return Some(format!(
                    "{} came to the front instead of {app}, so FERB stopped.",
                    if front.name.is_empty() {
                        "Another app"
                    } else {
                        &front.name
                    }
                ))
            }
            None => {
                return Some(format!(
                    "FERB couldn't tell that {app} is in front, so it stopped."
                ))
            }
        }
        let focused = driver.ax.focused_window_frame(f.pid);
        if !focused.is_some_and(|fr| ferb_mac_hands::ax::same_frame(&fr, &win.bounds)) {
            return Some(format!(
                "Another window of {app} has the keyboard, not “{}”, so FERB stopped.",
                short(&win.title, 60)
            ));
        }
        None
    };
    if let Some(why) = in_front(&driver) {
        return Err(why.into());
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
                if let Some(why) = in_front(&driver) {
                    return Err(format!(
                        "{why} Your words may be typed there without being sent; check it in Watch."
                    )
                    .into());
                }
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
    match out {
        Ok(r) if r.ok => Ok(()),
        Ok(r) => Err(r.detail.into()),
        Err(e) => Err(e.to_string().into()),
    }
}

/// What the Mac shows of the focused element in `pid`: its own words, its chain's, and whether it is a terminal.
#[cfg(target_os = "macos")]
fn focus_of(driver: &Arc<ferb_mac_hands::driver::MacDriver>, bundle: &str, pid: i32) -> Focus {
    let Some(el) = driver.ax.focused_element(pid) else {
        return Focus::default();
    };
    let chain = driver.ax.surface_facts(&el).unwrap_or_default();
    let words = |facts: &[&serde_json::Map<String, Value>]| {
        facts
            .iter()
            .flat_map(|m| {
                ["title", "description"]
                    .into_iter()
                    .filter_map(|k| m.get(k))
            })
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n")
    };
    // the terminal's own words: the focused element and its containers below the window (not the window's title)
    let own: Vec<_> = chain
        .iter()
        .take_while(|m| {
            !matches!(
                m.get("role").and_then(Value::as_str),
                Some("AXWebArea" | "AXWindow" | "AXApplication")
            )
        })
        .collect();
    let all: Vec<_> = chain.iter().collect();
    Focus {
        own: words(&own),
        around: words(&all),
        terminal: ferb_policy::terminal::surface(bundle, &chain, &[], false, false).is_some(),
        terminal_app: ferb_policy::terminal::is_terminal_app(bundle),
    }
}

fn app_words(app: &str) -> &str {
    if app.is_empty() {
        "its app"
    } else {
        app
    }
}

fn changed_words(app: &str) -> String {
    format!(
        "The terminal you chose for this session in {} isn't there as it was, so FERB typed nothing. Choose it again.",
        app_words(app)
    )
}

/// The words of one choice: at most `n` characters of its first line.
fn short(text: &str, n: usize) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() <= n {
        line.to_owned()
    } else {
        format!(
            "{}…",
            line.chars().take(n - 1).collect::<String>().trim_end()
        )
    }
}

/// The app's normal titled windows (its own, by pid).
#[cfg(target_os = "macos")]
fn titled_windows(pid: i32) -> Vec<WinInfo> {
    host_windows(pid)
        .into_iter()
        .map(|(w, _)| w)
        .filter(|w| !w.title.trim().is_empty())
        .collect()
}

/// The app's normal windows that draw (alpha above 0), front to back, each with the facts that find a session's
/// window (ferb-sessions' `window`: its title, size and whether it is on the Space on screen now).
#[cfg(target_os = "macos")]
fn host_windows(pid: i32) -> Vec<(WinInfo, ferb_sessions::window::Win)> {
    let list = ferb_mac_hands::native::windows::NativeWindows;
    let here: std::collections::HashSet<i64> = list
        .listed(false)
        .unwrap_or_default()
        .into_iter()
        .map(|l| l.window.id)
        .collect();
    list.listed(true)
        .unwrap_or_default()
        .into_iter()
        .map(|l| l.window)
        .filter(|w| w.pid == pid && w.normal_layer() && w.alpha > 0.0)
        .map(|w| {
            let facts = ferb_sessions::window::Win {
                id: w.id,
                title: w.title.clone(),
                size: (w.bounds.2, w.bounds.3),
                on_screen: here.contains(&w.id),
            };
            (w, facts)
        })
        .collect()
}

/// The terminals in one window, by structure (ferb-policy's surface), in reading order: each with its element and
/// the app's own words for it.
#[cfg(target_os = "macos")]
fn window_terminals(
    driver: &Arc<ferb_mac_hands::driver::MacDriver>,
    bundle: &str,
    app: &str,
    w: &WinInfo,
) -> Vec<(ferb_mac_hands::ax::El, String)> {
    let _ = app;
    let Some((_, nodes)) = ax_nodes(driver, w) else {
        return Vec::new();
    };
    nodes
        .into_iter()
        .filter(|n| matches!(n.ax_role.as_str(), "AXTextField" | "AXTextArea") && !n.secret)
        .filter(|n| {
            let chain = driver.ax.surface_facts(&n.r#ref).unwrap_or_default();
            ferb_policy::terminal::surface(bundle, &chain, &[], false, false).is_some()
        })
        .map(|n| (n.r#ref, n.label))
        .collect()
}

/// HAND-683: the terminals the owner may choose from in the session's app: the one in front now (when the focus is a
/// terminal), and every terminal in its windows, by the app's own words (its window's title and the terminal's name).
#[cfg(target_os = "macos")]
fn terminal_choices(
    driver: &Arc<ferb_mac_hands::driver::MacDriver>,
    f: &Foreground,
) -> Vec<TerminalChoice> {
    let mut out = Vec::new();
    let focus = focus_of(driver, &f.bundle_id, f.pid);
    if focus.terminal {
        out.push(TerminalChoice {
            id: "front".into(),
            label: format!(
                "The terminal in front in {} now ({})",
                app_words(&f.app),
                short(&focus.own, 40)
            ),
        });
    }
    for w in titled_windows(f.pid).iter().take(6) {
        for (n, (_, label)) in window_terminals(driver, &f.bundle_id, &f.app, w)
            .into_iter()
            .enumerate()
        {
            out.push(TerminalChoice {
                id: format!("w{}:{}:{}", w.id, n, label),
                label: format!("{} · {}", short(&w.title, 40), short(&label, 40)),
            });
        }
    }
    out.truncate(12);
    out
}

/// HAND-682, HAND-683: the terminal to type into, in front and focused. Proven: the session's by its words (None).
/// Chosen by the owner: found again as it was and focused (its element, for the check right before typing). Else
/// the ones he may choose from.
#[cfg(target_os = "macos")]
fn terminal_for(
    driver: &Arc<ferb_mac_hands::driver::MacDriver>,
    f: &Foreground,
) -> Result<Option<ferb_mac_hands::ax::El>, NotTyped> {
    let app = app_words(&f.app);
    if !f.chosen.is_empty() {
        let again = || NotTyped::Choose {
            why: changed_words(&f.app),
            terminals: terminal_choices(driver, f),
        };
        if f.chosen == "front" {
            let focus = focus_of(driver, &f.bundle_id, f.pid);
            return match driver.ax.focused_element(f.pid) {
                Some(el) if focus.terminal => Ok(Some(el)),
                _ => Err(again()),
            };
        }
        // "w<window>:<n>:<its words>": that window's n-th terminal, with the same words
        let mut parts = f.chosen.splitn(3, ':').skip(1);
        let (n, label) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        let (Some(wid), Ok(n)) = (
            ferb_sessions::window::chosen_window(&f.chosen),
            n.parse::<usize>(),
        ) else {
            return Err(again());
        };
        let Some(win) = titled_windows(f.pid).into_iter().find(|x| x.id == wid) else {
            return Err(again());
        };
        driver.ax.raise_window(f.pid, win.bounds);
        let terms = window_terminals(driver, &f.bundle_id, &f.app, &win);
        let Some((el, _)) = terms.get(n).filter(|(_, l)| l == label) else {
            return Err(again());
        };
        if !driver.ax.focus(el) {
            return Err(again());
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
        return match driver.ax.focused_element(f.pid) {
            Some(now) if driver.ax.same(&now, el) => Ok(Some(el.clone())),
            _ => Err(again()),
        };
    }
    match prove_terminal(driver, f) {
        Ok(()) => Ok(None),
        Err(why) => {
            let words = match why {
                Unproven::NotATerminal => format!(
                    "The cursor in {app} isn't in a terminal, and FERB couldn't tell which terminal there is this \
                     session's, so it typed nothing."
                ),
                Unproven::NotTheSession => {
                    format!("FERB couldn't tell which terminal in {app} is this session's, so it typed nothing.")
                }
            };
            let terminals = terminal_choices(driver, f);
            if terminals.is_empty() {
                Err(NotTyped::Failed(format!(
                    "{words} Open the session's terminal in {app}, then send again."
                )))
            } else {
                Err(NotTyped::Choose {
                    why: format!("{words} Choose it, and FERB types only there."),
                    terminals,
                })
            }
        }
    }
}

/// HAND-660, HAND-682: the focused terminal must be the session's. When it isn't, and the session's program and folder
/// are offered: in the one window of the app that names the folder, the one text input whose own words name the
/// program (xterm.js's, in an editor) gets the cursor, and the proof is read again. Nothing is typed here.
#[cfg(target_os = "macos")]
fn prove_terminal(
    driver: &Arc<ferb_mac_hands::driver::MacDriver>,
    f: &Foreground,
) -> Result<(), Unproven> {
    let first = proven(&f.proof, &focus_of(driver, &f.bundle_id, f.pid));
    if first.is_ok() || f.proof.program.is_empty() || f.proof.folder.is_empty() {
        return first;
    }
    let all = host_windows(f.pid);
    let facts: Vec<_> = all.iter().map(|(_, x)| x.clone()).collect();
    let windows: Vec<&WinInfo> = ferb_sessions::window::naming_folder(&facts, &f.proof.folder)
        .into_iter()
        .filter_map(|x| all.iter().find(|(w, _)| w.id == x.id).map(|(w, _)| w))
        .collect();
    let [w] = windows.as_slice() else {
        return first;
    };
    driver.ax.raise_window(f.pid, w.bounds);
    let Some((_, nodes)) = ax_nodes(driver, w) else {
        return first;
    };
    let inputs: Vec<_> = nodes
        .iter()
        .filter(|n| matches!(n.ax_role.as_str(), "AXTextField" | "AXTextArea") && !n.secret)
        .collect();
    let labels: Vec<&str> = inputs.iter().map(|n| n.label.as_str()).collect();
    let Some(i) = terminal_input(&labels, &f.proof.program) else {
        return first;
    };
    if !driver.ax.focus(&inputs[i].r#ref) {
        return first;
    }
    std::thread::sleep(std::time::Duration::from_millis(150));
    proven(&f.proof, &focus_of(driver, &f.bundle_id, f.pid))
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

/// HAND-707: Hand to FERB's session driver as this program composes it: the supervisor's driver task and its fresh
/// worker step (ferb-supervisor `flow::driver`), code's typing through the gateway's whole pipeline (`agent_session.send`
/// or `.answer` for the drive's worker task: GATE-585's question, one Yes per app for the whole drive, and every other
/// rule), and one notice in FERB's chat for its last line.
pub struct SupDriver {
    pub sup: ferb_supervisor::flow::Supervisor,
    pub gateway: Arc<Gateway>,
    pub notifier: Arc<ferb_daemon::areas_b::notify::Notifier>,
}

impl Driver for SupDriver {
    fn begin(
        &self,
        session: &str,
        goal: &str,
        app: &str,
    ) -> BoxFuture<'static, Result<String, String>> {
        let sup = self.sup.clone();
        let (session, goal, app) = (session.to_owned(), goal.to_owned(), app.to_owned());
        Box::pin(async move { Ok(sup.drive_begin(&session, &goal, &app).await) })
    }

    fn step(&self, task: &str, step: Step) -> BoxFuture<'static, Result<Decision, String>> {
        use ferb_supervisor::flow::driver as d;
        let sup = self.sup.clone();
        let gateway = self.gateway.clone();
        let task = task.to_owned();
        Box::pin(async move {
            let step = d::DriverStep {
                round: step.round,
                goal: step.goal,
                session: step.session,
                exchanges: step.exchanges,
                asking: step.asking,
                decided: step.decided,
            };
            let decided = sup.drive_step(&task, &step).await;
            // decision: HAND-714 — a step FERB's guard stopped (ORCH-691) is said as such, so the owner is asked
            let worker =
                ferb_supervisor::flow::tree::step_task(&task, ferb_supervisor::flow::tree::WORKER);
            if let Some(why) = gateway.take_stuck(&worker) {
                return Ok(Decision::Stuck(ferb_gateway::stuck::words(&why)));
            }
            decided.map(|decided| match decided {
                d::Decision::Send(t) => Decision::Send(t),
                d::Decision::Done(t) => Decision::Done(t),
                d::Decision::Ask(t) => Decision::Ask(t),
            })
        })
    }

    fn keep_trying(&self, task: &str) {
        let worker =
            ferb_supervisor::flow::tree::step_task(task, ferb_supervisor::flow::tree::WORKER);
        self.gateway.keep_trying(&worker);
    }

    fn deliver(
        &self,
        task: &str,
        session: &str,
        act: Act,
    ) -> BoxFuture<'static, Result<(), String>> {
        let gateway = self.gateway.clone();
        let scopes = self.sup.inner().deps.daemon.all_scopes();
        let grant = ferb_rpc::auth::RunGrant {
            task_id: ferb_supervisor::flow::tree::step_task(
                task,
                ferb_supervisor::flow::tree::WORKER,
            ),
            worker_id: format!("driver-{task}"),
            scopes,
            identity: ferb_rpc::auth::Identity::Owner,
            expires_at: f64::INFINITY,
            observe_only: false,
            tools: std::collections::BTreeSet::new(),
            max_calls: 0,
        };
        let (name, args) = match act {
            // code waits for the agent's turn itself: the tool only types and proves delivery
            Act::Send(text) => (
                "agent_session.send",
                json!({"session": session, "text": text, "wait": false}),
            ),
            Act::Answer { card, choice } => (
                "agent_session.answer",
                json!({"session": session, "card": card, "choice": choice}),
            ),
        };
        let args = crate::basics::object(args);
        Box::pin(async move {
            let reply = gateway
                .call(&grant, name, &args)
                .await
                .map_err(|e| match e {
                    ferb_gateway::gateway::CallError::Cancelled => "It was stopped.".to_string(),
                    ferb_gateway::gateway::CallError::Internal(m) => m,
                })?;
            if !reply.is_error {
                return Ok(());
            }
            Err(reply
                .content
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| reply.content.to_string()))
        })
    }

    fn stopped(&self, task: &str) -> bool {
        self.sup.drive_stopped(task)
    }

    fn end(&self, task: &str, status: &str, summary: &str) -> BoxFuture<'static, ()> {
        let sup = self.sup.clone();
        let (task, status, summary) = (task.to_owned(), status.to_owned(), summary.to_owned());
        Box::pin(async move { sup.drive_end(&task, &status, &summary).await })
    }

    fn say(&self, ident: &str, line: &str) {
        let n = self.notifier.clone();
        let (ident, line) = (ident.to_owned(), line.to_owned());
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                let _ = n
                    .tell_owner(
                        &ident,
                        &[],
                        &line,
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
    }
}

/// Builds the area and registers `sessions.*` on `server`.
pub fn build(server: &Server, p: Parts) -> Sessions {
    let mac: Arc<dyn Mac> = native_mac(&p);
    let config = p.config.clone();
    let config2 = p.config.clone();
    let config3 = p.config.clone();
    let journal = p.journal.clone();
    let journal2 = p.journal.clone();
    let events = p.events.clone();
    let redactor = p.redactor.clone();
    let gw = p.gateway.clone();
    let driver_gw = p.gateway.clone();
    let driver_notifier = p.notifier.clone();
    let sup = p.supervisor.clone();
    let clock = p.clock.clone();
    let clock2 = p.clock.clone();
    let ids = p.ids.clone();
    let notifier = p.notifier.clone();
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
        // decision: HAND-707 — Hand to FERB runs the session driver: the supervisor's task and fresh step, the gateway's
        // typing, FERB's chat for its last line; no chat turn, planner or checker
        driver: Arc::new(SupDriver {
            sup: sup.clone(),
            gateway: driver_gw,
            notifier: driver_notifier,
        }),
        // decision: HAND-706 — a session's finished turn is one Mac notification per session (its ident replaces the
        // last), shown while FERB.app isn't showing it, never a line in the owner's chat
        tell: Arc::new(move |ident: &str, text: &str| {
            let n = notifier.clone();
            let banner = ferb_daemon::areas_b::banners::Banner {
                title: "FERB".into(),
                text: text.to_owned(),
                category: "task_finished".into(),
                when_hidden: true,
                ident: ident.to_owned(),
                target: serde_json::Map::new(),
                card: serde_json::Map::new(),
            };
            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                rt.spawn(async move { n.notify(banner, false).await });
            }
        }),
        now: Arc::new(move || ferb_core::when::isoformat(&clock.now())),
        mono: Arc::new(move || clock2.monotonic()),
        new_id: Arc::new(move || ids.new_id()),
        version: super::VERSION.to_owned(),
        // HAND-711: the agent and chat apps the owner added (config [sessions] chat_apps, bundle ids): their windows
        chat_apps: Arc::new(move || match config3.get(SECTION, "chat_apps") {
            Ok(Some(Value::Array(a))) => a
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        }),
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
            clock: p.clock.clone(),
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
                    _ => s.task_ended(&text("task_id"), &text("status")),
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

/// HAND-713: a Watch whose frames come closer together than this is one Watch; a longer gap starts a new one.
pub const WATCH_GAP_S: f64 = 5.0;

/// When each session's Watch last asked for a frame (one Watch brings its window onto the screen once).
fn watch_seen() -> &'static std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>
{
    static SEEN: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>,
    > = std::sync::OnceLock::new();
    SEEN.get_or_init(Default::default)
}

/// PHN-797: one frame of a session's window for its Watch view (the `tasks.preview` answer with `session`): the host
/// app's window that shows the session, found as a send finds it (the daemon's `Place`: an app window's own, the
/// terminal the owner chose, the window naming the session's title or folder; ferb-sessions' `window`), else the
/// app's front window; captured as the live preview captures (APP-922: as it is, memory only); the caller reads its
/// bytes for FERB.app or the paired phone (PHN-454). HAND-713 (the owner: "just switch the desktop"): when a Watch
/// opens on a window that is minimized or on another desktop, FERB brings it onto the screen first, once per Watch, so
/// a Watch left open never pulls the owner back if he moves away on the Mac. The fields go before the image.
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
    use ferb_sessions::window::{session_window, Unseen};
    let area = parts.sessions().ok_or("FERB doesn't see your sessions.")?;
    let s = area.session(id).ok_or("That session has ended.")?;
    // decision: HAND-682, HAND-683 — the send path's facts: one way to find where the session is
    let place = area
        .place(id)
        .ok_or("No app on your Mac shows this session.")?;
    let driver = parts
        .mac()
        .map(|m| m.tools.driver.clone())
        .ok_or("FERB's Mac hands aren't running.")?;
    let opening = {
        let mut seen = watch_seen().lock().unwrap_or_else(|e| e.into_inner());
        let now = std::time::Instant::now();
        let opening = seen
            .get(id)
            .is_none_or(|t| now.duration_since(*t).as_secs_f64() > WATCH_GAP_S);
        seen.insert(id.to_owned(), now);
        seen.retain(|_, t| now.duration_since(*t).as_secs_f64() <= 600.0);
        opening
    };
    let (pid, app) = (place.host.pid, place.host.name.clone());
    let (d, p2) = (driver.clone(), place.clone());
    let (window, found) = tokio::task::spawn_blocking(move || {
        // decision: HAND-713 — a new Watch brings its window onto the screen (desktop switched, back from the Dock)
        if opening {
            let w = bring_into_view(
                &d,
                &p2.host.bundle_id,
                &p2.host.name,
                pid,
                &p2.proof,
                &p2.chosen,
                p2.window,
            )?;
            let on_screen = d.shots.on_screen(w.id);
            let found = ferb_sessions::window::Found {
                id: w.id,
                title: w.title.clone(),
                on_screen,
            };
            return Ok((w, found));
        }
        let wins = host_windows(pid);
        let facts: Vec<_> = wins.iter().map(|(_, x)| x.clone()).collect();
        let found = session_window(&facts, &p2.proof, &p2.chosen, p2.window)
            .ok_or_else(|| Unseen::NoWindow.words(&p2.host.name, ""))?;
        let w = wins
            .into_iter()
            .find(|(w, _)| w.id == found.id)
            .map(|(w, _)| w)
            .ok_or_else(|| Unseen::NoWindow.words(&p2.host.name, ""))?;
        Ok::<_, String>((w, found))
    })
    .await
    .map_err(|e| e.to_string())??;
    let _ = &app;
    // off this desktop now (he moved away during this Watch): tried, as the window server may still draw it
    let elsewhere = |e: String| {
        if found.on_screen {
            e
        } else {
            Unseen::OtherSpace.words(&app, &found.title)
        }
    };
    let (_target, capture) = driver
        .owner_preview_source(
            &place.host.bundle_id,
            Some(&window.id.to_string()),
            None,
            None,
        )
        .map_err(|e| elsewhere(e.to_string()))?;
    let (frame, _) = capture().await.map_err(|e| elsewhere(e.to_string()))?;
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
        ferb_daemon::sessions::folder_name(&s.cwd)
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
