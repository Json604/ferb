//! `project.publish` and `project.fetch` (HAND-623, HAND-624, HAND-641..643): a project folder's git repository pushed
//! to, or got from, its remote with the owner's own git login, by FERB's code outside the shell sandbox (ferb-shell
//! `publish` and `fetch`). Rust additions: their schemas and descriptions are in ferb-gateway's
//! `catalog_additions.json`.
//!
//! The gateway asks before a folder's first push to an address, its first get from an address (or a clone), its first
//! use of the GitHub CLI's login, and before every forced push (`prepare_publish` works the step out first, rule 5),
//! and records the owner's Yes. `run` works the step out again and goes ahead only on what the owner approved for that
//! folder: an address changed while the question was open is refused (the next call asks again); a forced push
//! replaces only the remote commit the owner was shown, once.

use std::sync::Arc;

use ferb_core::interfaces::BoxFuture;
use ferb_core::types::{ActionSpec, ToolContext};
use ferb_gateway::tool::{Args, Tool, ToolError, ToolOutput};
use ferb_policy::projects;
use ferb_shell::fetch::FetchAsk;
use ferb_shell::publish::{Ask, Publisher};
use serde_json::{Map, Value};

use super::Parts;
use crate::basics::SharedDb;

const NO_SHELL: &str = "FERB's shell isn't available here (its protection lists couldn't be read)";

pub struct PublishTool {
    publisher: Option<Arc<Publisher>>,
    db: SharedDb,
    clock: Arc<dyn ferb_core::interfaces::Clock>,
    ids: Arc<dyn ferb_core::interfaces::IdSource>,
}

pub struct FetchTool {
    publisher: Option<Arc<Publisher>>,
    db: SharedDb,
}

fn publisher(p: &Parts) -> Option<Arc<Publisher>> {
    p.shells().map(|s| {
        Arc::new(Publisher::new(
            s.sandbox().clone(),
            p.user_home().clone(),
            p.home().join("run").join("publish"),
        ))
    })
}

/// project.publish, over the shell's sandbox (none: refused, as the shell tools are without it).
pub fn tool(p: &Parts) -> Arc<dyn Tool> {
    Arc::new(PublishTool {
        publisher: publisher(p),
        db: p.db(),
        clock: p.clock(),
        ids: p.ids(),
    })
}

/// project.fetch (HAND-642).
pub fn fetch_tool(p: &Parts) -> Arc<dyn Tool> {
    Arc::new(FetchTool {
        publisher: publisher(p),
        db: p.db(),
    })
}

fn failed(e: rusqlite::Error) -> ToolError {
    ToolError::failed("RuntimeError", e.to_string())
}

/// The project as the owner's answers left it, if `folder` is one.
fn project(db: &SharedDb, folder: &str) -> Result<Option<projects::Project>, ToolError> {
    Ok(projects::store::list(&crate::basics::lock(db))
        .map_err(failed)?
        .into_iter()
        .find(|p| p.folder == folder))
}

const CHANGED: &str =
    "which the owner hasn't approved for this folder (it changed after the question); call again \
and FERB asks";

impl Tool for PublishTool {
    fn name(&self) -> &str {
        projects::TOOL
    }

    fn describe(&self, args: &Args) -> ActionSpec {
        let mut s = ActionSpec::new(projects::KIND);
        s.target.path = args
            .get("folder")
            .and_then(Value::as_str)
            .map(str::to_string);
        s
    }

    fn prepare_publish<'a>(
        &'a self,
        args: &'a Args,
    ) -> BoxFuture<'a, Option<Result<Map<String, Value>, String>>> {
        Box::pin(async move {
            let Some(p) = self.publisher.clone() else {
                return Some(Err(NO_SHELL.to_string()));
            };
            Some(
                p.prepare(&Ask::from_args(args))
                    .await
                    .map(|plan| plan.to_json()),
            )
        })
    }

    fn run<'a>(
        &'a self,
        args: &'a Args,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let publisher = self
                .publisher
                .clone()
                .ok_or_else(|| ToolError::refused(NO_SHELL))?;
            let ask = Ask::from_args(args);
            // the push as it stands now (no network): what the owner's answers must cover
            let plan = publisher.plan(&ask).await.map_err(ToolError::refused)?;
            let p = project(&self.db, &plan.folder)?;
            if !p
                .as_ref()
                .is_some_and(|p| p.destinations.contains(&plan.url))
            {
                return Err(ToolError::refused(format!(
                    "the push address is now {}, {CHANGED}",
                    plan.url
                )));
            }
            if ask.gh && !p.as_ref().is_some_and(|p| p.gh_hosts.contains(&plan.host)) {
                return Err(ToolError::refused(
                    "the owner hasn't allowed the GitHub CLI's login for this project; call again and FERB asks",
                ));
            }
            let lease = if ask.force {
                let taken = projects::store::take_force(
                    &crate::basics::lock(&self.db),
                    self.clock.as_ref(),
                    self.ids.as_ref(),
                    &ctx.task_id,
                    &plan.folder,
                    &plan.url,
                    &plan.branch,
                    &plan.commit,
                )
                .map_err(failed)?;
                match taken {
                    Some(remote) => Some(remote),
                    // nothing to replace when it was worked out: an ordinary push (git refuses a diverged one)
                    None if !needs_force(&publisher, &ask).await => None,
                    None => {
                        return Err(ToolError::refused(
                            "the branch or the address changed after the owner's Yes to overwrite it; call again and \
                             FERB asks with what is there now",
                        ))
                    }
                }
            } else {
                None
            };
            let facts = publisher
                .push(&plan, ask.set_upstream, lease.as_deref())
                .await
                .map_err(ToolError::refused)?;
            Ok(ferb_shell::publish::result(facts, "pushed").into())
        })
    }
}

/// Whether a forced push would replace anything now (the remote branch fetched again).
async fn needs_force(publisher: &Publisher, ask: &Ask) -> bool {
    publisher
        .prepare(ask)
        .await
        .map(|p| p.force.is_some())
        .unwrap_or(true)
}

impl Tool for FetchTool {
    fn name(&self) -> &str {
        projects::FETCH_TOOL
    }

    fn describe(&self, args: &Args) -> ActionSpec {
        let mut s = ActionSpec::new(projects::FETCH_KIND);
        s.target.path = args
            .get("folder")
            .and_then(Value::as_str)
            .map(str::to_string);
        s
    }

    fn prepare_publish<'a>(
        &'a self,
        args: &'a Args,
    ) -> BoxFuture<'a, Option<Result<Map<String, Value>, String>>> {
        Box::pin(async move {
            let Some(p) = self.publisher.clone() else {
                return Some(Err(NO_SHELL.to_string()));
            };
            Some(
                p.prepare_fetch(&FetchAsk::from_args(args))
                    .await
                    .map(|plan| plan.to_json()),
            )
        })
    }

    fn run<'a>(
        &'a self,
        args: &'a Args,
        _ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let publisher = self
                .publisher
                .clone()
                .ok_or_else(|| ToolError::refused(NO_SHELL))?;
            let ask = FetchAsk::from_args(args);
            let plan = publisher
                .prepare_fetch(&ask)
                .await
                .map_err(ToolError::refused)?;
            let p = project(&self.db, &plan.folder)?;
            if !p.as_ref().is_some_and(|p| p.gets_from(&plan.url)) {
                return Err(ToolError::refused(format!(
                    "the address is now {}, {CHANGED}",
                    plan.url
                )));
            }
            if ask.gh && !p.as_ref().is_some_and(|p| p.gh_hosts.contains(&plan.host)) {
                return Err(ToolError::refused(
                    "the owner hasn't allowed the GitHub CLI's login for this project; call again and FERB asks",
                ));
            }
            let facts = publisher.fetch(&plan).await.map_err(ToolError::refused)?;
            Ok(ferb_shell::publish::result(facts, "fetched").into())
        })
    }
}
