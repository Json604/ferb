//! The owner's review of what FERB learned about him (kb MEM-460..457, MEM-485, MEM-486), as both phones draw it: one
//! list, each line ticked until he unticks it, answered all at once (`memory.review`, `memory.review.all`, both
//! private: the unlock window), in FERB's own question's card (one to five waiting) or in the sheet the day's note
//! opens. The words, the parts, the groups and the buttons are the Mac's (ferb-learning `import_review`); the phone
//! only lays them out. Nothing here is stored on the phone. `memory.review.daily` tells the Mac the owner opened FERB.

use serde::Serialize;
use serde_json::{json, Value};

use crate::api::{Mobile, MobileResult, Shared};

/// The capability of the review's question: a question with it is drawn as the review card.
pub const CAPABILITY: &str = "memory.review";

/// One answer a button gives: what is sent (`answer`) and its words (`label`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct ReviewButton {
    pub answer: String,
    pub label: String,
}

/// One item: the memory's id, the line in plain words, a small note under it ("Already always on", "Last said …"),
/// and its own buttons when they differ from its part's (one already always on: Keep / Remove, MEM-464); empty: the
/// part's `answers`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct ReviewItem {
    pub id: String,
    pub text: String,
    pub note: String,
    pub answers: Vec<ReviewButton>,
}

impl ReviewItem {
    /// The buttons this item shows: its own, else its part's.
    pub fn buttons<'a>(&'a self, part: &'a ReviewPart) -> &'a [ReviewButton] {
        if self.answers.is_empty() {
            &part.answers
        } else {
            &self.answers
        }
    }
}

/// A group of items under its title ("" for none), and its own buttons for all of its items at once when they are all
/// already always on and the part has others (Keep all / Remove all, MEM-464); usually empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct ReviewGroup {
    pub title: String,
    pub items: Vec<ReviewItem>,
    pub all: Vec<ReviewButton>,
}

impl ReviewGroup {
    /// Every item's id, for the group's answer to all of them at once.
    pub fn ids(&self) -> Vec<String> {
        self.items.iter().map(|i| i.id.clone()).collect()
    }
}

/// One part of the review: always_on, still_true or offers, its question, the line under it, its groups, the answers
/// each item takes and the answers for all of them at once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct ReviewPart {
    pub part: String,
    pub title: String,
    pub hint: String,
    pub groups: Vec<ReviewGroup>,
    pub answers: Vec<ReviewButton>,
    pub all: Vec<ReviewButton>,
}

/// The review as it stands: its parts (none when everything is answered), how many items are left, and the whole
/// list's buttons at its top (MEM-486): `all` (Approve all, Decline all), `some` ("Approve {n}", in place of
/// Approve all while some lines are unticked) and `hint` (the line above the items).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct MemoryReview {
    pub parts: Vec<ReviewPart>,
    pub count: u64,
    pub all: Vec<ReviewButton>,
    pub some: String,
    pub hint: String,
}

fn text(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn list<T>(v: &Value, k: &str, f: impl Fn(&Value) -> T) -> Vec<T> {
    v.get(k)
        .and_then(Value::as_array)
        .map(|a| a.iter().map(f).collect())
        .unwrap_or_default()
}

fn button(v: &Value) -> ReviewButton {
    ReviewButton {
        answer: text(v, "answer"),
        label: text(v, "label"),
    }
}

impl MemoryReview {
    /// The engine's answer (`memory.review`), read leniently: a missing field is empty.
    pub fn read(v: &Value) -> MemoryReview {
        MemoryReview {
            parts: list(v, "parts", |p| ReviewPart {
                part: text(p, "part"),
                title: text(p, "title"),
                hint: text(p, "hint"),
                groups: list(p, "groups", |g| ReviewGroup {
                    title: text(g, "title"),
                    items: list(g, "items", |i| ReviewItem {
                        id: text(i, "id"),
                        text: text(i, "text"),
                        note: text(i, "note"),
                        answers: list(i, "answers", button),
                    }),
                    all: list(g, "all", button),
                }),
                answers: list(p, "answers", button),
                all: list(p, "all", button),
            }),
            count: v.get("count").and_then(Value::as_u64).unwrap_or(0),
            all: list(v, "all", button),
            some: text(v, "some"),
            hint: text(v, "hint"),
        }
    }

    /// Every item's id, in the order shown.
    pub fn ids(&self) -> Vec<String> {
        self.parts.iter().flat_map(ReviewPart::ids).collect()
    }

    /// The main button's words with `ticked` of the list's items ticked (MEM-486): Approve all when every one is,
    /// "Approve {n}" when some are unticked, none when no item is ticked (only Decline all is left).
    pub fn approve_label(&self, ticked: usize) -> Option<String> {
        let total = self.ids().len();
        if ticked == 0 || total == 0 {
            return None;
        }
        let all = self
            .all
            .first()
            .map(|b| b.label.clone())
            .unwrap_or_default();
        Some(if ticked >= total || self.some.is_empty() {
            all
        } else {
            self.some.replace("{n}", &ticked.to_string())
        })
    }
}

impl ReviewPart {
    /// Every item's id, for an answer to all of them at once.
    pub fn ids(&self) -> Vec<String> {
        self.groups
            .iter()
            .flat_map(|g| g.items.iter().map(|i| i.id.clone()))
            .collect()
    }
}

impl Mobile {
    /// The review of what an import brought in (`memory.review`; needs the unlock). Network.
    pub fn memory_review(&self) -> MobileResult<MemoryReview> {
        let t = self.shared.transport_online()?;
        Ok(MemoryReview::read(&Shared::call(
            &t,
            "memory.review",
            json!({}),
        )?))
    }

    /// The owner's answer to items of one part, one or all at once (`memory.review.answer`; needs the unlock): the
    /// review as it stands after it. Network.
    pub fn answer_memory_review(
        &self,
        part: &str,
        item_ids: &[String],
        answer: &str,
    ) -> MobileResult<MemoryReview> {
        let t = self.shared.transport_online()?;
        Ok(MemoryReview::read(&Shared::call(
            &t,
            "memory.review.answer",
            json!({"part": part, "item_ids": item_ids, "answer": answer}),
        )?))
    }

    /// The whole list answered at once (`memory.review.all`; needs the unlock): `approve` the ticked lines, `decline`
    /// the unticked ones (Decline all: every line). The review as it stands after it. Network.
    pub fn answer_memory_review_all(
        &self,
        approve: &[String],
        decline: &[String],
    ) -> MobileResult<MemoryReview> {
        let t = self.shared.transport_online()?;
        Ok(MemoryReview::read(&Shared::call(
            &t,
            "memory.review.all",
            json!({"approve": approve, "decline": decline}),
        )?))
    }

    /// The owner opened FERB on this phone (`memory.review.daily`, MEM-485): the Mac's first open of the day shows
    /// what waits to check in the chat. `client` names the phone ("android", "iphone"). Network; a failure is the
    /// caller's to ignore (the next open says it again).
    pub fn review_opened(&self, client: &str) -> MobileResult<()> {
        let t = self.shared.transport_online()?;
        Shared::call(&t, "memory.review.daily", json!({"client": client}))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // decision: MEM-460 — the phone draws the Mac's parts, groups, words and buttons as they come
    #[test]
    fn the_review_reads_as_the_engine_sends_it() {
        let v = json!({"count": 2, "parts": [
            {"part": "always_on", "title": "Keep these in mind all the time?", "hint": "h",
             "groups": [{"title": "How you like me to work",
                         "items": [{"id": "i1", "text": "You prefer short answers", "note": ""},
                                   {"id": "i2", "text": "You use Arc", "note": "Already always on",
                                    "answers": [{"answer": "approve", "label": "Keep"},
                                                {"answer": "decline", "label": "Remove"}]}]}],
             "answers": [{"answer": "approve", "label": "Approve"}, {"answer": "decline", "label": "Decline"}],
             "all": [{"answer": "approve", "label": "Approve all"}]}]});
        let r = MemoryReview::read(&v);
        assert_eq!(r.count, 2);
        assert_eq!(r.parts[0].title, "Keep these in mind all the time?");
        assert_eq!(r.parts[0].groups[0].items[1].note, "Already always on");
        assert_eq!(r.parts[0].answers[1].label, "Decline");
        assert_eq!(r.parts[0].ids(), vec!["i1".to_string(), "i2".to_string()]);
        // MEM-464: one already always on shows Keep / Remove, the others their part's buttons
        let g = &r.parts[0].groups[0];
        assert_eq!(g.items[0].buttons(&r.parts[0])[0].label, "Approve");
        assert_eq!(g.items[1].buttons(&r.parts[0])[1].label, "Remove");
        assert!(g.all.is_empty());
        assert_eq!(MemoryReview::read(&json!({})), MemoryReview::default());
    }

    // decision: MEM-488 — the whole list's buttons come from the Mac; the main one names how many are ticked
    #[test]
    fn the_main_button_says_how_many_are_ticked() {
        let v = json!({"count": 3, "some": "Approve {n}", "hint": "Untick anything that isn't right.",
            "all": [{"answer": "approve", "label": "Approve all"}, {"answer": "decline", "label": "Decline all"}],
            "parts": [{"part": "always_on", "groups": [{"title": "", "items": [{"id": "a"}, {"id": "b"}]}]},
                      {"part": "still_true", "groups": [{"title": "", "items": [{"id": "c"}]}]}]});
        let r = MemoryReview::read(&v);
        assert_eq!(r.ids(), vec!["a", "b", "c"]);
        assert_eq!(r.hint, "Untick anything that isn't right.");
        assert_eq!(r.all[1].label, "Decline all");
        assert_eq!(r.approve_label(3).as_deref(), Some("Approve all"));
        assert_eq!(r.approve_label(2).as_deref(), Some("Approve 2"));
        assert_eq!(r.approve_label(0), None);
    }
}
