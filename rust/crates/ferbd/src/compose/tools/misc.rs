//! The other tools: the registry's first ones (`build()`), recovery, the Mac's, the context readers, time and
//! learning; and the runtime check's two tools.

use std::collections::BTreeSet;
use std::sync::Arc;

use ferb_core::types::{ActionSpec, ToolContext, ToolResult};
use ferb_gateway::tool::{Args, Tool, ToolError, ToolOutput};
use serde_json::{json, Map, Value};

use super::{Catalogued, Parts, Run};

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
        super::context::tool_named(p, "abilities.list"),
        super::context::tool_named(p, "abilities.read"),
        fs(ferb_fs::files::FS_LIST),
        fs(ferb_fs::files::FS_READ),
        Arc::new(FsLook { env: fs_env(p) }) as Arc<dyn Tool>,
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

/// `add_recovery`: recovery.snapshot over S1's `Snapshots` (`recovery.Snapshots(local_snapshot if take_snapshots
/// else None, ...)`): a local APFS snapshot with `tmutil localsnapshot` when the Mac's parts are on; a daemon started
/// without them (a test) has snapshots off and says so, as Python's `snapshots=False`.
pub fn recovery_snapshot(p: &Parts) -> Arc<dyn Tool> {
    let take: Option<ferb_fs::recovery::Take> =
        p.mac().is_some_and(|m| m.images.native).then(|| {
            Arc::new(|| {
                Ok(ferb_fs::recovery::tmutil::local_snapshot(
                    ferb_fs::recovery::SNAPSHOT_TIMEOUT_S,
                ))
            }) as ferb_fs::recovery::Take
        });
    let conn =
        rusqlite::Connection::open(p.home().join("ferb.db")).expect("the store was opened already");
    let r = p.redactor();
    let snapshots = Arc::new(ferb_fs::recovery::Snapshots::new(
        take,
        Arc::new(std::sync::Mutex::new(conn)),
        p.clock(),
        p.ids(),
        Arc::new(move |v: &Value| r.value(v)),
    ));
    let _ = p.set_snapshots(snapshots.clone());
    Arc::new(Catalogued {
        name: ferb_fs::recovery::SNAPSHOT_TOOL,
        describe: Arc::new(|_a: &Args| ActionSpec::new("read")),
        run: Arc::new(move |a: &Args, ctx: &ToolContext| {
            let snapshots = snapshots.clone();
            let before = a
                .get("before")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let task = ctx.task_id.clone();
            Box::pin(async move {
                let args = ferb_fs::recovery::SnapshotArgs { before };
                Ok(
                    ferb_fs::recovery::recovery_snapshot(&args, &task, &snapshots)
                        .await
                        .into(),
                )
            })
        }),
    })
}

/// fs.look (HAND-517): a picture or a PDF page on disk as an image, decoded by ImageIO / PDFKit (src/native.rs).
pub struct FsLook {
    pub env: ferb_fs::files::FsEnv,
}

impl Tool for FsLook {
    fn name(&self) -> &str {
        ferb_fs::look::NAME
    }
    fn describe(&self, args: &Args) -> ActionSpec {
        let mut s = ActionSpec::new("read");
        let path = args.get("path").and_then(Value::as_str).unwrap_or("");
        s.target.path = ferb_fs::files::describe_path(path, &self.env.home).ok();
        s
    }
    fn locks(&self, args: &Args) -> Vec<String> {
        let path = args.get("path").and_then(Value::as_str).unwrap_or("");
        ferb_fs::files::declared::locks(path, &self.env.home).unwrap_or_default()
    }
    fn file_paths(&self, args: &Args) -> Option<Vec<(String, String)>> {
        let path = args.get("path").and_then(Value::as_str).unwrap_or("");
        Some(vec![(path.to_string(), "read".to_string())])
    }
    fn real_path(&self, args: &Args, workspace: &str) -> Option<ferb_gateway::tool::RealPath> {
        let path = args.get("path").and_then(Value::as_str)?;
        Some(
            ferb_fs::files::declared::real(path, workspace, &self.env.home)
                .map(Into::into)
                .map_err(|_| ()),
        )
    }
    fn run<'a>(
        &'a self,
        args: &'a Args,
        ctx: &'a ToolContext,
    ) -> ferb_core::interfaces::BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let a: ferb_fs::look::Args = serde_json::from_value(Value::Object(args.clone()))
                .map_err(|e| ToolError::failed("ValidationError", e.to_string()))?;
            let (env, ctx) = (self.env.clone(), ctx.clone());
            // decoding runs off the loop (DMN-13)
            let out = tokio::task::spawn_blocking(move || {
                ferb_fs::look::fs_look(&a, &ctx, &env, &crate::native::MacRenderer)
            })
            .await
            .map_err(|e| ToolError::failed("RuntimeError", e.to_string()))?;
            match out {
                Ok(result) => {
                    let mut measured = Vec::new();
                    ferb_gateway::adapters::numbers(
                        &Value::Object(result.content.clone()),
                        "",
                        &mut measured,
                    );
                    Ok(ToolOutput { result, measured })
                }
                Err(e) => Err(raised(e)),
            }
        })
    }
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
    let ctx = |name: &'static str| super::context::tool_named(p, name);
    vec![
        ctx("trail.query"),
        ctx("trail.days"),
        ctx("task.evidence"),
        time(ferb_clock::tools::REMIND_AT),
        time(ferb_clock::tools::SET_TIMER),
        super::waits::sleep_until(p),
        super::waits::wait_for(p),
        ctx("skills.read"),
        ctx("chat.history"),
        ctx("memory.read"),
        ctx("secrets.list"),
        ctx("about.read"),
        ctx("conversation.read"),
        ctx("memory.learned"),
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
