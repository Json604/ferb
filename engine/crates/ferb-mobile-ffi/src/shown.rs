//! FERB's words and answers as FERB.app draws them (ferb-mobile `rich`, `shown`; kb PHN-720..), for both phones from
//! one place. Pure functions: no state, no network, no store. The apps call them off the main thread and keep each
//! result per entry (the text of an entry doesn't change without a new revision), so a list redraw never parses again.

use serde::Serialize;

use ferb_mobile::rich as fr;
use ferb_mobile::shown as fs;

use crate::records::{AnswerView, Conv, ResultCard};

/// A run of text with one look; `link` an address to open (http, https or mailto only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, uniffi::Record)]
pub struct RichSpan {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub link: Option<String>,
}

impl From<fr::Span> for RichSpan {
    fn from(s: fr::Span) -> RichSpan {
        let fr::Span {
            text,
            bold,
            italic,
            code,
            strike,
            link,
        } = s;
        RichSpan {
            text,
            bold,
            italic,
            code,
            strike,
            link,
        }
    }
}

fn spans(s: Vec<fr::Span>) -> Vec<RichSpan> {
    s.into_iter().map(RichSpan::from).collect()
}

/// A table row's cells, each its runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, uniffi::Record)]
pub struct RichRow {
    pub cells: Vec<RichCell>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, uniffi::Record)]
pub struct RichCell {
    pub spans: Vec<RichSpan>,
}

/// One block of FERB's words (FERB.app's `MarkdownDocument.Block`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, uniffi::Enum)]
pub enum RichBlock {
    Paragraph {
        spans: Vec<RichSpan>,
    },
    /// `level` 1 to 6.
    Heading {
        level: u32,
        spans: Vec<RichSpan>,
    },
    /// `marker` "•" or the number as written; `level` 0 and up.
    ListItem {
        marker: String,
        level: u32,
        spans: Vec<RichSpan>,
    },
    Quote {
        spans: Vec<RichSpan>,
    },
    Code {
        text: String,
        language: String,
    },
    /// `align` per column: leading, center or trailing.
    Table {
        header: Vec<RichCell>,
        rows: Vec<RichRow>,
        align: Vec<String>,
    },
    Divider,
}

fn cell(c: fr::Cell) -> RichCell {
    RichCell {
        spans: spans(c.spans),
    }
}

impl From<fr::Block> for RichBlock {
    fn from(b: fr::Block) -> RichBlock {
        match b {
            fr::Block::Paragraph { spans: s } => RichBlock::Paragraph { spans: spans(s) },
            fr::Block::Heading { level, spans: s } => RichBlock::Heading {
                level,
                spans: spans(s),
            },
            fr::Block::ListItem {
                marker,
                level,
                spans: s,
            } => RichBlock::ListItem {
                marker,
                level,
                spans: spans(s),
            },
            fr::Block::Quote { spans: s } => RichBlock::Quote { spans: spans(s) },
            fr::Block::Code { text, language } => RichBlock::Code { text, language },
            fr::Block::Table {
                header,
                rows,
                align,
            } => RichBlock::Table {
                header: header.into_iter().map(cell).collect(),
                rows: rows
                    .into_iter()
                    .map(|r| RichRow {
                        cells: r.cells.into_iter().map(cell).collect(),
                    })
                    .collect(),
                align,
            },
            fr::Block::Divider => RichBlock::Divider,
        }
    }
}

fn blocks(b: Vec<fr::Block>) -> Vec<RichBlock> {
    b.into_iter().map(RichBlock::from).collect()
}

/// A file under an answer: its path, and its place among the task's files when FERB has it (what a tap fetches).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, uniffi::Record)]
pub struct FileShown {
    pub path: String,
    pub index: Option<u32>,
}

/// An answer as FERB.app draws it (`AnswerView`): see ferb-mobile `shown::AnswerShown`.
#[derive(Debug, Clone, PartialEq, Serialize, uniffi::Record)]
pub struct AnswerShown {
    pub bubble: bool,
    pub blocks: Vec<RichBlock>,
    /// The bubble's words as plain text: what Copy gives.
    pub copy: String,
    pub note: Option<String>,
    pub lines: Vec<String>,
    pub details: Vec<RichBlock>,
    pub sites: Vec<String>,
    pub above: Vec<ResultCard>,
    pub below: Vec<ResultCard>,
    pub pages: Vec<ResultCard>,
    pub files: Vec<FileShown>,
    pub status_words: Option<String>,
}

/// A notice's cards as FERB.app lays them out (`NoticeView`).
#[derive(Debug, Clone, PartialEq, Serialize, uniffi::Record)]
pub struct NoticeShown {
    pub box_says: bool,
    pub box_words: Option<String>,
    pub above: Vec<ResultCard>,
    pub notes: Vec<ResultCard>,
    pub pages: Vec<ResultCard>,
}

/// FERB's words (Markdown) as blocks of styled runs: FERB.app's `MarkdownDocument` and inline reading, bounded for any
/// input (ferb-mobile `rich`). Pure; call it off the main thread and keep the result.
#[uniffi::export]
pub fn rich_text(source: String) -> Vec<RichBlock> {
    blocks(fr::document(&source))
}

/// The plain words of `source` as Copy gives them.
#[uniffi::export]
pub fn rich_plain(source: String) -> String {
    fr::plain_text(&fr::document(&source))
}

/// An answer as FERB.app draws it: `card` when its turn has a card of parts (then only its cards and files),
/// `hidden` the item's boxes a later item draws (`TurnItem.hidden`). Pure; off the main thread, kept per entry.
#[uniffi::export]
pub fn answer_shown(answer: AnswerView, card: bool, hidden: Vec<String>) -> AnswerShown {
    let core: ferb_mobile::fold::AnswerView = answer.conv();
    let s = fs::answer(&core, card, &hidden);
    let card = |v: Vec<ferb_mobile::fold::model::ResultCard>| {
        v.into_iter().map(Conv::conv).collect::<Vec<ResultCard>>()
    };
    AnswerShown {
        bubble: s.bubble,
        copy: fr::plain_text(&s.blocks),
        blocks: blocks(s.blocks),
        note: s.note,
        lines: s.lines,
        details: blocks(s.details),
        sites: s.sites,
        above: card(s.above),
        below: card(s.below),
        pages: card(s.pages),
        files: s
            .files
            .into_iter()
            .map(|f| FileShown {
                path: f.path,
                index: f.index,
            })
            .collect(),
        status_words: s.status_words,
    }
}

/// A notice's cards as FERB.app lays them out: `quotes` how many quotes it has, `hidden` its boxes a later item draws.
#[uniffi::export]
pub fn notice_shown(
    text: String,
    quotes: u32,
    cards: Vec<ResultCard>,
    hidden: Vec<String>,
) -> NoticeShown {
    let core: Vec<ferb_mobile::fold::model::ResultCard> =
        cards.into_iter().map(Conv::conv).collect();
    let s = fs::notice(&text, quotes as usize, &core, &hidden);
    let card = |v: Vec<ferb_mobile::fold::model::ResultCard>| {
        v.into_iter().map(Conv::conv).collect::<Vec<ResultCard>>()
    };
    NoticeShown {
        box_says: s.box_says,
        box_words: s.box_words,
        above: card(s.above),
        notes: card(s.notes),
        pages: card(s.pages),
    }
}

/// A question as FERB.app shows it (kb PHN-739) from its fields as JSON text (a question entry's body, a status
/// approval, or the fields an app holds); not JSON: the Mac's words for an empty question.
#[uniffi::export]
pub fn question_shown(body_json: String) -> crate::records::QuestionShown {
    let v: serde_json::Value = serde_json::from_str(&body_json).unwrap_or(serde_json::Value::Null);
    ferb_mobile::question::shown(&v).conv()
}

/// A task's end as FERB.app's banner says it (ferb-mobile `notify::task_ended`, kb PHN-795): the task's short name
/// ("FERB finished" or "FERB" without one) and the answer in the chat's words (`display_text` when FERB shortened it).
/// `held`: the words were held back, the text is empty for the app's own line. None: an end the Mac posts no banner for
/// (cancelled: the owner's own stop, a question he declined, a restart's first turn).
#[uniffi::export]
pub fn task_end_words(
    status: String,
    name: String,
    answer: String,
    display_text: Option<String>,
    held: bool,
) -> Option<crate::records::NotificationWords> {
    ferb_mobile::notify::task_ended(&status, &name, &answer, display_text.as_deref(), held)
        .map(Conv::conv)
}

/// A question as FERB.app's banner says it (ferb-mobile `notify::question`, kb PHN-795): "FERB needs you" and "FERB
/// needs your OK: …"; the planner's question "FERB needs you" and its own words.
#[uniffi::export]
pub fn question_words(shown: crate::records::QuestionShown) -> crate::records::NotificationWords {
    let core: ferb_mobile::question::QuestionShown = shown.conv();
    ferb_mobile::notify::question(&core).conv()
}
