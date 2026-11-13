//! policy/carried.py: stored provenance of what FERB writes from what it was shown (T-1007 design B, GATE-519,
//! GATE-522, GATE-524, GATE-529, GATE-534): a model-written row keeps its sources' kinds and the owner's values its own
//! text holds, never the union of every label its writer saw. Shape and provenance only (rule 4).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use ferb_sanitize::origin;
use serde_json::{json, Map, Value};

use crate::confirm::{fine_split, page_scope, spellings, whole_values};
use crate::provenance::{self, MIN_LEN};
use crate::py;

/// The JSON field a derived row keeps it in.
pub const PROVENANCE: &str = "provenance";
const TRAIL: &str = ".,;:!?)]}>\"'";

fn owners(o: &str) -> bool {
    o == origin::PRIVATE || o == origin::OWNER
}

/// `_JOINED.sub("", text)`: separators between two digits dropped (`(?<=\d)[\s.,+\-]+(?=\d)`).
#[doc(hidden)]
pub fn joined(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let sep = |c: char| py::is_space(c) || ".,+-".contains(c);
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if sep(chars[i]) && i > 0 && py::is_decimal(chars[i - 1]) {
            let mut j = i;
            while j < chars.len() && sep(chars[j]) {
                j += 1;
            }
            if j < chars.len() && py::is_decimal(chars[j]) {
                i = j;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn forms(text: &str) -> (String, String) {
    let folded = py::casefold(text);
    let j = joined(&folded);
    (folded, j)
}

fn holds(forms: &(String, String), spelled: &HashSet<String>) -> bool {
    [&forms.0, &forms.1].iter().any(|t| {
        spelled
            .iter()
            .any(|v| !v.is_empty() && provenance::contains(t, v))
    })
}

fn web_hosts(labels: &BTreeSet<String>) -> BTreeSet<String> {
    let prefix = format!("{}:web:", origin::EXTERNAL);
    labels
        .iter()
        .filter(|o| o.starts_with(&prefix) && o.matches(':').count() >= 2)
        .map(|o| py::lower(o.splitn(3, ':').nth(2).unwrap_or("")))
        .collect()
}

fn sites(labels: &BTreeSet<String>, page: &str) -> BTreeSet<String> {
    let prefix = format!("{}:web:", origin::EXTERNAL);
    let web = web_hosts(labels);
    if web.is_empty()
        || labels
            .iter()
            .any(|o| o != origin::PRIVATE && !o.starts_with(&prefix))
    {
        return BTreeSet::new();
    }
    let scope = if page.is_empty() {
        String::new()
    } else {
        page_scope(page)
    };
    if !scope.is_empty() && web.contains(scope.split('/').next().unwrap_or("")) {
        [format!("site:{scope}")].into()
    } else {
        BTreeSet::new()
    }
}

/// One record shown to the writer: its labels, its text, the values FERB stored for it (a row it wrote itself),
/// and the page it was read from.
#[derive(Debug, Clone, Default)]
pub struct Shown {
    pub labels: BTreeSet<String>,
    pub text: String,
    pub stored: Option<BTreeMap<String, Vec<String>>>,
    pub page: String,
}

struct Record {
    labels: BTreeSet<String>,
    forms: (String, String),
    stored: Option<Vec<(String, Vec<String>)>>,
    page: String,
}

/// `derive`: `{"kinds": [...], "values": {value: [classes]}}` for a row FERB writes with `text`, its writer shown
/// `shown`; `made` are the row's own labels.
pub fn derive(text: &str, shown: &[Shown], made: &[String]) -> Value {
    let made: BTreeSet<String> = made.iter().cloned().collect();
    let records: Vec<Record> = shown
        .iter()
        .filter(|s| !s.text.is_empty())
        .map(|s| Record {
            labels: s.labels.clone(),
            forms: forms(&s.text),
            stored: s
                .stored
                .as_ref()
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
            page: s.page.clone(),
        })
        .collect();
    let all_labels: Vec<&str> = records
        .iter()
        .flat_map(|r| r.labels.iter().map(String::as_str))
        .chain(made.iter().map(String::as_str))
        .collect();
    let kinds: Vec<String> = origin::kinds(all_labels).into_iter().collect();
    let saw_private = made.iter().any(|o| owners(o))
        || records.iter().any(|r| r.labels.iter().any(|o| owners(o)));
    let owner_classes = |labels: &BTreeSet<String>, page: &str| -> BTreeSet<String> {
        let site = if labels.contains(origin::PRIVATE) {
            sites(labels, page)
        } else {
            BTreeSet::new()
        };
        let mut out: BTreeSet<String> = labels
            .iter()
            .filter(|o| *o == origin::OWNER)
            .cloned()
            .collect();
        if site.is_empty() {
            out.extend(labels.iter().filter(|o| *o == origin::PRIVATE).cloned());
        } else {
            out.extend(site);
        }
        out
    };
    let mut values: Vec<(String, Vec<String>)> = Vec::new();
    for value in whole_values(text) {
        let spelled = spellings(&value);
        let mut classes: BTreeSet<String> = BTreeSet::new();
        let mut accounted = false;
        for r in &records {
            if let Some(stored) = &r.stored {
                let held: Vec<String> = stored
                    .iter()
                    .filter(|(v, _)| holds(&forms(v), &spelled))
                    .flat_map(|(_, cs)| cs.iter().cloned())
                    .collect();
                accounted = accounted || (held.is_empty() && holds(&r.forms, &spelled));
                classes.extend(held);
            } else if r.labels.iter().any(|o| owners(o)) {
                if holds(&r.forms, &spelled) {
                    classes.extend(owner_classes(&r.labels, &r.page));
                }
            } else if holds(&r.forms, &spelled) {
                accounted = true;
            }
        }
        if classes.is_empty() && saw_private && !accounted {
            classes.insert(origin::PRIVATE.to_string());
        }
        if !classes.is_empty() {
            values.push((value, classes.into_iter().collect()));
        }
    }
    let folded = py::casefold(text);
    let words: BTreeSet<String> = fine_split(&folded)
        .into_iter()
        .filter(|x| !x.is_empty())
        .map(|x| x.trim_end_matches(|c| TRAIL.contains(c)).to_string())
        .filter(|w| py::char_len(w) >= MIN_LEN && !values.iter().any(|(v, _)| v == w))
        .collect();
    if !words.is_empty() {
        let vocab: Vec<(&BTreeSet<String>, &str, HashSet<String>)> = records
            .iter()
            .filter(|r| r.stored.is_none() && r.labels.iter().any(|o| owners(o)))
            .map(|r| {
                let mut seen: HashSet<String> = fine_split(&r.forms.0)
                    .into_iter()
                    .map(str::to_string)
                    .collect();
                seen.extend(provenance::tokens(&r.forms.0));
                (&r.labels, r.page.as_str(), seen)
            })
            .collect();
        let derived: Vec<&Vec<(String, Vec<String>)>> =
            records.iter().filter_map(|r| r.stored.as_ref()).collect();
        for word in words {
            let mut classes: BTreeSet<String> = BTreeSet::new();
            for (labels, page, seen) in &vocab {
                if seen.contains(&word) {
                    classes.extend(owner_classes(labels, page));
                }
            }
            for stored in &derived {
                if let Some((_, cs)) = stored.iter().find(|(k, _)| *k == word) {
                    classes.extend(cs.iter().cloned());
                }
            }
            if !classes.is_empty() {
                values.push((word, classes.into_iter().collect()));
            }
        }
    }
    let values: Map<String, Value> = values.into_iter().map(|(k, v)| (k, json!(v))).collect();
    json!({"kinds": kinds, "values": values})
}

/// One entry of a stored record of FERB's conversation (`segments`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Segment {
    pub len: usize,
    pub kinds: Vec<String>,
    pub whole: bool,
    pub values: Option<BTreeMap<String, Vec<String>>>,
}

impl Segment {
    /// From a stored segment's JSON.
    pub fn from_json(v: &Value) -> Segment {
        Segment {
            len: v.get("len").and_then(Value::as_u64).unwrap_or(0) as usize,
            kinds: v
                .get("kinds")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(ferb_py::value::py_str).collect())
                .unwrap_or_default(),
            whole: v.get("whole").is_some_and(ferb_py::value::truthy),
            values: v.get("values").and_then(Value::as_object).map(|m| {
                m.iter()
                    .map(|(k, cs)| {
                        let cs = cs
                            .as_array()
                            .map(|a| a.iter().map(ferb_py::value::py_str).collect())
                            .unwrap_or_default();
                        (k.clone(), cs)
                    })
                    .collect()
            }),
        }
    }
}

/// A recorded read (`SpanStore.payload_outputs`): its labels, text, segments and page.
#[derive(Debug, Clone, Default)]
pub struct Output {
    pub origins: BTreeSet<String>,
    pub text: String,
    pub segments: Vec<Segment>,
    pub page: String,
}

/// `text[at:at + size]` by code points.
fn slice(text: &str, at: usize, size: usize) -> String {
    text.chars().skip(at).take(size).collect()
}

/// `shown_records`: a task's recorded reads as `derive` takes them.
pub fn shown_records(outputs: &[Output]) -> Vec<Shown> {
    let mut out = Vec::new();
    for result in outputs {
        if result.segments.is_empty() {
            out.push(Shown {
                labels: result.origins.clone(),
                text: result.text.clone(),
                stored: None,
                page: result.page.clone(),
            });
            continue;
        }
        let mut at = 0;
        for seg in &result.segments {
            let labels: BTreeSet<String> = seg.kinds.iter().cloned().collect();
            out.push(Shown {
                labels,
                text: slice(&result.text, at, seg.len),
                stored: if seg.whole {
                    None
                } else {
                    Some(seg.values.clone().unwrap_or_default())
                },
                page: String::new(),
            });
            at += seg.len + 2;
        }
    }
    out
}

/// `sink_parts`: what of a stored record of FERB's conversation is the owner's data for the sink (GATE-522), and the
/// values stored as read only on sites' own pages, by those sites (GATE-524).
pub fn sink_parts(
    payload: &str,
    segments: &[Segment],
) -> (String, Vec<(BTreeSet<String>, String)>) {
    let mut kept: Vec<String> = Vec::new();
    let mut by_site: Vec<(BTreeSet<String>, Vec<String>)> = Vec::new();
    let mut at = 0;
    for seg in segments {
        let piece = slice(payload, at, seg.len);
        at += seg.len + 2;
        if seg.whole {
            if seg.kinds.iter().any(|k| owners(k)) {
                kept.push(piece);
            }
        } else if let Some(values) = &seg.values {
            for (value, classes) in values {
                let site_set: BTreeSet<String> = classes
                    .iter()
                    .filter_map(|c| c.strip_prefix("site:"))
                    .map(str::to_string)
                    .collect();
                if !site_set.is_empty() && site_set.len() == classes.len() {
                    match by_site.iter_mut().find(|(s, _)| *s == site_set) {
                        Some(slot) => slot.1.push(value.clone()),
                        None => by_site.push((site_set, vec![value.clone()])),
                    }
                } else {
                    kept.push(value.clone());
                }
            }
        }
    }
    (
        kept.join("\n"),
        by_site
            .into_iter()
            .map(|(s, v)| (s, v.join("\n")))
            .collect(),
    )
}

/// `owner_said` (GATE-534): the owner's own earlier messages in a stored record of FERB's conversation.
pub fn owner_said(payload: &str, segments: &[Segment]) -> String {
    let mut said = Vec::new();
    let mut at = 0;
    for seg in segments {
        let kinds: BTreeSet<&str> = seg.kinds.iter().map(String::as_str).collect();
        if seg.whole && kinds.len() == 1 && kinds.contains(origin::OWNER) {
            said.push(slice(payload, at, seg.len));
        }
        at += seg.len + 2;
    }
    said.join("\n")
}

/// `sink_text`: `sink_parts`, every value together.
pub fn sink_text(payload: &str, segments: &[Segment]) -> String {
    let (main, by_site) = sink_parts(payload, segments);
    let mut parts = vec![main];
    parts.extend(by_site.into_iter().map(|(_, v)| v));
    parts.join("\n").trim_matches('\n').to_string()
}
