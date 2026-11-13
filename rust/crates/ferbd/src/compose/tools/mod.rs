//! Every tool the gateway runs, registered through its `Tool` trait in Python's registry order (`ferb/registry.py`
//! `build`, `add_recording`, `add_recovery`, `add_owner_browsers`, `add_mac`, the shell, `add_trail`, the context
//! readers, `add_identity_browser`): the 97 tools of `rust/fixtures/catalog/tools.json`.
//!
//! A tool's schema, description, scopes, identities and facts come from the frozen catalog through the trait's
//! defaults, so a Rust tool can't serve a schema the models were never given. Each family's `describe` and `run`
//! call the crate that holds them; a tool whose part isn't composed in this build answers a refusal that says so
//! (`not available in this build`), never a made-up result.

pub mod browser;
pub mod misc;
pub mod shell;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use ferb_core::config::Config;
use ferb_core::interfaces::{BoxFuture, Clock, IdSource};
use ferb_core::types::{ActionSpec, ToolContext};
use ferb_gateway::gateway::Gateway;
use ferb_gateway::tool::{Args, Registry, Tool, ToolError, ToolOutput};
use ferb_sanitize::redact::Redactor;

use super::Options;
use crate::basics::{Events, Journal, SharedDb};

/// What a tool whose part isn't composed in this build answers.
pub fn not_in_this_build(what: &str) -> ToolError {
    ToolError::refused(format!(
        "not available in this build: {what} isn't part of the Rust daemon yet"
    ))
}

/// A run, as a family gives it.
pub type Run = Arc<
    dyn for<'a> Fn(&'a Args, &'a ToolContext) -> BoxFuture<'a, Result<ToolOutput, ToolError>>
        + Send
        + Sync,
>;

/// A tool of the frozen catalog whose behavior is given by functions: `describe` and `run`, everything else the
/// catalog's (the `Tool` trait's defaults).
pub struct Catalogued {
    pub name: &'static str,
    pub describe: Arc<dyn Fn(&Args) -> ActionSpec + Send + Sync>,
    pub run: Run,
}

impl Tool for Catalogued {
    fn name(&self) -> &str {
        self.name
    }
    fn describe(&self, args: &Args) -> ActionSpec {
        (self.describe)(args)
    }
    fn run<'a>(
        &'a self,
        args: &'a Args,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        (self.run)(args, ctx)
    }
}

/// A catalog tool whose part isn't composed in this build: described as the catalog's facts say (a read for an
/// observation, else its kind is the tool's own name), and refused when run.
pub fn unavailable(name: &'static str, what: &'static str) -> Arc<dyn Tool> {
    let observation = ferb_gateway::tool::Facts::from_catalog(name)
        .map(|f| f.observation)
        .unwrap_or(false);
    Arc::new(Catalogued {
        name,
        describe: Arc::new(move |_a| ActionSpec::new(if observation { "read" } else { name })),
        run: Arc::new(move |_a, _c| Box::pin(async move { Err(not_in_this_build(what)) })),
    })
}

struct Inner {
    home: PathBuf,
    user_home: PathBuf,
    native: bool,
    clock: Arc<dyn ferb_clock::clock::ZonedClock>,
    ids: Arc<dyn IdSource>,
    redactor: Arc<Redactor>,
    config: Arc<Config>,
    db: SharedDb,
    journal: Arc<Journal>,
    events: Arc<Events>,
    shells: Option<Arc<ferb_shell::manager::ShellManager>>,
    shell_protection: Option<Arc<dyn ferb_shell::protection::Protection>>,
    /// The thread gateway calls run on: a call's future holds what can't move between threads.
    calls: ferb_daemon::shared::LoopThread,
    browsers: OnceLock<browser::Browsers>,
    gateway: OnceLock<Weak<Gateway>>,
    secrets: Mutex<Vec<ferb_gateway::catalog::SavedSecret>>,
}

/// The parts the tools are built over, shared with the areas that drive the same parts (shell processes, browsers).
#[derive(Clone)]
pub struct Parts(Arc<Inner>);

impl Parts {
    pub fn new(
        opts: &Options,
        config: &Arc<Config>,
        redactor: &Arc<Redactor>,
        db: &SharedDb,
        journal: &Arc<Journal>,
        events: &Arc<Events>,
    ) -> Parts {
        let (shells, shell_protection) = match shell::manager(opts, config, redactor) {
            Some((m, p)) => (Some(m), Some(p)),
            None => (None, None),
        };
        let p = Parts(Arc::new(Inner {
            home: opts.home.clone(),
            user_home: opts.user_home.clone(),
            native: opts.native,
            clock: opts.clock.clone(),
            ids: opts.ids.clone(),
            redactor: redactor.clone(),
            config: config.clone(),
            db: db.clone(),
            journal: journal.clone(),
            events: events.clone(),
            shells,
            shell_protection,
            calls: ferb_daemon::shared::LoopThread::start("ferb-calls"),
            browsers: OnceLock::new(),
            gateway: OnceLock::new(),
            secrets: Mutex::new(Vec::new()),
        }));
        let _ = p.0.browsers.set(browser::Browsers::build(opts, &p));
        p
    }

    pub fn home(&self) -> &PathBuf {
        &self.0.home
    }
    pub fn user_home(&self) -> &PathBuf {
        &self.0.user_home
    }
    pub fn native(&self) -> bool {
        self.0.native
    }
    pub fn clock(&self) -> Arc<dyn Clock> {
        self.0.clock.clone()
    }
    pub fn zoned(&self) -> Arc<dyn ferb_clock::clock::ZonedClock> {
        self.0.clock.clone()
    }
    pub fn ids(&self) -> Arc<dyn IdSource> {
        self.0.ids.clone()
    }
    pub fn redactor(&self) -> Arc<Redactor> {
        self.0.redactor.clone()
    }
    pub fn config(&self) -> Arc<Config> {
        self.0.config.clone()
    }
    pub fn db(&self) -> SharedDb {
        self.0.db.clone()
    }
    pub fn journal(&self) -> Arc<Journal> {
        self.0.journal.clone()
    }
    pub fn events(&self) -> Arc<Events> {
        self.0.events.clone()
    }
    pub fn shells(&self) -> Option<Arc<ferb_shell::manager::ShellManager>> {
        self.0.shells.clone()
    }
    pub fn shell_protection(&self) -> Option<Arc<dyn ferb_shell::protection::Protection>> {
        self.0.shell_protection.clone()
    }

    /// One gateway call, run on the calls' thread (its future isn't `Send`: listed as a seam mismatch).
    pub async fn call(
        &self,
        gateway: Arc<Gateway>,
        grant: Arc<ferb_rpc::auth::RunGrant>,
        name: String,
        args: ferb_gateway::tool::Args,
        caller_left: ferb_gateway::gateway::Left,
    ) -> Result<ferb_gateway::reply::Reply, ferb_gateway::gateway::CallError> {
        self.0
            .calls
            .run(move || async move {
                gateway
                    .call_as(
                        &grant,
                        &name,
                        &args,
                        ferb_gateway::gateway::Cancel::new(),
                        caller_left,
                    )
                    .await
            })
            .await
            .unwrap_or_else(|_| {
                Err(ferb_gateway::gateway::CallError::Internal(
                    "the calls' thread has stopped".into(),
                ))
            })
    }

    pub fn browsers(&self) -> Option<&browser::Browsers> {
        self.0.browsers.get()
    }
    pub fn owner_driver(&self) -> Option<Arc<ferb_browser::driver::Driver>> {
        self.browsers().map(|b| b.router.clone())
    }
    pub fn ferb_driver(&self) -> Option<Arc<ferb_browser::driver::Driver>> {
        self.browsers().and_then(|b| b.identity.clone())
    }
    /// The `ferb_browser.*` tools (FERB's own profile), for when it attaches.
    pub fn ferb_browser_tools(&self) -> Vec<Arc<dyn Tool>> {
        self.browsers().map(|b| b.ferb_tools()).unwrap_or_default()
    }

    /// The gateway, once built (the tools that read it at run time: secrets.list's sites).
    pub fn attach_gateway(&self, gw: &Arc<Gateway>) {
        let _ = self.0.gateway.set(Arc::downgrade(gw));
    }

    pub fn gateway(&self) -> Option<Arc<Gateway>> {
        self.0.gateway.get().and_then(Weak::upgrade)
    }

    /// What a description reads when the listing is made (saved secrets, the downloads total).
    pub fn live(&self) -> ferb_gateway::catalog::Live {
        ferb_gateway::catalog::Live {
            secrets: crate::basics::lock(&self.0.secrets).clone(),
            downloads_total_bytes: None,
        }
    }
}

/// The registry in Python's order (see the module's text). FERB's own browser's tools come last, as when its
/// profile is attached.
pub fn registry(p: &Parts) -> Registry {
    let mut r = Registry::new();
    let mut add = |t: Arc<dyn Tool>| {
        r.add(t).expect("each tool once");
    };
    // build(): actions.search, abilities, the file tools, owner.ask
    for t in misc::first(p) {
        add(t);
    }
    // add_recording, add_recovery
    add(unavailable("recording.stop", "recording"));
    add(unavailable("recording.inspect", "recording"));
    add(misc::recovery_snapshot(p));
    // _build_browsers: the owner's browsers
    if let Some(b) = p.browsers() {
        for t in b.owner_tools() {
            add(t);
        }
    }
    // _build_mac_and_shell: Mac tools, hearing, the shell
    for t in misc::mac(p) {
        add(t);
    }
    add(unavailable("audio.listen", "hearing an app's sound"));
    for t in shell::tools(p) {
        add(t);
    }
    // add_trail, the task's evidence, time, skills, the context readers, secrets, about, conversations, learning
    for t in misc::rest(p) {
        add(t);
    }
    // add_identity_browser: FERB's own profile's tools, only once it is attached (`use_identity_browser`)
    if p.ferb_driver().is_some() {
        for t in p.ferb_browser_tools() {
            add(t);
        }
    }
    r
}

pub use browser::browsers_deps;
