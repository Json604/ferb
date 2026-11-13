//! The policy's stores, on ferb-store's `Db` (Mac-side, ENG-5): destinations FERB knows and the owner's recorded
//! confirmations (policy/destinations.py), standing send permissions (policy/standing.py), the daily security facts
//! (policy/security_digest.py) and the sites a task tree contacted (policy/sites.py). Rows are written byte for byte
//! as Python writes them.

use std::collections::BTreeSet;

use ferb_core::interfaces::{Clock, IdSource};
use ferb_core::types::{ActionSpec, Target};
use ferb_core::when;
use ferb_store::spans::tree_where;
use ferb_store::Db;
use rusqlite::{params, params_from_iter, OptionalExtension};
use serde_json::{json, Map, Value};

use crate::confirm::{account_key, site_key};
use crate::destinations::{stable, stored, Redact, HOW};
use crate::rules::Kind;
use crate::standing::{sends, SendScope};
use crate::urls;

pub type Result<T> = std::result::Result<T, rusqlite::Error>;

/// destinations.py:69 `KnownDestinations`.
pub struct KnownDestinations<'a> {
    pub db: &'a Db,
    pub clock: &'a dyn Clock,
    pub ids: &'a dyn IdSource,
    pub redact: &'a dyn Redact,
}

impl KnownDestinations<'_> {
    /// Known for normal use: named in a request, chosen in an answer or confirmed.
    pub fn known(&self, kind: &str, value: &str) -> Result<bool> {
        let Some(s) = stored(self.redact, kind, value) else {
            return Ok(false);
        };
        Ok(self
            .db
            .conn()
            .query_row(
                "SELECT 1 FROM known_destinations WHERE kind = ? AND value = ?",
                params![kind, s],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Known for the sink rule (GATE-205): chosen in an answer or confirmed; one only named in a request is not.
    pub fn approved(&self, kind: &str, value: &str) -> Result<bool> {
        let Some(s) = stored(self.redact, kind, value) else {
            return Ok(false);
        };
        Ok(self
            .db
            .conn()
            .query_row(
                "SELECT 1 FROM known_destinations WHERE kind = ? AND value = ? AND how IN ('answered', 'confirmed')",
                params![kind, s],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Keep a destination; one already known keeps how it first became known, except that the owner's approval of
    /// one only named before upgrades it (GATE-205).
    pub fn record(
        &self,
        kind: &str,
        value: &str,
        how: &str,
        request_id: Option<&str>,
        span_id: Option<&str>,
    ) -> Result<()> {
        let Some(s) = stored(self.redact, kind, value) else {
            return Ok(());
        };
        if !HOW.contains(&how) {
            return Ok(());
        }
        self.db.conn().execute(
            "INSERT INTO known_destinations (id, kind, value, how, request_id, span_id, at) VALUES (?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(kind, value) DO UPDATE SET how = excluded.how, request_id = excluded.request_id, \
             span_id = excluded.span_id, at = excluded.at \
             WHERE known_destinations.how = 'named' AND excluded.how != 'named'",
            params![self.ids.new_id(), kind, s, how, request_id, span_id, when::isoformat(&self.clock.now())],
        )?;
        Ok(())
    }
}

/// One confirmations row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmation {
    pub id: String,
    pub tree_id: String,
    pub rule: String,
    pub key: String,
    pub granted: bool,
    pub ask_span: Option<String>,
    pub used: bool,
    pub at: String,
}

/// destinations.py:126 `Confirmations`: the owner's Yes or No to FERB's question in one task tree, keyed by what it
/// covers; it carries across that tree's retries.
pub struct Confirmations<'a> {
    pub db: &'a Db,
    pub clock: &'a dyn Clock,
    pub ids: &'a dyn IdSource,
    pub redact: &'a dyn Redact,
}

impl Confirmations<'_> {
    /// The owner's latest answer in this task tree to the question `key` names (never for a key holding a secret);
    /// a No whose id is in `reopened` reads as unanswered.
    pub fn find(
        &self,
        tree_id: &str,
        key: &str,
        reopened: &[String],
    ) -> Result<Option<Confirmation>> {
        let Some(s) = stable(self.redact, key) else {
            return Ok(None);
        };
        let row = self
            .db
            .conn()
            .query_row(
                "SELECT id, tree_id, rule, key, granted, ask_span, used, at FROM confirmations WHERE tree_id = ? AND \
                 key = ? ORDER BY rowid DESC LIMIT 1",
                params![tree_id, s],
                |r| {
                    Ok(Confirmation {
                        id: r.get(0)?,
                        tree_id: r.get(1)?,
                        rule: r.get(2)?,
                        key: r.get(3)?,
                        granted: r.get::<_, i64>(4)? != 0,
                        ask_span: r.get(5)?,
                        used: r.get::<_, i64>(6)? != 0,
                        at: r.get(7)?,
                    })
                },
            )
            .optional()?;
        Ok(row.filter(|r| r.granted || !reopened.contains(&r.id)))
    }

    /// The ids of this tree's answers that are each question's latest and a No.
    pub fn noes(&self, tree_id: &str) -> Result<Vec<String>> {
        let mut stmt = self.db.conn().prepare(
            "SELECT id, key, granted FROM confirmations WHERE tree_id = ? ORDER BY rowid",
        )?;
        let rows = stmt.query_map(params![tree_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        let mut latest: Vec<(String, String, i64)> = Vec::new();
        for row in rows {
            let (id, key, granted) = row?;
            match latest.iter_mut().find(|(_, k, _)| *k == key) {
                Some(slot) => *slot = (id, key, granted),
                None => latest.push((id, key, granted)),
            }
        }
        Ok(latest
            .into_iter()
            .filter(|(_, _, g)| *g == 0)
            .map(|(id, _, _)| id)
            .collect())
    }

    /// The keys whose latest answer in this task tree is No.
    pub fn refused(&self, tree_id: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .db
            .conn()
            .prepare("SELECT key, granted FROM confirmations WHERE tree_id = ? ORDER BY rowid")?;
        let rows = stmt.query_map(params![tree_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        let mut latest: Vec<(String, i64)> = Vec::new();
        for row in rows {
            let (key, granted) = row?;
            match latest.iter_mut().find(|(k, _)| *k == key) {
                Some(slot) => slot.1 = granted,
                None => latest.push((key, granted)),
            }
        }
        Ok(latest
            .into_iter()
            .filter(|(_, g)| *g == 0)
            .map(|(k, _)| k)
            .collect())
    }

    /// Whether row `cid` is a Yes to the question `key` names (never for a key holding a secret).
    pub fn covers(&self, cid: &str, key: &str) -> Result<bool> {
        let Some(s) = stable(self.redact, key) else {
            return Ok(false);
        };
        Ok(self
            .db
            .conn()
            .query_row(
                "SELECT 1 FROM confirmations WHERE id = ? AND key = ? AND granted = 1",
                params![cid, s],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// The owner's answer, keyed by its stable form; a key holding a specific secret is stored redacted (it never
    /// matches again, so FERB asks again).
    pub fn record(
        &self,
        tree_id: &str,
        rule: &str,
        key: &str,
        granted: bool,
        ask_span: Option<&str>,
    ) -> Result<String> {
        let cid = self.ids.new_id();
        let stored_key = stable(self.redact, key).unwrap_or_else(|| self.redact.redact(key));
        self.db.conn().execute(
            "INSERT INTO confirmations (id, tree_id, rule, key, granted, ask_span, used, at) VALUES (?, ?, ?, ?, ?, ?, 0, ?)",
            params![cid, tree_id, rule, stored_key, i64::from(granted), ask_span, when::isoformat(&self.clock.now())],
        )?;
        Ok(cid)
    }

    /// Seconds since that answer was recorded (None when its time can't be read).
    pub fn age_s(&self, row: &Confirmation) -> Option<f64> {
        let at = when::parse(&row.at)?;
        let d = self.clock.now() - at;
        Some(
            d.num_microseconds()
                .map_or(d.num_seconds() as f64, |us| us as f64 / 1e6),
        )
    }

    /// A Yes that covers one run only is claimed by the call about to run.
    pub fn use_(&self, cid: &str) -> Result<()> {
        self.db.conn().execute(
            "UPDATE confirmations SET used = 1 WHERE id = ?",
            params![cid],
        )?;
        Ok(())
    }

    /// The claiming call never made its attempt, or failed: the Yes covers its retry again.
    pub fn release(&self, cid: &str) -> Result<()> {
        self.db.conn().execute(
            "UPDATE confirmations SET used = 0 WHERE id = ?",
            params![cid],
        )?;
        Ok(())
    }
}

/// policy/standing.py `StandingPermissions`: tree-scoped, exact sends.
pub struct StandingPermissions<'a> {
    pub db: &'a Db,
}

impl StandingPermissions<'_> {
    pub fn grant(&self, task_id: &str, scopes: &[SendScope], approval_id: &str) -> Result<()> {
        let each: Vec<(SendScope, String)> = scopes
            .iter()
            .map(|s| (s.clone(), approval_id.to_string()))
            .collect();
        self.grant_each(task_id, &each)
    }

    /// The task's scopes, each under the statement (approval id) it is granted under.
    pub fn grant_each(&self, task_id: &str, granted: &[(SendScope, String)]) -> Result<()> {
        for (index, (scope, approval_id)) in granted.iter().enumerate() {
            self.db.conn().execute(
                "INSERT OR REPLACE INTO standing_permissions VALUES(?,?,?,?,1)",
                params![
                    format!("{task_id}:{index}"),
                    task_id,
                    scope.dump_json(),
                    approval_id
                ],
            )?;
        }
        Ok(())
    }

    /// Whether a scope covers one need of a send-class call (GATE-221): the call as declared (tool, identity,
    /// account, content) and the need's own target (`reaches`), else the call's target.
    pub fn covers(
        &self,
        task_id: &str,
        capability: &str,
        identity: &str,
        spec: &ActionSpec,
        reaches: Option<&Target>,
    ) -> Result<bool> {
        if !sends(spec) {
            return Ok(false);
        }
        let target = reaches.unwrap_or(&spec.target);
        let mut recipients: Vec<String> = target
            .recipients
            .iter()
            .map(|r| account_key(Some(r)))
            .collect();
        recipients.sort();
        let mut stmt = self
            .db
            .conn()
            .prepare("SELECT scope FROM standing_permissions WHERE task_id=? AND active=1")?;
        let rows: Vec<String> = stmt
            .query_map(params![task_id], |r| r.get(0))?
            .collect::<Result<_>>()?;
        for row in rows {
            let Ok(scope) = serde_json::from_str::<Value>(&row) else {
                continue;
            };
            let text = |k: &str| scope.get(k).and_then(Value::as_str);
            let mut scope_recipients: Vec<String> = scope
                .get("recipients")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|r| account_key(r.as_str())).collect())
                .unwrap_or_default();
            scope_recipients.sort();
            let content_ok = match scope.get("content") {
                Some(Value::Null) => true,
                Some(c) => {
                    spec.content.as_deref().is_some_and(|x| !x.is_empty())
                        && c.as_str() == spec.content.as_deref()
                }
                None => false,
            };
            let has_place = scope
                .get("recipients")
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty())
                || text("site").is_some_and(|s| !s.is_empty());
            if text("capability") == Some(capability)
                && text("identity") == Some(identity)
                && account_key(text("account")) == account_key(spec.target.account.as_deref())
                && site_key(text("site")) == site_key(target.site.as_deref())
                && scope_recipients == recipients
                && content_ok
                && has_place
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The task's scopes in force, in their stated order.
    pub fn scopes(&self, task_id: &str) -> Result<Vec<SendScope>> {
        Ok(self
            .in_force(task_id)?
            .into_iter()
            .map(|(s, _)| s)
            .collect())
    }

    /// The task's scopes in force, each with the statement it was granted under.
    pub fn in_force(&self, task_id: &str) -> Result<Vec<(SendScope, String)>> {
        let mut stmt = self.db.conn().prepare(
            "SELECT scope, approval_id FROM standing_permissions WHERE task_id=? AND active=1 ORDER BY rowid",
        )?;
        let rows = stmt.query_map(params![task_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (scope, approval) = row?;
            if let Ok(s) = serde_json::from_str::<SendScope>(&scope) {
                out.push((s, approval));
            }
        }
        Ok(out)
    }

    /// The statements the task's scopes in force were granted under.
    pub fn approvals(&self, task_id: &str) -> Result<Vec<String>> {
        let mut stmt = self.db.conn().prepare(
            "SELECT DISTINCT approval_id FROM standing_permissions WHERE task_id=? AND active=1 ORDER BY approval_id",
        )?;
        let out = stmt.query_map(params![task_id], |r| r.get(0))?.collect();
        out
    }

    /// Whether any task still holds a scope granted under this statement.
    pub fn held(&self, approval_id: &str) -> Result<bool> {
        Ok(self
            .db
            .conn()
            .query_row(
                "SELECT 1 FROM standing_permissions WHERE approval_id=? AND active=1 LIMIT 1",
                params![approval_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn revoke(&self, task_id: &str) -> Result<()> {
        self.db.conn().execute(
            "UPDATE standing_permissions SET active=0 WHERE task_id=?",
            params![task_id],
        )?;
        Ok(())
    }

    /// Withdraw every scope granted under one statement, from every task holding it; returns those tasks.
    pub fn revoke_approval(&self, approval_id: &str) -> Result<Vec<String>> {
        let tasks: Vec<String> = {
            let mut stmt = self.db.conn().prepare(
                "SELECT DISTINCT task_id FROM standing_permissions WHERE approval_id=? AND active=1 ORDER BY task_id",
            )?;
            let out = stmt
                .query_map(params![approval_id], |r| r.get(0))?
                .collect::<Result<_>>()?;
            out
        };
        self.db.conn().execute(
            "UPDATE standing_permissions SET active=0 WHERE approval_id=?",
            params![approval_id],
        )?;
        Ok(tasks)
    }
}

fn row_dict(r: &rusqlite::Row<'_>, names: &[&str]) -> Result<Value> {
    let mut m = Map::new();
    for (i, n) in names.iter().enumerate() {
        let v = match r.get_ref(i)? {
            rusqlite::types::ValueRef::Null => Value::Null,
            rusqlite::types::ValueRef::Integer(x) => json!(x),
            rusqlite::types::ValueRef::Real(x) => json!(x),
            rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
            rusqlite::types::ValueRef::Blob(b) => json!(String::from_utf8_lossy(b)),
        };
        m.insert(n.to_string(), v);
    }
    Ok(Value::Object(m))
}

fn loads_or_empty(text: Option<String>) -> Value {
    match text.filter(|t| !t.is_empty()) {
        Some(t) => ferb_py::json::loads(&t).unwrap_or(Value::Null),
        None => json!({}),
    }
}

/// policy/security_digest.py `build`: S10's code-built daily security facts.
pub fn security_digest(db: &Db, day: &str) -> Result<Value> {
    let c = db.conn();
    let collect = |sql: &str, names: &[&str]| -> Result<Vec<Value>> {
        let mut stmt = c.prepare(sql)?;
        let rows = stmt.query_map(params![day], |r| row_dict(r, names))?;
        rows.collect()
    };
    let destinations = collect(
        "SELECT kind, value, how FROM known_destinations WHERE substr(at,1,10)=?",
        &["kind", "value", "how"],
    )?;
    let confirmations = collect(
        "SELECT rule, granted, used FROM confirmations WHERE substr(at,1,10)=?",
        &["rule", "granted", "used"],
    )?;
    let mut blocked = Vec::new();
    let mut standing = Vec::new();
    {
        let mut stmt = c.prepare(
            "SELECT id, kind, input_json, output_json, error FROM spans WHERE substr(started_at,1,10)=? \
             AND kind IN ('tool.refused','tool.call')",
        )?;
        let rows = stmt.query_map(params![day], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?;
        for row in rows {
            let (id, kind, input, output, error) = row?;
            let inp = loads_or_empty(input);
            let out = loads_or_empty(output);
            let get = |v: &Value, k: &str| v.get(k).cloned().unwrap_or(Value::Null);
            if kind == "tool.call" {
                if let Some(covered) = inp.get("standing").filter(|v| v.is_object()) {
                    standing.push(
                        json!({"span": id, "tool": get(&inp, "tool"), "target": get(&inp, "target"),
                        "declared": get(&inp, "declared"), "standing_task": get(covered, "task"),
                        "rules": get(covered, "rules"), "error": error}),
                    );
                }
            }
            if let Some(network) = out.get("network").filter(|v| v.is_object()) {
                if let Some(refused) = network.get("refused").and_then(Value::as_array) {
                    for refusal in refused {
                        blocked.push(json!({"span": id, "network": refusal}));
                    }
                }
            }
            if kind == "tool.refused" {
                blocked.push(json!({"span": id, "tool": get(&inp, "tool"), "reason": error}));
            }
        }
    }
    let memory = collect(
        "SELECT id, origin, status FROM memory_items WHERE substr(created_at,1,10)=? \
         AND (origin='third_party' OR id IN (SELECT ms.item_id FROM memory_sources ms \
         LEFT JOIN trail t ON ms.kind='trail' AND ms.source_id=t.id \
         LEFT JOIN journal j ON ms.kind='journal' AND ms.source_id=j.id \
         WHERE t.source IN ('window','browser_tab','email','message','notification') OR COALESCE(j.origin,'') LIKE '%external:%'))",
        &["id", "origin", "status"],
    )?;
    Ok(
        json!({"new_destinations": destinations, "confirmations": confirmations, "blocked_flows": blocked,
        "standing_sends": standing, "outside_origin_memory": memory,
        "meaning": "recorded facts; missing observations are not proof of no activity"}),
    )
}

fn web_host(url: Option<&Value>) -> String {
    let Some(Value::String(u)) = url else {
        return String::new();
    };
    if u.is_empty() {
        return String::new();
    }
    match urls::parse(u) {
        Some(p) if p.scheme == "http" || p.scheme == "https" => p.host,
        _ => String::new(),
    }
}

/// policy/sites.py `sites_contacted`: the hosts this task tree reached, each once, sorted.
pub fn sites_contacted(db: &Db, tree_id: &str) -> Result<Vec<String>> {
    let c = db.conn();
    let (where_, p) = tree_where(tree_id);
    let mut hosts: Vec<String> = Vec::new();
    {
        let sql = format!(
            "SELECT data_json FROM (SELECT data_json, json_extract(data_json, '$.task_id') AS task_id FROM journal \
             WHERE kind = 'shell.network') WHERE {where_}"
        );
        let mut stmt = c.prepare(&sql)?;
        let rows: Vec<String> = stmt
            .query_map(params_from_iter(p.iter()), |r| r.get(0))?
            .collect::<Result<_>>()?;
        for row in rows {
            let data = ferb_py::json::loads(&row).unwrap_or(Value::Null);
            if let Some(list) = data.get("destinations").and_then(Value::as_array) {
                for d in list {
                    hosts.push(match d.get("host") {
                        Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
                        _ => String::new(),
                    });
                }
            }
        }
    }
    {
        let sql = format!(
            "SELECT input_json, output_json FROM spans WHERE kind = 'tool.call' AND ended_at IS NOT NULL \
             AND (error IS NULL OR json_extract(input_json, '$.outcome') = 'unknown') \
             AND json_extract(input_json, '$.kind') IN (?, ?) AND {where_}"
        );
        let mut args: Vec<String> = vec![Kind::NAVIGATE.into(), Kind::OPEN_TAB.into()];
        args.extend(p.iter().cloned());
        let mut stmt = c.prepare(&sql)?;
        let rows: Vec<(String, Option<String>)> = stmt
            .query_map(params_from_iter(args.iter()), |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?
            .collect::<Result<_>>()?;
        for (input, output) in rows {
            let inp = ferb_py::json::loads(&input).unwrap_or(Value::Null);
            let asked = inp
                .get("args")
                .filter(|a| ferb_py::value::truthy(a))
                .and_then(|a| a.get("url"));
            if web_host(asked).is_empty() {
                continue;
            }
            let out = loads_or_empty(output);
            hosts.push(web_host(asked));
            hosts.push(web_host(out.get("url")));
        }
    }
    {
        let sql = format!(
            "SELECT data_json FROM (SELECT data_json, task_id FROM trail WHERE source = 'browser_tab' \
             AND actor = 'ferb') WHERE {where_}"
        );
        let mut stmt = c.prepare(&sql)?;
        let rows: Vec<String> = stmt
            .query_map(params_from_iter(p.iter()), |r| r.get(0))?
            .collect::<Result<_>>()?;
        for row in rows {
            let data = ferb_py::json::loads(&row).unwrap_or(Value::Null);
            let url = data.get("url");
            hosts.push(if url.is_some_and(ferb_py::value::truthy) {
                web_host(url)
            } else {
                match data.get("host") {
                    Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
                    _ => String::new(),
                }
            });
        }
    }
    let set: BTreeSet<String> = hosts
        .iter()
        .map(|h| crate::destinations::normalize("host", h))
        .filter(|h| !h.is_empty() && h != "excluded")
        .collect();
    Ok(set.into_iter().collect())
}
