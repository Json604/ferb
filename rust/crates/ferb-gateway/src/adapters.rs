//! Real tools through the one `Tool` trait, as worked examples that it fits: FERB's file tools (ferb-fs) and the
//! timer tools (ferb-clock). Neither crate is changed: each adapter reads its arguments into the crate's own types and
//! calls its functions; what the gateway reads off a capability (`file_paths`, `locks`, `real_path`,
//! `recovery_reason`) comes from the same crate's `declared` functions.

use std::sync::Arc;

use ferb_core::interfaces::{BoxFuture, Clock};
use ferb_core::types::{ActionSpec, ToolContext, ToolResult};
use serde_json::Value;

use crate::tool::{Args, Tool, ToolError, ToolOutput};

/// Every JSON number in `v`, as pointers: a value a tool's own code put there.
pub fn numbers(v: &Value, at: &str, out: &mut Vec<String>) {
    match v {
        Value::Number(_) => out.push(at.to_string()),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .for_each(|(i, x)| numbers(x, &format!("{at}/{i}"), out)),
        Value::Object(m) => m.iter().for_each(|(k, x)| {
            let k = k.replace('~', "~0").replace('/', "~1");
            numbers(x, &format!("{at}/{k}"), out)
        }),
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------------------------------
// ferb-clock: time.set_timer and time.remind_at
// ---------------------------------------------------------------------------------------------------------------

/// The timer store, shared (`ferb_clock::tools::TimerBook`).
pub type Book = Arc<dyn ferb_clock::tools::TimerBook + Send + Sync>;

/// `time.set_timer` or `time.remind_at`.
pub struct TimeTool {
    pub name: &'static str,
    pub book: Option<Book>,
    pub clock: Arc<dyn Clock>,
    pub zone: ferb_clock::zone::Zone,
}

impl Tool for TimeTool {
    fn name(&self) -> &str {
        self.name
    }

    fn describe(&self, args: &Args) -> ActionSpec {
        ferb_clock::tools::describe(self.name, &Value::Object(args.clone()))
    }

    fn run<'a>(
        &'a self,
        args: &'a Args,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let now = self.clock.now();
            let book = self
                .book
                .as_ref()
                .map(|b| b.as_ref() as &dyn ferb_clock::tools::TimerBook);
            let a = Value::Object(args.clone());
            let r: ToolResult = match self.name {
                ferb_clock::tools::REMIND_AT => {
                    ferb_clock::tools::remind_at(book, &a, &now, &self.zone, ctx)
                }
                _ => ferb_clock::tools::set_timer(book, &a, &now, &self.zone, ctx),
            };
            Ok(r.into())
        })
    }
}

// ---------------------------------------------------------------------------------------------------------------
// ferb-fs: the file tools, and S1's recovery copies on the store (Mac-side)
// ---------------------------------------------------------------------------------------------------------------

#[cfg(all(feature = "store", target_os = "macos"))]
pub mod fs {
    use std::sync::{Arc, Mutex};

    use ferb_core::interfaces::{BoxFuture, Clock, IdSource};
    use ferb_core::types::{ActionSpec, ToolContext};
    use ferb_fs::files::{self, declared, FsEnv, Raised};
    use ferb_store::Db;
    use serde::de::DeserializeOwned;
    use serde_json::Value;

    use super::numbers;
    use crate::seams::Recovery;
    use crate::tool::{Args, RealPath, Tool, ToolError, ToolOutput};

    /// One of `fs.list`, `fs.read`, `fs.write`, `fs.edit`, `fs.copy`, `fs.move`, `fs.trash`.
    pub struct FsTool {
        pub info: files::ToolInfo,
        pub env: FsEnv,
    }

    fn typed<T: DeserializeOwned>(args: &Args) -> Option<T> {
        serde_json::from_value(Value::Object(args.clone())).ok()
    }

    fn path(args: &Args) -> String {
        args.get("path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }

    impl FsTool {
        pub fn new(info: files::ToolInfo, env: FsEnv) -> FsTool {
            FsTool { info, env }
        }

        fn spec(&self, args: &Args) -> Result<ActionSpec, files::Fail> {
            let home = &self.env.home;
            match self.info.name {
                "fs.list" => declared::list_describe(&typed(args).ok_or_else(bad)?, home),
                "fs.read" => declared::read_describe(&typed(args).ok_or_else(bad)?, home),
                "fs.write" => declared::write_describe(&typed(args).ok_or_else(bad)?, home),
                "fs.edit" => declared::edit_describe(&typed(args).ok_or_else(bad)?, home),
                "fs.copy" => declared::copy_describe(&typed(args).ok_or_else(bad)?, home),
                "fs.move" => declared::move_describe(&typed(args).ok_or_else(bad)?, home),
                _ => declared::trash_describe(&typed(args).ok_or_else(bad)?, home),
            }
        }
    }

    fn bad() -> files::Fail {
        files::Fail::Raised("arguments the gateway didn't validate".into())
    }

    impl Tool for FsTool {
        fn name(&self) -> &str {
            self.info.name
        }

        /// A path Python can't expand (`~nobody`) raises out of `describe`; here the call is described with the path
        /// as given and the tool itself then refuses it.
        fn describe(&self, args: &Args) -> ActionSpec {
            self.spec(args).unwrap_or_else(|_| {
                let mut s = ActionSpec::new(if self.info.reversible {
                    "read"
                } else {
                    "write_file"
                });
                s.target.path = Some(path(args));
                s
            })
        }

        fn file_paths(&self, args: &Args) -> Option<Vec<(String, String)>> {
            let own = |v: Vec<(String, &'static str)>| {
                v.into_iter().map(|(p, a)| (p, a.to_string())).collect()
            };
            Some(match self.info.name {
                "fs.copy" => own(declared::copy_file_paths(&typed(args)?)),
                "fs.move" => own(declared::move_file_paths(&typed(args)?)),
                _ => own(declared::file_paths(&path(args), &self.info)),
            })
        }

        fn locks(&self, args: &Args) -> Vec<String> {
            let home = &self.env.home;
            let dest = args.get("destination").and_then(Value::as_str);
            match (self.info.name, dest) {
                ("fs.copy" | "fs.move", Some(d)) => {
                    declared::two_locks(&path(args), d, home).unwrap_or_default()
                }
                _ => declared::locks(&path(args), home).unwrap_or_default(),
            }
        }

        fn real_path(&self, args: &Args, workspace: &str) -> Option<RealPath> {
            let home = &self.env.home;
            let r = match self.info.name {
                "fs.move" => declared::move_real(&typed(args)?, workspace, home),
                "fs.trash" => declared::trash_real(&typed(args)?, workspace, home),
                "fs.copy" => declared::real(args.get("destination")?.as_str()?, workspace, home),
                _ => declared::real(&path(args), workspace, home),
            };
            Some(r.map(Into::into).map_err(|_| ()))
        }

        fn recovery_reason(&self, args: &Args) -> Option<Option<String>> {
            let r = match self.info.name {
                "fs.write" => declared::write_reason(&typed(args)?),
                "fs.edit" => declared::edit_reason(&typed(args)?),
                "fs.copy" => declared::copy_reason(&typed(args)?),
                "fs.move" => declared::move_reason(&typed(args)?),
                _ => return None,
            };
            Some(r.map(str::to_string))
        }

        fn run<'a>(
            &'a self,
            args: &'a Args,
            ctx: &'a ToolContext,
        ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
            Box::pin(async move {
                let env = &self.env;
                let out = match self.info.name {
                    "fs.list" => files::fs_list(&typed(args).ok_or_else(raised)?, ctx, env),
                    "fs.read" => files::fs_read(&typed(args).ok_or_else(raised)?, ctx, env),
                    "fs.write" => files::fs_write(&typed(args).ok_or_else(raised)?, ctx, env),
                    "fs.edit" => files::fs_edit(&typed(args).ok_or_else(raised)?, ctx, env),
                    "fs.copy" => files::fs_copy(&typed(args).ok_or_else(raised)?, ctx, env),
                    "fs.move" => files::fs_move(&typed(args).ok_or_else(raised)?, ctx, env),
                    _ => files::fs_trash(&typed(args).ok_or_else(raised)?, ctx, env),
                };
                match out {
                    Ok(result) => {
                        // A file tool's numbers (sizes, offsets, its stat facts: mtime_ns, inode) are its own
                        // measurements; a file's contents reach a result only as text
                        let mut measured = Vec::new();
                        numbers(&Value::Object(result.content.clone()), "", &mut measured);
                        Ok(ToolOutput { result, measured })
                    }
                    Err(Raised::Refused(m)) => Err(ToolError::refused(m)),
                    Err(Raised::Os(e)) => Err(ToolError::failed(
                        ferb_fs::usage::error_class(&e),
                        e.to_string(),
                    )),
                    Err(Raised::Error(m)) => Err(ToolError::failed("RuntimeError", m)),
                }
            })
        }
    }

    fn raised() -> ToolError {
        ToolError::failed("ValidationError", "arguments the gateway didn't validate")
    }

    /// S1 on the store (`ferb/act/fs/recovery.py`): copies before a change, the Trash record.
    pub struct FsRecovery {
        pub db: Arc<Mutex<Db>>,
        pub clock: Arc<dyn Clock>,
        pub ids: Arc<dyn IdSource>,
        /// `~/.ferb/recovery`.
        pub root: String,
    }

    /// `error_name(e)` (call_hooks.py): the exception's type, with its errno for an OSError; never its text (it can
    /// hold an absolute path).
    fn why(f: files::Fail) -> String {
        match f {
            files::Fail::Refused(_) => "PathRefused".into(),
            files::Fail::Raised(_) => "RuntimeError".into(),
            files::Fail::Os(e) if e.errno >= 0 => {
                format!("{} (errno {})", ferb_fs::usage::error_class(&e), e.errno)
            }
            files::Fail::Os(e) => ferb_fs::usage::error_class(&e).to_string(),
            files::Fail::Decode(_) => "UnicodeDecodeError".into(),
        }
    }

    impl Recovery for FsRecovery {
        fn preserve<'a>(
            &'a self,
            path: &'a str,
            reason: &'a str,
            task_id: &'a str,
        ) -> BoxFuture<'a, Result<Option<String>, String>> {
            Box::pin(async move {
                let db = self
                    .db
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                ferb_fs::recovery::preserve(
                    path,
                    reason,
                    Some(task_id),
                    db.conn(),
                    self.clock.as_ref(),
                    self.ids.as_ref(),
                    &self.root,
                )
                .map_err(why)
            })
        }

        fn discard_if_unchanged<'a>(
            &'a self,
            record: &'a str,
            path: &'a str,
        ) -> BoxFuture<'a, Result<bool, String>> {
            Box::pin(async move {
                let db = self
                    .db
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                ferb_fs::recovery::discard_if_unchanged(record, path, db.conn(), &self.root)
                    .map_err(why)
            })
        }

        fn mark_written<'a>(
            &'a self,
            record: &'a str,
            path: &'a str,
        ) -> BoxFuture<'a, Result<(), String>> {
            Box::pin(async move {
                let db = self
                    .db
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                ferb_fs::recovery::mark_written(record, path, db.conn()).map_err(why)
            })
        }

        fn record_trash(
            &self,
            original: &str,
            trashed_to: Option<&str>,
            device: Option<i64>,
            inode: Option<i64>,
            reason: &str,
            task_id: &str,
        ) -> Result<String, String> {
            let db = self
                .db
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ferb_fs::recovery::record_trash(
                original,
                trashed_to,
                device,
                inode.map(|i| i as u64),
                reason,
                Some(task_id),
                db.conn(),
                self.clock.as_ref(),
                self.ids.as_ref(),
            )
            .map_err(|e| e.to_string())
        }
    }
}
