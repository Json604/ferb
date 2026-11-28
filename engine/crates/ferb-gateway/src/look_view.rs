//! ORCH-753 (ORCH-719 item 2, `kb/plans/context.md`): a look the worker already has is sent as what changed.
//!
//! The record keeps every look whole (the call's span, read by the checker, `task.evidence` and `task.trace`); what
//! the model receives for a repeated look at the same place is the difference from the last look it can rebuild:
//! the page's own facts, the elements that are new or changed (with their new ids), the earlier ones that went,
//! position changes, and the text's changed lines. An element is left out only when the hand says its earlier id
//! still works (the result's `carried` ids); one whose id didn't carry is listed again.
//!
//! The whole look is sent on a run's first look at a place, at a new place (another URL or window), when more than
//! half of the elements or the text changed, every 6th look in a row at one place, after the run's memory was
//! compacted, and when the worker asks (`whole: true`). Code decides all of it from structure, never from content's
//! meaning.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value};

/// The look tools whose results get the view (a Mac look only at offset 0, with no subtree and no `full`).
pub const LOOK_TOOLS: [&str; 3] = ["browser.snapshot", "ferb_browser.snapshot", "mac.snapshot"];
/// Every 6th look in a row at one place is sent whole (ORCH-719).
pub const EVERY_NTH_WHOLE: u32 = 6;
/// More than half changed: the whole look is sent.
pub const CHANGED_SHARE: f64 = 0.5;
/// A text whose lines changed more than this much is sent whole without measuring further.
const MAX_CHANGED_LINES: usize = 1000;

/// An element's fields that say where it sits in this look, not what it is (they move with scrolling; `container`
/// is a per-look number): left out when an element's content is compared, told under `moved` when they change.
const POSITION: [&str; 4] = ["in_view", "occluded", "offscreen", "container"];
/// A position field's value when the element leaves it out (`snapshot::element_json` writes only the exceptions).
fn position_default(key: &str) -> Value {
    match key {
        "in_view" => Value::Bool(true),
        "occluded" => Value::Bool(false),
        _ => Value::Null,
    }
}

/// The look's fields about the whole listing that stay the same from one look to the next: sent only when they
/// changed (null when one went). Every other field (the page's own facts, the call's news) is always sent.
const STEADY: [&str; 20] = [
    // the browser's
    "media",
    "pdf",
    "bot_check",
    "truncated",
    "omitted_elements",
    "omitted_index",
    "omitted_in_view_elements",
    "omitted_note",
    "total_text_chars",
    "next",
    // the Mac's
    "next_offset",
    "profile",
    "repeated_ax_nodes",
    "left_out",
    "windows",
    "headings",
    "open",
    "retrieval",
    "continuation",
    "offset",
];

/// The view's one line for the model (model-facing words; ORCH-753).
pub const NOTE_PAGE: &str = "Only what changed since your last look at this page. Elements not listed are unchanged \
                             and keep their ids from your earlier looks. changed_since is the last whole look \
                             (task.evidence span_id); whole: true sends the page whole.";
pub const NOTE_WINDOW: &str = "Only what changed since your last look at this window. Elements not listed are \
                               unchanged and keep their ids from your earlier looks. changed_since is the last whole \
                               look (task.evidence span_id); whole: true sends the window whole.";

/// One look as the gateway has it after the call: the redacted content the model would receive whole.
pub struct Look<'a> {
    pub tool: &'a str,
    pub args: &'a Map<String, Value>,
    pub content: &'a Map<String, Value>,
    /// The hand's carried ids ({earlier id: id in this look}); None when the tool says nothing about them.
    pub carried: Option<&'a Map<String, Value>>,
    /// The call's span: its record holds this look whole.
    pub span_id: &'a str,
}

/// Why a look went whole (for tests and the record of what happened; never the model).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Whole {
    First,
    Asked,
    Nth,
    MoreThanHalf,
}

/// What the model receives for a look.
#[derive(Debug, Clone, PartialEq)]
pub enum Sent {
    /// Not a look this view handles: the content as it is, nothing kept.
    Untouched,
    Whole(Whole),
    /// What changed, in place of the content.
    Changed(Map<String, Value>),
}

/// The last look at one place the worker can rebuild from what it was sent.
#[derive(Debug, Clone)]
struct Known {
    /// Its elements, each with the id the worker knows it by.
    elements: Vec<Map<String, Value>>,
    lines: Vec<String>,
    steady: Map<String, Value>,
    whole_span: String,
    /// Looks sent as what changed since the last whole one.
    streak: u32,
}

/// Per model run (its worker id), per place, the last look it can rebuild.
#[derive(Debug, Default)]
pub struct Looks {
    runs: HashMap<String, HashMap<String, Known>>,
}

impl Looks {
    /// The run's memory was compacted, or the run ended: its next look at every place is whole.
    pub fn forget(&mut self, run: &str) {
        self.runs.remove(run);
    }

    /// What run `run` receives for this look; the state moves on to it.
    pub fn view(&mut self, run: &str, look: &Look) -> Sent {
        let Some(place) = place(look) else {
            return Sent::Untouched;
        };
        let Some(elements) = look.content.get("elements").and_then(Value::as_array) else {
            return Sent::Untouched;
        };
        let elements: Vec<Map<String, Value>> = elements
            .iter()
            .filter_map(|e| e.as_object().cloned())
            .collect();
        let lines = lines_of(look.content);
        let steady = steady_of(look.content);
        let places = self.runs.entry(run.to_string()).or_default();
        let whole = |why| {
            (
                Known {
                    elements: elements.clone(),
                    lines: lines.clone(),
                    steady: steady.clone(),
                    whole_span: look.span_id.to_string(),
                    streak: 0,
                },
                Sent::Whole(why),
            )
        };
        let (known, sent) = match places.get(&place) {
            None => whole(Whole::First),
            Some(_) if look.args.get("whole") == Some(&Value::Bool(true)) => whole(Whole::Asked),
            Some(k) if k.streak + 1 >= EVERY_NTH_WHOLE => whole(Whole::Nth),
            Some(k) => match changed(k, &elements, &lines, look) {
                None => whole(Whole::MoreThanHalf),
                Some((rebuilt, view)) => (
                    Known {
                        elements: rebuilt,
                        lines: lines.clone(),
                        steady: steady.clone(),
                        whole_span: k.whole_span.clone(),
                        streak: k.streak + 1,
                    },
                    Sent::Changed(view),
                ),
            },
        };
        places.insert(place, known);
        sent
    }
}

/// Where a look is: a browser tab at its URL without the fragment; a Mac app's window. None: not a look this view
/// handles (another tool, a Mac look's later page, a subtree or a full listing).
pub fn place(look: &Look) -> Option<String> {
    let s = |k: &str| {
        look.content
            .get(k)
            .map(ferb_py::value::py_str)
            .unwrap_or_default()
    };
    match look.tool {
        "browser.snapshot" | "ferb_browser.snapshot" => {
            let url = s("url");
            let url = url.split('#').next().unwrap_or_default();
            Some(format!("{} tab {} {url}", look.tool, s("tab")))
        }
        "mac.snapshot" => {
            let a = |k: &str| look.args.get(k).filter(|v| ferb_py::value::truthy(v));
            if a("offset").is_some() || a("subtree").is_some() || a("full").is_some() {
                return None;
            }
            if look
                .content
                .get("offset")
                .is_some_and(ferb_py::value::truthy)
            {
                return None;
            }
            let current = look
                .content
                .get("windows")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|w| w.get("current").is_some_and(ferb_py::value::truthy))
                .and_then(|w| w.get("id"))
                .map(ferb_py::value::py_str);
            let window = current.unwrap_or_else(|| format!("titled {}", s("window")));
            Some(format!("app {} window {window}", s("bundle_id")))
        }
        _ => None,
    }
}

fn lines_of(content: &Map<String, Value>) -> Vec<String> {
    match content.get("text") {
        Some(Value::String(t)) => t.split('\n').map(str::to_string).collect(),
        Some(Value::Null) | None => vec![],
        Some(other) => vec![ferb_py::value::py_str(other)],
    }
}

fn steady_of(content: &Map<String, Value>) -> Map<String, Value> {
    STEADY
        .iter()
        .filter_map(|k| content.get(*k).map(|v| (k.to_string(), v.clone())))
        .collect()
}

/// An element as what it is: without its id and where it sits in this look.
fn ident(e: &Map<String, Value>) -> String {
    let what: Map<String, Value> = e
        .iter()
        .filter(|(k, _)| k.as_str() != "id" && !POSITION.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    ferb_py::json::dumps(&Value::Object(what))
}

fn position(e: &Map<String, Value>) -> Vec<Value> {
    POSITION
        .iter()
        .map(|k| e.get(*k).cloned().unwrap_or_else(|| position_default(k)))
        .collect()
}

fn id_of(e: &Map<String, Value>) -> String {
    e.get("id").map(ferb_py::value::py_str).unwrap_or_default()
}

/// What the model receives for a look in place of its content.
type View = Map<String, Value>;

/// The view of a look against the last one the worker can rebuild, and what it can rebuild after it; None when more
/// than half changed.
fn changed(
    known: &Known,
    elements: &[Map<String, Value>],
    lines: &[String],
    look: &Look,
) -> Option<(Vec<Map<String, Value>>, View)> {
    // each element the worker knows whose id the hand carried into this look, by the id it names now
    let mut by_now: HashMap<String, usize> = HashMap::new();
    if let Some(carried) = look.carried {
        for (i, k) in known.elements.iter().enumerate() {
            if let Some(now) = carried.get(&id_of(k)).and_then(Value::as_str) {
                by_now.entry(now.to_string()).or_insert(i);
            }
        }
    }
    let mut used: HashSet<usize> = HashSet::new();
    let (mut shown, mut moved, mut rebuilt) = (vec![], vec![], vec![]);
    for e in elements {
        let earlier = by_now
            .get(&id_of(e))
            .copied()
            .filter(|i| !used.contains(i) && ident(&known.elements[*i]) == ident(e));
        match earlier {
            Some(i) => {
                used.insert(i);
                let k = &known.elements[i];
                let mut kept = e.clone();
                kept.insert("id".into(), k.get("id").cloned().unwrap_or(Value::Null));
                if position(k) != position(e) {
                    let mut m = Map::new();
                    m.insert("id".into(), k.get("id").cloned().unwrap_or(Value::Null));
                    for key in POSITION {
                        if e.contains_key(key) || k.contains_key(key) {
                            m.insert(
                                key.into(),
                                e.get(key).cloned().unwrap_or_else(|| position_default(key)),
                            );
                        }
                    }
                    moved.push(Value::Object(m));
                }
                rebuilt.push(kept);
            }
            None => {
                shown.push(Value::Object(e.clone()));
                rebuilt.push(e.clone());
            }
        }
    }
    let mut gone = Map::new();
    for (i, k) in known.elements.iter().enumerate() {
        if !used.contains(&i) {
            gone.insert(
                id_of(k),
                k.get("label")
                    .cloned()
                    .unwrap_or(Value::String(String::new())),
            );
        }
    }
    let share = (shown.len() + gone.len()) as f64 / (elements.len() + gone.len()).max(1) as f64;
    if share > CHANGED_SHARE {
        return None;
    }
    let total = known.lines.len() + lines.len();
    let cap = (total / 2).min(MAX_CHANGED_LINES);
    let hunks = line_diff(&known.lines, lines, cap)?;
    let edits: usize = hunks.iter().map(|h| h.removed + h.added.len()).sum();
    if total > 0 && edits as f64 / total as f64 > CHANGED_SHARE {
        return None;
    }
    let mut view = Map::new();
    let steady = steady_of(look.content);
    for (k, v) in look.content {
        if k == "elements" || k == "text" {
            continue;
        }
        if STEADY.contains(&k.as_str()) && known.steady.get(k) == Some(v) {
            continue;
        }
        view.insert(k.clone(), v.clone());
    }
    for k in known.steady.keys() {
        if !steady.contains_key(k) {
            view.insert(k.clone(), Value::Null);
        }
    }
    let unchanged = rebuilt.len() - shown.len();
    view.insert(
        "changed_since".into(),
        Value::String(known.whole_span.clone()),
    );
    view.insert("unchanged".into(), Value::from(unchanged));
    let note = if look.tool == "mac.snapshot" {
        NOTE_WINDOW
    } else {
        NOTE_PAGE
    };
    view.insert("note".into(), Value::String(note.into()));
    view.insert("elements".into(), Value::Array(shown));
    if !gone.is_empty() {
        view.insert("gone".into(), Value::Object(gone));
    }
    if !moved.is_empty() {
        view.insert("moved".into(), Value::Array(moved));
    }
    if !hunks.is_empty() {
        view.insert(
            "text_changes".into(),
            Value::Array(hunks.iter().map(Hunk::to_json).collect()),
        );
    }
    Some((rebuilt, view))
}

/// One run of changed lines: at `line` (from 1, in the new text) `removed` earlier lines gave way to `added`.
#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    pub line: usize,
    pub removed: usize,
    pub added: Vec<String>,
}

impl Hunk {
    fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("line".into(), Value::from(self.line));
        if self.removed > 0 {
            m.insert("removed".into(), Value::from(self.removed));
        }
        if !self.added.is_empty() {
            m.insert(
                "added".into(),
                Value::Array(self.added.iter().cloned().map(Value::String).collect()),
            );
        }
        Value::Object(m)
    }

    fn of_json(v: &Value) -> Option<Hunk> {
        Some(Hunk {
            line: v.get("line")?.as_u64()? as usize,
            removed: v.get("removed").and_then(Value::as_u64).unwrap_or(0) as usize,
            added: v
                .get("added")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(ferb_py::value::py_str).collect())
                .unwrap_or_default(),
        })
    }
}

/// The fewest line changes from `a` to `b` (Myers' diff), grouped into runs; None when more than `max_edits` lines
/// change.
pub fn line_diff(a: &[String], b: &[String], max_edits: usize) -> Option<Vec<Hunk>> {
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..]
        .iter()
        .rev()
        .zip(b[pre..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (x, y) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let ops = myers(x, y, max_edits)?;
    let mut hunks: Vec<Hunk> = vec![];
    let mut j = 0usize;
    let mut open: Option<Hunk> = None;
    for op in ops {
        match op {
            Op::Keep => {
                if let Some(h) = open.take() {
                    hunks.push(h);
                }
                j += 1;
            }
            Op::Remove => {
                open.get_or_insert(Hunk {
                    line: pre + j + 1,
                    removed: 0,
                    added: vec![],
                })
                .removed += 1;
            }
            Op::Add => {
                open.get_or_insert(Hunk {
                    line: pre + j + 1,
                    removed: 0,
                    added: vec![],
                })
                .added
                .push(y[j].clone());
                j += 1;
            }
        }
    }
    if let Some(h) = open.take() {
        hunks.push(h);
    }
    Some(hunks)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Keep,
    Remove,
    Add,
}

fn myers(a: &[String], b: &[String], max_d: usize) -> Option<Vec<Op>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    if n == 0 && m == 0 {
        return Some(vec![]);
    }
    let width = (max_d as isize) + 1;
    let at = |k: isize| (k + width) as usize;
    let mut v = vec![0isize; (2 * width + 1) as usize];
    // trace[d]: V as it was when step d began, for k in -(d+1)..=(d+1)
    let mut trace: Vec<Vec<isize>> = vec![];
    let mut found = None;
    'outer: for d in 0..=(max_d as isize) {
        trace.push(v[at(-d - 1)..=at(d + 1)].to_vec());
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) {
                v[at(k + 1)]
            } else {
                v[at(k - 1)] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[at(k)] = x;
            if x >= n && y >= m {
                found = Some(d);
                break 'outer;
            }
            k += 2;
        }
    }
    let d_end = found?;
    let get = |d: isize, k: isize| -> isize {
        let row = &trace[d as usize];
        let lo = (-d - 1).max(-width);
        row[(k - lo) as usize]
    };
    let mut ops = vec![];
    let (mut x, mut y) = (n, m);
    for d in (0..=d_end).rev() {
        let k = x - y;
        if d == 0 {
            while x > 0 && y > 0 {
                ops.push(Op::Keep);
                x -= 1;
                y -= 1;
            }
            break;
        }
        let prev_k = if k == -d || (k != d && get(d, k - 1) < get(d, k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = get(d, prev_k);
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            ops.push(Op::Keep);
            x -= 1;
            y -= 1;
        }
        ops.push(if x == prev_x { Op::Add } else { Op::Remove });
        x = prev_x;
        y = prev_y;
    }
    ops.reverse();
    Some(ops)
}

/// What a worker that had `earlier` (the look it could rebuild, as a look's content) knows after receiving `view`:
/// the view's meaning, written as code (the tests' "nothing lost" check). Elements keep the ids the worker knows
/// them by.
pub fn rebuild(earlier: &Map<String, Value>, view: &Map<String, Value>) -> Map<String, Value> {
    let gone: HashSet<String> = view
        .get("gone")
        .and_then(Value::as_object)
        .map(|g| g.keys().cloned().collect())
        .unwrap_or_default();
    let moved: HashMap<String, Map<String, Value>> = view
        .get("moved")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .map(|m| (id_of(m), m.clone()))
        .collect();
    let mut elements: Vec<Value> = vec![];
    for e in earlier
        .get("elements")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
    {
        if gone.contains(&id_of(e)) {
            continue;
        }
        let mut e = e.clone();
        if let Some(m) = moved.get(&id_of(&e)) {
            for key in POSITION {
                match m.get(key) {
                    Some(v) if *v == position_default(key) => {
                        e.shift_remove(key);
                    }
                    Some(v) => {
                        e.insert(key.into(), v.clone());
                    }
                    None => {}
                }
            }
        }
        elements.push(Value::Object(e));
    }
    elements.extend(
        view.get("elements")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .cloned(),
    );
    let old = lines_of(earlier);
    let mut text: Vec<String> = vec![];
    let mut i = 0usize;
    for h in view
        .get("text_changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Hunk::of_json)
    {
        let keep = (h.line - 1).saturating_sub(text.len());
        text.extend(old[i..(i + keep).min(old.len())].iter().cloned());
        i = (i + keep + h.removed).min(old.len());
        text.extend(h.added);
    }
    text.extend(old[i..].iter().cloned());
    let mut out = Map::new();
    let view_only = [
        "changed_since",
        "unchanged",
        "note",
        "gone",
        "moved",
        "text_changes",
    ];
    for (k, v) in earlier {
        if STEADY.contains(&k.as_str()) {
            out.insert(k.clone(), v.clone());
        }
    }
    for (k, v) in view {
        if view_only.contains(&k.as_str()) || k == "elements" {
            continue;
        }
        if v.is_null() && STEADY.contains(&k.as_str()) {
            out.shift_remove(k);
        } else {
            out.insert(k.clone(), v.clone());
        }
    }
    out.insert("elements".into(), Value::Array(elements));
    if earlier.contains_key("text") || view.contains_key("text_changes") {
        out.insert("text".into(), Value::String(text.join("\n")));
    }
    out
}

/// Two looks hold the same thing when their fields agree and their elements are the same, ids aside (the rebuilt
/// one keeps the ids the worker knows).
pub fn same_look(a: &Map<String, Value>, b: &Map<String, Value>) -> bool {
    let fields = |m: &Map<String, Value>| -> Map<String, Value> {
        m.iter()
            .filter(|(k, _)| k.as_str() != "elements")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<std::collections::BTreeMap<_, _>>()
            .into_iter()
            .collect()
    };
    let els = |m: &Map<String, Value>| -> Vec<String> {
        let mut v: Vec<String> = m
            .get("elements")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
            .map(|e| {
                let mut e = e.clone();
                e.shift_remove("id");
                ferb_py::json::dumps(&Value::Object(
                    e.into_iter()
                        .collect::<std::collections::BTreeMap<_, _>>()
                        .into_iter()
                        .collect(),
                ))
            })
            .collect();
        v.sort();
        v
    };
    fields(a) == fields(b) && els(a) == els(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(s: &str) -> Vec<String> {
        s.split('\n').map(str::to_string).collect()
    }

    fn apply(a: &[String], hunks: &[Hunk]) -> Vec<String> {
        let mut out = vec![];
        let mut i = 0;
        for h in hunks {
            let keep = h.line - 1 - out.len();
            out.extend(a[i..i + keep].iter().cloned());
            i += keep + h.removed;
            out.extend(h.added.iter().cloned());
        }
        out.extend(a[i..].iter().cloned());
        out
    }

    // decision: ORCH-753
    #[test]
    fn a_line_diff_rebuilds_the_new_text() {
        let cases = [
            ("a\nb\nc\nd", "a\nx\nc\nd"),
            ("a\nb\nc", "a\nb\nc\nd\ne"),
            ("a\nb\nc", "c"),
            ("", "a"),
            ("a\nb\na\nb\nc", "b\na\nc\nb\na"),
            (
                "one\ntwo\nthree\nfour\nfive\nsix",
                "zero\none\nthree\nfour\n4.5\nfive",
            ),
        ];
        for (a, b) in cases {
            let (a, b) = (l(a), l(b));
            let hunks = line_diff(&a, &b, 100).unwrap();
            assert_eq!(apply(&a, &hunks), b, "{a:?} -> {b:?}: {hunks:?}");
        }
        assert_eq!(line_diff(&l("a\nb"), &l("a\nb"), 0), Some(vec![]));
        // past the cap: no diff (sent whole)
        assert_eq!(line_diff(&l("a\nb\nc\nd"), &l("w\nx\ny\nz"), 3), None);
    }
}
