//! App chats (kb HAND-668..): the desktop chat apps' conversations (the Claude app, the ChatGPT and Codex apps, Cursor's
//! agent pane, any chat-style app the owner adds) read from the app's own window through Accessibility, by code only.
//!
//! One generic way for every app. What only an app's own structure can tell is kept as data ([`AppFacts`], the table
//! in `appchat.json`): the marks of its chat pane and its composer, its stop control's label, which side its owner's
//! messages sit on, a label it puts on the owner's messages.
//!
//! A window is a chat only when its chat pane is proven by structure (HAND-698, the owner's HAND-697): an editable
//! element carrying the app's composer marks, inside a container carrying its pane marks, and never an editor's or a
//! terminal's input ([`NOT_COMPOSER`]). No proof: no session, and nothing is ever typed there. Messages are only the
//! text inside that pane, above the composer and in its column, in reading order, grouped by gaps, with icon-font
//! glyphs taken out; who said what comes from the app's layout (its data, else alignment within the column), and
//! FERB's own sent text only confirms it. Nothing here reads meaning into the words (AGENTS.md rule 4); what the apps show is data
//! (rule 9).
//!
//! Whether the app is still replying (HAND-671, the numbers the owner's HAND-674): an app with a stop control has
//! finished once it has been gone for [`STOP_GONE_S`]; an app with none, once nothing changed for [`STILL_S`].

use serde::{Deserialize, Serialize};

// decision: HAND-674 — the owner's numbers
/// An app with a stop control has finished its reply once the control has been gone this long (seconds).
pub const STOP_GONE_S: f64 = 3.0;
/// An app with no stop control has finished once its chat has not changed for this long.
pub const STILL_S: f64 = 10.0;
/// How often a watched chat is looked at while the app replies, and otherwise.
pub const LOOK_REPLYING_S: f64 = 2.0;
pub const LOOK_IDLE_S: f64 = 10.0;
/// How often a window on another Space (read by bringing its app forward, only while the owner is away) is looked at.
pub const FOREGROUND_LOOK_S: f64 = 30.0;

/// Two text runs closer than this (points, vertically) belong to one message.
pub const GAP: f64 = 14.0;
/// A message whose left edge is this share of the column's width in from the column's left, and whose right edge is
/// this close to the column's right, sits on the right (a chat's own bubbles).
pub const RIGHT_INSET: f64 = 0.18;
pub const RIGHT_FLUSH: f64 = 0.12;

/// One element of a window's Accessibility tree, as the Mac reads it (flat, in reading order).
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Node {
    /// The Accessibility role (AXStaticText, AXTextArea, AXButton, AXGroup, …).
    pub role: String,
    /// Its title or description.
    #[serde(default)]
    pub label: String,
    /// Its value (a text's words, a field's contents).
    #[serde(default)]
    pub value: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub focused: bool,
    /// Its value can be set (a text field the owner types into).
    #[serde(default)]
    pub editable: bool,
    /// HAND-698: the chat pane's proven composer (the one element FERB may type into).
    #[serde(default)]
    pub composer: bool,
}

fn yes() -> bool {
    true
}

/// Where an app puts its owner's messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// Read from the layout: a message set in and flush right within the transcript's column is the owner's.
    #[default]
    Layout,
    /// The owner's messages are the ones on the right (as Layout, said by the app's data).
    Right,
    /// The owner's messages are the ones on the left; the app's sit on the right.
    Left,
}

/// What only an app's own structure tells, as data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AppFacts {
    pub bundle_id: String,
    pub name: String,
    /// The labels of its stop-replying control (a button present only while it replies).
    #[serde(default)]
    pub stop_labels: Vec<String>,
    #[serde(default)]
    pub owner_side: Side,
    /// A label the app puts on (or just before) its owner's messages.
    #[serde(default)]
    pub owner_labels: Vec<String>,
    /// HAND-698: what its composer carries: a DOM class, an identifier, or words of its label or placeholder.
    #[serde(default)]
    pub composer_marks: Vec<String>,
    /// HAND-698: what its chat pane (a container around the composer and the messages) carries: a DOM class, an
    /// identifier, or "role:<AXRole>". None: the composer's window is the pane.
    #[serde(default)]
    pub pane_marks: Vec<String>,
}

/// The chat apps FERB knows (data: `appchat.json`). Any other app the owner adds gets the generic facts.
pub fn known() -> Vec<AppFacts> {
    serde_json::from_str(include_str!("appchat.json")).unwrap_or_default()
}

/// The facts for an app: its row of the table, else the generic ones.
pub fn facts_for(bundle_id: &str, name: &str, added: &[AppFacts]) -> AppFacts {
    known()
        .into_iter()
        .chain(added.iter().cloned())
        .find(|f| f.bundle_id == bundle_id)
        .unwrap_or_else(|| AppFacts {
            bundle_id: bundle_id.to_owned(),
            name: name.to_owned(),
            ..AppFacts::default()
        })
}

/// Who said a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Speaker {
    Owner,
    App,
}

/// One message as read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub text: String,
    pub speaker: Speaker,
    pub x: f64,
    pub y: f64,
    pub w: f64,
}

/// One look at a window.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Look {
    /// The composer's index among the nodes, None when the window has none (nothing can be typed there).
    pub composer: Option<usize>,
    pub blocks: Vec<Block>,
    /// The app's stop control is there and enabled.
    pub stop_control: bool,
}

const TEXT_ROLES: [&str; 3] = ["AXStaticText", "AXHeading", "text"];

fn is_text(n: &Node) -> bool {
    TEXT_ROLES.contains(&n.role.as_str()) && !n.editable
}

fn words(n: &Node) -> &str {
    if !n.value.trim().is_empty() {
        n.value.trim()
    } else {
        n.label.trim()
    }
}

/// The composer: the one node the proof marked (HAND-698). Never a guess: no mark, no composer.
pub fn composer(nodes: &[Node]) -> Option<usize> {
    let mut marked = nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.composer && n.editable && n.enabled);
    let first = marked.next()?.0;
    marked.next().is_none().then_some(first)
}

/// Marks no composer ever carries: an editor's or a terminal's input (VS Code's and Cursor's Monaco editor, xterm.js),
/// a find box. Structure, never words: an editor-only window is never a chat (HAND-697).
pub const NOT_COMPOSER: [&str; 8] = [
    "monaco-editor",
    "inputarea",
    "native-edit-context",
    "monaco-inputbox",
    "find-widget",
    "xterm",
    "xterm-helper-textarea",
    "AXSearchField",
];

/// One element of an editable element's chain, itself first, up to its window: its role, its structural marks
/// (identifier, DOM identifier, DOM classes, subrole) and its frame.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Link {
    pub role: String,
    pub marks: Vec<String>,
    /// (x, y, w, h); None when the Mac didn't say.
    pub frame: Option<(f64, f64, f64, f64)>,
}

/// An editable text element that could be a chat's composer.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Candidate {
    /// Its index among the window's nodes.
    pub index: usize,
    /// Its own label (title, description or placeholder).
    pub label: String,
    pub focused: bool,
    /// Its bottom edge (the lower of two proven composers is the chat's).
    pub bottom: f64,
    /// Itself first, then each container up to (not past) its window.
    pub chain: Vec<Link>,
}

/// A proven chat pane: its composer (an index among the window's nodes) and the pane (how many containers above the
/// composer it is: 0 is the composer itself).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Proven {
    pub composer: usize,
    pub pane: usize,
}

fn carries(link: &Link, mark: &str) -> bool {
    match mark.strip_prefix("role:") {
        Some(role) => link.role == role,
        None => link.marks.iter().any(|m| m.eq_ignore_ascii_case(mark)),
    }
}

/// Whether the facts name nothing (an app the owner added that FERB has no marks for): its windows are proven by the
/// generic rule alone (one editable element that is no editor's, with a message list above it: [`read`] checks).
pub fn generic(facts: &AppFacts) -> bool {
    facts.composer_marks.is_empty() && facts.pane_marks.is_empty()
}

/// HAND-698: the chat pane of a window, proven by structure, or None (not a chat: no session, never typed into).
pub fn prove(cands: &[Candidate], facts: &AppFacts) -> Option<Proven> {
    let ok: Vec<&Candidate> = cands
        .iter()
        .filter(|c| {
            !c.chain
                .iter()
                .any(|l| NOT_COMPOSER.iter().any(|m| carries(l, m)))
        })
        .collect();
    let mut proven: Vec<(&Candidate, usize)> = Vec::new();
    for c in &ok {
        let composer = if facts.composer_marks.is_empty() {
            generic(facts) || !facts.pane_marks.is_empty()
        } else {
            facts.composer_marks.iter().any(|m| {
                (!c.label.trim().is_empty() && c.label.to_lowercase().contains(&m.to_lowercase()))
                    || c.chain.iter().any(|l| carries(l, m))
            })
        };
        if !composer {
            continue;
        }
        let pane = if facts.pane_marks.is_empty() {
            Some(c.chain.len().saturating_sub(1))
        } else {
            c.chain
                .iter()
                .enumerate()
                .skip(1)
                .find(|(_, l)| facts.pane_marks.iter().any(|m| carries(l, m)))
                .map(|(i, _)| i)
        };
        if let Some(p) = pane {
            proven.push((c, p));
        }
    }
    // an app the owner added, with nothing to go by: only one editable element can be the composer
    if generic(facts) && proven.len() != 1 {
        return None;
    }
    proven
        .into_iter()
        .max_by(|(a, _), (b, _)| {
            a.focused
                .cmp(&b.focused)
                .then(a.bottom.total_cmp(&b.bottom))
        })
        .map(|(c, p)| Proven {
            composer: c.index,
            pane: p,
        })
}

/// Text with icon-font glyphs (Unicode's private use areas) taken out.
pub fn plain(text: &str) -> String {
    let kept: String = text
        .chars()
        .filter(|c| {
            !matches!(*c as u32, 0xE000..=0xF8FF | 0xF0000..=0xFFFFD | 0x100000..=0x10FFFD | 0xFFFD)
        })
        .collect();
    kept.trim().to_owned()
}

const LIST_ROLES: [&str; 4] = ["AXScrollArea", "AXList", "AXTable", "AXOutline"];

/// One look at a chat pane's tree (the pane's nodes only; its composer marked by the proof). `sent`: what FERB typed
/// into this chat (it only confirms a message is the owner's). No marked composer: no chat (`composer` None).
pub fn read(nodes: &[Node], facts: &AppFacts, sent: &[String]) -> Look {
    let Some(ci) = composer(nodes) else {
        return Look::default();
    };
    let c = &nodes[ci];
    // the messages' column: the composer's, give or take its own margin
    let (col_l, col_r) = (c.x - 40.0, c.x + c.w + 40.0);
    let in_column = |n: &Node| {
        let overlap = (n.x + n.w).min(col_r) - n.x.max(col_l);
        overlap >= 0.5 * n.w.max(1.0)
    };
    let inside_composer =
        |n: &Node| n.y >= c.y - 1.0 && n.y + n.h <= c.y + c.h + 1.0 && in_column(n);
    // an app FERB has no marks for must show a message list above its composer
    if generic(facts)
        && !nodes
            .iter()
            .any(|n| LIST_ROLES.contains(&n.role.as_str()) && n.y < c.y && in_column(n))
    {
        return Look::default();
    }
    let comp = Some(ci);
    let floor = c.y;
    let stop_control = nodes.iter().any(|n| {
        n.role == "AXButton"
            && n.enabled
            && facts
                .stop_labels
                .iter()
                .any(|l| l.eq_ignore_ascii_case(n.label.trim()))
    });
    // the owner's-message labels the app shows (data): the next text after one is his
    let mut runs: Vec<(Node, bool)> = Vec::new();
    let mut marked = false;
    for n in nodes
        .iter()
        .filter(|n| is_text(n) && n.y + n.h <= floor + 1.0 && in_column(n) && !inside_composer(n))
    {
        let w = plain(words(n));
        if w.is_empty() {
            continue;
        }
        let w = w.as_str();
        if facts.owner_labels.iter().any(|l| l.eq_ignore_ascii_case(w)) {
            marked = true;
            continue;
        }
        runs.push((n.clone(), std::mem::take(&mut marked)));
    }
    runs.sort_by(|a, b| a.0.y.total_cmp(&b.0.y).then(a.0.x.total_cmp(&b.0.x)));
    // the transcript's column: its left and right edges
    let left = runs.iter().map(|(n, _)| n.x).fold(f64::INFINITY, f64::min);
    let right = runs
        .iter()
        .map(|(n, _)| n.x + n.w)
        .fold(f64::NEG_INFINITY, f64::max);
    let width = (right - left).max(1.0);
    let on_right =
        |n: &Node| n.x - left > RIGHT_INSET * width && right - (n.x + n.w) < RIGHT_FLUSH * width;
    // group runs into messages: close below the last, on the same side
    let mut groups: Vec<(Vec<Node>, bool, bool)> = Vec::new(); // (runs, right side, marked owner)
    for (mut n, mark) in runs {
        n.value = plain(words(&n));
        let side = on_right(&n);
        let joins = groups.last().is_some_and(|(g, s, _)| {
            let last = g.last().expect("a group has a run");
            *s == side && n.y - (last.y + last.h) < GAP && !mark
        });
        if joins {
            if let Some(g) = groups.last_mut() {
                g.0.push(n);
            }
        } else {
            groups.push((vec![n], side, mark));
        }
    }
    let blocks = groups
        .into_iter()
        .map(|(g, side, mark)| {
            let text = g.iter().map(words).collect::<Vec<_>>().join("\n");
            let x = g.iter().map(|n| n.x).fold(f64::INFINITY, f64::min);
            let r = g
                .iter()
                .map(|n| n.x + n.w)
                .fold(f64::NEG_INFINITY, f64::max);
            let owner_by_layout = match facts.owner_side {
                Side::Layout | Side::Right => side,
                Side::Left => !side,
            };
            let confirmed = sent.iter().any(|s| one_line(s) == one_line(&text));
            let speaker = if mark || owner_by_layout || confirmed {
                Speaker::Owner
            } else {
                Speaker::App
            };
            Block {
                text,
                speaker,
                x,
                y: g[0].y,
                w: r - x,
            }
        })
        .collect();
    Look {
        composer: comp,
        blocks,
        stop_control,
    }
}

fn one_line(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether the app is still replying, look after look (code only: it never asks a model).
#[derive(Clone, Debug, Default)]
pub struct Watch {
    last_text: String,
    changed_at: f64,
    blocks: usize,
    seen: bool,
}

/// What a look says of the chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Its stop control is up or its last message is still changing.
    Replying,
    /// Settled: the stop control gone for STOP_GONE_S (an app with one), or nothing changed for STILL_S (one without).
    Finished,
}

impl Watch {
    /// One look at time `now` (seconds). `has_stop`: the app has a stop control of its own (its data names one), so
    /// its going decides; else the chat holding still does.
    pub fn see(&mut self, look: &Look, has_stop: bool, now: f64) -> State {
        let settle = if has_stop { STOP_GONE_S } else { STILL_S };
        let last = look
            .blocks
            .last()
            .map(|b| b.text.clone())
            .unwrap_or_default();
        if !self.seen || last != self.last_text || look.blocks.len() != self.blocks {
            if self.seen {
                self.changed_at = now;
            } else {
                // the first look: what is there was there before FERB looked
                self.changed_at = now - settle;
            }
            self.seen = true;
            self.last_text = last;
            self.blocks = look.blocks.len();
        }
        if look.stop_control {
            // the clock that decides "gone for STOP_GONE_S" runs from the last look that saw it
            self.changed_at = now;
            return State::Replying;
        }
        if now - self.changed_at < settle {
            State::Replying
        } else {
            State::Finished
        }
    }

    /// Something changed since the last look: a reply started (FERB just sent, or the owner typed in the app).
    pub fn touch(&mut self, now: f64) {
        self.changed_at = now;
    }
}

// ---------- a chat pane learned with one model call (HAND-699, the owner's HAND-697) ----------

/// One look at a chat app's window: its nodes (reading order) and the editable elements that could be a composer.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct WindowRead {
    pub nodes: Vec<Node>,
    pub cands: Vec<Candidate>,
}

/// One link of a learned fingerprint: a role and the marks it had (none: any).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FpLink {
    pub role: String,
    #[serde(default)]
    pub marks: Vec<String>,
}

/// HAND-699: a chat pane's structural fingerprint in one app's layout: its composer's chain from the composer up to the
/// pane (roles and marks, never words). Code finds the chat by it with no model call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Fingerprint {
    pub links: Vec<FpLink>,
}

/// The fingerprint of a proven pane.
pub fn fingerprint(c: &Candidate, pane: usize) -> Fingerprint {
    Fingerprint {
        links: c
            .chain
            .iter()
            .take(pane + 1)
            .map(|l| FpLink {
                role: l.role.clone(),
                marks: l.marks.clone(),
            })
            .collect(),
    }
}

/// An editor's or a terminal's input, or a find box ([`NOT_COMPOSER`]): never a chat's composer.
pub fn rejected(c: &Candidate) -> bool {
    c.chain
        .iter()
        .any(|l| NOT_COMPOSER.iter().any(|m| carries(l, m)))
}

/// The candidate a fingerprint names now (each link the same role, sharing a mark where the fingerprint has any),
/// never an editor's or a terminal's input. Several: the focused, else the lowest.
pub fn matches(fp: &Fingerprint, cands: &[Candidate]) -> Option<Proven> {
    if fp.links.is_empty() {
        return None;
    }
    cands
        .iter()
        .filter(|c| !rejected(c) && c.chain.len() >= fp.links.len())
        .filter(|c| {
            fp.links.iter().zip(&c.chain).all(|(f, l)| {
                f.role == l.role
                    && (f.marks.is_empty()
                        || f.marks
                            .iter()
                            .any(|m| l.marks.iter().any(|x| x.eq_ignore_ascii_case(m))))
            })
        })
        .max_by(|a, b| {
            a.focused
                .cmp(&b.focused)
                .then(a.bottom.total_cmp(&b.bottom))
        })
        .map(|c| Proven {
            composer: c.index,
            pane: fp.links.len() - 1,
        })
}

/// A window's layout as code tells it apart: a digest of its editable elements' role paths (no words). A layout
/// the model was asked about once is never asked about again.
pub fn structure_key(cands: &[Candidate]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for c in cands {
        for l in &c.chain {
            for b in l.role.bytes().chain(std::iter::once(b'/')) {
                h ^= u64::from(b);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        }
        h ^= u64::from(b'|');
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// The most elements an outline shows, and the longest label in it.
pub const OUTLINE_NODES: usize = 400;
pub const OUTLINE_LABEL: usize = 40;

/// HAND-699: the outline of a window the model is shown: each element's number, role, label (cut short; the caller
/// redacts it) and position, the editable ones marked. Never an element's value (what is written in it).
pub fn outline(read: &WindowRead) -> String {
    let editable: Vec<usize> = read.cands.iter().map(|c| c.index).collect();
    read.nodes
        .iter()
        .enumerate()
        .take(OUTLINE_NODES)
        .map(|(i, n)| {
            let label: String = plain(&n.label).chars().take(OUTLINE_LABEL).collect();
            format!(
                "{i} {}{}{} @{},{} {}x{}",
                n.role,
                if editable.contains(&i) {
                    " editable"
                } else {
                    ""
                },
                if label.is_empty() {
                    String::new()
                } else {
                    format!(" \"{label}\"")
                },
                n.x.round(),
                n.y.round(),
                n.w.round(),
                n.h.round()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// What the model answered about a window: a chat or not, and which elements are its message list and reply box.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Pick {
    pub chat: bool,
    #[serde(default)]
    pub message_list: Option<usize>,
    #[serde(default)]
    pub reply_box: Option<usize>,
}

/// The schema of the model's answer.
pub fn pick_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "chat": {"type": "boolean"},
            "message_list": {"type": ["integer", "null"]},
            "reply_box": {"type": ["integer", "null"]}
        },
        "required": ["chat", "message_list", "reply_box"],
        "additionalProperties": false
    })
}

fn contains(outer: (f64, f64, f64, f64), n: &Node) -> bool {
    n.x >= outer.0 - 1.0
        && n.y >= outer.1 - 1.0
        && n.x + n.w <= outer.0 + outer.2 + 1.0
        && n.y + n.h <= outer.1 + outer.3 + 1.0
}

/// HAND-699: code's check of the model's pick, before anything is learned or typed: the reply box is one of the
/// editable elements and no editor's or terminal's input; the message list sits above it in its column, is taller
/// than it and holds at least two texts; a container of the reply box holds the list too (the pane). Anything else:
/// None (the pick is dropped, nothing is typed).
pub fn check_pick(read: &WindowRead, pick: &Pick) -> Option<Proven> {
    if !pick.chat {
        return None;
    }
    let reply = read
        .cands
        .iter()
        .find(|c| Some(c.index) == pick.reply_box)?;
    if rejected(reply) {
        return None;
    }
    let r = read.nodes.get(reply.index)?;
    let list = read.nodes.get(pick.message_list?)?;
    if !(LIST_ROLES.contains(&list.role.as_str()) || list.role == "AXGroup") {
        return None;
    }
    let overlap = (list.x + list.w).min(r.x + r.w) - list.x.max(r.x);
    if list.y >= r.y || list.h <= r.h || overlap < 0.5 * r.w.min(list.w) {
        return None;
    }
    let texts = read
        .nodes
        .iter()
        .filter(|n| is_text(n) && !plain(words(n)).is_empty())
        .filter(|n| contains((list.x, list.y, list.w, list.h), n))
        .count();
    if texts < 2 {
        return None;
    }
    let pane = reply
        .chain
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, l)| l.frame.is_some_and(|f| contains(f, list) && contains(f, r)))?;
    Some(Proven {
        composer: reply.index,
        pane: pane.0,
    })
}
