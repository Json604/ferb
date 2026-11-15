//! The running summary's fold (supervisor.py:4161-4294, L28, ORCH-303, ORCH-304, ORCH-326): the planner reads FERB's
//! running summary of the older conversation and every entry after what it covers; once the tail beyond the last 20
//! exchanges holds a chunk (FOLD_CHUNK_TOKENS), or the whole tail passes RECENT_CONVERSATION_TOKENS +
//! FOLD_CHUNK_TOKENS, the entries beyond those exchanges fold into a new summary in the background after the
//! decision, a bounded chunk per summary call, each written (a `chat.summary` row citing what it folded) before the
//! next is made. One fold per chat at a time; a failed call ends the refresh (`chat.summary_failed`; the next turn
//! tries again).

use std::collections::BTreeSet;

use ferb_context::assemble::{overhead_chars, overhead_tokens, ContextBlock};
use ferb_core::types::{Identity, Task, TaskRole};
use ferb_policy::carried::Shown;
use ferb_sanitize::origin;
use serde_json::{json, Map, Value};

use super::store::Entry;
use super::Inner;
use crate::history::{recent_entries, timeline, Entry as HistEntry, Timeline};
use crate::lines::{stored_values, summary_blocks, within_tokens};
use crate::seams::JournalRow;
use crate::words::{
    CONVERSATION_KINDS, ENTRY, FOLD_CHUNK_TOKENS, RECENT_CONVERSATION_TOKENS, SELF_POSTS,
    SUMMARY_KIND,
};

impl Inner {
    /// supervisor.py:4161 `_summary`: FERB's latest running summary of this chat, if it has one.
    pub fn summary_of(&self, conversation: &str) -> Option<Entry> {
        self.of_kind(SUMMARY_KIND, "session_id", conversation, None)
            .last()
            .cloned()
    }

    /// supervisor.py:4166 `_covered`: the entries (by lineage handle) every running summary of that summary's chat
    /// folded in.
    pub fn covered(&self, summary: Option<&Entry>) -> BTreeSet<String> {
        let Some(summary) = summary else {
            return BTreeSet::new();
        };
        self.of_kind(
            SUMMARY_KIND,
            "session_id",
            &summary.text("session_id"),
            None,
        )
        .iter()
        .flat_map(|r| {
            r.get("covered")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(ferb_py::value::py_str).collect::<Vec<_>>())
                .unwrap_or_default()
        })
        .collect()
    }

    /// `_timeline("", conversation, summary)[0]`: the chat's entries after the summary, oldest first.
    pub(crate) fn fold_timeline(
        &self,
        conversation: &str,
        summary: Option<&Entry>,
    ) -> Vec<HistEntry> {
        let covered = self.covered(summary);
        let mut kinds_read: Vec<&str> = CONVERSATION_KINDS.to_vec();
        kinds_read.extend(SELF_POSTS);
        let rows: Vec<JournalRow> = self
            .entries_of(&kinds_read)
            .iter()
            .map(Entry::row)
            .collect();
        let task_ids = self
            .continuity(|c| c.task_ids(conversation))
            .unwrap_or_default();
        let current = BTreeSet::from([String::new()]);
        let zone = self.deps.clock.zone();
        timeline(&Timeline {
            current: &current,
            conversation: Some(conversation),
            rows: &rows,
            task_ids: Some(&task_ids),
            covered: &covered,
            reads: &super::turn::Reads(self),
            zone: &zone,
        })
        .0
    }

    /// supervisor.py:4186 `_keep_recent`: when the tail beyond the last 20 exchanges holds one chunk, or the whole tail
    /// passes RECENT_CONVERSATION_TOKENS + FOLD_CHUNK_TOKENS, the entries beyond those exchanges fold in the
    /// background (`summarize`). One fold per chat at a time.
    pub(crate) fn start_keep_recent(&self, conversation: &str) {
        if self.live().summarizing.contains(conversation) {
            return;
        }
        let summary = self.summary_of(conversation);
        let entries = self.fold_timeline(conversation, summary.as_ref());
        let count = entries.len() - recent_entries(&entries).len();
        let tokens = self.deps.tokenizer;
        let excess: usize = entries[..count].iter().map(|e| tokens.count(&e.text)).sum();
        let total = excess
            + entries[count..]
                .iter()
                .map(|e| tokens.count(&e.text))
                .sum::<usize>();
        if count == 0
            || (excess < FOLD_CHUNK_TOKENS
                && total <= RECENT_CONVERSATION_TOKENS + FOLD_CHUNK_TOKENS)
        {
            return;
        }
        self.live().summarizing.insert(conversation.to_string());
        // Python's job runs as soon as the turn's routing first yields: up to its first summary call's start (its
        // request id and span), before the reply is recorded; the rest runs beside the turn
        let previous = summary;
        let mut first = entries;
        first.truncate(count);
        let opened = self.fold_open(conversation, first.len());
        let me = self.me();
        let conversation = conversation.to_string();
        tokio::spawn(async move {
            me.summarize(&conversation, count, Some((previous, first, opened)))
                .await;
            me.live().summarizing.remove(&conversation);
        });
    }

    /// supervisor.py:4207 `_summarize`: fold the `count` oldest entries after the current summary, a chunk per call;
    /// a failed call ends the refresh. `first`: the first call, already opened (`start_keep_recent`).
    #[allow(clippy::type_complexity)]
    pub(crate) async fn summarize(
        &self,
        conversation: &str,
        mut count: usize,
        mut first: Option<(Option<Entry>, Vec<HistEntry>, (String, String))>,
    ) {
        while count > 0 {
            let (previous, entries, opened) = match first.take() {
                Some(f) => f,
                None => {
                    let previous = self.summary_of(conversation);
                    let mut entries = self.fold_timeline(conversation, previous.as_ref());
                    entries.truncate(count);
                    if entries.is_empty() {
                        return;
                    }
                    let opened = self.fold_open(conversation, entries.len());
                    (previous, entries, opened)
                }
            };
            if entries.is_empty() {
                return;
            }
            let folded = self
                .fold(conversation, previous.as_ref(), &entries, opened)
                .await;
            if folded == 0 {
                return;
            }
            count = count.saturating_sub(folded);
        }
    }

    /// `_fold`'s start: its request id and its `model.summary` span.
    fn fold_open(&self, conversation: &str, entries: usize) -> (String, String) {
        let request_id = format!("summary-{}", self.new_id());
        let span = self.span_start(
            "model.summary",
            None,
            None,
            None,
            json!({"session_id": conversation, "entries": entries}),
        );
        (request_id, span)
    }

    /// supervisor.py:4223 `_fold`: one summary call over `previous` and the oldest of `entries` that fit it
    /// (`foldable`; an oldest entry too big alone is sent to be refused by name). How many entries it folded; 0 when it
    /// failed (recorded).
    async fn fold(
        &self,
        conversation: &str,
        previous: Option<&Entry>,
        entries: &[HistEntry],
        (request_id, span): (String, String),
    ) -> usize {
        let mut final_ = String::new();
        match self
            .fold_call(
                conversation,
                previous,
                entries,
                &request_id,
                &span,
                &mut final_,
            )
            .await
        {
            Ok(folded) => folded,
            Err((class, message)) => {
                let error = format!("{class}: {message}");
                self.span_finish(
                    &span,
                    Some(json!({"text": final_})),
                    Some(&error),
                    None,
                    None,
                );
                self.journal(
                    "chat.summary_failed",
                    json!({"session_id": conversation, "error": error}),
                    None,
                );
                0
            }
        }
    }

    async fn fold_call(
        &self,
        conversation: &str,
        previous: Option<&Entry>,
        entries: &[HistEntry],
        request_id: &str,
        span: &str,
        final_: &mut String,
    ) -> Result<usize, (String, String)> {
        let failed = |e: ferb_worker::Error| (e.class(), e.to_string());
        let schema = ferb_context::schemas::summary_schema();
        let rt = self
            .deps
            .daemon
            .make_runtime("orchestrator", "supervisor", &[], &schema, None)
            .map_err(failed)?;
        let system = self
            .deps
            .prompts
            .load(&format!("{}/summary.md", crate::prompts::APPROACH), &[])
            .unwrap_or_else(|e| panic!("the prompt package: {e}"));
        let previous_text = previous.map(|p| p.text("text"));
        let mut folded = self.foldable(&*rt, &system, previous_text.as_deref(), entries)?;
        if folded.is_empty() {
            folded = entries[..1].to_vec();
        }
        let tokens = self.deps.tokenizer;
        // a notice's text has its own purge and forget paths: it is never copied into the summary
        let copied: Vec<HistEntry> = folded
            .iter()
            .filter(|e| e.kind != "chat.notice")
            .map(|e| HistEntry {
                text: within_tokens(&e.text, FOLD_CHUNK_TOKENS, tokens),
                ..e.clone()
            })
            .collect();
        let task = Task {
            id: format!("{request_id}.summary"),
            parent_id: None,
            goal: "conversation summary".into(),
            device_id: self.deps.device_id.clone(),
            identity: Identity::Owner,
            scopes: BTreeSet::new(),
            role: TaskRole::Mixed,
            request_id: request_id.to_string(),
            success_conditions: Vec::new(),
            deadline: None,
        };
        let now = self.now();
        let zone = self.deps.clock.zone();
        let texts: Vec<String> = copied.iter().map(|e| e.text.clone()).collect();
        let wrap = |t: &str| self.wrap(t);
        let blocks = summary_blocks(
            previous_text.as_deref(),
            &texts,
            Some(&now),
            &zone,
            tokens,
            &wrap,
        );
        let worker = self.worker.get().expect("the worker").clone();
        let call = ferb_worker::worker::Call {
            task: &task,
            system,
            context: blocks,
            token: "",
            parent_span: span,
            tree: None,
            worker_id: "",
            images: Vec::new(),
            hooks: Default::default(),
            retry_silent: true,
        };
        *final_ = worker.run_call(&rt, call).await.map_err(failed)?;
        let text = summary_text(final_)?;
        let mut made: BTreeSet<String> = BTreeSet::from([origin::INFERRED.to_string()]);
        for e in &copied {
            made.extend(e.origins.iter().cloned());
        }
        if let Some(p) = previous {
            made.extend(origin::split(Some(&p.origin)));
        }
        let mut refs: BTreeSet<String> = copied.iter().map(|e| e.handle.clone()).collect();
        if let Some(p) = previous {
            refs.insert(format!("{ENTRY}{}", p.id));
        }
        // what it was shown (GATE-519): each entry's own text, the summary before it, and FERB's own words
        let mut shown: Vec<(BTreeSet<String>, String, Option<Value>)> = copied
            .iter()
            .map(|e| (e.origins.clone(), e.payload.clone(), e.values.clone()))
            .collect();
        if let Some(p) = previous {
            shown.push((
                origin::split(Some(&p.origin)),
                p.text("text"),
                stored_values(SUMMARY_KIND, p.data.as_object().unwrap_or(&Map::new())),
            ));
        }
        let bound = summary_blocks(None, &[], Some(&now), &zone, tokens, &wrap)[0]
            .text
            .clone();
        let mut own = vec![bound];
        own.extend(copied.iter().map(|e| {
            format!(
                "({})",
                ferb_clock::py::fmt(&zone.local(&e.at), "%a %d %b %H:%M")
            )
        }));
        shown.push((
            BTreeSet::from([origin::FERB.to_string()]),
            own.join("\n"),
            None,
        ));
        let made_list: Vec<String> = made.iter().cloned().collect();
        let provenance = ferb_policy::carried::derive(&text, &shown_of(&shown), &made_list);
        let kinds = origin::kinds(made.iter().map(String::as_str));
        let mut data = Map::new();
        data.insert("session_id".into(), json!(conversation));
        data.insert("text".into(), json!(text));
        data.insert(
            "covered".into(),
            json!(folded.iter().map(|e| e.handle.clone()).collect::<Vec<_>>()),
        );
        data.insert(
            "source_refs".into(),
            json!(refs.into_iter().collect::<Vec<_>>()),
        );
        data.insert(ferb_policy::carried::PROVENANCE.into(), provenance);
        self.journal(
            SUMMARY_KIND,
            Value::Object(data),
            Some(&origin::join(kinds.iter().map(String::as_str))),
        );
        // MEM-429: this chat's prefix changes anyway
        self.deps
            .daemon
            .settle_identity("a chat's running summary folded");
        self.span_finish(
            span,
            Some(json!({"text": final_.clone(), "folded": folded.len()})),
            None,
            None,
            None,
        );
        Ok(folded.len())
    }

    /// supervisor.py:4270 `_foldable`: the oldest entries for one summary call beside `previous`: at most
    /// FOLD_CHUNK_TOKENS of them, within the model's window and its runtime's per-input character cap, counted as the
    /// call assembles them (each entry with its separator).
    fn foldable(
        &self,
        rt: &dyn ferb_worker::ports::ModelRuntime,
        system: &str,
        previous: Option<&str>,
        entries: &[HistEntry],
    ) -> Result<Vec<HistEntry>, (String, String)> {
        let tokens = self.deps.tokenizer;
        let limits = rt.limits().map_err(|e| {
            (
                "ServiceError".to_string(),
                format!("no documented context window: {e}"),
            )
        })?;
        let budget = limits.context_budget();
        let reserved =
            crate::planner::reserve_tokens(tokens, system, &[], rt.output_schema().as_ref());
        let zone = self.deps.clock.zone();
        let wrap = |t: &str| self.wrap(t);
        let base = summary_blocks(previous, &[], None, &zone, tokens, &wrap)
            .iter()
            .filter(|b| !b.text.is_empty())
            .map(|b: &ContextBlock| b.text.clone())
            .collect::<Vec<_>>()
            .join("\n\n");
        let mut used =
            reserved + overhead_tokens(tokens, budget, reserved) + tokens.count(&base) as i64;
        let mut chars = overhead_chars(tokens, budget, reserved) + base.chars().count() as i64;
        let cap = limits.max_input_chars;
        let mut chunk: i64 = 0;
        let mut fitting = Vec::new();
        let sep = tokens.count("\n\n") as i64;
        for entry in entries {
            if entry.kind != "chat.notice" {
                // as wrap_untrusted writes it
                let text = entry.text.replace("</untrusted-data", "<\\/untrusted-data");
                let cost = tokens.count(&text) as i64 + sep;
                used += cost;
                chunk += cost;
                chars += text.chars().count() as i64 + 2;
                if chunk > FOLD_CHUNK_TOKENS as i64
                    || used > budget
                    || cap.is_some_and(|c| chars > c)
                {
                    break;
                }
            }
            fitting.push(entry.clone());
        }
        Ok(fitting)
    }
}

/// `SummaryOutcome.model_validate_json(final).summary`.
fn summary_text(final_: &str) -> Result<String, (String, String)> {
    let v: Value = serde_json::from_str(final_).map_err(|e| {
        (
            "ValidationError".to_string(),
            format!("1 validation error for SummaryOutcome\n  Invalid JSON: {e}"),
        )
    })?;
    match v.get("summary") {
        Some(Value::String(s)) => Ok(s.clone()),
        _ => Err((
            "ValidationError".into(),
            "1 validation error for SummaryOutcome\nsummary\n  Field required".into(),
        )),
    }
}

fn shown_of(shown: &[(BTreeSet<String>, String, Option<Value>)]) -> Vec<Shown> {
    super::turn::shown_records(shown)
}
