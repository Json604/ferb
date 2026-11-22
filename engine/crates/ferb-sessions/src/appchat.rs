//! App chats (kb HAND-668..): the desktop chat apps' conversations (the Claude app, the ChatGPT and Codex apps, Cursor's
//! agent pane, any chat-style app the owner adds) read from the app's own window through Accessibility, by code only.
//!
//! One generic way for every app. What only an app's own structure can tell is kept as data ([`AppFacts`], the table
//! in `appchat.json`): its stop control's label, which side its owner's messages sit on, a label it puts on the owner's
//! messages. Everything else is the window's structure: the composer is the focused editable text element, else the
//! lowest editable one (the agent-remote lab's x4); messages are the text above it in reading order, grouped by gaps;
//! who said what comes from the app's layout (its data, else alignment within the transcript's column), and FERB's own
//! sent text only confirms it. Nothing here reads meaning into the words (AGENTS.md rule 4); what the apps show is data
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

/// The composer: the focused editable text element, else the lowest editable one (the lab's x4 rule).
pub fn composer(nodes: &[Node]) -> Option<usize> {
    let editable = |n: &Node| {
        n.editable
            && n.enabled
            && matches!(n.role.as_str(), "AXTextArea" | "AXTextField" | "AXComboBox")
    };
    if let Some(i) = nodes.iter().position(|n| editable(n) && n.focused) {
        return Some(i);
    }
    nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| editable(n))
        .max_by(|(_, a), (_, b)| (a.y + a.h).total_cmp(&(b.y + b.h)))
        .map(|(i, _)| i)
}

/// One look at a window's tree. `sent`: what FERB typed into this chat (it only confirms a message is the owner's).
pub fn read(nodes: &[Node], facts: &AppFacts, sent: &[String]) -> Look {
    let comp = composer(nodes);
    let floor = comp.map(|i| nodes[i].y).unwrap_or(f64::INFINITY);
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
        .filter(|n| is_text(n) && n.y + n.h <= floor + 1.0)
    {
        let w = words(n);
        if w.is_empty() {
            continue;
        }
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
    for (n, mark) in runs {
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
