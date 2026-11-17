//! ferb/orchestrator/cards.py: typed cards built from records FERB itself keeps (ORCH-329), never from classifying
//! text, and the readers of a tree's recorded calls and receipts the action checks use (ORCH-664, ORCH-666, ORCH-613,
//! ORCH-629).
//!
//! decision: ORCH-321, ORCH-329, ORCH-664, ORCH-666, ORCH-613, ORCH-629

use std::collections::BTreeSet;

use ferb_store::spans::tree_where;
use ferb_store::Db;
use serde_json::{json, Map, Value};

use crate::ports::Policy;
use crate::sql::{self, text, Result};

pub const PAY_RULE: &str = "floor:pay";
/// cards.py:40 `SEND_KINDS`.
pub const SEND_KINDS: &[&str] = &["send", "reply", "post", "mailbox_send", "call"];

/// cards.py:43 `event`.
#[allow(clippy::too_many_arguments)]
pub fn event(
    kind: &str,
    ident: &str,
    what: &str,
    when: &str,
    line: &str,
    repeat: Option<&str>,
    watch: Option<&str>,
    until: Option<&str>,
    started: Option<&str>,
) -> Value {
    let mut out = Map::new();
    for (k, v) in [
        ("type", "event"),
        ("kind", kind),
        ("id", ident),
        ("what", what),
        ("when", when),
        ("line", line),
    ] {
        out.insert(k.into(), json!(v));
    }
    for (k, v) in [
        ("repeat", repeat),
        ("watch", watch),
        ("until", until),
        ("started", started),
    ] {
        if let Some(v) = v.filter(|v| !v.is_empty()) {
            out.insert(k.into(), json!(v));
        }
    }
    Value::Object(out)
}

/// cards.py:52 `note`.
pub fn note(item_id: &str, text_: &str, state: &str, line: &str) -> Value {
    json!({"type": "note", "item_id": item_id, "text": text_, "state": state, "line": line})
}

/// cards.py:57 `links`.
pub fn links(sites: &[String], line: &str) -> Vec<Value> {
    sites
        .iter()
        .map(|host| json!({"type": "link", "host": host, "line": line}))
        .collect()
}

/// cards.py:62 `files`: only paths that exist now (`home` is what `~` expands to).
pub fn files(paths: &[String], home: &str) -> Vec<Value> {
    paths
        .iter()
        .filter(|p| !p.is_empty() && ferb_fs::pypath::exists(&ferb_fs::pypath::expanduser(p, home)))
        .map(|p| json!({"type": "file", "path": p, "name": ferb_fs::pypath::name(p)}))
        .collect()
}

fn tree_args(tree_id: &str) -> (&'static str, Vec<rusqlite::types::Value>) {
    let (w, p) = tree_where(tree_id);
    (w, p.iter().map(|x| text(x)).collect())
}

fn obj(v: Option<&Value>) -> Map<String, Value> {
    match v {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    }
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(ferb_py::value::truthy)
}

/// cards.py:75 `_calls`: the tree's tool calls that ran and finished without an error.
pub fn calls(db: &Db, tree_id: &str) -> Result<Vec<Map<String, Value>>> {
    let (where_, args) = tree_args(tree_id);
    let mut out = Vec::new();
    for row in sql::rows(
        db.conn(),
        &format!(
            "SELECT input_json, output_json FROM spans WHERE kind = 'tool.call' AND ended_at IS NOT NULL AND error IS NULL \
             AND {where_} ORDER BY started_at, rowid"
        ),
        &args,
    )? {
        let raw = sql::col(&row, "input_json");
        let mut data = obj(Some(&sql::loads(if raw.is_empty() { "{}" } else { &raw })?));
        if truthy(data.get("boundary_refused")) || truthy(data.get("ferb_question")) {
            continue;
        }
        let raw_out = sql::col(&row, "output_json");
        let output = obj(Some(&sql::loads(if raw_out.is_empty() { "{}" } else { &raw_out })?));
        let after = obj(output.get("after"));
        let changed = output.get("observed_change") == Some(&Value::Bool(true)) || after.get("changed") == Some(&Value::Bool(true));
        data.insert("observed_change".into(), Value::Bool(changed));
        out.push(data);
    }
    Ok(out)
}

/// A recorded call (as `calls` gives it) and its error, if it ended with one.
pub type Attempt = (Map<String, Value>, Option<String>);

/// ORCH-660: every tool call of the tree that finished, with or without an error (an error doesn't record that nothing
/// happened), but a question FERB asked and a call the boundary refused: (the call as `calls` gives it, its error).
pub fn attempted(db: &Db, tree_id: &str) -> Result<Vec<Attempt>> {
    let (where_, args) = tree_args(tree_id);
    let mut out = Vec::new();
    for row in sql::rows(
        db.conn(),
        &format!(
            "SELECT input_json, output_json, error FROM spans WHERE kind = 'tool.call' AND ended_at IS NOT NULL \
             AND {where_} ORDER BY started_at, rowid"
        ),
        &args,
    )? {
        let raw = sql::col(&row, "input_json");
        let mut data = obj(Some(&sql::loads(if raw.is_empty() { "{}" } else { &raw })?));
        if truthy(data.get("boundary_refused")) || truthy(data.get("ferb_question")) {
            continue;
        }
        let raw_out = sql::col(&row, "output_json");
        let output = obj(Some(&sql::loads(if raw_out.is_empty() { "{}" } else { &raw_out })?));
        let after = obj(output.get("after"));
        let changed = output.get("observed_change") == Some(&Value::Bool(true))
            || after.get("changed") == Some(&Value::Bool(true));
        data.insert("observed_change".into(), Value::Bool(changed));
        let error = row
            .get("error")
            .filter(|e| !e.is_null())
            .map(crate::py::str_of);
        out.push((data, error));
    }
    Ok(out)
}

/// rules.py:45 `declared_of`.
pub fn declared_of(recorded: Option<&Value>) -> BTreeSet<String> {
    match recorded {
        Some(Value::String(s)) if !s.is_empty() => [s.clone()].into(),
        Some(Value::Array(items)) => items.iter().map(crate::py::str_of).collect(),
        _ => BTreeSet::new(),
    }
}

/// `str(call.get("kind") or "")`.
fn own_kind(call: &Map<String, Value>) -> String {
    match call.get("kind") {
        Some(v) if ferb_py::value::truthy(v) => crate::py::str_of(v),
        _ => String::new(),
    }
}

fn fact_kinds(call: &Map<String, Value>) -> BTreeSet<String> {
    match call.get("fact_kinds") {
        Some(Value::Array(a)) => a.iter().map(crate::py::str_of).collect(),
        _ => BTreeSet::new(),
    }
}

/// cards.py:94 `_kinds`.
pub fn kinds(call: &Map<String, Value>) -> BTreeSet<String> {
    let mut out = BTreeSet::from([own_kind(call)]);
    out.extend(declared_of(call.get("declared")));
    out.extend(fact_kinds(call));
    out
}

/// cards.py:236 `structural_kinds`.
pub fn structural_kinds(call: &Map<String, Value>) -> BTreeSet<String> {
    let mut out = BTreeSet::from([own_kind(call)]);
    out.extend(fact_kinds(call));
    out
}

fn host(policy: &dyn Policy, url: Option<&Value>) -> String {
    match url {
        Some(v) if ferb_py::value::truthy(v) => {
            let s = crate::py::str_of(v);
            ferb_py::url::hostname(&s)
                .ok()
                .flatten()
                .unwrap_or_default()
        }
        _ => {
            let _ = policy;
            String::new()
        }
    }
}

/// cards.py:107 `_yes_of`.
fn yes_of(call: &Map<String, Value>) -> Vec<String> {
    let mut ids: Vec<Value> = match call.get("confirmations") {
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    };
    ids.push(
        obj(call.get("payment"))
            .get("covered_by")
            .cloned()
            .unwrap_or(Value::Null),
    );
    ids.iter()
        .filter(|i| ferb_py::value::truthy(i))
        .map(crate::py::str_of)
        .collect()
}

fn pay_granted(db: &Db, calls: &[Map<String, Value>]) -> Result<BTreeSet<String>> {
    let ids: BTreeSet<String> = calls.iter().flat_map(yes_of).collect();
    if ids.is_empty() {
        return Ok(BTreeSet::new());
    }
    let marks = vec!["?"; ids.len()].join(",");
    let mut args = vec![text(PAY_RULE)];
    args.extend(ids.iter().map(|i| text(i)));
    Ok(sql::rows(
        db.conn(),
        &format!("SELECT id FROM confirmations WHERE rule = ? AND granted = 1 AND id IN ({marks})"),
        &args,
    )?
    .iter()
    .map(|r| sql::col(r, "id"))
    .collect())
}

/// cards.py:122 `purchase` (ORCH-629): (site, tab chain).
pub fn purchase(policy: &dyn Policy, call: &Map<String, Value>) -> (String, String) {
    let facts = obj(call.get("facts"));
    let target_site = obj(call.get("target")).get("site").cloned();
    let site = match target_site.filter(ferb_py::value::truthy) {
        Some(v) => crate::py::str_of(&v),
        None => host(policy, facts.get("page_url")),
    };
    let chain = match facts.get("tab_chain").filter(|v| ferb_py::value::truthy(v)) {
        Some(v) => crate::py::str_of(v),
        None => match obj(call.get("payment"))
            .get("scope")
            .filter(|v| ferb_py::value::truthy(v))
        {
            Some(v) => crate::py::str_of(v),
            None => String::new(),
        },
    };
    (policy.site_key(Some(&site)), chain)
}

/// cards.py:130 `withdrawn` (ORCH-629).
pub fn withdrawn(
    db: &Db,
    policy: &dyn Policy,
    tree_id: &str,
) -> Result<BTreeSet<(String, String)>> {
    let (where_, args) = tree_args(tree_id);
    let mut last: Vec<((String, String), Map<String, Value>)> = Vec::new();
    for row in sql::rows(
        db.conn(),
        &format!("SELECT input_json FROM spans WHERE kind = 'tool.call' AND ended_at IS NOT NULL AND {where_} ORDER BY started_at, rowid"),
        &args,
    )? {
        let raw = sql::col(&row, "input_json");
        let data = obj(Some(&sql::loads(if raw.is_empty() { "{}" } else { &raw })?));
        if truthy(data.get("ferb_question")) || !kinds(&data).contains("pay") {
            continue;
        }
        if truthy(data.get("boundary_refused")) && !(truthy(data.get("declined")) || truthy(data.get("confirmations"))) {
            continue;
        }
        let key = purchase(policy, &data);
        match last.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = data,
            None => last.push((key, data)),
        }
    }
    Ok(last
        .into_iter()
        .filter(|(_, c)| truthy(c.get("boundary_refused")))
        .map(|(k, _)| k)
        .collect())
}

/// cards.py:150 `payments`: one expense card per purchase the owner said Yes to that then ran.
pub fn payments(db: &Db, policy: &dyn Policy, tree_id: &str) -> Result<Vec<Value>> {
    let all: Vec<Map<String, Value>> = calls(db, tree_id)?
        .into_iter()
        .filter(|c| kinds(c).contains("pay"))
        .collect();
    let granted = pay_granted(db, &all)?;
    if granted.is_empty() {
        return Ok(Vec::new());
    }
    let gone = withdrawn(db, policy, tree_id)?;
    let mut by: Vec<(String, Map<String, Value>)> = Vec::new();
    let mut of: Vec<(String, (String, String))> = Vec::new();
    for c in &all {
        let Some(yes) = yes_of(c).into_iter().find(|i| granted.contains(i)) else {
            continue;
        };
        let facts = obj(c.get("facts"));
        let target = obj(c.get("target"));
        if !of.iter().any(|(k, _)| *k == yes) {
            of.push((yes.clone(), purchase(policy, c)));
        }
        if !by.iter().any(|(k, _)| *k == yes) {
            let mut card = Map::new();
            card.insert("type".into(), json!("payment"));
            by.push((yes.clone(), card));
        }
        let card = &mut by.iter_mut().find(|(k, _)| *k == yes).expect("added").1;
        let site = {
            let h = host(policy, facts.get("page_url"));
            if h.is_empty() {
                target.get("site").cloned()
            } else {
                Some(Value::String(h))
            }
        };
        for (key, value) in [
            ("amount", facts.get("page_amount").cloned()),
            ("merchant", facts.get("page_merchant").cloned()),
            ("site", site),
            ("card", c.get("card_entry").cloned()),
        ] {
            if let Some(v) = value.filter(ferb_py::value::truthy) {
                if !card.contains_key(key) {
                    card.insert(key.into(), Value::String(crate::py::str_of(&v)));
                }
            }
        }
    }
    // each purchase's last Yes
    let mut fin: Vec<((String, String), String)> = Vec::new();
    for (yes, key) in &of {
        match fin.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = yes.clone(),
            None => fin.push((key.clone(), yes.clone())),
        }
    }
    Ok(by
        .into_iter()
        .filter(|(yes, _)| {
            let key = &of.iter().find(|(k, _)| k == yes).expect("recorded").1;
            !gone.contains(key) && fin.iter().find(|(k, _)| k == key).map(|(_, y)| y) == Some(yes)
        })
        .map(|(_, card)| Value::Object(card))
        .collect())
}

/// cards.py:179 `sends`: one contact card per recipient and channel a send-class call reached.
pub fn sends(db: &Db, tree_id: &str) -> Result<Vec<Value>> {
    let mut out: Vec<((String, String), Value)> = Vec::new();
    for c in calls(db, tree_id)? {
        let found: BTreeSet<String> = kinds(&c)
            .into_iter()
            .filter(|k| SEND_KINDS.contains(&k.as_str()))
            .collect();
        let Some(first) = found.iter().next().cloned() else {
            continue;
        };
        let target = obj(c.get("target"));
        let mut recipients: Vec<String> = match target.get("recipients") {
            Some(Value::Array(a)) => a.iter().map(crate::py::str_of).collect(),
            _ => Vec::new(),
        };
        if truthy(target.get("contact")) {
            recipients.push(crate::py::str_of(&target["contact"]));
        }
        let site = match target.get("site") {
            Some(v) if ferb_py::value::truthy(v) => crate::py::str_of(v),
            _ => String::new(),
        };
        let tool = match c.get("tool") {
            Some(v) if ferb_py::value::truthy(v) => crate::py::str_of(v),
            _ => String::new(),
        };
        let place: Option<(&str, String)> = if tool.starts_with("mac.") && !site.is_empty() {
            Some(("app", site.clone()))
        } else if !site.is_empty() {
            Some(("site", site.clone()))
        } else {
            None
        };
        let list = if recipients.is_empty() {
            vec![String::new()]
        } else {
            recipients
        };
        for recipient in list {
            if recipient.is_empty() && place.is_none() {
                continue;
            }
            let key = (recipient.clone(), site.clone());
            if out.iter().any(|(k, _)| *k == key) {
                continue;
            }
            let mut card = Map::new();
            card.insert("type".into(), json!("send"));
            card.insert("kind".into(), json!(first));
            if !recipient.is_empty() {
                card.insert("recipient".into(), json!(recipient));
            }
            if let Some((k, v)) = &place {
                card.insert((*k).into(), json!(v));
            }
            if truthy(target.get("account")) {
                card.insert(
                    "account".into(),
                    json!(crate::py::str_of(&target["account"])),
                );
            }
            out.push((key, Value::Object(card)));
        }
    }
    Ok(out.into_iter().map(|(_, v)| v).collect())
}

/// cards.py:204 `ran`.
pub fn ran(db: &Db, tree_id: &str, want: &[&str]) -> Result<Vec<Map<String, Value>>> {
    Ok(calls(db, tree_id)?
        .into_iter()
        .filter(|c| kinds(c).iter().any(|k| want.contains(&k.as_str())))
        .collect())
}

/// cards.py:210 `tried` (ORCH-613).
pub fn tried(db: &Db, tree_id: &str, want: &[&str]) -> Result<Vec<Map<String, Value>>> {
    let (where_, args) = tree_args(tree_id);
    let mut out = Vec::new();
    for row in sql::rows(
        db.conn(),
        &format!("SELECT input_json FROM spans WHERE kind = 'tool.call' AND ended_at IS NOT NULL AND {where_} ORDER BY started_at, rowid"),
        &args,
    )? {
        let raw = sql::col(&row, "input_json");
        let data = obj(Some(&sql::loads(if raw.is_empty() { "{}" } else { &raw })?));
        if truthy(data.get("ferb_question")) || (truthy(data.get("boundary_refused")) && !truthy(data.get("declined"))) {
            continue;
        }
        if kinds(&data).iter().any(|k| want.contains(&k.as_str())) {
            out.push(data);
        }
    }
    Ok(out)
}

/// cards.py:228 `approved`.
pub fn approved(db: &Db, call: &Map<String, Value>) -> Result<bool> {
    let ids: BTreeSet<String> = match call.get("confirmations") {
        Some(Value::Array(a)) => a
            .iter()
            .filter(|i| ferb_py::value::truthy(i))
            .map(crate::py::str_of)
            .collect(),
        _ => BTreeSet::new(),
    };
    if ids.is_empty() {
        return Ok(false);
    }
    let marks = vec!["?"; ids.len()].join(",");
    let args: Vec<_> = ids.iter().map(|i| text(i)).collect();
    Ok(sql::row(
        db.conn(),
        &format!("SELECT 1 FROM confirmations WHERE granted = 1 AND id IN ({marks}) LIMIT 1"),
        &args,
    )?
    .is_some())
}

/// cards.py:242 `paid`.
pub fn paid(db: &Db, policy: &dyn Policy, tree_id: &str) -> Result<Vec<Map<String, Value>>> {
    let all = ran(db, tree_id, &["pay"])?;
    let granted = pay_granted(db, &all)?;
    if granted.is_empty() {
        return Ok(Vec::new());
    }
    let gone = withdrawn(db, policy, tree_id)?;
    Ok(all
        .into_iter()
        .filter(|c| {
            !gone.contains(&purchase(policy, c)) && yes_of(c).iter().any(|i| granted.contains(i))
        })
        .collect())
}

/// cards.py:253 `downloaded`.
pub fn downloaded(db: &Db, tree_id: &str) -> Result<Vec<String>> {
    let (where_, args) = tree_args(tree_id);
    Ok(sql::rows(
        db.conn(),
        &format!("SELECT path FROM downloads WHERE state = 'taken' AND path IS NOT NULL AND {where_} ORDER BY at, rowid"),
        &args,
    )?
    .iter()
    .map(|r| crate::py::str_of(&r["path"]))
    .collect())
}

/// cards.py:261 `target_path`.
pub fn target_path(call: &Map<String, Value>) -> String {
    match obj(call.get("target")).get("path") {
        Some(v) if ferb_py::value::truthy(v) => crate::py::str_of(v),
        _ => String::new(),
    }
}

/// ORCH-666: one finished tool call of a step as recorded, with what its tool wrote when it acted (its output: a
/// receipt). Calls the boundary refused and FERB's own questions are left out, as `attempted` leaves them.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub task_id: String,
    pub rowid: i64,
    pub input: Map<String, Value>,
    pub output: Map<String, Value>,
    pub error: Option<String>,
}

/// The kinds of call that can carry a file's bytes from the Mac other than FERB's upload: input through the Mac tools,
/// a script, a shell command.
pub const BYTE_ROUTES: &[&str] = &["mac_input", "applescript", "run_shell"];

impl Recorded {
    pub fn tool(&self) -> String {
        own_str(self.input.get("tool"))
    }

    pub fn target(&self) -> Map<String, Value> {
        obj(self.input.get("target"))
    }

    /// The site (or, for the Mac tools, the app) the call was aimed at.
    pub fn site(&self) -> Option<String> {
        self.target()
            .get("site")
            .filter(|v| ferb_py::value::truthy(v))
            .map(crate::py::str_of)
    }

    /// The browser tab alias it acted in.
    pub fn tab(&self) -> Option<String> {
        obj(self.input.get("args"))
            .get("tab")
            .filter(|v| ferb_py::value::truthy(v))
            .map(crate::py::str_of)
    }

    /// Its own kind and the structural facts' kinds, never the worker's declaration.
    pub fn structural(&self) -> BTreeSet<String> {
        structural_kinds(&self.input)
    }

    /// False when the tool's own result says it didn't act (`performed: false`).
    pub fn performed(&self) -> bool {
        self.output.get("performed") != Some(&Value::Bool(false))
    }

    /// Recorded as more than a read (its kind isn't `read`) and not one of FERB's context reads.
    pub fn acting(&self) -> bool {
        let kind = own_kind(&self.input);
        !kind.is_empty()
            && kind != "read"
            && !crate::evidence::CONTEXT_READS.contains(&self.tool().as_str())
            && self.performed()
    }

    /// A form submit was seen: predicted before the act (the call's facts or kind) or reported by the page during it.
    pub fn submit_seen(&self) -> bool {
        let said = |m: &Map<String, Value>| {
            obj(m.get("facts")).get("submits_form") == Some(&Value::Bool(true))
        };
        self.structural().contains("submit") || said(&self.input) || said(&self.output)
    }

    /// FERB's upload tool's receipt: each file's (path, SHA-256) as read when it was handed to the page.
    pub fn uploaded_files(&self) -> Option<Vec<(String, String)>> {
        if self.output.get("source").and_then(Value::as_str) != Some("local") {
            return None;
        }
        let files = self.output.get("files")?.as_array()?;
        Some(
            files
                .iter()
                .map(|f| (own_str(f.get("path")), own_str(f.get("sha256"))))
                .filter(|(p, _)| !p.is_empty())
                .collect(),
        )
    }

    /// A call by which a file's bytes could have left the Mac without FERB's upload tool.
    pub fn carries_bytes(&self) -> bool {
        let kinds = self.structural();
        BYTE_ROUTES.iter().any(|k| kinds.contains(*k)) || self.tool().starts_with("shell.")
    }
}

fn own_str(v: Option<&Value>) -> String {
    match v {
        Some(v) if ferb_py::value::truthy(v) => crate::py::str_of(v),
        _ => String::new(),
    }
}

/// ORCH-666: the finished tool calls of a task's tree, in order, with their outputs.
pub fn recorded(db: &Db, tree_id: &str) -> Result<Vec<Recorded>> {
    let (where_, args) = tree_args(tree_id);
    let mut out = Vec::new();
    for row in sql::rows(
        db.conn(),
        &format!(
            "SELECT rowid AS n, task_id, input_json, output_json, error FROM spans WHERE kind = 'tool.call' \
             AND ended_at IS NOT NULL AND {where_} ORDER BY started_at, rowid"
        ),
        &args,
    )? {
        let raw = sql::col(&row, "input_json");
        let input = obj(Some(&sql::loads(if raw.is_empty() { "{}" } else { &raw })?));
        if truthy(input.get("boundary_refused")) || truthy(input.get("ferb_question")) {
            continue;
        }
        let raw_out = sql::col(&row, "output_json");
        let output = obj(Some(&sql::loads(if raw_out.is_empty() { "{}" } else { &raw_out })?));
        out.push(Recorded {
            task_id: sql::col(&row, "task_id"),
            rowid: row.get("n").and_then(Value::as_i64).unwrap_or(0),
            input,
            output,
            error: row
                .get("error")
                .filter(|e| !e.is_null())
                .map(crate::py::str_of),
        });
    }
    Ok(out)
}

/// ORCH-666: a task tree's taken downloads: (where the file was put, its SHA-256 when recorded).
pub fn taken_downloads(db: &Db, tree_id: &str) -> Result<Vec<(String, Option<String>)>> {
    let (where_, args) = tree_args(tree_id);
    Ok(sql::rows(
        db.conn(),
        &format!("SELECT path, sha256 FROM downloads WHERE state = 'taken' AND path IS NOT NULL AND {where_} ORDER BY at, rowid"),
        &args,
    )?
    .iter()
    .map(|r| {
        (
            crate::py::str_of(&r["path"]),
            r.get("sha256").and_then(Value::as_str).map(String::from),
        )
    })
    .collect())
}
