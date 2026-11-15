//! policy/rules.py: action metadata and stable ledger hashes. Never classifies request or UI text, never authorizes.

use std::collections::BTreeSet;

use ferb_core::types::{ActionSpec, Identity, Target};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::py;

/// Agent-declared annotations: they grant no authority and infer no meaning.
pub const EFFECTS: &[&str] = &[
    "navigate",
    "edit",
    "send",
    "pay",
    "destroy",
    "account_security",
];
/// The agent's own notes on a call: recorded, never part of what the call is (P19).
pub const ANNOTATIONS: &[&str] = &["why", "effect", "purpose", "contact"];

/// rules.py:19 `Kind`.
pub struct Kind;

impl Kind {
    pub const READ: &'static str = "read";
    pub const NAVIGATE: &'static str = "navigate";
    pub const SEARCH: &'static str = "search";
    pub const OPEN_TAB: &'static str = "open_tab";
    pub const DRAFT: &'static str = "draft";
    pub const FILL_FORM: &'static str = "fill_form";
    pub const SUBMIT: &'static str = "submit";
    pub const SEND: &'static str = "send";
    pub const POST: &'static str = "post";
    pub const REPLY: &'static str = "reply";
    pub const PAY: &'static str = "pay";
    pub const DELETE_PERMANENT: &'static str = "delete_permanent";
    pub const TRASH: &'static str = "trash";
    pub const WRITE_FILE: &'static str = "write_file";
    pub const RUN_SHELL: &'static str = "run_shell";
    pub const APPLESCRIPT: &'static str = "applescript";
    pub const MAC_INPUT: &'static str = "mac_input";
    pub const FILL_SECRET: &'static str = "fill_secret";
    pub const MAILBOX_READ: &'static str = "mailbox_read";
    pub const MAILBOX_SEND: &'static str = "mailbox_send";
    pub const TIMER: &'static str = "timer";
    pub const ACCOUNT_SECURITY: &'static str = "account_security";
    pub const CALL: &'static str = "call";
    pub const CLICK: &'static str = "click";
    pub const SNAPSHOT: &'static str = "snapshot";
    pub const STOP_PROCESS: &'static str = "stop_process";
}

/// W2: input whose physical kind says nothing of what it does.
pub const GENERIC_INPUT: &[&str] = &[
    Kind::CLICK,
    Kind::FILL_FORM,
    Kind::MAC_INPUT,
    Kind::APPLESCRIPT,
];
/// Honoured on any acting call, not only an input.
pub const ALWAYS_DECLARED: &[&str] = &[Kind::PAY, Kind::ACCOUNT_SECURITY];
/// Reversible kinds never get a ledger row.
pub const REVERSIBLE: &[&str] = &[
    Kind::READ,
    Kind::NAVIGATE,
    Kind::SEARCH,
    Kind::OPEN_TAB,
    Kind::DRAFT,
    Kind::TRASH,
    Kind::TIMER,
    Kind::SNAPSHOT,
];
/// Kinds whose result is content the tool read (S3).
pub const READING_KINDS: &[&str] = &[Kind::READ, Kind::SEARCH, Kind::NAVIGATE, Kind::MAILBOX_READ];

/// `EFFECT_KINDS.get(effect)`.
pub fn effect_kind(effect: &str) -> Option<&'static str> {
    match effect {
        "send" => Some(Kind::SEND),
        "pay" => Some(Kind::PAY),
        "destroy" => Some(Kind::DELETE_PERMANENT),
        "account_security" => Some(Kind::ACCOUNT_SECURITY),
        _ => None,
    }
}

/// `declared_kinds`: every kind a call's declarations add.
pub fn declared_kinds(spec: &ActionSpec) -> BTreeSet<String> {
    [&spec.declared, &spec.effect_kind]
        .into_iter()
        .flatten()
        .filter(|k| !k.is_empty())
        .cloned()
        .collect()
}

/// `declared_of`: the declared kinds a tool.call span records (one kind as a string, several as a list).
pub fn declared_of(recorded: &Value) -> BTreeSet<String> {
    match recorded {
        Value::String(s) if !s.is_empty() => [s.clone()].into(),
        Value::Array(items) => items.iter().map(ferb_py::value::py_str).collect(),
        _ => BTreeSet::new(),
    }
}

fn or_none(a: &Option<String>, b: Option<&str>) -> Option<String> {
    match a {
        Some(x) if !x.is_empty() => Some(x.clone()),
        _ => b.filter(|x| !x.is_empty()).map(str::to_string),
    }
}

/// `declare` (W2): a generic input checked as the effect the agent declares for it; it only adds.
#[allow(clippy::too_many_arguments)]
pub fn declare(
    spec: &ActionSpec,
    effect: &str,
    recipients: &[String],
    account: Option<&str>,
    amount: Option<f64>,
    item: Option<&str>,
    currency: Option<&str>,
) -> ActionSpec {
    let Some(kind) = effect_kind(&py::lower(py::strip(effect))) else {
        return spec.clone();
    };
    let k = spec.kind.as_str();
    if REVERSIBLE.contains(&k) || (!GENERIC_INPUT.contains(&k) && !ALWAYS_DECLARED.contains(&kind))
    {
        return spec.clone();
    }
    let t = &spec.target;
    let mut all: Vec<String> = t.recipients.clone();
    for r in recipients {
        let r = py::strip(r);
        if !r.is_empty() {
            all.push(r.to_string());
        }
    }
    let mut seen = std::collections::HashSet::new();
    all.retain(|r| seen.insert(r.clone()));
    let account = account.map(py::strip);
    let currency = currency.map(|c| py::prefix(py::strip(c), 8).to_string());
    let item = item.map(py::strip);
    let target = Target {
        recipients: all,
        account: or_none(&t.account, account),
        amount: t.amount.or(amount),
        currency: or_none(&t.currency, currency.as_deref()),
        item: or_none(&t.item, item),
        ..t.clone()
    };
    ActionSpec {
        effect_kind: Some(kind.to_string()),
        target,
        ..spec.clone()
    }
}

/// rules.py:111 `ActionRequest`: recorded action metadata, with no authorization verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionRequest {
    pub capability: String,
    pub kind: String,
    pub identity: Identity,
    pub scopes: BTreeSet<String>,
    pub task_id: String,
    pub worker_id: String,
    pub request_id: String,
    pub task_scopes: BTreeSet<String>,
    pub target: Target,
    pub content: Option<String>,
    pub effect: String,
    pub why: String,
}

/// `json.dumps(value, sort_keys=True, separators=(",", ":"))` (ensure_ascii on).
pub fn compact_sorted(value: &Value) -> String {
    match value {
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(compact_sorted)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", ferb_py::json::dumps(k), compact_sorted(&map[k])))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        scalar => ferb_py::json::dumps(scalar),
    }
}

/// `payload_hash`: binds ledger rows to the exact call (target, content, amount and the validated arguments, the
/// agent's notes left out).
pub fn payload_hash(a: &ActionRequest, args: Option<&Map<String, Value>>) -> String {
    let call_args: Map<String, Value> = args
        .map(|m| {
            m.iter()
                .filter(|(k, _)| !ANNOTATIONS.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default();
    let mut recipients = a.target.recipients.clone();
    recipients.sort();
    let canon = json!({
        "capability": a.capability,
        "identity": a.identity.as_str(),
        "account": a.target.account,
        "site": a.target.site,
        "contact": a.target.contact,
        "recipients": recipients,
        "path": a.target.path,
        "amount": a.target.amount,
        "content": a.content,
        "args": Value::Object(call_args),
    });
    hex::encode(Sha256::digest(compact_sorted(&canon).as_bytes()))
}
