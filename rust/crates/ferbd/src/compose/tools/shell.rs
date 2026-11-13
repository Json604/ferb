//! FERB's shell (`_build_mac_and_shell`'s shell half): the sandbox over the protection lists, the process manager,
//! and the eight `shell.*` tools. The egress proxy is not composed in this build: without it the sandbox gives a
//! command no way off the Mac (its profile names no proxy port), which is the safe side of rule 11.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ferb_core::config::Config;
use ferb_core::types::{ActionSpec, ToolContext, ToolResult};
use ferb_gateway::tool::{Args, Tool, ToolError, ToolOutput};
use ferb_sanitize::redact::Redactor;
use ferb_shell::manager::{ManagerConfig, ShellManager};
use ferb_shell::protection::{Entry, LocalPort, Protection};
use ferb_shell::tools as st;
use serde_json::Value;

use super::{Catalogued, Parts};
use crate::compose::Options;

/// ferb-policy's protection lists behind ferb-shell's `Protection` seam (no crate implements it: listed).
pub struct ShellProtection {
    p: ferb_policy::protection::Protection<'static>,
    home: PathBuf,
    ferb_home: PathBuf,
    workspace: PathBuf,
    uploads: PathBuf,
    reads: Vec<Entry>,
    writes: Vec<Entry>,
    ports: Vec<LocalPort>,
}

// SAFETY: `Protection<'static>` holds owned data and the stateless `&'static PosixFiles` (each call goes straight to
// libc), as ferb-gateway's `SyncProtection` says; nothing in it has interior mutability.
unsafe impl Send for ShellProtection {}
// SAFETY: as above: read-only data and a stateless file-call table.
unsafe impl Sync for ShellProtection {}

fn entries(list: &[ferb_policy::protection::Entry]) -> Vec<Entry> {
    list.iter()
        .map(|e| Entry {
            id: e.id.clone(),
            path: PathBuf::from(&e.path),
            typed: PathBuf::from(&e.typed),
        })
        .collect()
}

impl ShellProtection {
    pub fn new(p: ferb_policy::protection::Protection<'static>) -> ShellProtection {
        ShellProtection {
            home: PathBuf::from(&p.home),
            ferb_home: PathBuf::from(&p.ferb_home),
            workspace: PathBuf::from(&p.workspace),
            uploads: PathBuf::from(&p.uploads),
            reads: entries(&p.reads),
            writes: entries(&p.writes),
            ports: p
                .local_ports
                .iter()
                .map(|l| LocalPort {
                    port: l.port,
                    executable: PathBuf::from(&l.executable),
                })
                .collect(),
            p,
        }
    }
}

impl Protection for ShellProtection {
    fn home(&self) -> &Path {
        &self.home
    }
    fn ferb_home(&self) -> &Path {
        &self.ferb_home
    }
    fn workspace(&self) -> &Path {
        &self.workspace
    }
    fn uploads(&self) -> &Path {
        &self.uploads
    }
    fn reads(&self) -> &[Entry] {
        &self.reads
    }
    fn writes(&self) -> &[Entry] {
        &self.writes
    }
    fn app_entries(&self) -> Vec<PathBuf> {
        self.p
            .app_entries()
            .into_iter()
            .map(PathBuf::from)
            .collect()
    }
    fn service_files(&self) -> Vec<PathBuf> {
        self.p.service_files.iter().map(PathBuf::from).collect()
    }
    fn guarded_parents(&self) -> Vec<PathBuf> {
        self.p
            .guarded_parents()
            .into_iter()
            .map(PathBuf::from)
            .collect()
    }
    fn forms_now(&self, path: &Path) -> Vec<PathBuf> {
        self.p
            .forms_now(&path.to_string_lossy())
            .into_iter()
            .map(PathBuf::from)
            .collect()
    }
    fn app_data(&self, path: &Path) -> bool {
        self.p.app_data(&path.to_string_lossy())
    }
    fn local_ports(&self) -> &[LocalPort] {
        &self.ports
    }
    fn rules(&self) -> Vec<Value> {
        self.p.rules()
    }
}

/// The shell's process manager over its sandbox; None when the protection lists can't be read (the shell tools
/// then refuse, as Python's daemon would not start).
pub fn manager(
    opts: &Options,
    config: &Arc<Config>,
    redactor: &Arc<Redactor>,
) -> Option<(Arc<ShellManager>, Arc<dyn Protection>)> {
    let p = crate::compose::gateway::protection(
        config,
        &opts.user_home,
        &opts.home,
        opts.app_bundle.as_deref(),
    )
    .ok()?;
    let protection: Arc<dyn Protection> = Arc::new(ShellProtection::new(p));
    let sandbox = Arc::new(ferb_shell::sandbox::Sandbox::new(
        protection.clone(),
        opts.home.join("profiles"),
        Arc::new(|| None),
        None,
    ));
    let r = redactor.clone();
    let r2 = redactor.clone();
    let cfg = ManagerConfig::new(
        opts.home.join("logs"),
        Arc::new(move |t: &str| r.text(t)),
        Arc::new(move || ferb_sanitize::stream::RedactionStream::new(r2.clone())),
        sandbox,
    );
    Some((ShellManager::new(cfg), protection))
}

fn text(a: &Args, k: &str) -> String {
    match a.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(v) => ferb_py::value::py_str(v),
    }
}

fn opt(a: &Args, k: &str) -> Option<String> {
    match a.get(k) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn num(a: &Args, k: &str, d: f64) -> f64 {
    a.get(k).and_then(Value::as_f64).unwrap_or(d)
}

fn int(a: &Args, k: &str) -> i64 {
    a.get(k).and_then(Value::as_i64).unwrap_or(0)
}

fn start_args(a: &Args) -> st::StartArgs {
    st::StartArgs {
        command: text(a, "command"),
        cwd: opt(a, "cwd"),
        background: a
            .get("background")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        external_source: opt(a, "external_source"),
        escape: opt(a, "escape"),
    }
}

/// The fs tools' path rules for a command's working folder.
struct Resolver {
    home: String,
}

impl st::ResolvePath for Resolver {
    fn resolve(&self, path: &str, ctx: &ToolContext, must_exist: bool) -> Result<PathBuf, String> {
        ferb_fs::files::resolve(path, &ctx.workspace, &self.home, must_exist)
            .map(PathBuf::from)
            .map_err(|e| match e {
                ferb_fs::files::Fail::Refused(m) => m,
                other => format!("{other:?}"),
            })
    }
}

fn out(r: Result<ToolResult, ferb_shell::manager::ShellError>) -> Result<ToolOutput, ToolError> {
    match r {
        Ok(r) => Ok(r.into()),
        Err(ferb_shell::manager::ShellError::Refused(m)) => Err(ToolError::refused(m)),
        Err(e) => Err(ToolError::failed("RuntimeError", e.to_string())),
    }
}

/// shell.run, shell.start, shell.wait, shell.read, shell.send, shell.kill, shell.list, shell.rules (registry order).
pub fn tools(p: &Parts) -> Vec<Arc<dyn Tool>> {
    let names: [&'static str; 8] = [
        "shell.run",
        "shell.start",
        "shell.wait",
        "shell.read",
        "shell.send",
        "shell.kill",
        "shell.list",
        "shell.rules",
    ];
    let protection = p.shell_protection();
    let Some(shells) = p.shells() else {
        return names
            .iter()
            .map(|n| super::unavailable(n, "FERB's shell (its protection lists couldn't be read)"))
            .collect();
    };
    let home = p.user_home().to_string_lossy().into_owned();
    names
        .iter()
        .map(|&name| {
            let shells = shells.clone();
            let home = home.clone();
            let protection = protection.clone();
            let describe = Arc::new(move |a: &Args| -> ActionSpec {
                match name {
                    "shell.run" | "shell.start" => {
                        let mut s = ActionSpec::new(ferb_policy::rules::Kind::RUN_SHELL);
                        s.target.path = Some(
                            opt(a, "cwd")
                                .filter(|c| !c.is_empty())
                                .unwrap_or_else(|| ".".into()),
                        );
                        s.content = Some(text(a, "command"));
                        s
                    }
                    "shell.send" => {
                        let mut s = ActionSpec::new(ferb_policy::rules::Kind::RUN_SHELL);
                        s.content = Some(text(a, "text"));
                        s
                    }
                    "shell.kill" => ActionSpec::new(ferb_policy::rules::Kind::STOP_PROCESS),
                    _ => ActionSpec::new("read"),
                }
            });
            let run: super::Run = Arc::new(move |a: &Args, ctx: &ToolContext| {
                let shells = shells.clone();
                let protection = protection.clone();
                let resolver = Resolver { home: home.clone() };
                Box::pin(async move {
                    let s = shells.as_ref();
                    match name {
                        "shell.run" => out(st::shell_run(
                            s,
                            &st::RunArgs {
                                start: start_args(a),
                                timeout_s: num(a, "timeout_s", 30.0),
                            },
                            ctx,
                            &resolver,
                        )
                        .await),
                        "shell.start" => {
                            out(st::shell_start(s, &start_args(a), ctx, &resolver).await)
                        }
                        "shell.wait" => out(st::shell_wait(
                            s,
                            &st::WaitArgs {
                                id: text(a, "id"),
                                timeout_s: num(a, "timeout_s", 30.0),
                                since: int(a, "since"),
                            },
                            ctx,
                        )
                        .await),
                        "shell.read" => out(st::shell_read(
                            s,
                            &st::ReadArgs {
                                id: text(a, "id"),
                                since: int(a, "since"),
                            },
                            ctx,
                        )
                        .await),
                        "shell.send" => out(st::shell_send(
                            s,
                            &st::SendArgs {
                                id: text(a, "id"),
                                text: text(a, "text"),
                            },
                            ctx,
                        )
                        .await),
                        "shell.kill" => {
                            out(st::shell_kill(s, &st::KillArgs { id: text(a, "id") }, ctx).await)
                        }
                        "shell.list" => Ok(st::shell_list(s, ctx).into()),
                        _ => match &protection {
                            Some(p) => Ok(st::shell_rules(p.as_ref(), None).into()),
                            None => Err(super::not_in_this_build("FERB's shell")),
                        },
                    }
                })
            });
            Arc::new(Catalogued {
                name,
                describe,
                run,
            }) as Arc<dyn Tool>
        })
        .collect()
}
