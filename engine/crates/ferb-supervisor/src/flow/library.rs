//! Front door step 8 (kb/plans/front-door.md "The card and the library"; ORCH-719 items 6 and 7; ORCH-727): the
//! library's block in the planner's and the front door's input, and the conversation's tags as ids. The block is
//! ferb-learning's view ([`ferb_learning::library::index_text`]): the dated index the Librarian wrote (ferb-daemon
//! `areas_a::library`) and the contents list over `memory_items`, the same bytes all day, placed right after Layer 1
//! so it rides in the cached part. The tags: each entry's `[cN]` is its id; a lookup or an answer that names an entry,
//! a result or its task by its tag gets the 32-hex id it stands for here, from this turn's own view (what the model
//! was shown), never by a guess.

use std::collections::BTreeSet;

use ferb_context::assemble::ContextBlock;

use super::Inner;
use crate::history::Tagged;
use crate::history::{as_tag, Sourced};
use crate::words::LOOKUPS;

/// What a tag is asked for: the entry itself or the task it is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Named {
    Entry,
    Task,
    Result,
}

impl Inner {
    /// The library's block for a turn now (None when there is nothing to show or the store can't be read).
    pub(crate) fn library_block(&self) -> Option<Sourced> {
        let zone = self.deps.clock.zone();
        let today = ferb_clock::py::fmt(&zone.local(&self.now()), "%Y-%m-%d");
        let day_of = |at: &str| {
            ferb_clock::py::parse(at).map(|t| ferb_clock::py::fmt(&zone.local(&t), "%Y-%m-%d"))
        };
        let (text, origins) = {
            let db = self.db();
            ferb_learning::library::index_text(db.conn(), &today, &day_of).ok()?
        };
        if text.is_empty() {
            return None;
        }
        Some(Sourced {
            block: ContextBlock {
                protected: true,
                cache: true,
                ..ContextBlock::new(text, "chat.history and memory.read")
            },
            origins: origins.into_iter().collect::<BTreeSet<String>>(),
        })
    }

    /// The 32-hex id a conversation tag (`c3`, `[c3]`) names for the turn a lookup's grant belongs to
    /// (`<request>.lookups`): the entry's journal id, or the task it is about; None when `value` is no tag, or no
    /// turn of that request showed it (the caller then uses `value` as it is).
    pub fn tagged(&self, caller_task: &str, value: &str, named: Named) -> Option<String> {
        let tag = as_tag(value)?;
        let rid = caller_task.strip_suffix(LOOKUPS).unwrap_or(caller_task);
        let turn = self.turn_of(rid)?;
        let t = turn.lock().unwrap();
        pick(t.tags.get(&tag)?, named)
    }

    /// The same for a turn the caller holds: an answer's `earlier_results` and `control.task_id`.
    pub(crate) fn tagged_in(
        &self,
        turn: &std::sync::Arc<std::sync::Mutex<super::Turn>>,
        value: &str,
        named: Named,
    ) -> Option<String> {
        let tag = as_tag(value)?;
        let t = turn.lock().unwrap();
        pick(t.tags.get(&tag)?, named)
    }
}

fn pick(t: &Tagged, named: Named) -> Option<String> {
    let out = match named {
        Named::Entry => &t.id,
        Named::Task => &t.task,
        Named::Result => &t.result,
    };
    (!out.is_empty()).then(|| out.clone())
}
