//! A model run's tools (ORCH-719 item 3, ORCH-752, ORCH-756): the listing every runtime gets (the MCP bridge's
//! tools.list for Codex and Claude Code, the API loop's tools, the worker's token reserve), the core chosen at the run's
//! first listing and kept until the run ends, tools.find's answer, and the full description a bad call of a tool
//! listed by its line gets back.

use std::collections::hash_map::Entry;
use std::collections::BTreeSet;

use ferb_policy::destinations as dest;
use ferb_rpc::auth::RunGrant;
use serde_json::{json, Map, Value};

use super::{lock, Gateway, TaskRecord};
use crate::catalog::Live;
use crate::listing;
use crate::tool_core::{self, Kind, Targets, FIND};

/// One run's choice, kept until the run ends (`forget_calls`) or its task leaves.
#[derive(Debug, Clone)]
pub(super) struct RunCore {
    pub(super) task_id: String,
    /// The grant it was chosen for (a worker id is one run's, with one grant).
    grant: String,
    /// The tools listed in full; None: every tool the grant is offered.
    core: Option<BTreeSet<String>>,
    /// The descriptions as the run's latest listing read them: tools.find and a bad call's answer give the same.
    live: Live,
}

fn fingerprint(grant: &RunGrant) -> String {
    format!(
        "{}|{:?}|{}|{:?}",
        listing::identity(grant),
        grant.scopes,
        grant.observe_only,
        grant.tools
    )
}

impl Gateway {
    /// The listing a model run is given: a grant that names its tools gets them all in full (`listing::tools_for`);
    /// a worker's or the checker's run its core in full and every other tool it is offered by name and line.
    pub fn run_listing(&self, grant: &RunGrant, live: &Live) -> Vec<Value> {
        let full = listing::tools_for(&self.registry(), grant, live);
        if lock(&self.inner).tool_core_off {
            return full;
        }
        let Some(kind) = Kind::of(grant) else {
            return full;
        };
        let core = self.run_core(grant, kind, &full, live);
        tool_core::listing(full, core.as_ref())
    }

    /// ORCH-756: whether runs get a core and lines (true, the default) or every tool in full (false).
    pub fn set_tool_core(&self, on: bool) {
        lock(&self.inner).tool_core_off = !on;
    }

    /// The tools this run lists in full, as chosen (None: all, or no listing was made for it yet).
    pub fn run_core_of(&self, worker_id: &str) -> Option<BTreeSet<String>> {
        lock(&self.inner)
            .cores
            .get(worker_id)
            .and_then(|c| c.core.clone())
    }

    /// The run's core: the one chosen at its first listing, else chosen now. A run with a task records its choice on
    /// its span (`tool_core`: the tools in full, those listed by line, how many similar tasks were read).
    fn run_core(
        &self,
        grant: &RunGrant,
        kind: Kind,
        full: &[Value],
        live: &Live,
    ) -> Option<BTreeSet<String>> {
        let print = fingerprint(grant);
        {
            let mut i = lock(&self.inner);
            if let Some(c) = i.cores.get_mut(&grant.worker_id) {
                if c.grant == print {
                    c.live = live.clone();
                    return c.core.clone();
                }
            }
        }
        let offered: Vec<String> = full
            .iter()
            .filter_map(|r| r["name"].as_str())
            .filter(|n| *n != FIND)
            .map(str::to_string)
            .collect();
        let rec = self.task(&grant.task_id);
        let (similar, tasks) = match &rec {
            Some(r) => self.similar_use(r, kind),
            None => (Vec::new(), 0),
        };
        let core = tool_core::choose(kind, &offered, &similar);
        let Some(rec) = rec else {
            return core;
        };
        {
            let mut i = lock(&self.inner);
            match i.cores.entry(grant.worker_id.clone()) {
                Entry::Occupied(o) if o.get().grant == print => return o.get().core.clone(),
                Entry::Occupied(_) => return core,
                Entry::Vacant(v) => {
                    v.insert(RunCore {
                        task_id: grant.task_id.clone(),
                        grant: print,
                        core: core.clone(),
                        live: live.clone(),
                    });
                }
            }
        }
        let in_full = |n: &str| core.as_ref().is_none_or(|c| c.contains(n));
        let shown: Vec<&str> = full
            .iter()
            .filter_map(|r| r["name"].as_str())
            .filter(|n| in_full(n) && (core.is_some() || *n != FIND))
            .collect();
        let by_line: Vec<&String> = offered.iter().filter(|n| !in_full(n)).collect();
        let _ = self.spans(|s| {
            s.update_input(
                &rec.span_id,
                &json!({"tool_core": {"kind": kind.label(), "full": shown, "listed": by_line,
                    "similar_tasks": tasks}}),
            )
        });
        core
    }

    /// The tools earlier tasks similar to this one used (ORCH-752): tasks of other requests whose targets share an
    /// app, a site or a person with this task's request, newest first, read until their tools would fill the core;
    /// each tool with how many of their tasks of this kind called it (FERB's own questions left out), and how many
    /// tasks that is. None similar, or a request with no targets: nothing (the default order alone).
    fn similar_use(&self, rec: &TaskRecord, kind: Kind) -> (Vec<(String, usize)>, usize) {
        let req = self.request(&rec.request_id);
        let host = |s: &str| dest::site_host(s);
        let mine = Targets::new(
            req.targets.apps.iter().map(String::as_str),
            req.targets.sites.iter().map(String::as_str),
            req.targets.recipients.iter().map(String::as_str),
            host,
        );
        if mine.is_empty() {
            return (Vec::new(), 0);
        }
        let tree = rec.tree_id.clone().unwrap_or_else(|| rec.task_id.clone());
        let roles = kind
            .roles()
            .iter()
            .map(|r| format!("'{r}'"))
            .collect::<Vec<_>>()
            .join(", ");
        let db = lock(&self.d.db);
        let conn = db.conn();
        let read = || -> rusqlite::Result<Vec<(String, String)>> {
            let mut trees = conn.prepare(
                "SELECT task_id, json_extract(spec_json, '$.targets') FROM task_context WHERE task_id != ?1 \
                 ORDER BY rowid DESC",
            )?;
            let mut used = conn.prepare(&format!(
                "SELECT json_extract(s.input_json, '$.tool'), s.task_id FROM tasks t JOIN spans s ON s.task_id = t.id \
                 WHERE t.tree_id = ?1 AND t.role IN ({roles}) AND s.kind = 'tool.call' \
                 AND json_extract(s.input_json, '$.ferb_question') IS NULL"
            ))?;
            let mut calls: Vec<(String, String)> = Vec::new();
            let mut distinct: BTreeSet<String> = BTreeSet::new();
            let mut rows = trees.query([&tree])?;
            while let Some(row) = rows.next()? {
                if distinct.len() >= kind.size() {
                    break;
                }
                let id: String = row.get(0)?;
                let targets: Option<String> = row.get(1)?;
                let theirs = targets
                    .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                    .map(|v| Targets::of_spec(&v, host))
                    .unwrap_or_default();
                if !tool_core::shares(&mine, &theirs) {
                    continue;
                }
                let mut found = used.query([&id])?;
                while let Some(u) = found.next()? {
                    let (tool, task): (Option<String>, String) = (u.get(0)?, u.get(1)?);
                    if let Some(tool) = tool {
                        distinct.insert(tool.clone());
                        calls.push((tool, task));
                    }
                }
            }
            Ok(calls)
        };
        let calls = read().unwrap_or_default();
        let tasks: BTreeSet<&String> = calls.iter().map(|(_, t)| t).collect();
        let n = tasks.len();
        (tool_core::counted(&calls), n)
    }

    /// The run's full listing as its latest listing read the descriptions.
    fn full_listing(&self, grant: &RunGrant) -> Vec<Value> {
        let live = lock(&self.inner)
            .cores
            .get(&grant.worker_id)
            .map(|c| c.live.clone())
            .unwrap_or_default();
        listing::tools_for(&self.registry(), grant, &live)
    }

    /// tools.find's answer for this run (its content, and whether it is an error).
    pub(super) fn find_tools(
        &self,
        grant: &RunGrant,
        args: &Map<String, Value>,
    ) -> (Map<String, Value>, bool) {
        let text = |k: &str| {
            args.get(k)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        tool_core::find(&self.full_listing(grant), &text("name"), &text("query"))
    }

    /// A tool this run lists by its line, in full, for a call whose arguments didn't fit (None: the run lists it in
    /// full, or no listing was made for the run).
    pub(super) fn described(&self, grant: &RunGrant, name: &str) -> Option<Value> {
        let core = lock(&self.inner)
            .cores
            .get(&grant.worker_id)
            .and_then(|c| c.core.clone())?;
        if core.contains(name) {
            return None;
        }
        self.full_listing(grant)
            .into_iter()
            .find(|r| r["name"] == name)
    }
}
