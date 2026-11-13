//! ferb/orchestrator/plans.py: the plan vocabulary's mechanical checks (D30/P15, ORCH-305, ORCH-319, ORCH-580,
//! ORCH-581, ORCH-625). Task choices belong to the planner; these are repair facts only.
//!
//! decision: ORCH-305, ORCH-319, ORCH-322, ORCH-580, ORCH-581, ORCH-625, ORCH-626

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::io::Read;

use ferb_fs::pypath;

use crate::models::{Part, Plan, Step};
use crate::names::shown_name;
use crate::py::{repr_str, repr_strs, sorted_set, strip};

/// plans.py:376 `SINGLE`: the one step of a single task's plan.
pub const SINGLE: &str = "worker";
/// plans.py:377 `DONE`: a step that finished (confirmed, or done but unconfirmed).
pub const DONE: &[&str] = &["done", "done_unverified"];
/// plans.py:378 `ENDED`: never run again without the owner (reopen).
pub const ENDED: &[&str] = &[
    "done",
    "done_unverified",
    "failed",
    "blocked",
    "cancelled",
    "needs_user",
];
/// plans.py:358 `CHECKLIST_STEPS` (APP-894).
pub const CHECKLIST_STEPS: usize = 3;
/// plans.py:305 `TEXT_PROBE`.
pub const TEXT_PROBE: usize = 8192;

/// One file an inventory found: `{"path", "bytes", "text", "tokens_upper_bound"}`.
#[derive(Debug, Clone, PartialEq)]
pub struct FileFact {
    pub path: String,
    pub bytes: u64,
    pub text: bool,
    pub tokens_upper_bound: u64,
}

impl FileFact {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({"path": self.path, "bytes": self.bytes, "text": self.text,
                           "tokens_upper_bound": self.tokens_upper_bound})
    }
}

/// plans.py:136 `Inventory`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Inventory {
    pub files: Vec<FileFact>,
    pub stopped: Option<String>,
    pub counted: u64,
    pub bound: u64,
    pub parts: Vec<(String, u64)>,
    pub unreached: Vec<String>,
}

/// plans.py:150 `walked`.
pub fn walked(plan: &Plan, roots: &[String], normal: &dyn Fn(&str) -> String) -> Vec<String> {
    let listed: Vec<String> = plan
        .steps
        .iter()
        .flat_map(|s| s.inputs.iter().map(|v| normal(v)))
        .collect();
    if listed.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    for p in roots.iter().cloned().chain(listed) {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// Python's `Counter` with insertion order.
#[derive(Default, Clone)]
struct Counter(Vec<(String, u64)>);

impl Counter {
    fn add(&mut self, key: &str, n: u64) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 += n,
            None => self.0.push((key.to_string(), n)),
        }
    }

    fn get(&self, key: &str) -> u64 {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|x| x.1)
            .unwrap_or(0)
    }

    fn contains(&self, key: &str) -> bool {
        self.0.iter().any(|(k, _)| k == key)
    }
}

/// plans.py:161 `inventory`: stat the declared source files without following directory links. `home` is what `~`
/// expands to; `denied` names a file the caller's protections leave out. Synchronous: the daemon runs it off its
/// event loop (L23).
pub fn inventory(
    paths: &[String],
    denied: Option<&dyn Fn(&str) -> bool>,
    inputs: &[String],
    steps: usize,
    context_tokens: Option<u64>,
    home: &str,
) -> Inventory {
    let steps = steps.max(1) as u64;
    let mut found: Vec<FileFact> = Vec::new();
    let mut found_paths: HashSet<String> = HashSet::new();
    // each listed input's resolved path to its path as written
    let mut listed: Vec<(String, String)> = Vec::new();
    for value in inputs {
        if let Ok(real) = pypath::resolve(value) {
            if !listed.iter().any(|(r, _)| *r == real) {
                listed.push((real, value.clone()));
            }
        }
    }
    let mut per_input = Counter::default();
    let mut by_part: HashMap<String, Counter> = listed
        .iter()
        .map(|(r, _)| (r.clone(), Counter::default()))
        .collect();
    let mut by_root: Vec<(String, Counter)> = Vec::new();
    let mut total: u64 = 0;

    let finish = |found: &mut Vec<FileFact>| {
        let mut files = std::mem::take(found);
        files.sort_by(|a, b| a.path.cmp(&b.path));
        files
    };

    for value in paths {
        let root = pypath::expanduser(value, home);
        let Ok(real_root) = pypath::resolve(&root) else {
            continue;
        };
        let folder = pypath::is_dir(&root);
        let files: Box<dyn Iterator<Item = String>> = if folder {
            Box::new(Walk::new(&real_root))
        } else {
            Box::new(std::iter::once(root.clone()))
        };
        if !by_root.iter().any(|(k, _)| *k == real_root) {
            by_root.push((real_root.clone(), Counter::default()));
        }
        for file in files {
            if pypath::is_symlink(&file) || !pypath::is_file(&file) {
                continue;
            }
            let path = if folder {
                file.clone()
            } else {
                match pypath::resolve(&file) {
                    Ok(p) => p,
                    Err(_) => continue,
                }
            };
            if found_paths.contains(&path) || denied.is_some_and(|d| d(&path)) {
                continue;
            }
            let Ok(meta) = fs::metadata(&file) else {
                continue;
            };
            let size = meta.len();
            let text = is_text(&file);
            let tokens = if text { size } else { 0 };
            found_paths.insert(path.clone());
            found.push(FileFact {
                path: path.clone(),
                bytes: size,
                text,
                tokens_upper_bound: tokens,
            });
            let Some(context) = context_tokens else {
                continue;
            };
            if tokens == 0 {
                continue;
            }
            total += tokens;
            let slot = by_root
                .iter_mut()
                .find(|(k, _)| *k == real_root)
                .expect("added above");
            slot.1.add(&part_of(&path, &real_root), tokens);
            for (real, written) in &listed {
                if path == *real || path.starts_with(&format!("{}/", real.trim_end_matches('/'))) {
                    per_input.add(real, tokens);
                    by_part
                        .get_mut(real)
                        .expect("listed")
                        .add(&part_of(&path, real), tokens);
                    if per_input.get(real) > context {
                        let parts = by_part[real].clone();
                        let mut result = stopped(
                            Some(real),
                            context,
                            &parts,
                            &by_root,
                            per_input.get(real),
                            total,
                        );
                        result.files = finish(&mut found);
                        result.stopped = Some(written.clone());
                        return result;
                    }
                }
            }
            if total > steps * context {
                let mut joined = Counter::default();
                for (root_, counter) in &by_root {
                    for (name, n) in &counter.0 {
                        let key = if name.is_empty() {
                            root_.clone()
                        } else {
                            format!("{root_}/{name}")
                        };
                        joined.add(&key, *n);
                    }
                }
                let mut result = stopped(None, steps * context, &joined, &by_root, 0, total);
                result.files = finish(&mut found);
                return result;
            }
        }
    }
    Inventory {
        files: finish(&mut found),
        ..Inventory::default()
    }
}

fn stopped(
    real: Option<&str>,
    bound: u64,
    parts: &Counter,
    by_root: &[(String, Counter)],
    counted: u64,
    total: u64,
) -> Inventory {
    let folders: Vec<String> = match real {
        Some(r) => vec![r.to_string()],
        None => by_root.iter().map(|(k, _)| k.clone()).collect(),
    };
    let mut unreached = Vec::new();
    for folder in &folders {
        for name in subfolders(folder) {
            let key = if real.is_some() {
                name.clone()
            } else {
                format!("{folder}/{name}")
            };
            if !parts.contains(&key) {
                unreached.push(if real.is_none() {
                    format!("{folder}/{name}")
                } else {
                    name
                });
            }
        }
    }
    let mut sorted_parts = parts.0.clone();
    sorted_parts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Inventory {
        files: Vec::new(),
        stopped: real.map(String::from),
        counted: if real.is_none() { total } else { counted },
        bound,
        parts: sorted_parts,
        unreached,
    }
}

/// plans.py:244 `_files`: depth first in name order, links to folders not followed, yielded as found.
struct Walk {
    pending: Vec<String>,
    current: VecDeque<String>,
}

impl Walk {
    fn new(root: &str) -> Walk {
        Walk {
            pending: vec![root.to_string()],
            current: VecDeque::new(),
        }
    }
}

fn sorted_entries(folder: &str) -> Option<Vec<(String, String)>> {
    let listing = fs::read_dir(folder).ok()?;
    let mut entries: Vec<(String, String)> = listing
        .filter_map(|e| e.ok())
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            (name.clone(), pypath::join(folder, &name))
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Some(entries)
}

impl Iterator for Walk {
    type Item = String;

    fn next(&mut self) -> Option<String> {
        loop {
            if let Some(next) = self.current.pop_front() {
                return Some(next);
            }
            let folder = self.pending.pop()?;
            let Some(entries) = sorted_entries(&folder) else {
                continue;
            };
            let mut below = Vec::new();
            for (_, path) in entries {
                match fs::symlink_metadata(&path) {
                    Ok(m) if m.is_dir() => below.push(path),
                    Ok(_) => self.current.push_back(path),
                    Err(_) => {}
                }
            }
            self.pending.extend(below.into_iter().rev());
        }
    }
}

/// plans.py:267 `_subfolders`.
fn subfolders(folder: &str) -> Vec<String> {
    let Ok(listing) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut names: Vec<String> = listing
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// plans.py:276 `_part`.
fn part_of(path: &str, folder: &str) -> String {
    let cut = folder.trim_end_matches('/').len() + 1;
    let rest = path.get(cut..).unwrap_or("");
    match rest.split_once('/') {
        Some((first, _)) => first.to_string(),
        None => String::new(),
    }
}

/// plans.py:282 `looking_at`.
pub fn looking_at(paths: &[String], home: &str) -> String {
    let shown: Vec<String> = paths
        .iter()
        .map(|p| {
            if p == home || p.starts_with(&format!("{home}/")) {
                format!("~{}", &p[home.len()..])
            } else {
                p.clone()
            }
        })
        .collect();
    let names = if shown.len() == 1 {
        shown[0].clone()
    } else {
        format!(
            "{} and {}",
            shown[..shown.len() - 1].join(", "),
            shown[shown.len() - 1]
        )
    };
    format!("Looking at the files in {names}")
}

/// plans.py:291 `too_big`.
pub fn too_big(measured: &Inventory, shown: &dyn Fn(&str) -> String) -> String {
    let stopped = measured.stopped.as_deref().filter(|s| !s.is_empty());
    let place = match stopped {
        Some(s) => format!("{s} holds"),
        None => "The plan's folders hold".into(),
    };
    let whose = if stopped.is_some() {
        "a worker's context"
    } else {
        "every input step's context together"
    };
    let parts: Vec<String> = measured
        .parts
        .iter()
        .filter(|(_, n)| *n != 0)
        .map(|(name, n)| {
            let label = if stopped.is_none() {
                shown(name)
            } else if name.is_empty() {
                "(files directly in it)".into()
            } else {
                name.clone()
            };
            format!("{label}: at least {n}")
        })
        .collect();
    let unreached: Vec<String> = measured
        .unreached
        .iter()
        .map(|n| {
            if stopped.is_none() {
                shown(n)
            } else {
                n.clone()
            }
        })
        .collect();
    let joined = if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join("; ")
    };
    let unreached_line = if unreached.is_empty() {
        String::new()
    } else {
        format!("Subfolders not reached: {}. ", unreached.join(", "))
    };
    format!(
        "{place} more text than {whose} ({} tokens): FERB stopped counting at {} tokens, so it can't be handed over \
         whole. Text tokens counted by then, by subfolder: {joined}. {unreached_line}Hand over a smaller part (a \
         subfolder, the files that matter) or split it across steps.",
        measured.bound, measured.counted
    )
}

/// plans.py:308 `_text`: no NUL and valid UTF-8 in the first TEXT_PROBE bytes (a character cut at the end counts);
/// a file FERB can't open counts as text.
pub fn is_text(path: &str) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return true;
    };
    let mut head = Vec::with_capacity(TEXT_PROBE);
    if file
        .by_ref()
        .take(TEXT_PROBE as u64)
        .read_to_end(&mut head)
        .is_err()
    {
        return true;
    }
    if head.contains(&0) {
        return false;
    }
    match std::str::from_utf8(&head) {
        Ok(_) => true,
        // an incomplete character at the end is fine; an invalid byte is not
        Err(e) => e.error_len().is_none(),
    }
}

/// plans.py:325 `resolve_inputs` (ORCH-580): each do step's inputs in the inventory's own form, in place.
pub fn resolve_inputs(plan: &mut Plan, facts: &[FileFact], normal: &dyn Fn(&str) -> String) {
    let known: Vec<&str> = facts.iter().map(|f| f.path.as_str()).collect();
    let found: HashSet<&str> = known.iter().copied().collect();
    let inventoried = |value: &str| -> Vec<String> {
        let written = normal(value);
        let Ok(real) = pypath::resolve(&written) else {
            return vec![written];
        };
        if found.contains(real.as_str()) {
            return vec![real];
        }
        let prefix = format!("{}/", real.trim_end_matches('/'));
        let under: Vec<String> = known
            .iter()
            .filter(|p| p.starts_with(&prefix))
            .map(|p| p.to_string())
            .collect();
        if under.is_empty() {
            vec![written]
        } else {
            under
        }
    };
    for step in &mut plan.steps {
        if step.inputs.is_empty() {
            continue;
        }
        let mut out: Vec<String> = Vec::new();
        for value in &step.inputs {
            for path in inventoried(value) {
                if !out.contains(&path) {
                    out.push(path);
                }
            }
        }
        step.inputs = out;
    }
}

/// plans.py:350 `single`.
pub fn single(goal: &str) -> Plan {
    let mut step = Step::new(SINGLE, "do");
    step.goal = goal.to_string();
    Plan::of(vec![step])
}

/// plans.py:361 `step_label` (ORCH-626).
pub fn step_label(step: &Step) -> String {
    let shown = shown_name(Some(&step.label), &step.goal);
    if shown.is_empty() {
        step.id.clone()
    } else {
        shown
    }
}

/// plans.py:368 `steps_done`: (done, total) of a checklist `{"steps": [...], "states": {...}}`.
pub fn steps_done(checklist: &serde_json::Value) -> (usize, usize) {
    fn truthy(v: Option<&serde_json::Value>) -> Option<&serde_json::Value> {
        v.filter(|x| ferb_py::value::truthy(x))
    }
    let empty = serde_json::Map::new();
    let states = truthy(checklist.get("states"))
        .and_then(|v| v.as_object())
        .unwrap_or(&empty);
    let steps: Vec<serde_json::Value> = truthy(checklist.get("steps"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let done = steps
        .iter()
        .filter(|s| {
            let id = s.get("id").map(ferb_py::value::py_str).unwrap_or_default();
            states
                .get(&id)
                .filter(|x| ferb_py::value::truthy(x))
                .and_then(|x| x.get("status"))
                == Some(&serde_json::Value::String("done".into()))
        })
        .count();
    (done, steps.len())
}

/// plans.py:381 `card`: several asks open a card.
pub fn card(parts: &[Part]) -> bool {
    parts.len() >= 2
}

/// plans.py:387 `answer_steps`.
pub fn answer_steps(plan: Option<&Plan>, parts: &[Part]) -> Vec<String> {
    let named: Vec<String> = parts
        .iter()
        .filter_map(|p| p.answer_step.clone().filter(|s| !s.is_empty()))
        .collect();
    if !named.is_empty() {
        return named;
    }
    let Some(plan) = plan else {
        return vec![SINGLE.to_string()];
    };
    let needed: HashSet<&str> = plan
        .steps
        .iter()
        .flat_map(|s| s.depends_on.iter().map(String::as_str))
        .collect();
    plan.steps
        .iter()
        .filter(|s| !needed.contains(s.id.as_str()))
        .map(|s| s.id.clone())
        .collect()
}

/// `{step.id: step for step in plan.steps}`: first place, last value.
fn by_id(plan: &Plan) -> Vec<(&str, &Step)> {
    let mut out: Vec<(&str, &Step)> = Vec::new();
    for step in &plan.steps {
        match out.iter_mut().find(|(k, _)| *k == step.id) {
            Some(slot) => slot.1 = step,
            None => out.push((step.id.as_str(), step)),
        }
    }
    out
}

fn lookup<'a>(map: &[(&'a str, &'a Step)], key: &str) -> Option<&'a Step> {
    map.iter().find(|(k, _)| *k == key).map(|(_, s)| *s)
}

/// plans.py:400 `part_problems`.
pub fn part_problems(parts: &[Part], plan: Option<&Plan>, conditions: usize) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();
    let steps = plan.map(by_id).unwrap_or_default();
    if parts.is_empty() {
        if let Some(plan) = plan {
            if answer_steps(Some(plan), parts).len() != 1 {
                errors.push(
                    "A plan for one ask ends in exactly one step (no other step depends on it): its checked answer \
                     is the answer, and every other step leads to it. For several asks, list each in parts with its \
                     own answer_step."
                        .into(),
                );
            }
        }
        return errors;
    }
    let ids: Vec<&str> = parts.iter().map(|p| p.id.as_str()).collect();
    let unique: HashSet<&str> = ids.iter().copied().collect();
    if unique.len() != ids.len() || ids.iter().any(|i| strip(i).is_empty() || i.contains(':')) {
        errors.push("Part ids must be unique, nonempty and contain no colon.".into());
    }
    if parts.iter().all(|p| p.direct.is_some()) {
        errors
            .push("Every part is answered directly: answer with a reply instead of a task.".into());
    }
    // index (as text, ordered as Python's ints) -> parts
    let mut owners: Vec<(crate::models::PyInt, Vec<String>)> = Vec::new();
    let mut answering: Vec<(String, Vec<String>)> = Vec::new();
    for p in parts {
        if p.direct.is_none() == p.answer_step.is_none() {
            errors.push(format!(
                "part {}: give exactly one of direct (answered now) or answer_step.",
                p.id
            ));
        }
        if strip(&p.label).is_empty() {
            errors.push(format!("part {}: needs a short label for its row.", p.id));
        }
        if let Some(step) = &p.answer_step {
            match answering.iter_mut().find(|(k, _)| k == step) {
                Some(slot) => slot.1.push(p.id.clone()),
                None => answering.push((step.clone(), vec![p.id.clone()])),
            }
            match plan {
                None if step != SINGLE => errors.push(format!(
                    "part {}: a task without a plan answers with its one step, answer_step \"{SINGLE}\".",
                    p.id
                )),
                Some(_) if lookup(&steps, step).is_none() => errors.push(format!(
                    "part {}: answer_step {} is not a step of the plan.",
                    p.id,
                    repr_str(step)
                )),
                Some(_) => {
                    let kind = &lookup(&steps, step).expect("present").kind;
                    if kind == "wait" || kind == "ask" {
                        errors.push(format!("part {}: a {kind} step can't answer a part.", p.id));
                    }
                }
                None => {}
            }
            if p.conditions.is_empty() {
                errors.push(format!(
                    "part {}: name the success conditions its answer must meet (their indexes).",
                    p.id
                ));
            }
        } else if !p.conditions.is_empty() {
            errors.push(format!(
                "part {}: a part answered directly has no success conditions.",
                p.id
            ));
        }
        for index in &p.conditions {
            match owners.iter_mut().find(|(k, _)| k == index) {
                Some(slot) => slot.1.push(p.id.clone()),
                None => owners.push((index.clone(), vec![p.id.clone()])),
            }
        }
    }
    for (step_id, named) in &answering {
        if named.len() > 1 {
            errors.push(format!(
                "step {step_id} answers several parts ({}): each part has its own answer step.",
                named.join(", ")
            ));
        }
    }
    let mut sorted_owners = owners.clone();
    sorted_owners.sort_by(|a, b| a.0.cmp_int(&b.0));
    for (index, named) in &sorted_owners {
        if !index.in_range(conditions) {
            errors.push(format!(
                "parts {}: success condition {index} doesn't exist.",
                named.join(", ")
            ));
        } else if named.len() > 1 {
            errors.push(format!(
                "success condition {index} belongs to several parts ({}).",
                named.join(", ")
            ));
        }
    }
    let missing: Vec<String> = (0..conditions)
        .filter(|i| !owners.iter().any(|(k, _)| k.0 == i.to_string()))
        .map(|i| i.to_string())
        .collect();
    if !missing.is_empty() {
        errors.push(format!(
            "success conditions [{}] belong to no part: every condition belongs to exactly one.",
            missing.join(", ")
        ));
    }
    if plan.is_some() {
        let mut leads: HashSet<String> = answering
            .iter()
            .map(|(k, _)| k.clone())
            .filter(|k| lookup(&steps, k).is_some())
            .collect();
        loop {
            let before = leads.len();
            let more: Vec<String> = leads
                .iter()
                .flat_map(|k| lookup(&steps, k).expect("a step").depends_on.iter())
                .filter(|d| lookup(&steps, d).is_some())
                .cloned()
                .collect();
            leads.extend(more);
            if leads.len() <= before {
                break;
            }
        }
        let idle: Vec<&str> = steps
            .iter()
            .map(|(k, _)| *k)
            .filter(|k| !leads.contains(*k))
            .collect();
        if !idle.is_empty() {
            errors.push(format!(
                "steps {} lead to no part's answer: each part's answer is posted on its own once its answer step is \
                 checked, so no step sums up or combines them (FERB adds the done count).",
                repr_strs(&idle)
            ));
        }
    }
    errors
}

/// plans.py:466 `problems`.
pub fn problems(plan: &Plan, facts: &[FileFact], context_tokens: u64) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();
    let ids: Vec<&str> = plan.steps.iter().map(|s| s.id.as_str()).collect();
    let id_set: HashSet<&str> = ids.iter().copied().collect();
    let steps = by_id(plan);
    if id_set.len() != ids.len() || ids.iter().any(|x| x.is_empty() || x.contains(':')) {
        errors.push("Step ids must be unique, nonempty and contain no colon.".into());
    }
    for step in &plan.steps {
        let missing: Vec<&str> = step
            .depends_on
            .iter()
            .map(String::as_str)
            .filter(|d| !id_set.contains(d))
            .collect();
        if !missing.is_empty() {
            errors.push(format!(
                "{}: missing dependencies {}",
                step.id,
                repr_strs(&sorted_set(missing))
            ));
        }
        let until = step.until.as_deref().is_some_and(|u| !u.is_empty());
        let event = step.event.as_deref().is_some_and(|e| !e.is_empty());
        if step.kind == "wait" && until == event {
            errors.push(format!("{}: wait needs one exact time or event", step.id));
        }
        if until {
            let text = step.until.as_deref().unwrap_or("").replace('Z', "+00:00");
            let aware =
                ferb_py::time::DateTime::from_isoformat(&text).is_ok_and(|d| d.offset.is_some());
            if !aware {
                errors.push(format!(
                    "{}: wait time must be an ISO timestamp with timezone",
                    step.id
                ));
            }
        }
        if step.kind == "converse" && step.conversation.is_none() && step.depends_on.is_empty() {
            errors.push(format!(
                "{}: converse needs an exact account, app/site, thread, recipients, topic and hand-over condition",
                step.id
            ));
        }
        if step.kind == "ask" && strip(&step.question).is_empty() {
            errors.push(format!("{}: ask needs an owner-facing question", step.id));
        }
        if step.repeat.is_some() && step.kind != "do" {
            errors.push(format!("{}: only a do step repeats", step.id));
        }
        if !step.inputs.is_empty() && step.kind != "do" {
            errors.push(format!("{}: only a do step takes an input list", step.id));
        }
        if step.loops() && strip(&step.goal).is_empty() {
            errors.push(format!("{}: a repeating step needs a goal", step.id));
        }
        if step.loops() && until {
            errors.push(format!(
                "{}: a repeating step's end goes in repeat.end_when; until is a wait's time",
                step.id
            ));
        }
        for dependency in &step.depends_on {
            if let Some(source) = lookup(&steps, dependency) {
                if source.loops() && strip(source.end_when()).is_empty() {
                    errors.push(format!(
                        "{}: depends on repeating step {dependency}, which runs until the owner says stop, so it \
                         could never start",
                        step.id
                    ));
                }
            }
        }
    }
    // the dependency order: Kahn's walk over the steps by id
    let mut outstanding: Vec<(&str, HashSet<&str>)> = steps
        .iter()
        .map(|(k, s)| {
            (
                *k,
                s.depends_on
                    .iter()
                    .map(String::as_str)
                    .filter(|d| id_set.contains(d))
                    .collect(),
            )
        })
        .collect();
    let mut ready: VecDeque<&str> = outstanding
        .iter()
        .filter(|(_, d)| d.is_empty())
        .map(|(k, _)| *k)
        .collect();
    let mut visited: HashSet<&str> = HashSet::new();
    while let Some(key) = ready.pop_front() {
        if !visited.insert(key) {
            continue;
        }
        for (other, dependencies) in outstanding.iter_mut() {
            if dependencies.remove(key) && dependencies.is_empty() {
                ready.push_back(other);
            }
        }
    }
    let unvisited: Vec<&str> = id_set
        .iter()
        .copied()
        .filter(|k| !visited.contains(k))
        .collect();
    if !unvisited.is_empty() {
        errors.push(format!(
            "dependency cycle: {}",
            sorted_set(unvisited).join(", ")
        ));
    }
    // An input list: every inventoried input covered by exactly one do step, each step's inputs fitting its worker
    let mut known: BTreeMap<&str, &FileFact> = BTreeMap::new();
    for f in facts {
        known.insert(f.path.as_str(), f);
    }
    let listed: Vec<&Step> = plan.steps.iter().filter(|s| !s.inputs.is_empty()).collect();
    let mut assigned: Vec<&str> = Vec::new();
    for step in &listed {
        assigned.extend(step.inputs.iter().map(String::as_str));
        let unknown: Vec<&str> = step
            .inputs
            .iter()
            .map(String::as_str)
            .filter(|p| !known.contains_key(p))
            .collect();
        if !unknown.is_empty() {
            errors.push(format!(
                "{}: inputs absent from inventory: {}",
                step.id,
                repr_strs(&sorted_set(unknown))
            ));
        }
        let tokens: u64 = step
            .inputs
            .iter()
            .filter_map(|p| known.get(p.as_str()))
            .map(|f| f.tokens_upper_bound)
            .sum();
        if tokens > context_tokens {
            errors.push(format!(
                "{}: its inputs need up to {tokens} tokens; worker context has {context_tokens}",
                step.id
            ));
        }
    }
    if !listed.is_empty() {
        let assigned_set: HashSet<&str> = assigned.iter().copied().collect();
        let missing: Vec<&str> = known
            .keys()
            .copied()
            .filter(|p| !assigned_set.contains(p))
            .collect();
        if !missing.is_empty() {
            errors.push(format!(
                "unassigned inputs: {}",
                repr_strs(&sorted_set(missing))
            ));
        }
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for p in &assigned {
            *counts.entry(p).or_insert(0) += 1;
        }
        let repeated: Vec<&str> = counts
            .iter()
            .filter(|(_, n)| **n > 1)
            .map(|(p, _)| *p)
            .collect();
        if !repeated.is_empty() {
            errors.push(format!(
                "inputs assigned more than once: {}",
                repr_strs(&sorted_set(repeated))
            ));
        }
    }
    if plan.big_job
        && (strip(&plan.time_estimate).is_empty() || strip(&plan.cost_estimate).is_empty())
    {
        errors.push("A big job needs time and cost estimates before owner approval.".into());
    }
    if plan.steps.is_empty() {
        errors.push("A plan needs at least one step.".into());
    }
    errors
}
