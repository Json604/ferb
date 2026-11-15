//! FERB's records of its own state for the planner (ORCH-577, ORCH-578): the pending reminders and timers the way the
//! schedules are listed, and the permissions in force for FERB's self-description. Port of
//! `ferb/daemon/areas/records.py`. Must not read a model's words to build a row, show a secret's value, or change
//! anything. `_saved_logins` reads the owner's saved logins and FERB's own secrets (the identity part's); the
//! composition builds it from those, as `saved_logins` takes them.

use ferb_clock::py::{self, Time};
use serde_json::{json, Map, Value};

use super::schedules::timer_label;
use super::State;

/// `_reminder_rows`: the owner's pending reminders and timers, soonest first.
pub fn reminder_rows(st: &State) -> Vec<Map<String, Value>> {
    let mut rows = st
        .timers_plain()
        .pending(&["reminder", "timer"])
        .unwrap_or_default();
    rows.sort_by_key(|t| t.fire_at);
    rows.iter()
        .map(|t| {
            crate::shared::object(json!({"id": t.id, "kind": t.kind, "name": timer_label(t),
                                         "message": t.message, "at": py::iso(&t.fire_at)}))
        })
        .collect()
}

/// `_reminder_facts`: the planner's list of them: a count by kind, then one line each.
pub fn reminder_facts(st: &State) -> Vec<String> {
    let rows = reminder_rows(st);
    let n = |kind: &str| rows.iter().filter(|r| r["kind"] == kind).count();
    let count = if rows.is_empty() {
        "none".to_owned()
    } else {
        let parts: Vec<String> = [("reminder", n("reminder")), ("timer", n("timer"))]
            .iter()
            .filter(|(_, k)| *k != 0)
            .map(|(kind, k)| format!("{k} {kind}{}", if *k != 1 { "s" } else { "" }))
            .collect();
        format!("{} ({})", rows.len(), parts.join(", "))
    };
    let mut lines = vec![format!(
        "The owner's pending reminders and timers, from FERB's records (the truth: answer anything about them only \
         from this list, never from the conversation): {count}"
    )];
    for r in &rows {
        let kind = ferb_py::value::py_str(&r["kind"]);
        let mut cap = kind.chars();
        let kind = cap
            .next()
            .map(|c| c.to_uppercase().collect::<String>() + cap.as_str())
            .unwrap_or_default();
        let at = py::parse(&ferb_py::value::py_str(&r["at"]))
            .map(|t| py::fmt(&st.clock.zone().local(&t), "%a %d %b %Y %H:%M"))
            .unwrap_or_default();
        lines.push(format!(
            "- {kind} {} “{}”: {} — {at}",
            ferb_py::value::py_str(&r["id"]),
            ferb_py::value::py_str(&r["name"]),
            ferb_py::value::py_str(&r["message"]),
        ));
    }
    lines
}

/// `_permissions_in_force`: the standing send scopes in force, the destinations the owner named or approved, and the
/// "don't ask for 30 minutes" window's end (`allow_until`: the gateway's).
pub fn permissions_in_force(st: &State, allow_until: Option<&Time>) -> Result<Value, String> {
    let conn = st.db().conn();
    let mut scopes = Vec::new();
    let mut stmt = conn
        .prepare("SELECT task_id, scope FROM standing_permissions WHERE active = 1 ORDER BY rowid")
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    for (task, raw) in rows {
        let scope = ferb_py::json::loads(&raw).map_err(|e| e.to_string())?;
        let get = |k: &str| scope.get(k).cloned().unwrap_or(Value::Null);
        let recipients = scope
            .get("recipients")
            .filter(|v| ferb_py::value::truthy(v))
            .cloned()
            .unwrap_or(json!([]));
        scopes.push(
            json!({"held_by": task, "capability": get("capability"), "identity": get("identity"),
                           "account": get("account"), "site": get("site"), "recipients": recipients,
                           "any_text": scope.get("content").is_none_or(Value::is_null)}),
        );
    }
    let mut stmt = conn
        .prepare("SELECT kind, value, how FROM known_destinations ORDER BY at")
        .map_err(|e| e.to_string())?;
    let known: Vec<Value> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|(kind, value, how)| {
            let shown = if value.starts_with("sha256:") {
                "(kept only as a fingerprint)".to_owned()
            } else {
                value
            };
            let approved = how == "answered" || how == "confirmed";
            json!({"kind": kind, "value": shown, "how": how, "approved": approved})
        })
        .collect();
    let window = allow_until
        .filter(|u| st.now() < **u)
        .map(|u| json!(py::iso(u)))
        .unwrap_or(Value::Null);
    Ok(
        json!({"standing_sends": scopes, "known_destinations": known, "routine_questions_off_until": window}),
    )
}

/// `_saved_logins`: the logins FERB can fill, by name and site only (never a value or a username). `owner`: the owner's
/// saved entries' inventory, or the error reading it; `mine`: FERB's own secrets (name, account hint, sites).
pub fn saved_logins(
    owner: Result<Vec<Value>, String>,
    mine: Vec<(String, String, Vec<String>)>,
) -> Value {
    let owner = owner.unwrap_or_else(|class| {
        vec![json!({"problem": format!("the saved logins couldn't be read ({class})")})]
    });
    let mine: Vec<Value> = mine
        .into_iter()
        .map(|(name, account, sites)| json!({"name": name, "account": account, "sites": sites}))
        .collect();
    json!({"owner_saved": owner, "ferb_own": mine})
}
