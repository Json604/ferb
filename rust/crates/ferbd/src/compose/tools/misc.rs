//! The other tools: the registry's first ones (`build()`), recovery, the Mac's, the context readers, time and
//! learning; and the runtime check's two tools.

use std::collections::BTreeSet;
use std::sync::Arc;

use ferb_core::types::{ActionSpec, ToolContext, ToolResult};
use ferb_gateway::tool::{Args, Tool, ToolError, ToolOutput};
use serde_json::{json, Map, Value};

use super::{unavailable, Catalogued, Parts, Run};

fn read_spec() -> Arc<dyn Fn(&Args) -> ActionSpec + Send + Sync> {
    Arc::new(|_a| ActionSpec::new("read"))
}

fn fs_env(p: &Parts) -> ferb_fs::files::FsEnv {
    let native = p.native();
    ferb_fs::files::FsEnv {
        home: p.user_home().to_string_lossy().into_owned(),
        trash: Arc::new(HomeTrash {
            home: p.user_home().clone(),
        }),
        full_disk_access: Arc::new(move || {
            native
                && ferb_mac_hands::native::permissions::full_disk_access()
                    == ferb_mac_hands::permissions::Status::Granted
        }),
    }
}

/// Where the Finder's Trash is for a path (written here: no crate has the Mac's Trash locator yet, STATUS.md): the
/// home volume's `~/.Trash`, another volume's `.Trashes/<uid>`.
pub struct HomeTrash {
    pub home: std::path::PathBuf,
}

impl ferb_fs::files::TrashLocator for HomeTrash {
    fn trash_url(&self, near: &str, create: bool) -> Option<String> {
        use std::os::unix::fs::MetadataExt;
        let dev = |p: &std::path::Path| std::fs::metadata(p).ok().map(|m| m.dev());
        let near_dev = std::path::Path::new(near).ancestors().find_map(dev)?;
        let trash = if Some(near_dev) == dev(&self.home) {
            self.home.join(".Trash")
        } else {
            // the volume's root: the topmost ancestor on the same device
            let mut root = std::path::PathBuf::from(near);
            while let Some(parent) = root.parent() {
                if dev(parent) != Some(near_dev) {
                    break;
                }
                root = parent.to_path_buf();
            }
            // SAFETY: getuid has no preconditions.
            root.join(".Trashes")
                .join(unsafe { libc::getuid() }.to_string())
        };
        if create {
            let _ = std::fs::create_dir_all(&trash);
        }
        trash.is_dir().then(|| trash.to_string_lossy().into_owned())
    }
}

fn raised(e: ferb_fs::files::Raised) -> ToolError {
    match e {
        ferb_fs::files::Raised::Refused(m) => ToolError::refused(m),
        ferb_fs::files::Raised::Os(e) => {
            ToolError::failed(ferb_fs::usage::error_class(&e), e.to_string())
        }
        ferb_fs::files::Raised::Error(m) => ToolError::failed("RuntimeError", m),
    }
}

/// `build()`: actions.search, abilities.list, abilities.read, the file tools, owner.ask.
pub fn first(p: &Parts) -> Vec<Arc<dyn Tool>> {
    let env = fs_env(p);
    let fs = |info: ferb_fs::files::ToolInfo| -> Arc<dyn Tool> {
        Arc::new(ferb_gateway::adapters::fs::FsTool::new(info, env.clone()))
    };
    let parts = p.clone();
    let search: Run = Arc::new(move |a: &Args, _ctx: &ToolContext| {
        let parts = parts.clone();
        Box::pin(async move {
            let caps: Vec<(String, String, Vec<String>, Vec<String>)> = match parts.gateway() {
                Some(gw) => {
                    let live = parts.live();
                    gw.registry()
                        .tools()
                        .iter()
                        .map(|t| {
                            (
                                t.name().to_string(),
                                t.description(&live),
                                t.scopes().into_iter().collect(),
                                t.identities().into_iter().collect(),
                            )
                        })
                        .collect()
                }
                None => Vec::new(),
            };
            let query = a.get("query").and_then(Value::as_str).unwrap_or("");
            let offset = a.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
            let limit = a.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
            let terms =
                |t: &str| -> BTreeSet<String> { ferb_sense::daily::terms(t).into_iter().collect() };
            let matches = ferb_mac_hands::tools::search_actions(&caps, query, &terms, None);
            let end = offset + limit;
            let content = json!({
                "actions": matches.iter().skip(offset).take(limit).cloned().collect::<Vec<_>>(),
                "total": matches.len(),
                "next_offset": if end < matches.len() { json!(end) } else { Value::Null },
                "match_method": "literal token overlap; query empty to list every installed action",
            });
            Ok(ToolOutput::from(ToolResult {
                content: crate::basics::object(content),
                ..ToolResult::default()
            }))
        })
    });
    let env2 = fs_env(p);
    let disk_usage: Run = Arc::new(move |a: &Args, ctx: &ToolContext| {
        let env = env2.clone();
        Box::pin(async move {
            let args: ferb_fs::usage::Args = serde_json::from_value(Value::Object(a.clone()))
                .map_err(|e| ToolError::failed("ValidationError", e.to_string()))?;
            let r = ferb_fs::usage::fs_disk_usage(&args, ctx, &env).map_err(raised)?;
            let mut measured = Vec::new();
            ferb_gateway::adapters::numbers(&Value::Object(r.content.clone()), "", &mut measured);
            Ok(ToolOutput {
                result: r,
                measured,
            })
        })
    });
    vec![
        Arc::new(Catalogued {
            name: "actions.search",
            describe: read_spec(),
            run: search,
        }) as Arc<dyn Tool>,
        unavailable("abilities.list", "FERB's abilities catalog"),
        unavailable("abilities.read", "FERB's abilities catalog"),
        fs(ferb_fs::files::FS_LIST),
        fs(ferb_fs::files::FS_READ),
        unavailable(
            "fs.look",
            "seeing a picture or PDF page on disk (the Mac's renderers)",
        ),
        Arc::new(Catalogued {
            name: "fs.disk_usage",
            describe: Arc::new(|a: &Args| {
                let mut s = ActionSpec::new("read");
                s.target.path = a.get("path").and_then(Value::as_str).map(str::to_string);
                s
            }),
            run: disk_usage,
        }),
        fs(ferb_fs::files::FS_WRITE),
        fs(ferb_fs::files::FS_EDIT),
        fs(ferb_fs::files::FS_TRASH),
        fs(ferb_fs::files::FS_COPY),
        fs(ferb_fs::files::FS_MOVE),
        Arc::new(Catalogued {
            name: "owner.ask",
            describe: read_spec(),
            // ask.py: answered by the gateway, never here
            run: Arc::new(|_a, _c| {
                Box::pin(async { Ok(ferb_mac_hands::tools::owner_ask_run().into()) })
            }),
        }),
    ]
}

/// `add_recovery`: recovery.snapshot (S1's local APFS snapshot) isn't composed: snapshots are off in this build.
pub fn recovery_snapshot(_p: &Parts) -> Arc<dyn Tool> {
    unavailable("recovery.snapshot", "a local snapshot of the Mac's disk")
}

/// `add_mac`: the Mac tools. The Mac driver (Accessibility, input, window capture) isn't composed in this build.
pub fn mac(_p: &Parts) -> Vec<Arc<dyn Tool>> {
    [
        "mac.apps",
        "mac.open",
        "mac.snapshot",
        "mac.menu",
        "mac.act",
        "mac.secret",
        "mac.applescript",
        "mac.screenshot",
        "mac.click",
        "mac.pointer",
        "mac.record",
        "mac.clipboard",
    ]
    .into_iter()
    .map(|n| unavailable(n, "FERB's Mac hands (the Mac driver)"))
    .collect()
}

/// The timer store on the daemon's database (`Timers(db, clock, ids)`), for time.set_timer and time.remind_at.
pub struct DbBook {
    pub db: crate::basics::SharedDb,
    pub clock: Arc<dyn ferb_core::interfaces::Clock>,
    pub ids: Arc<dyn ferb_core::interfaces::IdSource>,
}

impl ferb_clock::tools::TimerBook for DbBook {
    fn add_timer(
        &self,
        kind: &str,
        at: &ferb_clock::py::Time,
        message: &str,
        task_id: &str,
        request_id: &str,
    ) -> Result<(String, ferb_clock::py::Time), String> {
        let db = crate::basics::lock(&self.db);
        let ids = self.ids.clone();
        let timers =
            ferb_clock::timers::Timers::new(&db, self.clock.as_ref(), move || ids.new_id());
        timers.add_timer(kind, at, message, task_id, request_id)
    }
}

/// add_trail, task.evidence, time, wait.for, skills.read, the context readers, secrets.list, about.read,
/// conversation.read, memory.learned.
pub fn rest(p: &Parts) -> Vec<Arc<dyn Tool>> {
    let book: Arc<dyn ferb_clock::tools::TimerBook + Send + Sync> = Arc::new(DbBook {
        db: p.db(),
        clock: p.clock(),
        ids: p.ids(),
    });
    let zone = p.zoned().zone();
    let time = |name: &'static str| -> Arc<dyn Tool> {
        Arc::new(ferb_gateway::adapters::TimeTool {
            name,
            book: Some(book.clone()),
            clock: p.clock(),
            zone,
        })
    };
    vec![
        unavailable("trail.query", "searching the owner's recorded activity"),
        unavailable("trail.days", "searching the owner's recorded activity"),
        unavailable("task.evidence", "reading a task's recorded evidence"),
        time(ferb_clock::tools::REMIND_AT),
        time(ferb_clock::tools::SET_TIMER),
        unavailable(
            "time.sleep_until",
            "a task's durable sleep (it needs the supervisor)",
        ),
        unavailable(
            "wait.for",
            "waiting for a condition (it needs the supervisor)",
        ),
        unavailable("skills.read", "FERB's skills"),
        unavailable(
            "chat.history",
            "the conversation's history (it needs the supervisor)",
        ),
        unavailable("memory.read", "reading FERB's memory"),
        unavailable("secrets.list", "the saved secrets' names"),
        unavailable("about.read", "FERB's self-description"),
        unavailable(
            "conversation.read",
            "delegated conversations (they need the supervisor)",
        ),
        unavailable("memory.learned", "FERB's learning store"),
    ]
}

/// A runtime check's tool (check.py `CheckPing`, `CheckEcho`) in the gateway's registry.
pub struct CheckToolAdapter(pub Arc<dyn ferb_daemon::areas_b::runtimes::check::CheckTool>);

impl Tool for CheckToolAdapter {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn facts(&self) -> ferb_gateway::tool::Facts {
        ferb_gateway::tool::Facts {
            risk: 0,
            reversible: true,
            observation: true,
            ..ferb_gateway::tool::Facts::default()
        }
    }
    fn scopes(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }
    fn identities(&self) -> BTreeSet<String> {
        ["owner", "ferb"].into_iter().map(String::from).collect()
    }
    fn description(&self, _live: &ferb_gateway::catalog::Live) -> String {
        self.0.description().to_string()
    }
    fn input_schema(&self) -> Value {
        self.0.input_schema()
    }
    fn hooks(&self) -> Option<Box<dyn ferb_gateway::pydantic::Hooks + '_>> {
        None
    }
    fn describe(&self, _args: &Args) -> ActionSpec {
        ActionSpec::new("read")
    }
    fn run<'a>(
        &'a self,
        args: &'a Args,
        _ctx: &'a ToolContext,
    ) -> ferb_core::interfaces::BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            Ok(ToolOutput::from(ToolResult {
                content: crate::basics::object(self.0.run(args)),
                ..ToolResult::default()
            }))
        })
    }
}

pub fn _unused(_: Map<String, Value>) {}
