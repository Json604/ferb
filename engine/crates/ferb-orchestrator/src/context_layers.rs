//! ferb/orchestrator/context_layers.py: P3 context layer helpers: a task's targets as memory query words, and ranked
//! memory items kept whole with their fetch route.

use ferb_context::ContextBlock;
use ferb_sanitize::origin;
use serde_json::Value;

use crate::models::TargetsSpec;
use crate::ports::Policy;

/// context_layers.py:7 `target_words` (L36 §3, §4).
pub fn target_words(policy: &dyn Policy, targets: &TargetsSpec) -> String {
    let mut words: Vec<String> = Vec::new();
    for site in &targets.sites {
        let parsed = if site.contains("://") {
            policy.url_host(site)
        } else {
            None
        };
        let host = match parsed.filter(|h| !h.is_empty()) {
            Some(h) => h,
            None => crate::py::strip(site)
                .split('/')
                .next()
                .unwrap_or("")
                .to_string(),
        };
        words.push(
            host.split('.')
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    words.extend(targets.apps.iter().cloned());
    words.extend(targets.recipients.iter().cloned());
    words.extend(targets.paths.iter().map(|p| ferb_fs::pypath::name(p)));
    words
        .into_iter()
        .filter(|w| !crate::py::strip(w).is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// context_layers.py:22 `ranked_memory_blocks`: each item {text, reference, origin, source_refs?, fold?}, kept whole,
/// higher ranks first, with the origins it carries.
pub fn ranked_memory_blocks(
    candidates: &[Value],
) -> Vec<(ContextBlock, std::collections::BTreeSet<String>)> {
    let n = candidates.len() as i64;
    candidates
        .iter()
        .enumerate()
        .map(|(rank, item)| {
            let s = |k: &str| item.get(k).map(crate::py::str_of).unwrap_or_default();
            let refs: Vec<String> = item
                .get("source_refs")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().map(crate::py::str_of).collect())
                .unwrap_or_default();
            let block = ContextBlock {
                priority: 2 + n - rank as i64,
                origin: s("origin"),
                source_refs: refs,
                payload: Some(s("text")),
                fold: item.get("fold").map(crate::py::str_of).unwrap_or_default(),
                ..ContextBlock::new(
                    format!("Layer 4 — retrieved memory (data):\n{}", s("text")),
                    s("reference"),
                )
            };
            (block, origin::split(Some(&s("origin"))))
        })
        .collect()
}
