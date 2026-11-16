//! A tool around another: everything the gateway asks is the inner tool's, except how its run is driven.
//!
//! [`Threaded`]: the file tools' work in a worker thread (ferb/act/fs/files.py:423 `asyncio.to_thread(self._guarded,
//! ...)`). Python's loop runs whatever is ready meanwhile, so a chat stream takes the call's live line before its
//! `action.observed`; ferb-gateway's `FsTool` runs its file work inline (seam mismatch, reported), so the daemon puts
//! it on the blocking pool here.

use std::collections::BTreeSet;
use std::sync::Arc;

use ferb_core::interfaces::BoxFuture;
use ferb_core::types::{ActionSpec, TerminalInput, ToolContext};
use ferb_gateway::tool::{Args, Point, PreviewTarget, RealPath, Shot, Tool, ToolError, ToolOutput};
use serde_json::{Map, Value};

/// What a wrapper does with the inner tool's run.
pub trait Around: Send + Sync {
    fn run<'a>(
        &'a self,
        inner: &'a Arc<dyn Tool>,
        args: &'a Args,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>>;
}

/// `inner` with its run driven by `around`.
pub struct Wrapped<A: Around> {
    pub inner: Arc<dyn Tool>,
    pub around: A,
}

/// The run on the blocking pool (Python's worker thread).
pub struct InThread;

impl Around for InThread {
    fn run<'a>(
        &'a self,
        inner: &'a Arc<dyn Tool>,
        args: &'a Args,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        let (inner, args, ctx) = (inner.clone(), args.clone(), ctx.clone());
        Box::pin(async move {
            let handle = tokio::runtime::Handle::current();
            match tokio::task::spawn_blocking(move || handle.block_on(inner.run(&args, &ctx))).await
            {
                Ok(out) => out,
                Err(e) => Err(ToolError::refused(format!("the file work stopped: {e}"))),
            }
        })
    }
}

/// The file tools whose work Python runs in a thread (files.py `_Base`).
pub const THREADED: [&str; 7] = [
    "fs.list", "fs.read", "fs.write", "fs.edit", "fs.copy", "fs.move", "fs.trash",
];

pub fn threaded(inner: Arc<dyn Tool>) -> Arc<dyn Tool> {
    Arc::new(Wrapped {
        inner,
        around: InThread,
    })
}

impl<A: Around> Tool for Wrapped<A> {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn facts(&self) -> ferb_gateway::tool::Facts {
        self.inner.facts()
    }
    fn scopes(&self) -> BTreeSet<String> {
        self.inner.scopes()
    }
    fn identities(&self) -> BTreeSet<String> {
        self.inner.identities()
    }
    fn description(&self, live: &ferb_gateway::catalog::Live) -> String {
        self.inner.description(live)
    }
    fn input_schema(&self) -> Value {
        self.inner.input_schema()
    }
    fn hooks(&self) -> Option<Box<dyn ferb_gateway::pydantic::Hooks + '_>> {
        self.inner.hooks()
    }
    fn describe(&self, args: &Args) -> ActionSpec {
        self.inner.describe(args)
    }
    fn try_describe(&self, args: &Args) -> Result<ActionSpec, ToolError> {
        self.inner.try_describe(args)
    }
    fn precheck(&self, args: &Args) -> Option<String> {
        self.inner.precheck(args)
    }
    fn unavailable<'a>(&'a self, args: &'a Args) -> BoxFuture<'a, Option<Map<String, Value>>> {
        self.inner.unavailable(args)
    }
    fn locks(&self, args: &Args) -> Vec<String> {
        self.inner.locks(args)
    }
    fn file_paths(&self, args: &Args) -> Option<Vec<(String, String)>> {
        self.inner.file_paths(args)
    }
    fn real_path(&self, args: &Args, workspace: &str) -> Option<RealPath> {
        self.inner.real_path(args, workspace)
    }
    fn recovery_reason(&self, args: &Args) -> Option<Option<String>> {
        self.inner.recovery_reason(args)
    }
    fn ledger_args(&self, args: &Args, hashed: &Args) -> Option<Args> {
        self.inner.ledger_args(args, hashed)
    }
    fn terminal_input<'a>(
        &'a self,
        args: &'a Args,
        task_id: &'a str,
    ) -> BoxFuture<'a, Option<TerminalInput>> {
        self.inner.terminal_input(args, task_id)
    }
    fn browser_page<'a>(&'a self, args: &'a Args) -> BoxFuture<'a, Option<String>> {
        self.inner.browser_page(args)
    }
    fn window_reason<'a>(&'a self, args: &'a Args) -> BoxFuture<'a, Option<String>> {
        self.inner.window_reason(args)
    }
    fn pay_picture<'a>(&'a self, args: &'a Args, owner: &'a str) -> BoxFuture<'a, Option<Vec<u8>>> {
        self.inner.pay_picture(args, owner)
    }
    fn pay_press<'a>(
        &'a self,
        args: &'a Args,
        owner: &'a str,
        declared: Option<&'a str>,
    ) -> BoxFuture<'a, Option<ferb_policy::payments::Pressed>> {
        self.inner.pay_press(args, owner, declared)
    }
    fn preview_target(
        &self,
        args: &Args,
        owner: &str,
        observed: Option<&Map<String, Value>>,
    ) -> Result<Option<PreviewTarget>, String> {
        self.inner.preview_target(args, owner, observed)
    }
    fn point<'a>(
        &'a self,
        args: &'a Args,
        grant: &'a ferb_rpc::auth::RunGrant,
    ) -> BoxFuture<'a, Result<Option<Point>, String>> {
        self.inner.point(args, grant)
    }
    fn tab_picture<'a>(
        &'a self,
        args: &'a Args,
        task: &'a str,
    ) -> BoxFuture<'a, Result<(Vec<u8>, String), Shot>> {
        self.inner.tab_picture(args, task)
    }
    // the two the wrapper once left at the trait's defaults (audit of 2026-11-16): no wrapped tool answers them today
    fn background_pid(&self, args: &Args) -> Option<i64> {
        self.inner.background_pid(args)
    }
    fn put_back<'a>(
        &'a self,
        front: &'a Value,
        previous: &'a Value,
        task: &'a str,
    ) -> BoxFuture<'a, Option<(bool, Option<String>)>> {
        self.inner.put_back(front, previous, task)
    }
    fn run<'a>(
        &'a self,
        args: &'a Args,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        self.around.run(&self.inner, args, ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tool whose every hook the gateway reads answers something other than the trait's default.
    struct Answers;
    impl Tool for Answers {
        fn name(&self) -> &str {
            "fs.read"
        }
        fn describe(&self, _args: &Args) -> ActionSpec {
            ActionSpec::new("write_file")
        }
        fn precheck(&self, _args: &Args) -> Option<String> {
            Some("pre".into())
        }
        fn locks(&self, _args: &Args) -> Vec<String> {
            vec!["file:/x".into()]
        }
        fn file_paths(&self, _args: &Args) -> Option<Vec<(String, String)>> {
            Some(vec![("/x".into(), "read".into())])
        }
        fn real_path(&self, _args: &Args, _workspace: &str) -> Option<RealPath> {
            Some(Ok("/x".into()))
        }
        fn recovery_reason(&self, _args: &Args) -> Option<Option<String>> {
            Some(Some("edit".into()))
        }
        fn ledger_args(&self, args: &Args, _hashed: &Args) -> Option<Args> {
            Some(args.clone())
        }
        fn background_pid(&self, _args: &Args) -> Option<i64> {
            Some(7)
        }
        fn put_back<'a>(
            &'a self,
            _front: &'a Value,
            _previous: &'a Value,
            _task: &'a str,
        ) -> BoxFuture<'a, Option<(bool, Option<String>)>> {
            Box::pin(async { Some((true, None)) })
        }
        fn run<'a>(
            &'a self,
            _args: &'a Args,
            _ctx: &'a ToolContext,
        ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
            Box::pin(async { Err(ToolError::refused("unused")) })
        }
    }

    /// The wrapper answers every hook as the tool it wraps (the audit of 2026-11-16 found `background_pid` and
    /// `put_back` left at the defaults).
    // decision: ENG-20
    #[tokio::test]
    async fn a_wrapped_tool_answers_every_hook_as_its_inner_tool() {
        let w = threaded(Arc::new(Answers));
        let a = Args::new();
        assert_eq!(w.describe(&a).kind, "write_file");
        assert_eq!(w.precheck(&a).as_deref(), Some("pre"));
        assert_eq!(w.locks(&a), vec!["file:/x".to_string()]);
        assert!(w.file_paths(&a).is_some());
        assert!(w.real_path(&a, "/ws").is_some());
        assert!(w.recovery_reason(&a).is_some());
        assert!(w.ledger_args(&a, &a).is_some());
        assert_eq!(w.background_pid(&a), Some(7));
        assert_eq!(
            w.put_back(&Value::Null, &Value::Null, "t").await,
            Some((true, None))
        );
    }
}
