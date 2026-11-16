//! What a phone has fetched of one conversation: the newest copy of each entry, whatever order the pages, changes and
//! watch frames brought them in (README "What a phone handles that the Mac doesn't": keep what was fetched by id and
//! fold the union). Applying is idempotent and order-independent (`ChatEntry::newer_than`), so a duplicate, a late
//! older revision or a page that overlaps one already held changes nothing.

use std::collections::HashMap;

use crate::entry::ChatEntry;
use crate::fold::{self, Transcript};

#[derive(Debug, Clone, Default)]
pub struct EntrySet {
    by_id: HashMap<String, ChatEntry>,
}

/// What applying one entry did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// A new entry or a newer revision: kept.
    Kept,
    /// The same or an older revision than the one held: nothing changed.
    Stale,
    /// Its words were cut to fit a reply (`body_cut`): not kept; read it whole with `chat.entries.get`.
    Cut,
}

impl EntrySet {
    pub fn new() -> EntrySet {
        EntrySet::default()
    }

    pub fn from_entries(entries: impl IntoIterator<Item = ChatEntry>) -> EntrySet {
        let mut s = EntrySet::new();
        for e in entries {
            s.apply(e);
        }
        s
    }

    pub fn apply(&mut self, e: ChatEntry) -> Applied {
        if e.cut {
            return Applied::Cut;
        }
        match self.by_id.get(&e.id) {
            Some(kept) if !e.newer_than(kept) => Applied::Stale,
            _ => {
                self.by_id.insert(e.id.clone(), e);
                Applied::Kept
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<&ChatEntry> {
        self.by_id.get(id)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// The lowest `pos` held: where the next older page starts (`before_pos`).
    pub fn lowest_pos(&self) -> Option<i64> {
        self.by_id.values().map(|e| e.pos).min()
    }

    /// The highest `pos` held: the newest entry this phone has of the conversation.
    pub fn highest_pos(&self) -> Option<i64> {
        self.by_id.values().map(|e| e.pos).max()
    }

    /// The highest `seq` held.
    pub fn highest_seq(&self) -> Option<i64> {
        self.by_id.values().map(|e| e.seq).max()
    }

    pub fn entries(&self) -> impl Iterator<Item = &ChatEntry> {
        self.by_id.values()
    }

    pub fn clear(&mut self) {
        self.by_id.clear();
    }

    /// The conversation as a phone draws it.
    pub fn transcript(&self, utc_offset_s: i32) -> Transcript {
        let mut newest: Vec<&ChatEntry> = self.by_id.values().collect();
        newest.sort_by(|a, b| a.pos.cmp(&b.pos).then(a.id.cmp(&b.id)));
        fold::transcript_of(&newest, utc_offset_s)
    }
}
