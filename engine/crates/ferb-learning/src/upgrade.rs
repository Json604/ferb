//! `upgrade.py`: the one-time memory clean-up before the core profile is first built (STORE-403), and the one-time
//! close of the fact-node copies an older build kept. Each is journaled once and idempotent.

use serde_json::{json, Value};

use crate::items::Memory;
use crate::sql;
use crate::store::{marks, Sql, Store};
use crate::text::subject_key;
use crate::{py, Result};

pub const CLEANUP_KIND: &str = "memory.cleanup";
pub const CLEANUP_VERSION: i64 = 1;
pub const PERSONA_KEYS: [&str; 1] = ["setup_persona"];
pub const FACT_NODES_KIND: &str = "memory.fact_nodes_closed";

/// upgrade.py:30 `clean_before_tier1`.
pub fn clean_before_tier1(
    memory: &Memory,
    record: &dyn Fn(&str, Value) -> Result<()>,
) -> Result<Option<Value>> {
    let c = &memory.store;
    if c.exists(
        "SELECT 1 FROM journal WHERE kind = ? AND json_extract(data_json, '$.version') >= ?",
        sql![CLEANUP_KIND, CLEANUP_VERSION],
    )? {
        return Ok(None);
    }
    let now = py::iso(&memory.clock.now());
    c.plain("BEGIN IMMEDIATE")?;
    let body = (|| -> Result<Value> {
        let kept: Vec<String> = c
            .query(
                "SELECT id FROM memory_items WHERE origin = 'inferred' AND status = 'active' AND valid_to IS NULL",
                sql![],
            )?
            .iter()
            .map(|r| r.s_at(0))
            .collect();
        let guesses = c.exec(
            "UPDATE memory_items SET status = 'active' WHERE status = 'proposed' AND origin = 'inferred' \
             AND valid_to IS NULL",
            sql![],
        )?;
        let mut subjects = 0;
        for r in c.query("SELECT DISTINCT subject FROM memory_items", sql![])? {
            let old = r.s("subject");
            let norm = subject_key(&old);
            if norm != old {
                subjects += c.exec(
                    "UPDATE memory_items SET subject = ? WHERE subject = ?",
                    sql![norm, old],
                )?;
            }
        }
        let persona = c.exec(
            &format!(
                "UPDATE memory_items SET origin = 'observed' WHERE key IN ({}) AND origin = 'owner_stated'",
                marks(PERSONA_KEYS.len())
            ),
            PERSONA_KEYS.iter().map(|k| Sql::Text((*k).into())).collect(),
        )?;
        let mut merged = Vec::new();
        for g in c.query(
            "SELECT kind, subject, key FROM memory_items WHERE status = 'active' AND valid_to IS NULL \
             GROUP BY kind, subject, key HAVING COUNT(*) > 1",
            sql![],
        )? {
            let rows = c.query(
                "SELECT id, origin FROM memory_items WHERE kind = ? AND subject = ? AND key = ? \
                 AND status = 'active' AND valid_to IS NULL ORDER BY origin = 'owner_stated' DESC, seq DESC",
                sql![g.s("kind"), g.s("subject"), g.s("key")],
            )?;
            let keeper = rows[0].s("id");
            let others: Vec<String> = rows[1..].iter().map(|r| r.s("id")).collect();
            for other in &others {
                c.exec("UPDATE memory_items SET valid_to = ? WHERE id = ?", sql![now, other])?;
            }
            c.exec(
                "UPDATE memory_items SET supersedes = COALESCE(supersedes, ?) WHERE id = ?",
                sql![others[0], keeper],
            )?;
            merged.push(json!({"key": g.s("key"), "kept": keeper, "ended": others}));
        }
        let identity = c.exec(
            "UPDATE learning_nodes SET valid_to = ? WHERE kind = 'identity' AND valid_to IS NULL",
            sql![now],
        )?;
        c.plain("COMMIT")?;
        Ok(
            json!({"version": CLEANUP_VERSION, "owner_kept": kept, "guesses_kept": guesses,
                  "subjects_normalized": subjects, "persona_observed": persona, "merged": merged,
                  "identity_closed": identity}),
        )
    })();
    let out = match body {
        Ok(out) => out,
        Err(e) => {
            if c.in_transaction() {
                c.plain("ROLLBACK")?;
            }
            return Err(e);
        }
    };
    record(CLEANUP_KIND, out.clone())?;
    (memory.hooks.borrow().on_change)();
    Ok(Some(out))
}

/// upgrade.py:95 `close_fact_nodes`.
pub fn close_fact_nodes(
    store: &Store,
    now: &str,
    record: &dyn Fn(&str, Value) -> Result<()>,
) -> Result<Option<i64>> {
    if store.exists(
        "SELECT 1 FROM journal WHERE kind = ? LIMIT 1",
        sql![FACT_NODES_KIND],
    )? {
        return Ok(None);
    }
    let closed = store.exec(
        "UPDATE learning_nodes SET valid_to = ? WHERE kind IN ('fact', 'identity') AND valid_to IS NULL",
        sql![now],
    )? as i64;
    record(FACT_NODES_KIND, json!({"closed": closed}))?;
    Ok(Some(closed))
}
