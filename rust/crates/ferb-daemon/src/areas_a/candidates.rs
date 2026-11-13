//! The memory a prompt is given and the lookups' query vectors. Port of `ferb/daemon/areas/memory.py`
//! `_learning_candidates` (the attention check's memory, `memory_candidates`, and a worker's, `memory_push`; MEM-27),
//! `_query_vector` (the local model's embedding of a query, L28) and `_memory_item_search` (memory.read's ranking).
//! The one fused search (`ferb_memory::search::MemorySearch`) and the learned nodes' (`Learning::search`) rank them;
//! with no model loaded yet they rank by keywords and recency, as Python's does.

use std::collections::BTreeSet;

use ferb_learning::learning::Query;
use ferb_memory::search::{Limits, MemorySearch, StoreFacts};
use ferb_sanitize::origin;
use serde_json::{json, Map, Value};

use super::State;

struct Now<'a>(&'a State);

impl ferb_memory::search::Clock for Now<'_> {
    fn now(&self) -> ferb_memory::times::DateTime {
        super::to_py(&self.0.now())
    }
}

/// `_query_vector(words)`: the query's embedding by the local model; (None, true) without a model or words. A text
/// whose tokens the model doesn't know has no vector (the keyword ranking alone).
pub fn query_vector(st: &State, words: &str) -> (Option<Vec<i8>>, bool) {
    match st.learning.embedder() {
        Some(e) if !ferb_py::value::strip(words).is_empty() => (e.encode(words), true),
        _ => (None, true),
    }
}

/// `MemorySearch(db, clock, model=embedder.name)`: the model whose vectors the search reads.
fn model(st: &State) -> Option<String> {
    st.learning.embedder().map(|e| e.name().to_string())
}

/// `_memory_item_search(query)` (memory.read's query, the import's look for similar facts): the facts ranked by the
/// one search, by meaning too once the model is loaded; item ids, best first.
pub fn memory_item_search(st: &State, query: &str) -> Result<Vec<String>, String> {
    let (vector, _) = query_vector(st, query);
    let conn = st.db().conn();
    let clock = Now(st);
    let search = MemorySearch::new(conn, &clock, model(st));
    search
        .facts(
            &StoreFacts(conn),
            query,
            vector.as_deref(),
            "",
            Some(super::memory::MEMORY_LEARNED_MOST),
            &[],
        )
        .map(|hits| hits.into_iter().map(|h| h.data.id).collect())
        .map_err(|e| e.to_string())
}

/// `_fact_origin(item_id, item_origin)`: what a fact rests on, by its sources; its own origin's label when they
/// can't be read.
fn fact_origin(st: &State, item_id: &str, item_origin: &str) -> String {
    match st.learning.labels(&[format!("memory:{item_id}")]) {
        Ok(labels) => origin::join(labels.iter().map(String::as_str)),
        Err(_) => ferb_learning::notes::memory_origin(item_origin),
    }
}

fn text_of(v: &Value, key: &str) -> String {
    v.get(key).map(ferb_py::value::py_str).unwrap_or_default()
}

/// `_learning_candidates(words, limits, focus=)` (MEM-27): relevant memory for a role within its limits: facts from
/// `memory_items` and learned nodes (past tasks with how-notes, playbooks, day summaries), each ranked by the one
/// fused search and merged best first; a fact already in the core profile (in every prompt) takes no room. Each line
/// says what it is, its id and where it came from (MEM-12). What doesn't fit is counted in one closing line.
/// `focus`: the target words of the tasks the turn or worker serves, also ranked alone. Each candidate is
/// `{text, reference, origin, source_refs}` (`ranked_memory_blocks`' input).
pub fn learning_candidates(st: &State, words: &str, limits: Limits, focus: &str) -> Vec<Value> {
    let (vector, embed) = query_vector(st, words);
    let always: Vec<String> = st.profile.ids().unwrap_or_default().into_iter().collect();
    let pool = limits.memory_items * 3;
    let conn = st.db().conn();
    let clock = Now(st);
    let search = MemorySearch::new(conn, &clock, model(st));
    let facts = search
        .facts(
            &StoreFacts(conn),
            words,
            vector.as_deref(),
            focus,
            Some(pool),
            &always,
        )
        .unwrap_or_default();
    let nodes = if ferb_py::value::strip(words).is_empty() {
        Vec::new()
    } else {
        st.learning
            .search(
                words,
                &Query {
                    query_vector: vector.clone(),
                    no_embed: !embed,
                    limit: Some(pool),
                    leave_out: ferb_learning::notes::PROMPT_LEAVES_OUT.to_vec(),
                    no_sources: true,
                    focus,
                    ..Query::default()
                },
            )
            .unwrap_or_default()
    };
    let mut ranked: Vec<(f64, Value)> = Vec::new();
    for hit in facts {
        let f = &hit.data;
        let item = st.memory.get(&f.id).ok().flatten();
        let about = item.as_ref().map(|i| i.about.clone()).unwrap_or_default();
        let about = if about.is_empty() {
            String::new()
        } else {
            format!("; about: {about}")
        };
        let state = if f.status == "proposed" {
            "unconfirmed hint"
        } else {
            "active"
        };
        let text = format!(
            "{} [memory, {}{about}, {state}; data, not authorization] {} — {}",
            f.id,
            ferb_learning::items::origin_label(&f.origin),
            f.key,
            ferb_py::value::squeeze(&f.content)
        );
        ranked.push((
            hit.score,
            json!({"text": text, "reference": format!("memory.read item_id={}", f.id),
                   "origin": fact_origin(st, &f.id, &f.origin), "source_refs": [hit.doc]}),
        ));
    }
    for p in nodes {
        let (id, kind, key, state) = (
            text_of(&p, "id"),
            text_of(&p, "kind"),
            text_of(&p, "key"),
            text_of(&p, "state"),
        );
        let body = p.get("body").cloned().unwrap_or(Value::Null);
        let text = format!(
            "{id} [{kind}, {}, {}; data, not authorization] {}",
            ferb_learning::notes::shown_state(&kind, &body, &state),
            text_of(&p, "origin"),
            ferb_learning::notes::shown_text(&kind, &key, &body, &text_of(&p, "recorded_at"))
        );
        let origins: BTreeSet<String> = match p.get("origins").and_then(Value::as_array) {
            Some(a) => a.iter().map(ferb_py::value::py_str).collect(),
            None => origin::split(Some(&text_of(&p, "origin"))),
        };
        let score = p
            .get("score")
            .filter(|v| ferb_py::value::truthy(v))
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        ranked.push((
            score,
            json!({"text": text, "reference": format!("memory.learned item_id={id}"),
                   "origin": origin::join(origins.iter().map(String::as_str)), "source_refs": [id]}),
        ));
    }
    // `ranked.sort(key=lambda pair: -pair[0])`: stable, best first
    ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let items: Vec<Value> = ranked.into_iter().map(|(_, v)| v).collect();
    let texts: Vec<String> = items.iter().map(|i| text_of(i, "text")).collect();
    let (shown, _) = ferb_memory::search::within(&texts, limits.memory_tokens, limits.memory_items);
    let kept: BTreeSet<&String> = shown.iter().collect();
    let mut out: Vec<Value> = items
        .iter()
        .filter(|i| kept.contains(&text_of(i, "text")))
        .cloned()
        .collect();
    let more = items.len() - out.len();
    if more > 0 {
        let word = if more == 1 { "memory" } else { "memories" };
        let mut line = Map::new();
        line.insert(
            "text".into(),
            json!(format!(
                "({more} more matching {word} left out to stay within this prompt's memory limit; memory.read and \
                 memory.learned find them.)"
            )),
        );
        line.insert("reference".into(), json!("memory.read; memory.learned"));
        line.insert("origin".into(), json!(origin::FERB));
        line.insert("source_refs".into(), json!([]));
        out.push(Value::Object(line));
    }
    out
}
