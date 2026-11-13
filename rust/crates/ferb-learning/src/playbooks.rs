//! `playbooks.py` (MEM-29): verified how-notes of the same kind of work merged into one note, and the measure of
//! "gets better at repeated tasks". Code finds the candidates by goal similarity; one model call says same kind or
//! not and writes the steps; code checks every id.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use ferb_context::PromptRoot;
use ferb_memory::embeddings::{similarity, Embedder};
use ferb_memory::pystr;
use ferb_sanitize::origin;

use crate::calls::{self, learning_call, MakeRuntime, Spans};
use crate::learning::Learning;
use crate::notes::how_note;
use crate::py;
use crate::sql;
use crate::store::Store;
use crate::{Error, Result};

pub const PLAYBOOK_SIMILARITY: f64 = 0.5;
pub const MERGES_PER_PASS: usize = 3;
pub const GROUP_MOST: usize = 6;
pub const MADE_BY: &str = "model:playbook/1";
pub const DECLINED_KIND: &str = "memory.playbook_declined";

/// playbooks.py:32 `SCHEMA`.
pub fn schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false, "required": ["same_kind", "members", "title", "text"],
        "properties": {"same_kind": {"type": "boolean"}, "members": {"type": "array", "items": {"type": "string"}},
                       "title": {"type": "string"}, "text": {"type": "string"}}})
}

/// playbooks.py:38 `attempts`.
pub fn attempts(steps: &Value) -> i64 {
    let n = steps
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s.get("role") == Some(&json!("Worker")))
        .count() as i64;
    if n == 0 {
        1
    } else {
        n
    }
}

/// playbooks.py:43 `_group_key`.
pub fn group_key(task_ids: &[String]) -> String {
    let mut sorted = task_ids.to_vec();
    sorted.sort();
    py::sha256_hex(&ferb_py::json::dumps(&sorted))[..16].to_owned()
}

fn s(v: &Value, k: &str) -> String {
    v.get(k)
        .filter(|x| ferb_py::value::truthy(x))
        .map_or_else(String::new, py::py_str)
}

fn tasks_of(book: &Value) -> Vec<String> {
    book["body"]
        .get("tasks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(py::py_str)
        .collect()
}

/// playbooks.py:47 `verified_cards`.
pub fn verified_cards(learning: &Learning) -> Result<Vec<Value>> {
    Ok(learning
        .items(Some("episode"), false, false, &[])?
        .into_iter()
        .filter(|n| {
            let body = &n["body"];
            body.get("verified") == Some(&Value::Bool(true))
                && !pystr::strip(&s(body, "intent")).is_empty()
                && !how_note(body).is_empty()
        })
        .collect())
}

/// playbooks.py:58 `playbooks`.
pub fn playbooks(learning: &Learning) -> Result<Vec<Value>> {
    learning.items(Some("playbook"), false, false, &[])
}

fn goal(node: &Value) -> String {
    let body = &node["body"];
    pystr::strip(&format!("{} {}", s(body, "intent"), s(body, "request"))).to_owned()
}

/// One merge to ask about: an existing playbook (or none) and the verified runs to merge into it.
pub type Job = (Option<Value>, Vec<Value>);

/// playbooks.py:67 `candidates`.
pub fn candidates(learning: &Learning, embedder: Option<&dyn Embedder>) -> Result<Vec<Job>> {
    let cards = verified_cards(learning)?;
    let Some(embedder) = embedder.filter(|_| cards.len() >= 2) else {
        return Ok(Vec::new());
    };
    let mut vectors: BTreeMap<String, Option<Vec<i8>>> = BTreeMap::new();
    let mut by_task: Vec<(String, Value)> = Vec::new();
    for c in &cards {
        let key = py::py_str(&c["key"]);
        vectors.insert(key.clone(), embedder.encode(&goal(c)));
        match by_task.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = c.clone(),
            None => by_task.push((key, c.clone())),
        }
    }
    let books = playbooks(learning)?;
    let mut members: BTreeSet<String> = BTreeSet::new();
    for book in &books {
        members.extend(tasks_of(book));
    }
    let mut declined = BTreeSet::new();
    for r in learning.store.query(
        "SELECT data_json FROM journal WHERE kind = ?",
        sql![DECLINED_KIND],
    )? {
        let d = ferb_py::json::loads(&r.s_at(0)).map_err(Error::Json)?;
        declined.insert(d.get("group").map_or_else(|| "None".into(), py::py_str));
    }
    let near = |a: &str, b: &str| match (
        vectors.get(a).cloned().flatten(),
        vectors.get(b).cloned().flatten(),
    ) {
        (Some(va), Some(vb)) => similarity(&va, &vb) >= PLAYBOOK_SIMILARITY,
        _ => false,
    };
    let ended = |c: &Value| s(&c["body"], "ended_at");
    let mut ordered = by_task.clone();
    ordered.sort_by_key(|(_, c)| ended(c));
    let (mut jobs, mut taken): (Vec<Job>, BTreeSet<String>) = (Vec::new(), BTreeSet::new());
    for (task, card) in &ordered {
        if members.contains(task) || taken.contains(task) {
            continue;
        }
        let home = books.iter().find(|b| {
            py::py_str(&b["origin"]) != origin::OWNER && tasks_of(b).iter().any(|m| near(task, m))
        });
        if let Some(home) = home {
            let mut keys = tasks_of(home);
            keys.push(task.clone());
            if !declined.contains(&group_key(&keys)) {
                jobs.push((Some(home.clone()), vec![card.clone()]));
                taken.insert(task.clone());
            }
            continue;
        }
        let mut group = vec![card.clone()];
        for (t, other) in &by_task {
            if t != task && !members.contains(t) && !taken.contains(t) && near(task, t) {
                group.push(other.clone());
            }
        }
        if group.len() >= 2 {
            group.sort_by_key(|c| ended(c));
            let group: Vec<Value> = group[group.len().saturating_sub(GROUP_MOST)..].to_vec();
            let keys: Vec<String> = group.iter().map(|c| py::py_str(&c["key"])).collect();
            if !declined.contains(&group_key(&keys)) {
                taken.extend(keys);
                jobs.push((None, group));
            }
        }
    }
    Ok(jobs)
}

fn run_line(store: &Store, card: &Value) -> Result<String> {
    let body = &card["body"];
    let key = py::py_str(&card["key"]);
    let row = store.one(
        "SELECT seconds, steps_json FROM episodes WHERE task_id = ?",
        sql![key],
    )?;
    let (tries, seconds) = match &row {
        Some(r) => (
            attempts(&ferb_py::json::loads(&r.s("steps_json")).map_err(Error::Json)?),
            py::round_int(r.f("seconds")),
        ),
        None => (1, 0),
    };
    let squeeze = |t: String| pystr::split_ws(&t).collect::<Vec<_>>().join(" ");
    Ok(format!(
        "- task {key} ({}; {tries} attempt(s), {seconds} s)\n  request: {}\n  goal: {}\n  how-note: {}",
        pystr::prefix(&s(body, "ended_at"), 10),
        pystr::prefix(&squeeze(s(body, "request")), 400),
        squeeze(s(body, "intent")),
        how_note(body)
    ))
}

/// playbooks.py:121 `context`.
pub fn context(store: &Store, book: Option<&Value>, group: &[Value]) -> Result<String> {
    let mut lines = vec!["Verified past tasks:".to_owned()];
    for c in group {
        lines.push(run_line(store, c)?);
    }
    if let Some(book) = book {
        lines.push(String::new());
        lines.push(format!(
            "The playbook as it stands (from tasks {}):",
            tasks_of(book).join(", ")
        ));
        lines.push(format!(
            "title: {}",
            book["body"]
                .get("title")
                .map_or_else(|| "None".into(), py::py_str)
        ));
        lines.push(s(&book["body"], "text"));
    }
    Ok(lines.join("\n"))
}

/// playbooks.py:129 `apply`: members only from the runs shown; a new playbook needs two.
pub fn apply(
    learning: &Learning,
    book: Option<&Value>,
    group: &[Value],
    answer: &Value,
    record: &dyn Fn(&str, Value) -> Result<()>,
) -> Result<Option<Value>> {
    let shown: BTreeSet<String> = group.iter().map(|c| py::py_str(&c["key"])).collect();
    let named: Vec<String> = answer
        .get("members")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(py::py_str)
        .filter(|t| shown.contains(t))
        .collect();
    let old = book.map(tasks_of).unwrap_or_default();
    let mut tasks: Vec<String> = Vec::new();
    for t in old.iter().chain(&named) {
        if !tasks.contains(t) {
            tasks.push(t.clone());
        }
    }
    let same = answer.get("same_kind").is_some_and(ferb_py::value::truthy);
    if !same || named.is_empty() || tasks.len() < 2 || pystr::strip(&s(answer, "text")).is_empty() {
        let mut all = old.clone();
        all.extend(shown.iter().cloned());
        record(
            DECLINED_KIND,
            json!({"group": group_key(&all), "tasks": shown.iter().collect::<Vec<_>>()}),
        )?;
        return Ok(None);
    }
    let title = pystr::split_ws(&s(answer, "title"))
        .collect::<Vec<_>>()
        .join(" ");
    let title = pystr::prefix(&title, 120);
    let body = json!({"title": if title.is_empty() { "Repeated work" } else { title },
                      "text": pystr::prefix(pystr::strip(&py::py_str(&answer["text"])), 2000), "tasks": tasks});
    let key = match book {
        Some(b) => py::py_str(&b["key"]),
        None => format!("pb-{}", group_key(&tasks)),
    };
    let sources: Vec<String> = tasks.iter().map(|t| format!("episode:{t}")).collect();
    let node = learning.put(
        "playbook",
        &key,
        &body,
        &sources,
        origin::INFERRED,
        MADE_BY,
        None,
        "active",
        false,
        None,
        true,
    )?;
    record(
        "memory.playbook",
        json!({"id": node["id"], "key": key, "tasks": tasks, "rewritten": book.is_some()}),
    )?;
    Ok(Some(node))
}

/// playbooks.py:148 `merge`: one hourly pass's merges, at most MERGES_PER_PASS model calls.
#[allow(clippy::too_many_arguments)]
pub async fn merge(
    learning: &Learning,
    make_runtime: &MakeRuntime<'_>,
    prompts: &PromptRoot,
    embedder: Option<&dyn Embedder>,
    device_id: &str,
    spans: Option<&dyn Spans>,
    record: &dyn Fn(&str, Value) -> Result<()>,
    usage: &dyn Fn(&str, &Value),
) -> Result<Vec<String>> {
    let mut written = Vec::new();
    let mut jobs = candidates(learning, embedder)?;
    jobs.truncate(MERGES_PER_PASS);
    if jobs.is_empty() {
        return Ok(written);
    }
    let system = prompts
        .load("memory/playbook.md", &[])
        .map_err(|e| Error::Crash("OSError", e.to_string()))?;
    let schema = schema();
    for (book, group) in &jobs {
        let keys: Vec<String> = group.iter().map(|c| py::py_str(&c["key"])).collect();
        let ident = format!("playbook-{}", group_key(&keys));
        let task = calls::learning_task(&ident, "playbook merge", device_id, &ident);
        let made = make_runtime("worker", "supervisor", &[], &schema)?;
        let ctx = context(&learning.store, book.as_ref(), group)?;
        let text = learning_call(
            &made,
            &task,
            &system,
            &ctx,
            "playbook merge",
            &schema,
            "playbook",
            spans,
            usage,
        )
        .await?;
        let answer = calls::loads(&text)?;
        if answer.is_object() {
            if let Some(node) = apply(learning, book.as_ref(), group, &answer, record)? {
                written.push(py::py_str(&node["id"]));
            }
        }
    }
    Ok(written)
}

/// playbooks.py:171 `repeat_stats`: each current playbook's runs in order, whether each later run was shown the
/// playbook or an earlier run's how-note, and first-to-latest change. Read-only.
pub fn repeat_stats(store: &Store) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for r in store.query(
        "SELECT id, body_json FROM learning_nodes WHERE kind = 'playbook' AND valid_to IS \
         NULL AND state NOT IN ('retired','rolled_back') ORDER BY recorded_at",
        sql![],
    )? {
        let nid = r.s_at(0);
        let body = ferb_py::json::loads(&r.s_at(1)).map_err(Error::Json)?;
        let tasks: Vec<String> = body
            .get("tasks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(py::py_str)
            .collect();
        let mut runs: Vec<Value> = Vec::new();
        for task in &tasks {
            let Some(row) = store.one(
                "SELECT started_at, seconds, steps_json, verified FROM episodes WHERE task_id = ?",
                sql![task],
            )?
            else {
                continue;
            };
            let steps = row
                .opt("steps_json")
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "[]".into());
            runs.push(json!({"task_id": task, "started_at": crate::store::to_json(row.get("started_at")),
                             "attempts": attempts(&ferb_py::json::loads(&steps).map_err(Error::Json)?),
                             "seconds": ferb_memory::pymath::round(row.f("seconds"), 1),
                             "verified": row.i("verified") != 0}));
        }
        runs.sort_by_key(|r| {
            r.get("started_at")
                .filter(|v| ferb_py::value::truthy(v))
                .map_or_else(String::new, py::py_str)
        });
        let mut earlier: Vec<String> = Vec::new();
        for (n, run) in runs.iter_mut().enumerate() {
            let task = py::py_str(&run["task_id"]);
            let (wh, params) = ferb_store::spans::tree_where(&task);
            let p = || {
                params
                    .iter()
                    .map(|x| crate::store::Sql::Text(x.clone()))
                    .collect::<Vec<_>>()
            };
            let traced = store.exists(&format!("SELECT 1 FROM spans WHERE {wh} LIMIT 1"), p())?;
            let used = if !traced || n == 0 {
                Value::Null
            } else {
                let mut any = false;
                for r in std::iter::once(&nid).chain(earlier.iter()) {
                    let mut params = p();
                    params.push(crate::store::Sql::Text(r.clone()));
                    if store.exists(
                        &format!(
                            "SELECT 1 FROM spans WHERE {wh} AND kind IN ('context.exposure','context.source') \
                             AND instr(input_json, ?) > 0 LIMIT 1"
                        ),
                        params,
                    )? {
                        any = true;
                        break;
                    }
                }
                json!(any)
            };
            run["used_memory"] = used;
            earlier.extend(
                store
                    .query(
                        "SELECT id FROM learning_nodes WHERE kind = 'episode' AND key = ?",
                        sql![task],
                    )?
                    .iter()
                    .map(|r| r.s_at(0)),
            );
        }
        let change = if runs.len() >= 2 {
            let (first, last) = (&runs[0], &runs[runs.len() - 1]);
            let a =
                last["attempts"].as_i64().unwrap_or(0) - first["attempts"].as_i64().unwrap_or(0);
            let s = ferb_memory::pymath::round(
                last["seconds"].as_f64().unwrap_or(0.0) - first["seconds"].as_f64().unwrap_or(0.0),
                1,
            );
            Some(json!({"attempts": a, "seconds": s}))
        } else {
            None
        };
        let mut entry = json!({"id": nid, "title": body.get("title").cloned().unwrap_or(Value::Null), "runs": runs});
        if let Some(c) = change {
            entry["change"] = c;
        }
        out.push(entry);
    }
    Ok(out)
}
