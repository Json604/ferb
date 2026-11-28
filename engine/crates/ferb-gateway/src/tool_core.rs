//! A model run's tools (ORCH-719 item 3, ORCH-752; the mechanism ORCH-756): a worker's or the checker's run gets a
//! fixed core of tools in full (a worker at most 25, the checker at most 10: the owner's numbers) and every other tool
//! it is offered by name and one line saying what it does and when to use it (`tool_lines.json`). `tools.find` gives
//! a tool's full description; a direct call to any listed tool runs, and one whose arguments don't fit gets the
//! tool's full description in its error. The core is chosen once, at the run's first listing, and kept for the run
//! (the gateway's `run_listing`), so the listing never changes mid-run (prompt caching, Anthropic's thinking binding).
//!
//! The core is the tools similar earlier tasks used (tasks sharing an app, site or person of this task's targets;
//! most tasks first), then the default order measured on the owner's November records, then the rest the grant is
//! offered in registry order, until it is full. A grant that names its tools (the planner's lookups, the front
//! door) and a grant offered no more tools than its core holds get every tool in full, as before.
//!
//! Portable: this module decides and composes; the gateway (Mac side) keeps the per-run choice and reads the store.

use std::collections::{BTreeMap, BTreeSet};

use ferb_rpc::auth::RunGrant;
use serde_json::{json, Map, Value};

use crate::catalog;

/// The tool that gives listed tools' full descriptions.
pub const FIND: &str = "tools.find";

/// ORCH-719 (owner): a worker run's full core holds at most 25 tools, tools.find among them.
pub const WORKER_CORE: usize = 25;
/// ORCH-719 (owner): the checker's at most 10, tools.find among them.
pub const CHECKER_CORE: usize = 10;

/// A worker's default order (ORCH-756): the owner's records of 2026-11-01..28, read only, ranked by the number of
/// tasks that used each tool (chat.* left out): the first 25 over every role's tasks (93.6% of the workers' tool
/// calls), then the rest of what worker runs used. A fresh install with no records starts from this order; a
/// tool no record names comes after it, in registry order.
pub const WORKER_ORDER: &[&str] = &[
    "browser.snapshot",
    "browser.open",
    "browser.act",
    "browser.find",
    "browser.tabs",
    "browser.navigate",
    "mac.snapshot",
    "browser.wait",
    "owner.ask",
    "task.evidence",
    "browser.use_tab",
    "browser.html",
    "browser.screenshot",
    "mac.apps",
    "mac.act",
    "browser.read",
    "shell.run",
    "mac.screenshot",
    "wait.for",
    "mac.open",
    "fs.list",
    "fs.look",
    "mac.click",
    "fs.read",
    "browser.profiles",
    // the rest of what worker runs used, by tasks
    "browser.close",
    "browser.keep_open",
    "mac.menu",
    "browser.downloads",
    "browser.scroll",
    "mac.applescript",
    "browser.pdf",
    "browser.point",
    "memory.learned",
    "memory.read",
    "shell.read",
    "browser.pair",
    "fs.move",
    "browser.media",
    "browser.secret",
    "mac.pointer",
    "trail.query",
    "actions.search",
    "audio.listen",
    "fs.copy",
    "fs.write",
    "recording.inspect",
    "abilities.read",
    "about.diagnostics",
    "browser.login_code",
    "browser.record",
    "conversation.read",
    "device.facts",
    "mac.clipboard",
    "recording.stop",
    "secrets.list",
    "shell.rules",
    "shell.start",
    "shell.wait",
    "time.remind_at",
    "time.sleep_until",
];

/// The checker's default order (ORCH-756): the same records' checker runs, by tasks.
pub const CHECKER_ORDER: &[&str] = &[
    "browser.snapshot",
    "task.evidence",
    "browser.find",
    "fs.look",
    "browser.read",
    "fs.list",
    "browser.screenshot",
    "chat.history",
    "mac.snapshot",
    "browser.tabs",
    "fs.read",
    "fs.disk_usage",
    "audio.listen",
    "browser.profiles",
    "mac.screenshot",
];

/// Which run a grant is: a worker's or the checker's (observe-only). None: a grant that names its tools, listed whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Worker,
    Checker,
}

impl Kind {
    pub fn of(grant: &RunGrant) -> Option<Kind> {
        if !grant.tools.is_empty() {
            None
        } else if grant.observe_only {
            Some(Kind::Checker)
        } else {
            Some(Kind::Worker)
        }
    }

    /// How many tools its core holds at most.
    pub fn size(self) -> usize {
        match self {
            Kind::Worker => WORKER_CORE,
            Kind::Checker => CHECKER_CORE,
        }
    }

    pub fn order(self) -> &'static [&'static str] {
        match self {
            Kind::Worker => WORKER_ORDER,
            Kind::Checker => CHECKER_ORDER,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Worker => "worker",
            Kind::Checker => "checker",
        }
    }

    /// The task roles whose earlier calls count as this kind's use (`tasks.role`).
    pub fn roles(self) -> &'static [&'static str] {
        match self {
            Kind::Worker => &["worker", "mixed"],
            Kind::Checker => &["verifier"],
        }
    }
}

/// The tool a name of the default order stands for in this grant: FERB's own browser's `ferb_browser.X` where the
/// grant is offered it and not `browser.X` (a run in FERB's own identity).
fn stand_in<'a>(name: &str, offered: &'a [String]) -> Option<&'a String> {
    if let Some(n) = offered.iter().find(|n| *n == name) {
        return Some(n);
    }
    let ours = name.strip_prefix("browser.")?;
    offered
        .iter()
        .find(|n| n.strip_prefix("ferb_browser.") == Some(ours))
}

/// The run's core, chosen once (ORCH-752). `offered`: the grant's tools in registry order, tools.find left out;
/// `similar`: each tool similar earlier tasks used, with how many of them used it. None: every offered tool fits in
/// the core, so all are listed in full and tools.find is not listed. Otherwise tools.find and the first `size - 1`
/// of: the similar tasks' tools (most tasks first, ties by the default order), the default order, the rest offered.
pub fn choose(
    kind: Kind,
    offered: &[String],
    similar: &[(String, usize)],
) -> Option<BTreeSet<String>> {
    if offered.len() <= kind.size() {
        return None;
    }
    let rank = |n: &str| {
        kind.order()
            .iter()
            .position(|o| stand_in(o, offered).is_some_and(|s| s == n))
            .unwrap_or(usize::MAX)
    };
    let mut used: Vec<(&String, usize)> = similar
        .iter()
        .filter_map(|(n, c)| offered.iter().find(|o| *o == n).map(|o| (o, *c)))
        .collect();
    used.sort_by(|a, b| b.1.cmp(&a.1).then(rank(a.0).cmp(&rank(b.0))));
    let mut picked: Vec<&String> = Vec::new();
    let candidates = used
        .into_iter()
        .map(|(n, _)| n)
        .chain(kind.order().iter().filter_map(|o| stand_in(o, offered)))
        .chain(offered.iter());
    for n in candidates {
        if picked.len() + 1 >= kind.size() {
            break;
        }
        if !picked.contains(&n) {
            picked.push(n);
        }
    }
    let mut core: BTreeSet<String> = picked.into_iter().cloned().collect();
    core.insert(FIND.to_string());
    Some(core)
}

/// The schema a tool listed by its line carries: any arguments (the gateway checks them against the tool's own).
pub fn listed_only_schema() -> Value {
    json!({"type": "object", "properties": {}, "additionalProperties": true})
}

/// A tool listed by name and line (ORCH-752).
pub fn line_row(full: &Value) -> Value {
    let name = full["name"].as_str().unwrap_or("");
    let line = catalog::catalog().line(name);
    let description = if line.is_empty() {
        full["description"].clone()
    } else {
        Value::String(line.to_string())
    };
    let mut row = Map::new();
    row.insert("name".into(), Value::String(name.to_string()));
    row.insert("description".into(), description);
    row.insert("input_schema".into(), listed_only_schema());
    Value::Object(row)
}

/// The run's listing from the grant's full listing (`listing::tools_for`, registry order): with no core, every tool
/// in full but tools.find; else the core in full and the rest by line.
pub fn listing(full: Vec<Value>, core: Option<&BTreeSet<String>>) -> Vec<Value> {
    full.into_iter()
        .filter_map(|row| {
            let name = row["name"].as_str().unwrap_or("").to_string();
            match core {
                None if name == FIND => None,
                None => Some(row),
                Some(c) if c.contains(&name) => Some(row),
                Some(_) => Some(line_row(&row)),
            }
        })
        .collect()
}

/// The words a tool is found by: lower-case runs of letters and digits.
fn words(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// How many tools a query's answer gives in full.
pub const FOUND_IN_FULL: usize = 5;

/// tools.find's answer over the run's full listing (`full`, registry order). `name`: that tool exactly; else `query`:
/// the tools whose name, line and description share the most of its words (literal word overlap, best first, ties in
/// registry order), the first five in full and the other matches by name.
pub fn find(full: &[Value], name: &str, query: &str) -> (Map<String, Value>, bool) {
    let rows: Vec<&Value> = full.iter().filter(|r| r["name"] != FIND).collect();
    let name = name.trim();
    if !name.is_empty() {
        // a name as the MCP programs show it (browser_snapshot) is the same tool
        let found = rows.iter().find(|r| {
            let n = r["name"].as_str().unwrap_or("");
            n == name || n.replace('.', "_") == name
        });
        return match found {
            Some(r) => (obj(json!({"tools": [r]})), false),
            None => (
                obj(
                    json!({"error": format!("no tool named {name} is listed for this run"),
                    "next": "Give query (words for what you need to do) to find one."}),
                ),
                true,
            ),
        };
    }
    let asked = words(query);
    if asked.is_empty() {
        return (
            obj(
                json!({"error": "give name (a listed tool's exact name) or query (words for what you need to do)"}),
            ),
            true,
        );
    }
    let mut scored: Vec<(usize, usize, &Value)> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let n = r["name"].as_str().unwrap_or("");
            let text = format!(
                "{} {} {}",
                n,
                catalog::catalog().line(n),
                r["description"].as_str().unwrap_or("")
            );
            let have = words(&text);
            let score = asked.intersection(&have).count();
            (score > 0).then_some((score, i, *r))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let shown: Vec<Value> = scored
        .iter()
        .take(FOUND_IN_FULL)
        .map(|(_, _, r)| (*r).clone())
        .collect();
    let more: Vec<Value> = scored
        .iter()
        .skip(FOUND_IN_FULL)
        .map(|(_, _, r)| r["name"].clone())
        .collect();
    let mut content = obj(json!({"tools": shown}));
    if !more.is_empty() {
        content.insert("more".into(), Value::Array(more));
    }
    if shown_is_empty(&content) {
        content.insert(
            "note".into(),
            json!(
                "no listed tool shares a word with the query; try other words, or actions.search"
            ),
        );
    }
    (content, false)
}

fn shown_is_empty(content: &Map<String, Value>) -> bool {
    content["tools"].as_array().is_none_or(Vec::is_empty)
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// The similar earlier tasks' use, counted: each tool with how many distinct tasks called it.
pub fn counted(calls: &[(String, String)]) -> Vec<(String, usize)> {
    let mut by: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (tool, task) in calls {
        by.entry(tool).or_default().insert(task);
    }
    by.into_iter()
        .map(|(t, tasks)| (t.to_string(), tasks.len()))
        .collect()
}

/// Whether two tasks' targets share an app, a site or a person (ORCH-752's similar tasks): apps and people by their
/// words without case, sites by host.
pub fn shares(a: &Targets, b: &Targets) -> bool {
    !a.apps.is_disjoint(&b.apps)
        || !a.sites.is_disjoint(&b.sites)
        || !a.people.is_disjoint(&b.people)
}

/// A task's targets as similarity compares them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Targets {
    pub apps: BTreeSet<String>,
    pub sites: BTreeSet<String>,
    pub people: BTreeSet<String>,
}

impl Targets {
    /// From the task spec's lists (`targets.apps`, `.sites`, `.recipients`); `host` normalizes a site.
    pub fn new<'a>(
        apps: impl IntoIterator<Item = &'a str>,
        sites: impl IntoIterator<Item = &'a str>,
        people: impl IntoIterator<Item = &'a str>,
        host: impl Fn(&str) -> String,
    ) -> Targets {
        let plain = |s: &str| s.trim().to_lowercase();
        Targets {
            apps: apps
                .into_iter()
                .map(plain)
                .filter(|s| !s.is_empty())
                .collect(),
            sites: sites
                .into_iter()
                .filter(|s| !s.trim().is_empty())
                .map(&host)
                .filter(|s| !s.is_empty())
                .collect(),
            people: people
                .into_iter()
                .map(plain)
                .filter(|s| !s.is_empty())
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.apps.is_empty() && self.sites.is_empty() && self.people.is_empty()
    }

    /// From a task spec's `targets` object as stored (`task_context.spec_json`).
    pub fn of_spec(targets: &Value, host: impl Fn(&str) -> String) -> Targets {
        let list = |k: &str| -> Vec<&str> {
            targets[k]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default()
        };
        Targets::new(list("apps"), list("sites"), list("recipients"), host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_grant_with_few_tools_gets_them_all() {
        let offered = names(&["fs.list", "fs.read"]);
        assert_eq!(choose(Kind::Worker, &offered, &[]), None);
    }

    #[test]
    fn ferb_browser_stands_for_browser() {
        let mut offered: Vec<String> = (0..30).map(|i| format!("x.t{i}")).collect();
        offered.push("ferb_browser.snapshot".into());
        let core = choose(Kind::Checker, &offered, &[]).unwrap();
        assert!(core.contains("ferb_browser.snapshot"));
        assert_eq!(core.len(), CHECKER_CORE);
    }
}
