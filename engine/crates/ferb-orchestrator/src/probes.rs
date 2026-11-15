//! ferb/orchestrator/probes.py: a loop's probe (L31). The round's worker chose an observation tool, its arguments, a
//! field and an exact comparison; code validates it, reads the field and compares JSON values, never judging what a
//! value means (R2).
//!
//! decision: ORCH-505, ORCH-506

use ferb_context::pyvalue::{dumps, loads, Dumps, LoadError};
use ferb_context::Py;

use crate::models::Probe;
use crate::ports::CapFacts;

/// probes.py:40 `RUNTIME_PREFIXES`.
pub const RUNTIME_PREFIXES: &[&str] = &["mcp__ferb__", "mcp__ferb.", "ferb__"];

/// `json.loads` as Python raises it: `str(JSONDecodeError)` with its position, or the over-long integer's text.
pub fn json_loads(text: &str) -> Result<Py, String> {
    match loads(text) {
        Ok(v) => Ok(v),
        Err(LoadError::Value(msg)) => Err(match ferb_py::json::check(text) {
            Err(full) => full,
            Ok(()) => msg,
        }),
        Err(LoadError::TooDeep) => Err(
            "maximum recursion depth exceeded while decoding a JSON object from a unicode string"
                .into(),
        ),
    }
}

impl Probe {
    /// probes.py:27 `args`.
    pub fn args(&self) -> Result<indexmap::IndexMap<String, Py>, String> {
        let text = if self.args_json.is_empty() {
            "{}"
        } else {
            &self.args_json
        };
        match json_loads(text)? {
            Py::Dict(d) => Ok(d.0.clone()),
            _ => Err("probe arguments must be a JSON object".into()),
        }
    }

    /// probes.py:33 `value`.
    pub fn value(&self) -> Result<Py, String> {
        json_loads(if self.value_json.is_empty() {
            "null"
        } else {
            &self.value_json
        })
    }
}

/// probes.py:43 `resolve_tool`: (name, None) or (written, why) when two registered tools share the model's form.
pub fn resolve_tool(written: &str, registered: &[String]) -> (String, Option<String>) {
    let mut candidates = vec![written.to_string()];
    for p in RUNTIME_PREFIXES {
        if let Some(rest) = written.strip_prefix(p) {
            candidates.push(rest.to_string());
        }
    }
    for name in candidates {
        let mut found: Vec<&String> = registered
            .iter()
            .filter(|n| **n == name || n.replace('.', "_") == name)
            .collect();
        found.sort();
        if found.len() == 1 {
            return (found[0].clone(), None);
        }
        if !found.is_empty() {
            let names: Vec<&str> = found.iter().map(|s| s.as_str()).collect();
            return (
                written.to_string(),
                Some(format!(
                    "the probe's tool {written} could be any of {}: name one of them exactly",
                    names.join(", ")
                )),
            );
        }
    }
    (written.to_string(), None)
}

/// probes.py:58 `refusal`: why this probe can't be armed, or None.
pub fn refusal(
    probe: &Probe,
    cap: Option<&dyn CapFacts>,
    scopes: &[String],
    identity: &str,
) -> Option<String> {
    let Some(cap) = cap else {
        return Some(format!("{} is not a FERB tool", probe.tool));
    };
    if !cap.scopes().iter().all(|s| scopes.contains(s))
        || !cap.identities().iter().any(|i| i == identity)
    {
        return Some(format!("{} is outside this task", probe.tool));
    }
    if !cap.observation() || !cap.probe_safe() {
        return Some(format!(
            "{} can't be a probe: a probe must only read, move nothing and return at once",
            probe.tool
        ));
    }
    if crate::py::strip(&probe.field).is_empty() {
        return Some("a probe needs a field of the result to compare".into());
    }
    let checked = probe.args().and_then(|args| {
        let value = Py::Dict(ferb_context::pyvalue::Dict(args)).to_value();
        cap.validate(&value)?;
        if probe.op != "changed" {
            probe.value()?;
        }
        Ok(())
    });
    checked.err().map(|e| format!("invalid probe: {e}"))
}

/// probes.py:77 `field_value`: the value at a dot path; `None` is MISSING.
pub fn field_value<'a>(content: &'a Py, path: &str) -> Option<&'a Py> {
    let mut node = content;
    for part in path.split('.') {
        node = match node {
            Py::Dict(d) if d.0.contains_key(part) => &d.0[part],
            // `int(part)` of a digit that isn't decimal (a superscript) raises in Python; read here as MISSING
            Py::List(l) if crate::py::isdigit(part) => match crate::py::int_of_digits(part) {
                Some(Some(n)) if n < l.0.len() => &l.0[n],
                _ => return None,
            },
            _ => return None,
        };
    }
    Some(node)
}

const SORTED: Dumps = Dumps {
    ascii: true,
    compact: false,
    sort_keys: true,
};

/// probes.py:90 `same`: exact JSON equality (`true` is not `1`, `1` is not `1.0`); MISSING equals only MISSING.
pub fn same(a: Option<&Py>, b: Option<&Py>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => dumps(a, SORTED) == dumps(b, SORTED),
        _ => false,
    }
}

/// probes.py:98 `tripped`: whether this read shows the job needs doing again; a missing field always does.
pub fn tripped(
    probe: &Probe,
    observed: Option<&Py>,
    baseline: Option<&Py>,
) -> Result<bool, String> {
    let Some(observed) = observed else {
        return Ok(true);
    };
    if probe.op == "changed" {
        return Ok(!same(Some(observed), baseline));
    }
    let value = probe.value()?;
    Ok(same(Some(observed), Some(&value)) == (probe.op == "equals"))
}
