//! How FERB.app lays out what FERB said, for the phones (kb PHN-720..): what FERB.app's views compute for themselves
//! from a turn, in one place both phones read.
//!
//! - `answer`: FERB.app's `AnswerView.Parts` and its body (ConversationTranscript.swift, `AnswerView`): the bubble's
//!   words without the lines the cards stand for (`CardLayout.strip`, `CardLayout.bubble`) and without the sites line,
//!   the "Couldn't confirm" note and code's lines drawn under the words (`AnswerParts`), the full answer under
//!   "Details" when FERB shortened it (`display_text`), the sites row (`VisitedSites`, APP-700: the link cards' hosts,
//!   else a "Sites contacted:" line), the cards by where they go (`CardLayout`: reminders above the words; events,
//!   notes, payments, sends and schedules below; files as file chips; pages as buttons), and the task's files not
//!   already a file card. A card whose box shows further down the chat is left out (`CardBoxes`, APP-769).
//! - `boxes`: FERB.app's `CardBoxes.moved`: one box per timer, reminder and schedule, drawn at the last item carrying
//!   it.
//! - `status_words`: the line under an answer that didn't end done.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::fold::model::{ResultCard, CARD_TYPES};
use crate::fold::{AnswerView, Item, Transcript};
use crate::rich::{self, Block};

/// FERB.app's sites line, as code writes it into the answer.
pub const SITES: &str = "Sites contacted:";

/// A file to show under an answer: its path, and its place among the task's files when FERB has it (what a tap
/// fetches it by, kb PHN-535).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileShown {
    pub path: String,
    pub index: Option<u32>,
}

/// An answer as FERB.app draws it.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct AnswerShown {
    /// Whether FERB.app draws its bubble at all (words, a note, code's lines or Details).
    pub bubble: bool,
    /// The bubble's words (empty when only the note, lines or Details are in it).
    pub blocks: Vec<Block>,
    /// "Couldn't confirm: …" under the words.
    pub note: Option<String>,
    /// Code's own lines under the words (never the sites line, never a line a card stands for).
    pub lines: Vec<String>,
    /// The whole answer, under "Details", when FERB shortened what it shows.
    pub details: Vec<Block>,
    /// The sites row: the first one shown, "+N" opening all of them.
    pub sites: Vec<String>,
    /// Cards above the words (a reminder that came due).
    pub above: Vec<ResultCard>,
    /// Cards under the words, one surface each: event, note, payment, send, schedules.
    pub below: Vec<ResultCard>,
    /// Buttons that open a page (`page` cards).
    pub pages: Vec<ResultCard>,
    /// The files: file cards first, then the task's other files.
    pub files: Vec<FileShown>,
    /// A later answer's own ending in words ("Delivered, with parts not done", "Failed"…), none when done.
    pub status_words: Option<String>,
}

/// `AnswerParts`: the answer's own lines and note, cut from its end when they are there.
struct Parts {
    body: String,
    note: Option<String>,
    lines: Vec<String>,
}

fn trim(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace())
}

fn parts(answer: &str, note: Option<&str>, lines: &[String]) -> Parts {
    let mut body = answer.to_string();
    let mut shown_lines: Vec<String> = Vec::new();
    if !lines.is_empty() {
        let joined = lines.join("\n");
        if let Some(at) = body.rfind(&joined) {
            if at == 0 || body[..at].ends_with('\n') {
                // a line the daemon adds after them ("(ran late)") stays with them
                let after = &body[at + joined.len()..];
                shown_lines = lines.to_vec();
                shown_lines.extend(
                    after
                        .split('\n')
                        .map(|l| {
                            l.trim_matches(|c: char| c.is_whitespace() && c != '\n')
                                .to_string()
                        })
                        .filter(|l| !l.is_empty()),
                );
                body.truncate(at);
            }
        }
    }
    let mut body = trim(&body).to_string();
    let mut shown_note = None;
    if let Some(n) = note.filter(|n| !n.is_empty()) {
        let line = format!("Couldn't confirm: {n}.");
        if body.ends_with(&line) {
            body = trim(&body[..body.len() - line.len()]).to_string();
            shown_note = Some(n.to_string());
        }
    }
    Parts {
        body,
        note: shown_note,
        lines: shown_lines,
    }
}

/// `SitesContactedParts`: the text before the first "Sites contacted:" line, and that line's sites.
fn sites_line(source: &str) -> (String, String) {
    let mut offset = 0;
    for line in source.split('\n') {
        if let Some(rest) = line.strip_prefix(SITES) {
            let sites = trim(rest);
            if !sites.is_empty() {
                return (trim(&source[..offset]).to_string(), sites.to_string());
            }
        }
        offset += line.len() + 1;
    }
    (source.to_string(), String::new())
}

/// `CardLayout.lines`: the lines the drawn cards stand for.
fn card_lines(cards: &[ResultCard]) -> HashSet<&str> {
    cards
        .iter()
        .filter(|c| CARD_TYPES.contains(&c.kind_of.as_str()))
        .filter_map(|c| c.line.as_deref())
        .collect()
}

/// `CardLayout.strip`: whole lines the cards stand for left out, the blank runs they leave collapsed.
pub fn strip(text: &str, cards: &[ResultCard]) -> String {
    let drop = card_lines(cards);
    if drop.is_empty() {
        return text.to_string();
    }
    let all: Vec<&str> = text.split('\n').collect();
    let kept: Vec<&str> = all.iter().copied().filter(|l| !drop.contains(l)).collect();
    if kept.len() == all.len() {
        return text.to_string();
    }
    let mut out: Vec<&str> = Vec::new();
    for line in kept {
        let blank = trim(line).is_empty();
        if blank && out.last().is_none_or(|l| trim(l).is_empty()) {
            continue;
        }
        out.push(line);
    }
    trim(&out.join("\n")).to_string()
}

/// `CardLayout.remaining`: code's lines the cards don't stand for.
pub fn remaining(lines: &[String], cards: &[ResultCard]) -> Vec<String> {
    let drop = card_lines(cards);
    lines
        .iter()
        .filter(|l| !drop.contains(l.as_str()))
        .cloned()
        .collect()
}

/// `VisitedSites`: the hosts, each once, in order.
fn visited(hosts: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    hosts
        .into_iter()
        .map(|h| trim(&h).to_string())
        .filter(|h| !h.is_empty() && seen.insert(h.clone()))
        .collect()
}

/// A card's box (`ResultCard.box`): the timer, reminder or schedule it was set as or delivers.
pub fn card_box(c: &ResultCard) -> Option<String> {
    match c.kind_of.as_str() {
        "event" if ["timer", "reminder", "schedule"].contains(&c.kind.as_deref().unwrap_or("")) => {
            c.id.clone()
        }
        "reminder" => c
            .schedule_id
            .clone()
            .or_else(|| c.repeat_of.clone())
            .or_else(|| c.timer_id.clone()),
        _ => None,
    }
}

/// `cards` without those whose box shows further down (`CardBoxes.shown`).
pub fn shown_cards(cards: &[ResultCard], hidden: &[String]) -> Vec<ResultCard> {
    cards
        .iter()
        .filter(|c| card_box(c).is_none_or(|b| !hidden.contains(&b)))
        .cloned()
        .collect()
}

/// The words under an answer that didn't end done (ConversationTranscript.swift, `AnswerView`): "Delivered, with
/// parts not done" (ORCH-651), else the status as words ("Failed", "Interrupted"); none for done, working or merged.
pub fn status_words(status: &str) -> Option<String> {
    if ["done", "working", "merged"].contains(&status) {
        return None;
    }
    if status == "done_with_gaps" {
        return Some("Delivered, with parts not done".into());
    }
    // Swift's `capitalized`: each word's first letter up, the rest down
    let words = status.replace('_', " ");
    Some(
        words
            .split(' ')
            .map(|w| {
                let mut c = w.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// An answer as FERB.app draws it. `card`: the turn has a card of parts (its rows and footer are the answer: no
/// bubble, its cards go under the card). `hidden`: the boxes a later item draws.
pub fn answer(a: &AnswerView, card: bool, hidden: &[String]) -> AnswerShown {
    let visible = a
        .display_text
        .clone()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| a.text.clone());
    let code_lines = a.lines.clone().unwrap_or_default();
    let split = parts(&visible, a.note.as_deref(), &code_lines);
    let code_sites = split
        .lines
        .iter()
        .find(|l| l.starts_with(SITES))
        .map(|l| trim(&l[SITES.len()..]).to_string());
    let (visible_answer, visible_sites) = sites_line(&split.body);
    let (full_answer, full_sites) = sites_line(&a.text);
    let displayed = code_sites.unwrap_or(if visible_sites.is_empty() {
        full_sites
    } else {
        visible_sites
    });
    let cards = a.cards.clone().unwrap_or_default();
    let drawn = shown_cards(&cards, hidden);
    let lines = remaining(
        &split
            .lines
            .iter()
            .filter(|l| !l.starts_with(SITES))
            .cloned()
            .collect::<Vec<_>>(),
        &cards,
    );
    let of = |t: &str| {
        drawn
            .iter()
            .filter(|c| c.kind_of == t)
            .cloned()
            .collect::<Vec<_>>()
    };
    let links: Vec<String> = drawn
        .iter()
        .filter(|c| c.kind_of == "link")
        .filter_map(|c| c.host.clone())
        .collect();
    let file_cards: Vec<String> = cards
        .iter()
        .filter(|c| c.kind_of == "file")
        .filter_map(|c| c.path.clone())
        .collect();
    let index_of = |p: &str| a.artifacts.iter().position(|x| x == p).map(|i| i as u32);
    let mut files: Vec<FileShown> = drawn
        .iter()
        .filter(|c| c.kind_of == "file")
        .filter_map(|c| c.path.clone())
        .map(|p| FileShown {
            index: index_of(&p),
            path: p,
        })
        .collect();
    files.extend(
        a.artifacts
            .iter()
            .enumerate()
            .filter(|(_, p)| !file_cards.contains(p))
            .map(|(i, p)| FileShown {
                path: p.clone(),
                index: Some(i as u32),
            }),
    );
    let below = drawn
        .iter()
        .filter(|c| ["event", "note", "payment", "send", "schedules"].contains(&c.kind_of.as_str()))
        .cloned()
        .collect();
    let mut shown = AnswerShown {
        below,
        pages: of("page"),
        files,
        status_words: a.status.as_deref().and_then(status_words),
        ..AnswerShown::default()
    };
    if a.text.is_empty() || card {
        // a card's rows and footer are the answer: its cards and files only
        shown.sites = visited(links);
        return shown;
    }
    shown.above = of("reminder");
    let details = a.display_text.is_some() && visible != a.text;
    let words = strip(&visible_answer, &cards);
    let says = !trim(&words).is_empty()
        || split.note.as_deref().is_some_and(|n| !trim(n).is_empty())
        || !lines.is_empty();
    shown.bubble = says || details;
    if says {
        shown.blocks = rich::document(&words);
        shown.note = split.note;
        shown.lines = lines;
    }
    if details {
        shown.details = rich::document(&full_answer);
    }
    // the sites row: the link cards; a turn from before link cards, the "Sites contacted:" line
    shown.sites = if !links.is_empty() || cards.iter().any(|c| c.kind_of == "link") {
        visited(links)
    } else {
        visited(displayed.split(',').map(str::to_string))
    };
    shown
}

/// `CardBoxes.moved`: for each item of the chat, the boxes it leaves out because a later item draws them.
pub fn boxes(t: &Transcript, utc_offset_s: i32) -> HashMap<String, Vec<String>> {
    // FERB.app computes it over the shown day's entries: a box set yesterday and delivered today shows on both days
    let day_of = |i: &Item| -> i64 {
        let at = match i {
            Item::Turn(x) => x.at,
            Item::Reply(x) => x.at,
            Item::Notice(x) | Item::Reminder(x) => x.at,
        };
        crate::time::day(at, utc_offset_s)
    };
    let turns: HashMap<&str, &crate::fold::TurnItem> = t
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Turn(x) => Some((x.entry.as_str(), x)),
            _ => None,
        })
        .collect();
    let cards_of = |i: &Item| -> Vec<ResultCard> {
        match i {
            Item::Turn(x) => {
                let mut c = x
                    .first_reply
                    .as_ref()
                    .and_then(|f| f.receipt_cards.clone())
                    .unwrap_or_default();
                c.extend(
                    x.answer
                        .as_ref()
                        .and_then(|a| a.cards.clone())
                        .unwrap_or_default(),
                );
                c
            }
            Item::Reply(r) => match &r.said {
                Some(s) => s.cards.clone().unwrap_or_default(),
                None => turns
                    .get(r.answers.as_str())
                    .and_then(|x| x.answer.as_ref())
                    .and_then(|a| a.cards.clone())
                    .unwrap_or_default(),
            },
            Item::Notice(n) | Item::Reminder(n) => n.cards.clone().unwrap_or_default(),
        }
    };
    let all: Vec<Vec<String>> = t
        .items
        .iter()
        .map(|i| cards_of(i).iter().filter_map(card_box).collect())
        .collect();
    let days: Vec<i64> = t.items.iter().map(day_of).collect();
    let mut last: HashMap<(&str, i64), usize> = HashMap::new();
    for (i, b) in all.iter().enumerate() {
        for x in b {
            last.insert((x, days[i]), i);
        }
    }
    let mut out = HashMap::new();
    for (i, b) in all.iter().enumerate() {
        let mut gone: Vec<String> = b
            .iter()
            .filter(|x| last.get(&(x.as_str(), days[i])).is_some_and(|l| *l > i))
            .cloned()
            .collect();
        gone.sort();
        gone.dedup();
        if !gone.is_empty() {
            out.insert(t.items[i].entry().to_string(), gone);
        }
    }
    out
}

/// A notice as FERB.app draws it (`NoticeView`): reminders above (each one box, APP-769), its words in a bubble unless
/// its one box says them, remembered facts and page buttons in the bubble.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct NoticeShown {
    /// The box says it: no bubble.
    pub box_says: bool,
    /// The reminder's own words inside its box, when they say more than its title.
    pub box_words: Option<String>,
    pub above: Vec<ResultCard>,
    pub notes: Vec<ResultCard>,
    pub pages: Vec<ResultCard>,
}

/// `NoticeView`'s layout of a notice's cards.
pub fn notice(text: &str, quotes: usize, cards: &[ResultCard], hidden: &[String]) -> NoticeShown {
    let (box_says, box_words) = notice_box(text, quotes, cards);
    let drawn = shown_cards(cards, hidden);
    let of = |t: &str, from: &[ResultCard]| {
        from.iter()
            .filter(|c| c.kind_of == t)
            .cloned()
            .collect::<Vec<_>>()
    };
    NoticeShown {
        box_says,
        box_words,
        above: of("reminder", &drawn),
        notes: of("note", cards),
        pages: of("page", cards),
    }
}

/// `CardBoxes.boxSays`: a timer's or reminder's arrival is its one box (no bubble repeats it); with `words`, the
/// reminder's own words inside the box when they say more than its title.
pub fn notice_box(text: &str, quotes: usize, cards: &[ResultCard]) -> (bool, Option<String>) {
    let says = quotes == 0
        && cards.len() == 1
        && cards[0].kind_of == "reminder"
        && cards[0].kind.as_deref() != Some("schedule");
    if !says || cards[0].kind.as_deref() == Some("timer") {
        return (says, None);
    }
    let t = trim(text);
    (
        says,
        (!t.is_empty() && Some(t) != cards[0].title.as_deref()).then(|| t.to_string()),
    )
}
