//! Recoverable context assembly (ferb/core/context.py:165-372, L28/L29): budget complete context blocks and expose
//! omissions; never shorten a request or a generated artifact. The budget is a context allocation (the model's
//! window minus room for its reply), not a limit on the owner's work.
//!
//! Order of filling: the reserve (prompts and tools) first; every protected block; then the conversation entries
//! newest-first while they fit, the older ones reported in one note; then optional blocks by priority, each left out
//! whole with its retrieval reference kept visible. A run of optional blocks (`fold`, one retrieval route) whose
//! markers can't all fit folds its lowest-priority markers into one note. Protected text over the budget is
//! reported, never changed; protected text over the runtime's character cap is refused (`InputTooLarge`).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use ferb_core::errors::FerbError;
use ferb_core::types::ModelLimits;
use indexmap::IndexMap;
use serde_json::Value;

use crate::tokens::Tokenizer;

/// context.py:176 `ContextBlock`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContextBlock {
    pub text: String,
    pub reference: String,
    /// always included, whatever the budget (the situation, the request)
    pub protected: bool,
    /// repeats across this role's calls; the cacheable prefix ends after the last leading one (P20)
    pub cache: bool,
    /// optional layer allocation; original presentation order is preserved
    pub priority: i64,
    /// supplied by the source, never inferred from model-written text
    pub origin: String,
    pub source_refs: Vec<String>,
    /// source data without presentation or catalogue metadata; None keeps the text
    pub payload: Option<String>,
    /// one conversation entry: filled newest-first after the protected blocks, never with gaps
    pub history: bool,
    /// one run of optional blocks (its retrieval route): its lowest markers fold into a note
    pub fold: String,
    /// a derived record's stored owner values (GATE-519); None: a source's
    pub values: Option<Value>,
}

impl ContextBlock {
    pub fn new(text: impl Into<String>, reference: impl Into<String>) -> ContextBlock {
        ContextBlock {
            text: text.into(),
            reference: reference.into(),
            ..ContextBlock::default()
        }
    }
}

/// context.py:191 `Assembly`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assembly {
    /// the context in order, the budget report last
    pub parts: Vec<String>,
    /// leading parts that repeat across this role's calls
    pub cacheable: usize,
    /// per non-empty block, in order: handed to the model in full
    pub shown: Vec<bool>,
    /// older conversation entries that did not fit
    pub omitted_history: usize,
    /// the body's token estimate, before the budget report
    pub tokens: usize,
    /// characters of the whole text the parts make, the report included
    pub chars: usize,
}

/// Why `assemble` returned no context.
#[derive(Clone, Debug, PartialEq)]
pub enum AssembleError {
    /// context.py:225's `ValueError`: a budget that is not positive, a negative reserve or a cap that is not
    /// positive.
    Arguments(String),
    /// Protected text alone is over the runtime's character cap: refused, never sent (`InputTooLarge`).
    TooLarge(FerbError),
}

impl std::fmt::Display for AssembleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssembleError::Arguments(m) => f.write_str(m),
            AssembleError::TooLarge(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for AssembleError {}

/// context.py:165 `runtime_limits`: a runtime's documented model limits; a runtime that reports none is refused
/// (`ModelLimitsUnknown`), never given a number of FERB's own.
pub fn runtime_limits(
    name: Option<&str>,
    limits: Option<&ModelLimits>,
) -> Result<ModelLimits, FerbError> {
    limits.cloned().ok_or_else(|| {
        FerbError::ModelLimitsUnknown(format!(
            "the {} runtime doesn't report its model's documented context window",
            name.unwrap_or("model")
        ))
    })
}

fn note_text(count: usize, reference: &str) -> String {
    format!("[{count} older entries not shown here; fetch full text: {reference}]")
}

fn fold_text(count: usize, first: &str, last: &str, route: &str) -> String {
    format!(
        "[{count} more not shown or listed one by one; fetch full text: {route} — from {first} to {last}]"
    )
}

/// context.py:358 `_report`: the budget line (`json.dumps` of its fields, Python's spacing).
pub fn report(
    tokenizer: Tokenizer,
    budget: i64,
    reserved: i64,
    content: i64,
    over: bool,
    omitted: i64,
) -> String {
    format!(
        "Context budget: {{\"budget_tokens\": {budget}, \"reserved_tokens\": {reserved}, \
         \"content_tokens_estimate\": {content}, \"method\": \"{}\", \"over_budget\": {over}, \
         \"omitted_blocks\": {omitted}}}",
        tokenizer.method()
    )
}

const LONGEST: i64 = 1_000_000_000; // 10 ** 9: the report and a note counted at their longest

fn chars(text: &str) -> i64 {
    text.chars().count() as i64
}

/// `reference.split(" ", 1)[0]`.
fn first_word(reference: &str) -> &str {
    reference.split(' ').next().unwrap_or("")
}

/// context.py:200 `assemble` with o200k_base.
pub fn assemble(
    blocks: &[ContextBlock],
    budget_tokens: i64,
    reserved_tokens: i64,
    max_chars: Option<i64>,
) -> Result<Assembly, AssembleError> {
    assemble_with(
        Tokenizer::O200k,
        blocks,
        budget_tokens,
        reserved_tokens,
        max_chars,
    )
}

/// context.py:200 `assemble`, counting with `tokenizer`.
pub fn assemble_with(
    tokenizer: Tokenizer,
    blocks: &[ContextBlock],
    budget_tokens: i64,
    reserved_tokens: i64,
    max_chars: Option<i64>,
) -> Result<Assembly, AssembleError> {
    if budget_tokens <= 0 || reserved_tokens < 0 || max_chars.is_some_and(|m| m <= 0) {
        return Err(AssembleError::Arguments(
            "context budget must be positive and reserve nonnegative".into(),
        ));
    }
    let count = |text: &str| tokenizer.count(text) as i64;
    let active: Vec<&ContextBlock> = blocks.iter().filter(|b| !b.text.is_empty()).collect();
    let omission: Vec<String> = active
        .iter()
        .map(|b| format!("[Context omitted; fetch full text: {}]", b.reference))
        .collect();
    let history: Vec<usize> = (0..active.len())
        .filter(|&i| active[i].history && !active[i].protected)
        .collect();
    let mut kept: BTreeSet<usize> = (0..active.len()).filter(|&i| active[i].protected).collect();
    let separator = count("\n\n");
    let mut cost: BTreeMap<usize, i64> = BTreeMap::new();
    let mut gap: IndexMap<usize, i64> = IndexMap::new();
    for i in 0..active.len() {
        if kept.contains(&i) || history.contains(&i) {
            cost.insert(i, count(&active[i].text));
        } else {
            gap.insert(i, count(&omission[i]));
        }
    }
    let n = active.len() as i64;
    let longest_report = report(tokenizer, budget_tokens, reserved_tokens, LONGEST, true, n);
    let note_cost = if history.is_empty() {
        0
    } else {
        count(&note_text(history.len(), "chat.history")) + separator
    };
    let report_cost = count(&longest_report) + separator;
    let used = reserved_tokens
        + report_cost
        + note_cost
        + kept.iter().map(|i| cost[i] + separator).sum::<i64>()
        + gap.values().map(|g| g + separator).sum::<i64>();
    let cap = max_chars.unwrap_or(i64::MAX);
    let note_chars = if history.is_empty() {
        0
    } else {
        chars(&note_text(
            history.len(),
            first_word(&active[history[0]].reference),
        )) + 2
    };
    let used_chars = chars(&longest_report)
        + 2
        + note_chars
        + kept
            .iter()
            .map(|&i| chars(&active[i].text) + 2)
            .sum::<i64>()
        + gap.keys().map(|&i| chars(&omission[i]) + 2).sum::<i64>();
    // fold route -> its folded blocks, in folding order; routes in the order they first folded
    let mut folded: IndexMap<String, Vec<usize>> = IndexMap::new();

    // A run's note at its longest (two of its longest listed references), charged when its first block folds.
    let note = |gap: &IndexMap<usize, i64>, kept: &BTreeSet<usize>, route: &str| -> (i64, i64) {
        let run: Vec<usize> = gap
            .keys()
            .copied()
            .filter(|&i| active[i].fold == route && !kept.contains(&i))
            .collect();
        let longest = fold_text(1_000_000_000, "", "", route);
        let most_tokens = run.iter().map(|i| gap[i]).max().unwrap_or(0);
        let most_chars = run
            .iter()
            .map(|&i| chars(&active[i].reference))
            .max()
            .unwrap_or(0);
        (
            count(&longest) + 2 * most_tokens + separator,
            chars(&longest) + 2 * most_chars + 2,
        )
    };

    struct State {
        used: i64,
        used_chars: i64,
    }
    let mut state = State { used, used_chars };

    let fold = |index: usize,
                state: &mut State,
                gap: &mut IndexMap<usize, i64>,
                kept: &BTreeSet<usize>,
                folded: &mut IndexMap<String, Vec<usize>>| {
        let route = active[index].fold.clone();
        if !folded.contains_key(&route) {
            let (tokens, chars_) = note(gap, kept, &route);
            state.used += tokens;
            state.used_chars += chars_;
            folded.insert(route.clone(), Vec::new());
        }
        folded[&route].push(index);
        let marker = gap.shift_remove(&index).expect("a listed marker");
        state.used -= marker + separator;
        state.used_chars -= chars(&omission[index]) + 2;
    };

    // The listed run markers of lower priority than `index` that, folded lowest first, make room for it whole:
    // the fewest that do; None when even all of them wouldn't.
    let room = |index: usize,
                extra: i64,
                extra_chars: i64,
                state: &State,
                gap: &IndexMap<usize, i64>,
                kept: &BTreeSet<usize>,
                folded: &IndexMap<String, Vec<usize>>|
     -> Option<Vec<usize>> {
        let mut tokens = state.used + extra;
        let mut chars_ = state.used_chars + extra_chars;
        let mut noted: HashSet<String> = folded.keys().cloned().collect();
        let mut chosen = Vec::new();
        let mut lower: Vec<usize> = gap
            .keys()
            .copied()
            .filter(|&i| {
                !active[i].fold.is_empty()
                    && !kept.contains(&i)
                    && active[i].priority < active[index].priority
            })
            .collect();
        lower.sort_by_key(|&i| active[i].priority); // stable, as Python's sorted
        for i in lower {
            if !noted.contains(&active[i].fold) {
                let (note_tokens, note_chars) = note(gap, kept, &active[i].fold);
                tokens += note_tokens;
                chars_ += note_chars;
                noted.insert(active[i].fold.clone());
            }
            tokens -= gap[&i] + separator;
            chars_ -= chars(&omission[i]) + 2;
            chosen.push(i);
            if tokens <= budget_tokens && chars_ <= cap {
                return Some(chosen);
            }
        }
        None
    };

    let mut first_folds: Vec<usize> = gap
        .keys()
        .copied()
        .filter(|&i| !active[i].fold.is_empty())
        .collect();
    first_folds.sort_by_key(|&i| active[i].priority);
    for index in first_folds {
        if state.used <= budget_tokens && state.used_chars <= cap {
            break;
        }
        fold(index, &mut state, &mut gap, &kept, &mut folded);
    }
    for &index in history.iter().rev() {
        let block_chars = chars(&active[index].text) + 2;
        if state.used + cost[&index] + separator > budget_tokens
            || state.used_chars + block_chars > cap
        {
            break;
        }
        kept.insert(index);
        state.used += cost[&index] + separator;
        state.used_chars += block_chars;
    }
    let mut by_priority: Vec<usize> = gap.keys().copied().collect();
    by_priority.sort_by_key(|&i| -active[i].priority);
    for index in by_priority {
        if !gap.contains_key(&index) {
            // folded meanwhile to make room for a higher-priority block
            continue;
        }
        let block = active[index];
        let extra = count(&block.text) - gap[&index];
        let extra_chars = chars(&block.text) - chars(&omission[index]);
        if state.used + extra > budget_tokens || state.used_chars + extra_chars > cap {
            let Some(make_room) = room(index, extra, extra_chars, &state, &gap, &kept, &folded)
            else {
                continue;
            };
            for i in make_room {
                fold(i, &mut state, &mut gap, &kept, &mut folded);
            }
        }
        kept.insert(index);
        state.used += extra;
        state.used_chars += extra_chars;
    }

    let first_folded: HashSet<usize> = folded
        .values()
        .filter_map(|run| run.iter().min().copied())
        .collect();
    let in_folds: HashSet<usize> = folded.values().flatten().copied().collect();
    let omitted_history = history.iter().filter(|i| !kept.contains(i)).count();
    let mut parts: Vec<String> = Vec::new();
    let mut repeats: Vec<bool> = Vec::new();
    let mut noted = false;
    for (index, block) in active.iter().enumerate() {
        if kept.contains(&index) {
            parts.push(block.text.clone());
            repeats.push(block.cache);
        } else if history.contains(&index) {
            if !noted {
                // the older entries collapse into one note where the first one stood
                parts.push(note_text(omitted_history, first_word(&block.reference)));
                repeats.push(block.cache);
                noted = true;
            }
        } else if in_folds.contains(&index) {
            if first_folded.contains(&index) {
                let mut run = folded[&block.fold].clone();
                run.sort_unstable();
                parts.push(fold_text(
                    run.len(),
                    &active[run[0]].reference,
                    &active[run[run.len() - 1]].reference,
                    &block.fold,
                ));
                repeats.push(false);
            }
        } else {
            parts.push(omission[index].clone());
            repeats.push(false); // an omitted block ends the cacheable prefix
        }
    }
    if parts.is_empty() {
        parts.push(String::new());
    }
    let body = parts.join("\n\n");
    let content = count(&body);
    let over = content + reserved_tokens > budget_tokens;
    let omitted = n - kept.len() as i64;
    let mut line = report(
        tokenizer,
        budget_tokens,
        reserved_tokens,
        content,
        over,
        omitted,
    );
    // Header and omission references also consume context: report their unavoidable overhead truthfully.
    if !over && content + count(&line) + separator + reserved_tokens > budget_tokens {
        line = report(
            tokenizer,
            budget_tokens,
            reserved_tokens,
            content,
            true,
            omitted,
        );
    }
    parts.push(line);
    let total = chars(&parts.join("\n\n"));
    if let Some(max) = max_chars {
        if total > max {
            // protected text alone is over the cap: refused, never sent
            return Err(AssembleError::TooLarge(FerbError::input_too_large(
                total as u64,
                max as u64,
                None,
            )));
        }
    }
    let cacheable = repeats.iter().take_while(|r| **r).count();
    Ok(Assembly {
        parts,
        cacheable,
        shown: (0..active.len()).map(|i| kept.contains(&i)).collect(),
        omitted_history,
        tokens: content as usize,
        chars: total as usize,
    })
}

/// context.py:364 `overhead_tokens`: what `assemble` adds around one protected block at most.
pub fn overhead_tokens(tokenizer: Tokenizer, budget_tokens: i64, reserved_tokens: i64) -> i64 {
    tokenizer.count(&report(
        tokenizer,
        budget_tokens,
        reserved_tokens,
        LONGEST,
        true,
        1,
    )) as i64
        + 2 * tokenizer.count("\n\n") as i64
}

/// context.py:370 `overhead_chars`: `overhead_tokens` in characters.
pub fn overhead_chars(tokenizer: Tokenizer, budget_tokens: i64, reserved_tokens: i64) -> i64 {
    chars(&report(
        tokenizer,
        budget_tokens,
        reserved_tokens,
        LONGEST,
        true,
        1,
    )) + 2 * 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_runtime_without_limits_is_refused_in_python_s_words() {
        let err = runtime_limits(Some("codex"), None).unwrap_err();
        assert_eq!(
            err.to_string(),
            "the codex runtime doesn't report its model's documented context window"
        );
        assert_eq!(err.class(), "ModelLimitsUnknown");
        let limits = ModelLimits {
            model: "m".into(),
            context_window: 10,
            max_output: 2,
            source: "s".into(),
            window_includes_output: true,
            max_input_chars: None,
        };
        assert_eq!(runtime_limits(None, Some(&limits)).unwrap(), limits);
    }
}
