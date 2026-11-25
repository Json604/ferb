//! The owner's review of what an import brought in (MEM-460, MEM-461, MEM-462; his words of 2026-11-23): one list he
//! answers item by item or all at once, in three parts.
//!
//! - **Keep in mind all the time?** An import never puts anything in the always-on profile by itself (MEM-460,
//!   which replaced the join at once for imports). What FERB thinks should be always on (a recent repeat, IMP-47's
//!   rule, now a suggestion) is listed for his Approve / Decline (Keep / Remove for one already on, MEM-464). Approving adds it to the profile as his pin; declining keeps it
//!   remembered and searchable, never always on. Lines an earlier build put in the profile by themselves are listed
//!   too and stay on until he declines each (MEM-461): nothing changes behind his back.
//! - **Are these still true?** Lines from a while ago that steer FERB's work (how he likes it to work, his setup, his
//!   recurring things and projects) are asked once (MEM-462, for IMP-41); a fact about him that steers nothing is kept,
//!   marked "last said …" on the Memory page, and not asked.
//!   A fact whose check date came (the planner's `review_in_days`, an import's) is asked there too, whatever that
//!   limit says, found by code from its date alone (MEM-472): Yes keeps it and clears the date, No ends it, Not sure
//!   asks again in a week.
//! - **Still want help with these?** Offers 30 to 60 days old, at most five, newest first (MEM-444, IMP-51).
//!
//! FERB's own understanding (not an import's) is in the first part too (MEM-480, MEM-481): what would once have
//! joined always on by itself (MEM-424's two days or a week) waits there for the owner's Approve, as a suggestion.
//!
//! **How he is asked (MEM-485, MEM-486).** Never one by one, and never at the moment something is learned: code
//! collects what waits (no model), and at the day's first open of FERB (FERB.app or a phone; `Daily`) one to
//! `CARD_MOST` are one small card showing them all, with Approve all on top (each line ticked until he unticks it);
//! more are one note that opens the list. Nothing expires; what he hasn't answered waits.
//!
//! An answered item leaves the list; the list stays until every item is answered. A line's state is on the line
//! (`always_on`: suggested, on, approved, declined; `asking_old`; `asking`), so every client and the owner's typed
//! answer read the same list (`review`).

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use crate::import_job::{ImportJob, SOURCE_KIND};
use crate::import_profile::{object, Line, ASK_MOST, GUIDES};
use crate::profile::CoreProfile;
use crate::py;
use crate::sql;
use crate::{Error, Result};
use ferb_memory::pystr;

/// A line FERB suggests for the always-on profile, waiting for the owner's word.
pub const SUGGESTED: &str = "suggested";
/// A line an earlier build put in the profile by itself. Read only from older data: since MEM-480 such a line left the
/// profile and is `suggested` until the owner's word.
pub const ON: &str = "on";
pub const APPROVED: &str = "approved";
pub const DECLINED: &str = "declined";

/// The three parts, in the order shown.
pub const ALWAYS_ON: &str = "always_on";
pub const STILL_TRUE: &str = "still_true";
pub const OFFERS: &str = "offers";

/// Owner-facing words (listed in journal 178).
pub const ALWAYS_ON_TITLE: &str = "Keep these in mind all the time?";
pub const ALWAYS_ON_HINT: &str =
    "I'll remember all of them either way. The ones you approve, I'll keep in mind in everything I do.";
pub const STILL_TRUE_TITLE: &str = "Are these still true?";
pub const STILL_TRUE_HINT: &str = "You told me these a while ago.";
pub const OFFERS_TITLE: &str = "Still want help with these?";
pub const OFFERS_HINT: &str = "I won't start any of them on my own.";
pub const GROUP_WORK: &str = "How you like me to work";
pub const GROUP_YOU: &str = "About you";
pub const GROUP_SETUP: &str = "Your setup";
pub const ON_NOTE: &str = "Already always on";
/// The card's question, its first line and what it says under it (the card is its face; this is what the planner
/// reads). MEM-485: asked only by the day's first open, with five or fewer to check.
pub const ASK_TITLE: &str = "Can you check a few things I learned about you?";
pub const ASK_ANSWER_HOW: &str = "Untick anything that isn't right, or just tell me.";
/// MEM-485, MEM-486: the buttons for the whole list, at its top (the card's and the review list's): every ticked
/// item gets its part's yes (Approve, Yes, Yes), every unticked one its part's no (Decline, No, not any more, No
/// thanks). `APPROVE_SOME` stands in for Approve all while some are unticked: `{n}` is how many are ticked.
pub const APPROVE_ALL: &str = "Approve all";
pub const APPROVE_SOME: &str = "Approve {n}";
pub const DECLINE_ALL: &str = "Decline all";
/// What the list says above its items.
pub const UNTICK_HINT: &str = "Untick anything that isn't right.";
/// MEM-485: at the day's first open, more than this many waiting is a note, not a card (the owner's number).
pub const CARD_MOST: usize = 5;
/// MEM-485: the day's note when more than `CARD_MOST` wait: `{n}` is how many. Its button opens the review list.
pub const NOTE_LINE: &str = "I learned {n} things about you. Review them when you like.";
pub const NOTE_BUTTON: &str = "Review";

/// What the day's first open shows (MEM-485): nothing waiting, nothing; one to `CARD_MOST`, the card; more, the note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Daily {
    Nothing,
    Card,
    Note,
}

impl Daily {
    /// The rule, by the number waiting.
    pub fn of(count: usize) -> Daily {
        match count {
            0 => Daily::Nothing,
            n if n <= CARD_MOST => Daily::Card,
            _ => Daily::Note,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Daily::Nothing => "nothing",
            Daily::Card => "card",
            Daily::Note => "note",
        }
    }
}

/// The day's note, for `n` waiting.
pub fn note_line(n: usize) -> String {
    NOTE_LINE.replace("{n}", &n.to_string())
}

/// Each part's yes and no, what the whole list's buttons give a ticked and an unticked item (MEM-486).
pub const YES_NO: [(&str, &str, &str); 3] = [
    (ALWAYS_ON, "approve", "decline"),
    (STILL_TRUE, "yes", "no"),
    (OFFERS, "open", "not interested"),
];
/// Under a fact whose check date came (MEM-472): `{day}` is that date, "5 Dec 2026".
pub const DUE_NOTE: &str = "I said I'd check this on {day}";
/// How many days a fact the owner answered "Not sure" about waits before it is asked again (MEM-472, FERB's
/// proposal: T-1296 asks the owner).
pub const UNSURE_AGAIN_DAYS: i64 = 7;
const DAY: i64 = 86_400_000_000;

/// A remembered fact whose check date has come (MEM-472): any fact with a review date, the planner's
/// (`review_in_days`) or an import's, current and in use. Found by code from the date alone, no model.
#[derive(Clone, Debug)]
pub struct Dated {
    pub item_id: String,
    /// The fact's own words as the review shows them (an imported line's as it shows its lines).
    pub text: String,
    /// Its check day, on this Mac's calendar ("2026-12-05").
    pub day: String,
    /// The import line the fact is, when it is one.
    pub line: Option<Line>,
}

/// Whether this item came in by an import: it rests on an import's journal row, or is the item of an import line.
pub fn imported(store: &crate::store::Store, item_id: &str) -> Result<bool> {
    store.exists(
        "SELECT 1 FROM memory_sources s JOIN journal j ON j.id = s.source_id \
         WHERE s.item_id = ? AND s.kind = 'journal' AND j.kind = ? \
         UNION ALL SELECT 1 FROM memory_import_lines WHERE item_id = ? LIMIT 1",
        sql![item_id, SOURCE_KIND, item_id],
    )
}

/// The owner's word on an imported item's place in the always-on profile, as its line keeps it ("" when none).
pub fn always_on_state(store: &crate::store::Store, item_id: &str) -> Result<String> {
    Ok(store
        .one(
            "SELECT COALESCE(json_extract(data_json, '$.always_on'), '') FROM memory_import_lines WHERE item_id = ?",
            sql![item_id],
        )?
        .map(|r| r.s_at(0))
        .unwrap_or_default())
}

/// The journal row of the start's step that took FERB's understanding out of always on until the owner's Yes
/// (MEM-482), written only when it moved something: what left and which import lines are asked again.
pub const WAITING_KIND: &str = "memory.inferred_waiting";

/// Which group a line shows under (MEM-460: how you like FERB to work, about you, your setup).
pub fn group_of(line: &Line) -> &'static str {
    match line.s("use").as_str() {
        "follow" => GROUP_WORK,
        "choose" => GROUP_SETUP,
        _ => GROUP_YOU,
    }
}

fn button(answer: &str, label: &str) -> Value {
    json!({"answer": answer, "label": label})
}

/// Whether a review item is FERB's own understanding (no import line holds it, MEM-481).
fn understood(l: &Line) -> bool {
    l.id.is_empty()
}

impl ImportJob<'_> {
    /// The core profile as the prompts read it, for the review's reads (its own writes go through the daemon's).
    fn core(&self) -> CoreProfile {
        CoreProfile::new(self.memory.clone(), Box::new(|_, _| Ok(())))
    }

    /// What is always on now, by item, with who put it there: the entries every prompt carries (MEM-480: an item of
    /// FERB's understanding without the owner's Yes is not one of them, whatever row is left).
    fn in_profile(&self) -> Result<std::collections::HashMap<String, String>> {
        Ok(self
            .core()
            .entries()?
            .into_iter()
            .map(|e| (e.item.id, e.added_by))
            .collect())
    }

    /// FERB's own understanding waiting for the owner's Yes (MEM-481), shaped as review lines with no import line:
    /// its words as kept (never reworded), grouped by kind (a preference under how he likes FERB to work, his setup
    /// under setup), dated by when it was first understood.
    fn understood_lines(&self) -> Result<Vec<Line>> {
        let mut out = Vec::new();
        for it in self.core().waiting_for_yes()? {
            let mut data = Map::new();
            data.insert(ALWAYS_ON.into(), json!(SUGGESTED));
            let use_ = match it.kind.as_str() {
                "preference" => "follow",
                "env" => "choose",
                _ => "",
            };
            data.insert("use".into(), json!(use_));
            data.insert("said".into(), json!(self.day_of(&py::iso(&it.valid_from))));
            out.push(Line {
                id: String::new(),
                item_id: it.id,
                kind: it.kind,
                subject: it.subject,
                key: it.key,
                content: it.content,
                origin: it.origin,
                data,
            });
        }
        Ok(out)
    }

    /// The start's step (MEM-480, MEM-482): FERB's understanding is always on only on the owner's Yes. An imported
    /// line in the profile that he approved stays (his Approve all of 23 Nov stands, MEM-483); one an earlier build
    /// put there by itself goes back to the review as a suggestion; FERB's own understanding there leaves unless he
    /// added it himself. Nothing is deleted: each stays remembered and found by lookups. By code at every start,
    /// journaled (`WAITING_KIND`) only when it moved something. The number of import lines asked.
    pub fn wait_for_yes(&self, profile: &CoreProfile) -> Result<usize> {
        let rows: Vec<(String, String)> = self
            .store
            .query("SELECT item_id, added_by FROM memory_profile", sql![])?
            .iter()
            .map(|r| (r.s_at(0), r.s_at(1)))
            .collect();
        if rows.is_empty() {
            return Ok(0);
        }
        let lines = self.lines()?;
        let kept = profile.owner_kept()?;
        let (mut left, mut asked) = (Vec::new(), Vec::new());
        for (item_id, by) in rows {
            let Some(it) = self.memory.get(&item_id)? else {
                continue;
            };
            if it.origin != "inferred" || kept.contains(&it.id) {
                continue;
            }
            match lines.iter().find(|l| l.item_id == item_id) {
                // decision: MEM-483 — his Approve stands, whenever he gave it
                Some(l) if l.s(ALWAYS_ON) == APPROVED => continue,
                Some(l) => {
                    let mut l = l.clone();
                    if !l.gone() && l.s(ALWAYS_ON) != DECLINED {
                        l.data.insert(ALWAYS_ON.into(), json!(SUGGESTED));
                        l.data.remove("always_on_at");
                        self.write_line(&l, false)?;
                        asked.push(l.item_id.clone());
                    }
                }
                None if by == "owner" => continue,
                None => {}
            }
            left.push(item_id);
        }
        if left.is_empty() {
            return Ok(0);
        }
        profile.remove(&left, "code")?;
        self.journal.append(
            WAITING_KIND,
            Some(json!({"left": left.len(), "asked_again": asked.len()})),
            None,
        )?;
        Ok(asked.len())
    }

    /// Marks what the always-on part lists (MEM-460, MEM-480), on every line no answer settled: one always on now
    /// is there on the owner's word (his pin, an older build's Keep) and is `approved`; one an earlier build put there
    /// by itself (`on`) and one that holds up by IMP-47 (a recent repeat) are `suggested`. Run after every import and
    /// at the start. The number newly marked.
    pub fn suggest_always_on(&self) -> Result<usize> {
        let today = self.today();
        let shown = self.in_profile()?;
        let rows = self.core().ids()?;
        let mut marked = 0;
        for mut l in self.lines()? {
            let state = l.s(ALWAYS_ON);
            if l.gone() || [APPROVED, DECLINED].contains(&state.as_str()) {
                continue;
            }
            let new = if shown.contains_key(&l.item_id) {
                APPROVED
            } else if state == SUGGESTED || l.over(&today) {
                continue;
            } else if state == ON || rows.contains(&l.item_id) || self.joins(&l) {
                // MEM-480: one an earlier build put in always on by itself waits for his Yes like any suggestion
                SUGGESTED
            } else {
                continue;
            };
            l.data.insert(ALWAYS_ON.into(), json!(new));
            self.write_line(&l, false)?;
            marked += 1;
        }
        Ok(marked)
    }

    /// The lines each part lists now, in the order shown. The older lines leave out a fact whose check date came:
    /// it is asked once, as one (`due_checks`).
    pub fn review_lines(&self) -> Result<(Vec<Line>, Vec<Line>, Vec<Line>)> {
        let (on, _, old, offers) = self.review_all()?;
        Ok((on, old, offers))
    }

    /// The facts whose check date has come (MEM-472), the earliest date first: listed under "Are these still true?"
    /// because their date came, whatever MEM-462's limit on older lines says. One the always-on part lists waits
    /// for that answer (the parts are never mixed); one set aside is out of use and not asked.
    pub fn due_checks(&self) -> Result<Vec<Dated>> {
        Ok(self.review_all()?.1)
    }

    fn dated_among(&self, lines: &[Line], on: &[Line]) -> Result<Vec<Dated>> {
        let listed: BTreeSet<&str> = on.iter().map(|l| l.item_id.as_str()).collect();
        let mut out = Vec::new();
        for it in self.memory.due_for_review()? {
            let Some(at) = it.review_at else { continue };
            if it.set_aside || listed.contains(it.id.as_str()) {
                continue;
            }
            let line = lines
                .iter()
                .find(|l| l.item_id == it.id && !l.gone())
                .cloned();
            let text = match &line {
                Some(l) => l.scoped(&self.said_as(l)),
                None => pystr::split_ws(&it.content).collect::<Vec<_>>().join(" "),
            };
            out.push(Dated {
                item_id: it.id.clone(),
                text,
                day: self.day_of(&py::iso(&at)),
                line,
            });
        }
        Ok(out)
    }

    /// Every list the review shows: always on, the facts whose date came, the older lines, the offers.
    fn review_all(&self) -> Result<(Vec<Line>, Vec<Dated>, Vec<Line>, Vec<Line>)> {
        let all = self.lines()?;
        let (on, old, offers) = self.review_lists(&all)?;
        let dated = self.dated_among(&all, &on)?;
        let due: BTreeSet<&str> = dated.iter().map(|d| d.item_id.as_str()).collect();
        let old = old
            .into_iter()
            .filter(|l| !due.contains(l.item_id.as_str()))
            .collect();
        Ok((on, dated, old, offers))
    }

    fn review_lists(&self, all: &[Line]) -> Result<(Vec<Line>, Vec<Line>, Vec<Line>)> {
        let today = self.today();
        let lines: Vec<Line> = all
            .iter()
            .filter(|l| !l.gone() && !l.over(&today))
            .cloned()
            .collect();
        let order = |g: &str| {
            [GROUP_WORK, GROUP_YOU, GROUP_SETUP]
                .iter()
                .position(|x| *x == g)
        };
        let mut on: Vec<Line> = lines
            .iter()
            .filter(|l| [SUGGESTED, ON].contains(&l.s(ALWAYS_ON).as_str()))
            .cloned()
            .collect();
        // MEM-481: FERB's own understanding waiting for his Yes, in the same part
        on.extend(self.understood_lines()?);
        on.sort_by_key(|l| {
            (
                order(group_of(l)),
                std::cmp::Reverse(l.said()),
                l.id.clone(),
            )
        });
        let listed: BTreeSet<String> = on.iter().map(|l| l.id.clone()).collect();
        let mut old: Vec<Line> = lines
            .iter()
            .filter(|l| l.b("asking_old") && !listed.contains(&l.id))
            .cloned()
            .collect();
        old.sort_by_key(|l| std::cmp::Reverse((l.said(), l.id.clone())));
        let mut offers: Vec<Line> = lines
            .iter()
            .filter(|l| l.b("asking") && !l.offer().is_empty())
            .cloned()
            .collect();
        offers.sort_by_key(|l| std::cmp::Reverse((l.said(), l.id.clone())));
        Ok((on, old, offers))
    }

    /// The review as every client draws it (`memory.review`): its parts, each with its title, a line under it, its
    /// groups of items, the answers each item takes and the answers for all at once. Empty parts are left out.
    pub fn review(&self) -> Result<Value> {
        let (on, dated, old, offers) = self.review_all()?;
        let profile = self.in_profile()?;
        let item = |l: &Line, text: String, note: String| json!({"id": l.item_id, "text": text, "note": note});
        let mut parts = Vec::new();
        if !on.is_empty() {
            // MEM-464: one already always on is kept or removed (it stays remembered either way); one FERB suggests is
            // approved or declined. A group all already on has its own Keep all / Remove all; a part all already on
            // has them in place of Approve all / Decline all.
            let is_on = |l: &Line| profile.contains_key(&l.item_id);
            let keep = || vec![button("approve", "Keep"), button("decline", "Remove")];
            let keep_all = || {
                vec![
                    button("approve", "Keep all"),
                    button("decline", "Remove all"),
                ]
            };
            let every_on = on.iter().all(is_on);
            let mut groups: Vec<Value> = Vec::new();
            for g in [GROUP_WORK, GROUP_YOU, GROUP_SETUP] {
                let mine: Vec<&Line> = on.iter().filter(|l| group_of(l) == g).collect();
                if mine.is_empty() {
                    continue;
                }
                let items: Vec<Value> = mine
                    .iter()
                    .map(|l| {
                        let mut v = item(
                            l,
                            l.scoped(&self.said_as(l)),
                            if is_on(l) { ON_NOTE } else { "" }.into(),
                        );
                        v["answers"] = json!(if is_on(l) { keep() } else { Vec::new() });
                        v
                    })
                    .collect();
                let group_all = if !every_on && mine.len() > 1 && mine.iter().all(|l| is_on(l)) {
                    keep_all()
                } else {
                    Vec::new()
                };
                groups.push(json!({"title": g, "items": items, "all": group_all}));
            }
            parts.push(
                json!({"part": ALWAYS_ON, "title": ALWAYS_ON_TITLE, "hint": ALWAYS_ON_HINT,
                "groups": groups,
                "answers": [button("approve", "Approve"), button("decline", "Decline")],
                "all": if every_on { keep_all() } else {
                    vec![button("approve", "Approve all"), button("decline", "Decline all")] }}),
            );
        }
        if !dated.is_empty() || !old.is_empty() {
            // MEM-472: the facts whose check date came first, each with that date
            let items: Vec<Value> = dated
                .iter()
                .map(|d| json!({"id": d.item_id, "text": d.text, "note": due_note(&d.day)}))
                .chain(old.iter().map(|l| {
                    item(
                        l,
                        l.scoped(&self.said_as(l)),
                        format!("Last said {}", py_day(&l.said())),
                    )
                }))
                .collect();
            parts.push(json!({"part": STILL_TRUE, "title": STILL_TRUE_TITLE, "hint": STILL_TRUE_HINT,
                "groups": [{"title": "", "items": items, "all": []}],
                "answers": [button("yes", "Yes"), button("no", "No, not any more"), button("unsure", "Not sure")],
                "all": [button("yes", "Yes to all"), button("no", "No to all")]}));
        }
        if !offers.is_empty() {
            let items: Vec<Value> = offers
                .iter()
                .map(|l| item(l, l.scoped(&object(&l.offer())), String::new()))
                .collect();
            parts.push(
                json!({"part": OFFERS, "title": OFFERS_TITLE, "hint": OFFERS_HINT,
                "groups": [{"title": "", "items": items, "all": []}],
                "answers": [button("open", "Yes"), button("done", "Done already"),
                            button("not interested", "No thanks")],
                "all": [button("open", "Yes to all"), button("not interested", "No to all")]}),
            );
        }
        let count: usize = on.len() + dated.len() + old.len() + offers.len();
        // MEM-486: the whole list's buttons, at its top; each line is ticked until the owner unticks it
        Ok(json!({"parts": parts, "count": count,
            "all": [button("approve", APPROVE_ALL), button("decline", DECLINE_ALL)],
            "some": APPROVE_SOME, "hint": UNTICK_HINT}))
    }

    /// The line in plain words to the owner ("You …"), else its words as kept.
    fn said_as(&self, l: &Line) -> String {
        let you = l.s("you");
        if you.is_empty() {
            l.content.clone()
        } else {
            you
        }
    }

    /// The owner's answer to items of one part (`memory.review.answer`, and his words read by `apply_answer`):
    /// always on (approve, decline), still true (yes, no, unsure) or an offer (open, done, not interested). An item
    /// not in that part now (answered already, or gone) is passed over. The number answered.
    pub fn answer_review(
        &self,
        profile: &CoreProfile,
        part: &str,
        item_ids: &[String],
        answer: &str,
    ) -> Result<usize> {
        let (on, dated, old, offers) = self.review_all()?;
        let (listed, answers): (&[Line], &[&str]) = match part {
            ALWAYS_ON => (&on, &["approve", "decline"]),
            STILL_TRUE => (&old, &["yes", "no", "unsure"]),
            OFFERS => (&offers, &["open", "done", "not interested"]),
            _ => return Err(Error::Value(format!("no part {part} in the review"))),
        };
        if !answers.contains(&answer) {
            return Err(Error::Value(format!("{part} takes {}", answers.join(", "))));
        }
        let wanted: BTreeSet<&str> = item_ids.iter().map(String::as_str).collect();
        let mut n = 0;
        if part == STILL_TRUE {
            for d in dated.iter().filter(|d| wanted.contains(d.item_id.as_str())) {
                self.answer_dated(d, answer)?;
                n += 1;
            }
        }
        for l in listed
            .iter()
            .filter(|l| wanted.contains(l.item_id.as_str()))
        {
            let mut l = l.clone();
            match part {
                // MEM-481: FERB's own understanding: Approve pins it as his, Decline keeps it remembered only
                ALWAYS_ON if understood(&l) => {
                    if answer == "approve" {
                        profile.keep(&l.item_id, "owner")?;
                    } else {
                        profile.decline(&l.item_id, "owner")?;
                    }
                }
                ALWAYS_ON => {
                    let approve = answer == "approve";
                    l.data.insert(
                        ALWAYS_ON.into(),
                        json!(if approve { APPROVED } else { DECLINED }),
                    );
                    l.data.insert(
                        "always_on_at".into(),
                        json!(py::iso(&self.memory.clock.now())),
                    );
                    self.write_line(&l, false)?;
                    if approve {
                        profile.keep(&l.item_id, "owner")?;
                    } else {
                        profile.remove(std::slice::from_ref(&l.item_id), "owner")?;
                    }
                }
                STILL_TRUE => {
                    l.data.remove("asking_old");
                    match answer {
                        "yes" => {
                            l.data.insert(
                                "confirmed".into(),
                                json!(py::iso(&self.memory.clock.now())),
                            );
                            self.write_line(&l, false)?;
                            let rests = self.line_facts(&l.id)?;
                            self.settle(&l, &rests.iter().collect::<Vec<_>>(), "")?;
                        }
                        "no" => {
                            self.write_line(&l, false)?;
                            if self
                                .memory
                                .get(&l.item_id)?
                                .is_some_and(|i| i.valid_to.is_none() && i.status == "active")
                            {
                                self.memory
                                    .retire(&l.item_id, "the owner said it no longer holds")?;
                            }
                        }
                        _ => self.write_line(&l, false)?,
                    }
                }
                _ => {
                    self.answer_offer(&l.item_id, answer)?;
                }
            }
            n += 1;
        }
        Ok(n)
    }

    /// The whole list answered at once (`memory.review.all`, MEM-486): each of `approve` gets its part's yes, each of
    /// `decline` (and not in `approve`) its part's no, as `answer_review` gives them. An id the list no longer holds
    /// is passed over. The number answered.
    pub fn answer_all(
        &self,
        profile: &CoreProfile,
        approve: &[String],
        decline: &[String],
    ) -> Result<usize> {
        let decline: Vec<String> = decline
            .iter()
            .filter(|d| !approve.contains(d))
            .cloned()
            .collect();
        let mut n = 0;
        for (part, yes, no) in YES_NO {
            if !approve.is_empty() {
                n += self.answer_review(profile, part, approve, yes)?;
            }
            if !decline.is_empty() {
                n += self.answer_review(profile, part, &decline, no)?;
            }
        }
        Ok(n)
    }

    /// Today on this Mac's calendar ("2026-11-25"), the owner's day the daily review counts by (MEM-486).
    pub fn local_today(&self) -> String {
        self.today()
    }

    /// The owner's answer about a fact whose check date came (MEM-472), by code: Yes keeps it and clears the date
    /// (an import line made current again, as Yes to an older line does, and the same facts never set that date
    /// again); No ends it as No to an older line does; Not sure asks again in `UNSURE_AGAIN_DAYS`.
    fn answer_dated(&self, d: &Dated, answer: &str) -> Result<()> {
        let now = self.memory.clock.now();
        let again = py::shift(&now, UNSURE_AGAIN_DAYS * DAY);
        if let Some(l) = &d.line {
            let mut l = l.clone();
            l.data.remove("asking_old");
            l.data.insert("review_checked".into(), json!(d.day));
            let next = if answer == "unsure" {
                self.day_of(&py::iso(&again))
            } else {
                String::new()
            };
            l.data.insert("review_on".into(), json!(next));
            if answer == "yes" {
                l.data.insert("confirmed".into(), json!(py::iso(&now)));
            }
            self.write_line(&l, false)?;
            if answer == "yes" {
                let rests = self.line_facts(&l.id)?;
                self.settle(&l, &rests.iter().collect::<Vec<_>>(), "")?;
            }
        }
        match answer {
            "unsure" => self.memory.set_review(&d.item_id, Some(&again))?,
            "no" => {
                self.memory.set_review(&d.item_id, None)?;
                if self
                    .memory
                    .get(&d.item_id)?
                    .is_some_and(|i| i.valid_to.is_none() && i.status == "active")
                {
                    self.memory
                        .retire(&d.item_id, "the owner said it no longer holds")?;
                }
            }
            _ => self.memory.set_review(&d.item_id, None)?,
        }
        Ok(())
    }

    /// What the review asks after an import (MEM-444, MEM-460, MEM-462): the offers 30 to 60 days old (`ask_round`)
    /// and the older lines that steer FERB's work (`old_round`), each asked about once; the always-on suggestions are
    /// marked (`suggest_always_on`). Whether anything is to answer.
    pub fn review_round(&self) -> Result<bool> {
        self.ask_round()?;
        self.old_round()?;
        self.suggest_always_on()?;
        Ok(self.review()?["count"].as_u64().unwrap_or(0) > 0)
    }

    /// The question FERB asks for the review: its first line and the line under it (`None` when nothing is to
    /// answer). The card is its face; these words are what the planner and a client without the card read.
    pub fn review_question(&self) -> Result<Option<String>> {
        let (on, dated, old, offers) = self.review_all()?;
        let mut what = Vec::new();
        let n = |k: usize, one: &str, many: &str| {
            if k == 1 {
                one.to_owned()
            } else {
                many.replace("{n}", &k.to_string())
            }
        };
        if !on.is_empty() {
            what.push(n(
                on.len(),
                "1 thing to keep in mind all the time",
                "{n} things to keep in mind all the time",
            ));
        }
        if !dated.is_empty() {
            what.push(n(
                dated.len(),
                "1 thing I said I'd check with you",
                "{n} things I said I'd check with you",
            ));
        }
        if !old.is_empty() {
            what.push(n(
                old.len(),
                "1 older thing that may not be true any more",
                "{n} older things that may not be true any more",
            ));
        }
        if !offers.is_empty() {
            what.push(n(
                offers.len(),
                "1 thing I could help with",
                "{n} things I could help with",
            ));
        }
        if what.is_empty() {
            return Ok(None);
        }
        let list = match what.split_last() {
            Some((last, [])) => last.clone(),
            Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
            None => String::new(),
        };
        Ok(Some(format!(
            "{ASK_TITLE}\n{}. {ASK_ANSWER_HOW}",
            capitalized(&list)
        )))
    }

    /// The review's items numbered as shown, each `(n, part, line, text)`: what the owner's typed answer is read
    /// against (`apply_answer`).
    pub fn review_numbered(&self) -> Result<Vec<Value>> {
        let (on, dated, old, offers) = self.review_all()?;
        let mut out = Vec::new();
        let mut push = |what: &str, line: &str, item: &str, text: String| {
            out.push(
                json!({"n": out.len() + 1, "what": what, "line": line, "item": item, "text": text}),
            );
        };
        for l in &on {
            push(ALWAYS_ON, &l.id, &l.item_id, l.scoped(&self.said_as(l)));
        }
        // MEM-472: a fact whose check date came is answered as an older line is (still true, or not any more)
        for d in &dated {
            let line = d.line.as_ref().map_or("", |l| l.id.as_str());
            push("line", line, &d.item_id, d.text.clone());
        }
        for l in &old {
            push("line", &l.id, &l.item_id, l.scoped(&self.said_as(l)));
        }
        for l in &offers {
            push("offer", &l.id, &l.item_id, l.offer());
        }
        Ok(out)
    }

    /// The owner's "Always on" / "Not always on" for an imported line on the Memory page (MEM-460): his word on it, so
    /// it leaves the review. False when the item is no import line's.
    pub fn owner_word_always_on(&self, item_id: &str, approve: bool) -> Result<bool> {
        let Some(mut l) = self.lines()?.into_iter().find(|l| l.item_id == item_id) else {
            return Ok(false);
        };
        l.data.insert(
            ALWAYS_ON.into(),
            json!(if approve { APPROVED } else { DECLINED }),
        );
        l.data.insert(
            "always_on_at".into(),
            json!(py::iso(&self.memory.clock.now())),
        );
        self.write_line(&l, false)?;
        Ok(true)
    }

    /// An older build's offers question (MEM-444's numbered list, its ident's `items`) carried into the review
    /// (MEM-460): its offers stay asked (`asking`); its older lines are asked again only when they steer FERB's work
    /// (MEM-462). The number carried.
    pub fn carry_asked(&self, ident: &Value) -> Result<usize> {
        let asked: Vec<(String, String)> = ident
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|i| (py::py_str_opt(i.get("line")), py::py_str_opt(i.get("what"))))
            .collect();
        let today = self.today();
        let mut n = 0;
        for mut l in self.lines()? {
            let Some((_, what)) = asked.iter().find(|(id, _)| *id == l.id) else {
                continue;
            };
            if l.gone() || l.over(&today) {
                continue;
            }
            let key = if what == "offer" {
                if l.offer().is_empty() {
                    continue;
                }
                "asking"
            } else if steers(&l) && ![SUGGESTED, ON].contains(&l.s(ALWAYS_ON).as_str()) {
                "asking_old"
            } else {
                continue;
            };
            l.data.insert(key.into(), json!(true));
            self.write_line(&l, false)?;
            n += 1;
        }
        Ok(n)
    }

    /// Lines from a while ago asked about once (IMP-41), only those that steer FERB's work (MEM-462): how he likes it
    /// to work, his setup, his recurring things (`GUIDES`) and his projects. Never ones he corrected or gave an end
    /// date, never one asked about before; at most five, the newest first. The ones a later import's round no longer
    /// asks leave the list (as IMP-51 says of offers).
    pub(crate) fn old_round_lines(&self) -> Result<Vec<Line>> {
        let today = self.today();
        let lines = self.lines()?;
        for mut l in lines.iter().filter(|l| l.b("asking_old")).cloned() {
            l.data.remove("asking_old");
            self.write_line(&l, false)?;
        }
        let mut fresh: Vec<Line> = lines
            .into_iter()
            .filter(|l| {
                !l.gone()
                    && !l.over(&today)
                    && self.old(l)
                    && !l.data.contains_key("asked_old")
                    && l.offer().is_empty()
                    && steers(l)
                    && ![SUGGESTED, ON].contains(&l.s(ALWAYS_ON).as_str())
            })
            .collect();
        fresh.sort_by_key(|l| std::cmp::Reverse((l.said(), l.id.clone())));
        for (n, l) in fresh.iter_mut().enumerate() {
            l.data.insert("asked_old".into(), json!(today));
            if n < ASK_MOST {
                l.data.insert("asking_old".into(), json!(true));
            }
            self.write_line(l, false)?;
        }
        fresh.truncate(ASK_MOST);
        Ok(fresh)
    }
}

/// Whether a line steers FERB's work (MEM-462): how the owner likes it to work, his setup, his recurring things
/// (`GUIDES`), or a project of his. A fact about him that steers nothing is not asked about when it gets old.
pub fn steers(l: &Line) -> bool {
    GUIDES.contains(&l.s("use").as_str()) || l.kind == "project"
}

/// The note under a fact whose check date came: "I said I'd check this on 5 Dec 2026".
fn due_note(day: &str) -> String {
    DUE_NOTE.replace("{day}", &py_day(day))
}

/// "2026-09-30" as "30 Sep 2026" (owner-facing).
fn py_day(day: &str) -> String {
    let Some(d) = ferb_memory::times::Date::from_isoformat(day) else {
        return day.to_owned();
    };
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{} {} {}",
        d.day,
        MONTHS[(d.month as usize).clamp(1, 12) - 1],
        d.year
    )
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// The owner's typed answer's counts, as `answer_line` reads them.
pub fn counts(
    approved: i64,
    declined: i64,
    kept: i64,
    aside: i64,
    unclear: i64,
) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("approved".into(), json!(approved));
    out.insert("declined".into(), json!(declined));
    out.insert("still_on".into(), json!(kept));
    out.insert("set_aside".into(), json!(aside));
    out.insert("unclear".into(), json!(unclear));
    out
}
