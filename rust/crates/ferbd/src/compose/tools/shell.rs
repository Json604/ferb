//! FERB's shell (`_build_mac_and_shell`'s shell half): the sandbox over the protection lists, the process manager,
//! and the eight `shell.*` tools. The egress proxy (compose/egress.rs) is the only way off the Mac a command has: its
//! port is in the profile, and every destination is the gateway's to allow.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ferb_core::config::Config;
use ferb_core::types::{ActionSpec, EscapeGrant, ToolContext, ToolResult};
use ferb_gateway::tool::{Args, Tool, ToolError, ToolOutput};
use ferb_sanitize::redact::Redactor;
use ferb_shell::manager::{LocalRefusedFn, ManagerConfig, OnEscape, ShellManager};
use ferb_shell::protection::{Entry, LocalPort, Protection};
use ferb_shell::tools as st;
use serde_json::{json, Value};

use super::{Catalogued, Parts};
use crate::compose::egress::Egress;
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
    // the egress proxy is the shell's only way off the Mac (compose/egress.rs): its port goes in every profile
    let egress = opts.egress.clone();
    let proxy = match egress.build(redactor, protection.clone()) {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("FERB: the shell's network proxy didn't start: {e}");
            None
        }
    };
    let port = proxy.as_ref().map(|p| p.port());
    let sandbox = Arc::new(ferb_shell::sandbox::Sandbox::new(
        protection.clone(),
        opts.home.join("run").join("sandbox"),
        Arc::new(move || port),
        Some(ferb_core::paths::socket_path(&opts.home)),
    ));
    sandbox.sweep();
    let contained = sandbox.available();
    if !contained.0 {
        // no shell, so no proxy for it (`_build_mac_and_shell`)
        egress.close();
    }
    let _ = egress.containment.set(contained.clone());
    let r = redactor.clone();
    let r2 = redactor.clone();
    let mut cfg = ManagerConfig::new(
        opts.home.join("logs").join("shell"),
        Arc::new(move |t: &str| r.text(t)),
        Arc::new(move || ferb_sanitize::stream::RedactionStream::new(r2.clone())),
        sandbox,
    );
    if contained.0 {
        cfg.proxy = proxy;
    }
    cfg.on_network = Some(egress.on_network());
    // manager.py:140 needs `redactor` present for an approved admin command's hidden password; the daemon always
    // hands its Redactor (desktop.py:333), as this program always has one
    cfg.admin_ready = true;
    cfg.on_escape = Some(on_escape(&egress));
    cfg.local_refused = Some(local_refused(&egress));
    // desktop.py:327: with pinned local ports, the kernel's reports of direct connections are read while FERB runs
    if contained.0 && !protection.local_ports().is_empty() {
        let e = egress.clone();
        let reports = Arc::new(ferb_shell::local::DirectReports::new(
            None,
            Some(Arc::new(move |why: &str| {
                // `_local_reports_stopped`: told once, from the stream's thread
                note(&e, "shell.local_reports_stopped", json!({"why": why}))
            })),
        ));
        reports.start();
        cfg.reports = Some(reports);
    }
    let shells = ShellManager::new(cfg);
    let _ = egress.shells.set(Arc::downgrade(&shells));
    Some((shells, protection))
}

/// A journal row through the start's journal (filled once the store is open; nothing is written before).
fn note(egress: &Egress, kind: &str, data: Value) {
    if let Some(j) = egress.journal.get() {
        j.note(kind, data);
    }
}

/// desktop.py:334: an approved escape's run is journaled (`escape.used`) with its rule, command, folder and question.
pub fn on_escape(egress: &Arc<Egress>) -> OnEscape {
    let e = egress.clone();
    Arc::new(move |task: &str, grant: &EscapeGrant| {
        note(
            &e,
            "escape.used",
            json!({
                "task_id": task, "rule": grant.rule, "command": grant.command,
                "cwd": grant.cwd, "confirmation": grant.confirmation_id,
            }),
        )
    })
}

/// egress.py:39 `_local_refused` over the store: the task's tree (`tasks.tree_of`'s stored row; tree.py:84) and the
/// keys the owner said No to in it. A task FERB can't place, or a store it can't read, counts as refused: its
/// commands then reach pinned local ports only through the proxy.
pub fn local_refused(egress: &Arc<Egress>) -> LocalRefusedFn {
    let e = egress.clone();
    Arc::new(move |task: &str| {
        let Some(j) = e.journal.get() else {
            return true;
        };
        let db = crate::basics::lock(&j.db);
        let root: Option<String> = db
            .conn()
            .query_row("SELECT tree_id FROM tasks WHERE id = ?", [task], |r| {
                r.get::<_, Option<String>>(0)
            })
            .ok()
            .flatten();
        let Some(root) = root else {
            return true;
        };
        let refused = ferb_policy::stores::Confirmations {
            db: &db,
            clock: j.clock.as_ref(),
            ids: j.ids.as_ref(),
            redact: j.redactor.as_ref(),
        }
        .refused(&root);
        match refused {
            Ok(keys) => ferb_daemon::areas_b::egress::local_refused(Some(&root), &keys),
            Err(_) => true,
        }
    })
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
pub struct Resolver {
    pub home: String,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn journal(dir: &Path) -> Arc<crate::basics::Journal> {
        let db = crate::basics::open_store(&dir.join("ferb.db")).unwrap();
        Arc::new(crate::basics::Journal {
            db: Arc::new(Mutex::new(db)),
            clock: Arc::new(ferb_clock::clock::SystemClock::new(
                ferb_clock::zone::Zone::utc(),
            )),
            ids: Arc::new(ferb_core::ids::SequentialIds::new("j")),
            redactor: Arc::new(Redactor::new(&[], &[])),
        })
    }

    fn task(j: &crate::basics::Journal, id: &str, tree: &str) {
        crate::basics::lock(&j.db)
            .conn()
            .execute(
                "INSERT INTO tasks (id, tree_id, request_id, goal, identity, role, scopes_json, status, created_at, \
                 updated_at) VALUES (?1, ?2, 'r', 'g', 'owner', 'mixed', '[]', 'running', 'x', 'x')",
                [id, tree],
            )
            .unwrap();
    }

    fn no(j: &crate::basics::Journal, tree: &str, host: &str) {
        let db = crate::basics::lock(&j.db);
        ferb_policy::stores::Confirmations {
            db: &db,
            clock: j.clock.as_ref(),
            ids: j.ids.as_ref(),
            redact: j.redactor.as_ref(),
        }
        .record(
            tree,
            ferb_policy::confirm::SINK_DESTINATION,
            &ferb_policy::confirm::destination_key(host),
            false,
            None,
        )
        .unwrap();
    }

    /// egress.py:39 `_local_refused` as the shell manager asks it (desktop.py:337): a task FERB can't place counts as
    /// refused; the owner's No to one of this Mac's addresses anywhere in the tree is final for every task in it; a
    /// No to a remote host isn't one.
    // decision: HAND-214
    #[test]
    fn a_no_to_this_mac_in_the_tree_keeps_pinned_ports_behind_the_proxy() {
        let tmp = tempfile::tempdir().unwrap();
        let egress = Arc::new(Egress::default());
        let refused = local_refused(&egress);
        assert!(refused("t1"), "no store yet: refused");
        let j = journal(tmp.path());
        let _ = egress.journal.set(j.clone());
        assert!(refused("t1"), "a task FERB can't place");
        task(&j, "t1", "t1");
        task(&j, "t1.1", "t1");
        assert!(!refused("t1"));
        no(&j, "t1", "example.com");
        assert!(!refused("t1.1"), "a No to a remote host");
        no(&j, "t1", "localhost");
        assert!(refused("t1.1"));
        assert!(refused("t1"));
    }

    /// desktop.py:334: an approved escape's run is journaled with its rule, command, folder and question.
    // decision: STORE-418
    #[test]
    fn an_escape_used_is_journaled() {
        let tmp = tempfile::tempdir().unwrap();
        let egress = Arc::new(Egress::default());
        let j = journal(tmp.path());
        let _ = egress.journal.set(j.clone());
        let grant = EscapeGrant {
            rule: "write:~/Library/LaunchAgents".into(),
            confirmation_id: "c1".into(),
            command: "touch x".into(),
            cwd: "/tmp".into(),
            secret: None,
        };
        on_escape(&egress)("t1", &grant);
        let rows = j.of_kind("escape.used", "", "");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].1,
            json!({"task_id": "t1", "rule": "write:~/Library/LaunchAgents", "command": "touch x", "cwd": "/tmp",
                   "confirmation": "c1"})
        );
    }
}
