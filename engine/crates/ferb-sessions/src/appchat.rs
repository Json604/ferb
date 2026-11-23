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
    /// HAND-703: how many containers it sits below the pane's root (the tree's shape, in reading order); 0 for every
    /// node of a read with no shape (then the layout alone groups the words).
    #[serde(default)]
    pub depth: usize,
    /// HAND-703: its Accessibility subrole (AXCodeStyleGroup, AXApplicationStatus, …).
    #[serde(default)]
    pub subrole: String,
    /// HAND-703: a control that opens and closes what is under it says whether it is open now.
    #[serde(default)]
    pub expanded: Option<bool>,
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

/// One message as read: one of the owner's, or one whole reply of the app (HAND-703: every part of one turn).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Block {
    /// Its words, plain (one line per run or paragraph).
    pub text: String,
    pub speaker: Speaker,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    /// HAND-703: an app's reply in its parts, in order, as the app shows them (empty for the owner's).
    #[serde(default)]
    pub parts: Vec<crate::model::Part>,
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

/// Text with icon-font glyphs taken out, its own spaces kept (one run of a line beside others).
fn plain_kept(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !matches!(*c as u32, 0xE000..=0xF8FF | 0xF0000..=0xFFFFD | 0x100000..=0x10FFFD | 0xFFFD)
        })
        .collect()
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
///
/// HAND-703: each of the owner's messages is one block, and each reply of the app is one block whatever it holds,
/// its parts in the app's own order: words (paragraphs, headings, lists, tables, code, as Markdown), a thought, a step
/// it ran, a time or a state line. What a part is comes from the tree's structure only (its roles, subroles, a
/// control that opens and closes, a status region), never from the words (AGENTS.md rule 4). A read with no shape
/// (every depth 0) is grouped by the layout, as before, and an app's runs between two of the owner's messages are
/// still one reply.
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
    let floor = c.y;
    let stop_control = nodes.iter().any(|n| {
        n.role == "AXButton"
            && n.enabled
            && facts
                .stop_labels
                .iter()
                .any(|l| l.eq_ignore_ascii_case(n.label.trim()))
    });
    let tree = Tree::of(nodes);
    // the runs of words above the composer, in its column: texts, and a message the app shows in a read-only box of
    // its own (an earlier message of the owner's he can edit there: an input that isn't the composer)
    let mut runs: Vec<Run> = Vec::new();
    let mut marked = false;
    for (i, n) in nodes.iter().enumerate() {
        if i == ci || n.y + n.h > floor + 1.0 || !in_column(n) || inside_composer(n) {
            continue;
        }
        let boxed = n.editable && !n.composer && !n.value.trim().is_empty();
        if !is_text(n) && !boxed {
            continue;
        }
        if tree.shaped && tree.under(i, ci) {
            continue;
        }
        let w = plain(if boxed { n.value.trim() } else { words(n) });
        if w.is_empty() {
            continue;
        }
        if facts
            .owner_labels
            .iter()
            .any(|l| l.eq_ignore_ascii_case(&w))
        {
            marked = true;
            continue;
        }
        // a button's own words are its label (chrome), unless it opens and closes a part (a thought, a step)
        if tree.shaped && tree.in_plain_button(i) {
            continue;
        }
        let raw = plain_kept(if boxed || !n.value.trim().is_empty() {
            &n.value
        } else {
            &n.label
        });
        runs.push(Run {
            i,
            words: w,
            raw,
            marked: std::mem::take(&mut marked),
            boxed,
        });
    }
    if !tree.shaped {
        runs.sort_by(|a, b| {
            let (x, y) = (&nodes[a.i], &nodes[b.i]);
            x.y.total_cmp(&y.y).then(x.x.total_cmp(&y.x))
        });
    }
    // the transcript's column: its left and right edges
    let left = runs
        .iter()
        .map(|r| nodes[r.i].x)
        .fold(f64::INFINITY, f64::min);
    let right = runs
        .iter()
        .map(|r| nodes[r.i].x + nodes[r.i].w)
        .fold(f64::NEG_INFINITY, f64::max);
    let width = (right - left).max(1.0);
    let on_right =
        |n: &Node| n.x - left > RIGHT_INSET * width && right - (n.x + n.w) < RIGHT_FLUSH * width;
    let owner_side = |n: &Node| match facts.owner_side {
        Side::Layout | Side::Right => on_right(n),
        Side::Left => !on_right(n),
    };
    // rows: one message each (the transcript's own rows when the tree has a shape, else runs close together on one
    // side)
    let mut rows: Vec<Vec<Run>> = Vec::new();
    if tree.shaped {
        let top = tree.common(runs.iter().map(|r| r.i));
        let mut last: Option<usize> = None;
        for r in runs {
            let row = tree.child_toward(top, r.i);
            if last == Some(row) && !r.marked {
                if let Some(g) = rows.last_mut() {
                    g.push(r);
                    continue;
                }
            }
            last = Some(row);
            rows.push(vec![r]);
        }
    } else {
        for r in runs {
            let n = &nodes[r.i];
            let joins = rows.last().is_some_and(|g| {
                let prev = &nodes[g.last().expect("a row has a run").i];
                on_right(prev) == on_right(n) && n.y - (prev.y + prev.h) < GAP && !r.marked
            });
            match rows.last_mut() {
                Some(g) if joins => g.push(r),
                _ => rows.push(vec![r]),
            }
        }
    }
    // who said each row: the app's label before it, the owner's side (every run of it), his read-only box, or FERB's
    // own sent words
    let said: Vec<Speaker> = rows
        .iter()
        .map(|g| {
            let text = g
                .iter()
                .map(|r| r.words.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let confirmed = sent.iter().any(|s| one_line(s) == one_line(&text));
            let mark = g[0].marked;
            let side = g.iter().all(|r| owner_side(&nodes[r.i]));
            if mark || side || g.iter().all(|r| r.boxed) || confirmed {
                Speaker::Owner
            } else {
                Speaker::App
            }
        })
        .collect();
    // turns: each owner row alone; an app's rows in a row are one reply
    let mut blocks: Vec<Block> = Vec::new();
    let mut pending: Vec<Run> = Vec::new();
    let flush = |pending: &mut Vec<Run>, blocks: &mut Vec<Block>| {
        if pending.is_empty() {
            return;
        }
        let turn = std::mem::take(pending);
        blocks.push(reply(&turn, nodes, &tree));
    };
    for (g, who) in rows.into_iter().zip(said) {
        match who {
            Speaker::App => pending.extend(g),
            Speaker::Owner => {
                flush(&mut pending, &mut blocks);
                let (x, y, w) = extent(&g, nodes);
                blocks.push(Block {
                    text: g
                        .iter()
                        .map(|r| r.words.as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                    speaker: Speaker::Owner,
                    x,
                    y,
                    w,
                    parts: Vec::new(),
                });
            }
        }
    }
    flush(&mut pending, &mut blocks);
    Look {
        composer: Some(ci),
        blocks,
        stop_control,
    }
}

/// One run of words: its node, its words, whether the app's owner label came just before it, and whether it is a
/// message in a read-only box of its own.
#[derive(Clone, Debug)]
struct Run {
    i: usize,
    words: String,
    /// The words as the app gives them, their own spaces kept (runs of one line carry the spaces between them).
    raw: String,
    marked: bool,
    boxed: bool,
}

fn extent(g: &[Run], nodes: &[Node]) -> (f64, f64, f64) {
    let x = g.iter().map(|r| nodes[r.i].x).fold(f64::INFINITY, f64::min);
    let r = g
        .iter()
        .map(|r| nodes[r.i].x + nodes[r.i].w)
        .fold(f64::NEG_INFINITY, f64::max);
    let y = g.iter().map(|r| nodes[r.i].y).fold(f64::INFINITY, f64::min);
    (x, y, r - x)
}

/// Roles whose words sit inside their block's line (a text run, a link).
const INLINE_ROLES: [&str; 3] = ["AXStaticText", "AXLink", "text"];
/// The subrole of code (inline, or every run of a block: a code block).
const CODE_SUBROLE: &str = "AXCodeStyleGroup";
/// Subroles of the app's own small lines: a status region, a time.
const META_SUBROLES: [&str; 2] = ["AXApplicationStatus", "AXTimeGroup"];
/// Roles of a control that turns while the app works.
const BUSY_ROLES: [&str; 2] = ["AXProgressIndicator", "AXBusyIndicator"];

/// The pane's tree, from the nodes' depths (reading order).
struct Tree<'a> {
    nodes: &'a [Node],
    parent: Vec<Option<usize>>,
    /// Where each node's subtree ends (exclusive).
    end: Vec<usize>,
    /// The read has a shape (some node has a depth).
    shaped: bool,
}

impl<'a> Tree<'a> {
    fn of(nodes: &'a [Node]) -> Tree<'a> {
        let mut parent = vec![None; nodes.len()];
        let mut stack: Vec<usize> = Vec::new();
        for (i, n) in nodes.iter().enumerate() {
            while stack.last().is_some_and(|&j| nodes[j].depth >= n.depth) {
                stack.pop();
            }
            parent[i] = stack.last().copied();
            stack.push(i);
        }
        let mut end: Vec<usize> = (1..=nodes.len()).collect();
        for i in (0..nodes.len()).rev() {
            if let Some(p) = parent[i] {
                end[p] = end[p].max(end[i]);
            }
        }
        Tree {
            nodes,
            parent,
            end,
            shaped: nodes.iter().any(|n| n.depth > 0),
        }
    }

    /// `i`'s ancestors, nearest first (not itself).
    fn up(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        std::iter::successors(self.parent[i], move |&p| self.parent[p])
    }

    /// Whether `i` sits inside `j`'s subtree (or is `j`).
    fn under(&self, i: usize, j: usize) -> bool {
        i >= j && i < self.end[j]
    }

    /// The deepest node holding all of these (None: no common one).
    fn common(&self, mut ids: impl Iterator<Item = usize>) -> Option<usize> {
        let first = ids.next()?;
        let mut chain: Vec<usize> = self.up(first).collect();
        chain.insert(0, first);
        for i in ids {
            chain.retain(|&a| self.under(i, a));
        }
        chain.first().copied()
    }

    /// The child of `top` on the way down to `i` (`i` itself when `top` is its parent or None: the top-level node).
    fn child_toward(&self, top: Option<usize>, i: usize) -> usize {
        let mut cur = i;
        for a in self.up(i) {
            if Some(a) == top {
                return cur;
            }
            cur = a;
        }
        if top.is_none() {
            cur
        } else {
            i
        }
    }

    fn role(&self, i: usize) -> &str {
        self.nodes[i].role.as_str()
    }

    /// A control that opens and closes what is under it (a thought's or a step's row).
    fn opener(&self, i: usize) -> bool {
        self.nodes[i].expanded.is_some() || self.role(i) == "AXDisclosureTriangle"
    }

    /// The nearest opener holding `i` (itself included).
    fn opener_of(&self, i: usize) -> Option<usize> {
        std::iter::once(i)
            .chain(self.up(i))
            .find(|&a| self.opener(a))
    }

    /// `i` is a button's own words (its label), the button opening nothing.
    fn in_plain_button(&self, i: usize) -> bool {
        std::iter::once(i)
            .chain(self.up(i))
            .any(|a| self.role(a) == "AXButton" && !self.opener(a))
            && self.opener_of(i).is_none()
    }

    /// The nearest ancestor with this role.
    fn above(&self, i: usize, role: &str) -> Option<usize> {
        self.up(i).find(|&a| self.role(a) == role)
    }

    /// The block `i`'s words belong to: its nearest ancestor that isn't a run, a link or code.
    fn block(&self, i: usize) -> Option<usize> {
        self.up(i).find(|&a| {
            !INLINE_ROLES.contains(&self.role(a)) && self.nodes[a].subrole != CODE_SUBROLE
        })
    }

    /// `i` is code (inside a code group below its block).
    fn code(&self, i: usize) -> bool {
        let block = self.block(i);
        self.up(i)
            .take_while(|&a| Some(a) != block)
            .any(|a| self.nodes[a].subrole == CODE_SUBROLE)
            || self.nodes[i].subrole == CODE_SUBROLE
    }

    /// `i` is one of the app's small lines: in a status region or a time, beside a turning control, or the only words
    /// in a row of buttons (a reply's own bar: its time beside copy and share).
    fn meta(&self, i: usize, runs_in: &dyn Fn(usize) -> usize) -> bool {
        if self
            .up(i)
            .chain(std::iter::once(i))
            .any(|a| META_SUBROLES.contains(&self.nodes[a].subrole.as_str()))
        {
            return true;
        }
        let Some(b) = self.block(i) else {
            return false;
        };
        let kids = (b + 1)..self.end[b];
        let busy = kids.clone().any(|k| BUSY_ROLES.contains(&self.role(k)));
        let buttons = kids
            .filter(|&k| self.parent[k] == Some(b) && self.role(k) == "AXButton")
            .count();
        busy || (buttons > 0 && runs_in(b) == 1 && self.opener_of(i).is_none())
    }
}

/// HAND-703: one app reply from its runs, in its parts.
fn reply(turn: &[Run], nodes: &[Node], tree: &Tree) -> Block {
    use crate::model::{Part, PartKind};
    let (x, y, w) = extent(turn, nodes);
    let text = turn
        .iter()
        .map(|r| r.words.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let ids: Vec<usize> = turn.iter().map(|r| r.i).collect();
    let words_of = |i: usize| -> &str {
        turn.iter()
            .find(|r| r.i == i)
            .map(|r| r.words.as_str())
            .unwrap_or("")
    };
    let runs_in = |b: usize| ids.iter().filter(|&&i| tree.under(i, b)).count();
    // the reply's own holder: an opener's holder below it is that thought's or step's own
    let top = tree.common(ids.iter().copied());
    let mut parts: Vec<Part> = Vec::new();
    let mut md = String::new();
    let mut last_block: Option<Option<usize>> = None;
    let mut last_node: Option<usize> = None;
    let mut done: Vec<usize> = Vec::new(); // tables, lists and code blocks already written whole
    let mut steps: Vec<(usize, usize)> = Vec::new(); // (opener, its part)
    let flush = |md: &mut String, parts: &mut Vec<Part>| {
        let t = md.trim().to_owned();
        if !t.is_empty() {
            parts.push(Part {
                kind: PartKind::Text,
                text: t,
                label: String::new(),
                detail: String::new(),
            });
        }
        md.clear();
    };
    for (k, r) in turn.iter().enumerate() {
        let i = r.i;
        if done.iter().any(|&d| tree.under(i, d)) {
            continue;
        }
        if tree.shaped {
            // a thought's or a step's row, and what it opens
            if let Some(o) = tree.opener_of(i) {
                if let Some(&(_, at)) = steps.iter().find(|(s, _)| *s == o) {
                    let p: &mut Part = &mut parts[at];
                    if p.label.is_empty() {
                        p.label = r.words.clone();
                    } else {
                        p.detail = join_words(&p.detail, &r.words);
                    }
                    continue;
                }
                flush(&mut md, &mut parts);
                let icon = (o..tree.end[o]).any(|j| {
                    tree.role(j) == "AXImage"
                        || (is_text(&nodes[j])
                            && plain(words(&nodes[j])).is_empty()
                            && !words(&nodes[j]).is_empty())
                });
                parts.push(Part {
                    kind: if icon {
                        PartKind::Tool
                    } else {
                        PartKind::Thinking
                    },
                    text: String::new(),
                    label: r.words.clone(),
                    detail: String::new(),
                });
                steps.push((o, parts.len() - 1));
                last_block = None;
                continue;
            }
            // what an opener shows under it: the runs after its row inside its own holder
            if let Some(&(o, at)) = steps.last() {
                let holder = tree.parent[o];
                let within = holder.filter(|&h| {
                    Some(h) != top
                        && top.is_none_or(|t| tree.under(h, t))
                        && tree.under(i, h)
                        && i >= tree.end[o]
                });
                if within.is_some() {
                    let p: &mut Part = &mut parts[at];
                    if tree.code(i) {
                        p.kind = PartKind::Tool;
                    }
                    let joins =
                        p.text.is_empty() || tree.block(i) == last_node.and_then(|n| tree.block(n));
                    let sep = if joins { "" } else { "\n" };
                    p.text = format!("{}{sep}{}", p.text, r.words);
                    last_node = Some(i);
                    continue;
                }
            }
            if tree.meta(i, &runs_in) {
                flush(&mut md, &mut parts);
                parts.push(Part {
                    kind: PartKind::Meta,
                    text: r.words.clone(),
                    label: String::new(),
                    detail: String::new(),
                });
                last_block = None;
                continue;
            }
            // a table, a list or a code block: written whole at its first run
            if let Some(t) = tree.above(i, "AXTable").or_else(|| tree.above(i, "AXGrid")) {
                para_break(&mut md);
                md.push_str(&table_md(t, &ids, &words_of, tree));
                md.push_str("\n\n");
                done.push(t);
                last_block = None;
                continue;
            }
            if let Some(l) = tree.above(i, "AXList") {
                para_break(&mut md);
                md.push_str(&list_md(l, &ids, &words_of, tree));
                md.push_str("\n\n");
                done.push(l);
                last_block = None;
                continue;
            }
            let block = tree.block(i);
            if let Some(b) = block.filter(|&b| {
                let mine: Vec<usize> = ids.iter().copied().filter(|&j| tree.under(j, b)).collect();
                !mine.is_empty() && mine.iter().all(|&j| tree.code(j)) && tree.code(i)
            }) {
                para_break(&mut md);
                let body: Vec<&str> = ids
                    .iter()
                    .copied()
                    .filter(|&j| tree.under(j, b))
                    .map(&words_of)
                    .collect();
                md.push_str(&fence(&body.join("\n")));
                md.push_str("\n\n");
                done.push(b);
                last_block = None;
                continue;
            }
            // words: a new paragraph when the block changes
            if last_block != Some(block) {
                para_break(&mut md);
                if block.is_some_and(|b| tree.role(b) == "AXHeading") || tree.role(i) == "AXHeading"
                {
                    md.push_str("### ");
                }
                last_block = Some(block);
            }
            if tree.code(i) {
                md.push('`');
                md.push_str(&r.words.replace('`', "'"));
                md.push('`');
            } else {
                let start = md.is_empty() || md.ends_with('\n');
                let piece = escape(if start { r.raw.trim_start() } else { &r.raw }, start);
                if !md.is_empty()
                    && !md.ends_with(char::is_whitespace)
                    && !piece.starts_with(char::is_whitespace)
                    && needs_space(&md, &piece)
                {
                    md.push(' ');
                }
                md.push_str(&piece);
            }
        } else {
            // no shape: a gap starts a paragraph, a line break keeps the line
            let n = &nodes[i];
            if let Some(prev) = k.checked_sub(1).map(|p| &nodes[turn[p].i]) {
                if n.y - (prev.y + prev.h) >= GAP {
                    para_break(&mut md);
                } else {
                    md.push('\n');
                }
            }
            md.push_str(&escape(&r.words, true));
        }
        last_node = Some(i);
    }
    flush(&mut md, &mut parts);
    Block {
        text,
        speaker: Speaker::App,
        x,
        y,
        w,
        parts,
    }
}

fn join_words(a: &str, b: &str) -> String {
    if a.is_empty() {
        b.to_owned()
    } else {
        format!("{a} {b}")
    }
}

fn para_break(md: &mut String) {
    let t = md.trim_end_matches([' ', '\n']).len();
    md.truncate(t);
    if !md.is_empty() {
        md.push_str("\n\n");
    }
}

/// Runs inside one line usually carry their own spaces; two that meet with none between them are joined by a space
/// only when both sides are letters or digits (a link right after a word keeps its own spacing when it has it).
fn needs_space(before: &str, after: &str) -> bool {
    let a = before.chars().last();
    let b = after.chars().next();
    matches!((a, b), (Some(x), Some(y)) if x.is_alphanumeric() && y.is_alphanumeric())
}

/// The app's words made safe as Markdown (they are data, drawn as written): Markdown's marks are escaped, and at a
/// line's start what would begin a heading, a quote or a list.
pub fn escape(words: &str, line_start: bool) -> String {
    let mut out = String::with_capacity(words.len() + 8);
    for (n, line) in words.split('\n').enumerate() {
        if n > 0 {
            out.push('\n');
        }
        let starts = n > 0 || line_start;
        let body = line.trim_start();
        out.push_str(&line[..line.len() - body.len()]);
        let digits = body.chars().take_while(char::is_ascii_digit).count();
        if starts && matches!(body.chars().next(), Some('#' | '+' | '-')) {
            out.push('\\');
        }
        for (k, ch) in body.chars().enumerate() {
            let list_mark = starts && digits > 0 && k == digits && matches!(ch, '.' | ')');
            if list_mark
                || matches!(
                    ch,
                    '\\' | '`' | '*' | '_' | '[' | ']' | '|' | '~' | '<' | '>'
                )
            {
                out.push('\\');
            }
            out.push(ch);
        }
    }
    out
}

/// A fenced code block that its own words can't close.
fn fence(body: &str) -> String {
    let mut ticks = 3;
    let mut run = 0;
    for ch in body.chars() {
        if ch == '`' {
            run += 1;
            ticks = ticks.max(run + 1);
        } else {
            run = 0;
        }
    }
    let f = "`".repeat(ticks);
    format!("{f}\n{body}\n{f}")
}

/// A table's rows as a pipe table (its first row the header): each row's cells by structure (AXRow, then its cells).
fn table_md<'w>(
    t: usize,
    ids: &[usize],
    words_of: &dyn Fn(usize) -> &'w str,
    tree: &Tree,
) -> String {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row_of: Vec<(usize, usize)> = Vec::new(); // (row node, index in rows)
    for &i in ids.iter().filter(|&&i| tree.under(i, t)) {
        let row = tree
            .up(i)
            .take_while(|&a| a != t)
            .find(|&a| tree.role(a) == "AXRow");
        let Some(row) = row else { continue };
        let cell = tree.child_toward(Some(row), i);
        let at = match row_of.iter().find(|(r, _)| *r == row) {
            Some((_, at)) => *at,
            None => {
                rows.push(Vec::new());
                row_of.push((row, rows.len() - 1));
                rows.len() - 1
            }
        };
        let cells = &mut rows[at];
        let key = format!("{cell}");
        // cells are kept in order; a run in the same cell as the last one joins it
        match cells.last_mut() {
            Some(last) if last.starts_with(&format!("{key}\u{1}")) => {
                last.push(' ');
                last.push_str(&escape(words_of(i), false));
            }
            _ => cells.push(format!("{key}\u{1}{}", escape(words_of(i), false))),
        }
    }
    let rows: Vec<Vec<String>> = rows
        .into_iter()
        .map(|r| {
            r.into_iter()
                .map(|c| {
                    c.split_once('\u{1}')
                        .map(|(_, w)| w.replace('\n', " "))
                        .unwrap_or_default()
                })
                .collect()
        })
        .filter(|r: &Vec<String>| !r.is_empty())
        .collect();
    let Some(cols) = rows.iter().map(Vec::len).max() else {
        return String::new();
    };
    let line = |r: &Vec<String>| {
        let mut cells = r.clone();
        cells.resize(cols, String::new());
        format!("| {} |", cells.join(" | "))
    };
    let mut out = vec![line(&rows[0]), format!("|{}", " --- |".repeat(cols))];
    out.extend(rows[1..].iter().map(line));
    out.join("\n")
}

/// A list's items, one line each: its own marker when the app shows one as words ("1.", "•"), numbered when it is a
/// number, else a bullet.
fn list_md<'w>(
    l: usize,
    ids: &[usize],
    words_of: &dyn Fn(usize) -> &'w str,
    tree: &Tree,
) -> String {
    let mut items: Vec<(usize, Vec<String>)> = Vec::new();
    for &i in ids.iter().filter(|&&i| tree.under(i, l)) {
        let item = tree.child_toward(Some(l), i);
        match items.last_mut() {
            Some((it, ws)) if *it == item => ws.push(words_of(i).to_owned()),
            _ => items.push((item, vec![words_of(i).to_owned()])),
        }
    }
    let mut out = Vec::new();
    for (n, (_, ws)) in items.iter().enumerate() {
        let first = ws.first().map(|w| w.trim()).unwrap_or("");
        let numbered = first
            .trim_end_matches(['.', ')'])
            .chars()
            .all(|c| c.is_ascii_digit())
            && first.len() > 1
            && first.ends_with(['.', ')']);
        let bullet = first.chars().count() == 1 && !first.chars().all(char::is_alphanumeric);
        let rest: Vec<&String> = if numbered || bullet {
            ws.iter().skip(1).collect()
        } else {
            ws.iter().collect()
        };
        let mut body = String::new();
        for w in rest {
            let piece = escape(w, false);
            if !body.is_empty()
                && !body.ends_with(char::is_whitespace)
                && !piece.starts_with(char::is_whitespace)
                && needs_space(&body, &piece)
            {
                body.push(' ');
            }
            body.push_str(&piece);
        }
        let body = body.replace('\n', " ");
        out.push(if numbered {
            format!("{}. {}", n + 1, body.trim())
        } else {
            format!("- {}", body.trim())
        });
    }
    out.join("\n")
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
