//! `summary.py`: the permanent daily summary, written by code from the day's trail and journal with no model, and
//! the evidence handles distillation reads (E: the owner's activity, C: their own words, P: what they pasted, F:
//! FERB's own activity).

use std::collections::BTreeSet;

use serde_json::Value;

use ferb_sanitize::origin;

pub const SUMMARY_KIND: &str = "trail.day_summary";
pub const NARRATIVE_KIND: &str = "trail.day_narrative";
pub const SUMMARY_MADE_BY: &str = "code:summary/1";

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(ferb_py::value::truthy)
}

/// summary.py:33 `digest_origins`.
pub fn digest_origins(kind: &str, data: &Value) -> BTreeSet<String> {
    let web = origin::external("web", "trail");
    if kind == NARRATIVE_KIND {
        return BTreeSet::from([origin::INFERRED.to_owned(), web]);
    }
    let mut out = BTreeSet::new();
    if truthy(data.get("windows")) || truthy(data.get("sites")) || truthy(data.get("hours")) {
        out.insert(web);
    }
    if truthy(data.get("apps_min")) || truthy(data.get("folders")) {
        out.insert(origin::PRIVATE.to_owned());
    }
    if let Some(Value::Object(ferb)) = data.get("ferb") {
        if truthy(ferb.get("tasks")) {
            out.insert(origin::INFERRED.to_owned());
        }
    }
    if out.is_empty() {
        out.insert(origin::PRIVATE.to_owned());
    }
    out
}

/// One match of summary.py:29 `UNTRUSTED`: `<untrusted-data id="HEX">\n(text)\n</untrusted-data id="HEX">`, its
/// byte span and the text.
pub fn untrusted_parts(text: &str) -> Vec<(usize, usize, String)> {
    const OPEN: &str = "<untrusted-data id=\"";
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(found) = text[from..].find(OPEN) {
        let start = from + found;
        let id_at = start + OPEN.len();
        let id_len = text[id_at..]
            .bytes()
            .take_while(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
            .count();
        let body = id_at + id_len + 3;
        if id_len > 0 && text[id_at + id_len..].starts_with("\">\n") {
            let id = &text[id_at..id_at + id_len];
            let close = format!("\n</untrusted-data id=\"{id}\">");
            if let Some(end) = text[body..].find(&close) {
                let stop = body + end + close.len();
                out.push((start, stop, text[body..body + end].to_owned()));
                from = stop;
                continue;
            }
        }
        from = start + 1;
    }
    out
}

/// `UNTRUSTED.sub(replacement, text)`.
pub fn replace_untrusted(text: &str, replacement: &str) -> String {
    let mut out = String::new();
    let mut last = 0;
    for (s, e, _) in untrusted_parts(text) {
        out.push_str(&text[last..s]);
        out.push_str(replacement);
        last = e;
    }
    out.push_str(&text[last..]);
    out
}

#[cfg(feature = "store")]
pub use stored::*;

#[cfg(feature = "store")]
mod stored {
    use std::collections::BTreeSet;

    use indexmap_like::Ordered;
    use serde_json::{json, Map, Value};

    use ferb_memory::pystr;
    use ferb_memory::times::{self, LocalZone};

    use super::{
        replace_untrusted, untrusted_parts, NARRATIVE_KIND, SUMMARY_KIND, SUMMARY_MADE_BY,
    };
    use crate::journal::Journal;
    use crate::py;
    use crate::sql;
    use crate::store::{Row, Store};
    use crate::Result;

    /// A source: ("journal" | "trail", id).
    pub type Source = (String, String);

    /// What summary.py takes from other parts: test runs (GATE-550) and the day's security digest.
    pub trait Seams {
        /// `TestRuns.of(db).task(task_id)`: a task of a test run.
        fn test_task(&self, store: &Store, task_id: &str) -> Result<bool>;
        /// `ferb.policy.security_digest.build(db, day)`.
        fn security_digest(&self, store: &Store, day: &str) -> Result<Value>;
    }

    /// summary.py:52 `DayEvidence`.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct DayEvidence {
        pub day: String,
        pub digest: Value,
        pub lines: Vec<String>,
        pub handles: Vec<(String, Vec<Source>)>,
        pub owner_words: Vec<(String, (String, String))>,
    }

    impl DayEvidence {
        pub fn add(&mut self, prefix: &str, text: &str, sources: Vec<Source>) -> String {
            let n = self
                .handles
                .iter()
                .filter(|(k, _)| k.starts_with(prefix))
                .count()
                + 1;
            let h = format!("{prefix}{n}");
            self.handles.push((h.clone(), sources));
            self.lines.push(format!("{h}: {text}"));
            h
        }

        pub fn handle(&self, h: &str) -> Option<&Vec<Source>> {
            self.handles.iter().find(|(k, _)| k == h).map(|(_, v)| v)
        }

        pub fn owner_words(&self, h: &str) -> Option<&(String, String)> {
            self.owner_words
                .iter()
                .find(|(k, _)| k == h)
                .map(|(_, v)| v)
        }
    }

    /// An insertion-ordered map with Python dict semantics, enough for the counters here.
    mod indexmap_like {
        #[derive(Clone, Debug, Default)]
        pub struct Ordered<V>(pub Vec<(String, V)>);

        impl<V: Default> Ordered<V> {
            pub fn entry(&mut self, k: &str) -> &mut V {
                let at = match self.0.iter().position(|(key, _)| key == k) {
                    Some(at) => at,
                    None => {
                        self.0.push((k.to_owned(), V::default()));
                        self.0.len() - 1
                    }
                };
                &mut self.0[at].1
            }
        }

        impl Ordered<i64> {
            /// `Counter.most_common()`: by count, high first, ties in insertion order.
            pub fn most_common(&self) -> Vec<(String, i64)> {
                let mut v = self.0.clone();
                v.sort_by_key(|a| std::cmp::Reverse(a.1));
                v
            }

            pub fn total(&self) -> i64 {
                self.0.iter().map(|(_, n)| n).sum()
            }
        }
    }

    fn rows(store: &Store, day: &str) -> Result<Vec<(Row, ferb_py::time::DateTime)>> {
        let mut out = Vec::new();
        for r in store.query("SELECT * FROM trail WHERE day = ? ORDER BY seq", sql![day])? {
            let at = py::parse(&r.s("at"))?;
            out.push((r, at));
        }
        let mut err = None;
        out.sort_by(|a, b| {
            times::compare(&a.1, &b.1).unwrap_or_else(|c| {
                err = Some(c);
                std::cmp::Ordering::Equal
            })
        });
        if let Some(c) = err {
            return Err(c.into());
        }
        Ok(out)
    }

    fn journal_rows(
        store: &Store,
        seams: &dyn Seams,
        zone: &dyn LocalZone,
        day: &str,
    ) -> Result<Vec<Row>> {
        let mut out = Vec::new();
        for r in store.query(
            "SELECT * FROM journal WHERE kind IN ('chat.message', 'task.outcome') ORDER BY seq",
            sql![],
        )? {
            if py::local_day(&py::parse(&r.s("at"))?, zone) != day {
                continue;
            }
            let d = ferb_py::json::loads(&r.s("data_json")).map_err(crate::Error::Json)?;
            if d.get("test_run").is_some_and(ferb_py::value::truthy) {
                continue;
            }
            if r.s("kind") == "task.outcome" {
                let task = match d.get("task_id") {
                    Some(v) if ferb_py::value::truthy(v) => py::py_str(v),
                    _ => String::new(),
                };
                if seams.test_task(store, &task)? {
                    continue;
                }
            }
            out.push(r);
        }
        Ok(out)
    }

    fn get_str(d: &Value, k: &str) -> String {
        py::py_str_opt(d.get(k))
    }

    fn truthy(v: Option<&Value>) -> bool {
        v.is_some_and(ferb_py::value::truthy)
    }

    fn seconds(a: &ferb_py::time::DateTime, b: &ferb_py::time::DateTime) -> Result<f64> {
        Ok(times::total_seconds(times::minus(a, b)?))
    }

    /// summary.py:89 `build`.
    pub fn build(
        store: &Store,
        seams: &dyn Seams,
        zone: &dyn LocalZone,
        day: &str,
    ) -> Result<DayEvidence> {
        let rows = rows(store, day)?;
        let mut ev = DayEvidence {
            day: day.into(),
            digest: json!({}),
            ..DayEvidence::default()
        };
        let data: Vec<Value> = rows
            .iter()
            .map(|(r, _)| ferb_py::json::loads(&r.s("data_json")).map_err(crate::Error::Json))
            .collect::<Result<_>>()?;
        let mut app_s: Ordered<f64> = Ordered::default();
        let mut app_ids: Ordered<Vec<super::stored::Source>> = Ordered::default();
        let mut hourly: Vec<(u32, Ordered<i64>)> = Vec::new();
        let hour = |h: u32, name: &str, hourly: &mut Vec<(u32, Ordered<i64>)>| {
            let at = match hourly.iter().position(|(k, _)| *k == h) {
                Some(at) => at,
                None => {
                    hourly.push((h, Ordered::default()));
                    hourly.len() - 1
                }
            };
            *hourly[at].1.entry(name) += 1;
        };
        let mut current: Option<(String, ferb_py::time::DateTime)> = None;
        for ((r, at), d) in rows.iter().zip(&data) {
            let at = py::local(at, zone);
            let source = r.s("source");
            let idle = source == "idle" && d.get("state") == Some(&json!("idle"));
            if let Some((name, since)) = &current {
                if source == "app_focus" || idle {
                    *app_s.entry(name) += seconds(&at, since)?.min(3600.0);
                    current = None;
                }
            }
            if source == "app_focus" {
                let name = [d.get("app"), d.get("bundle_id")]
                    .into_iter()
                    .find(|v| truthy(*v))
                    .flatten()
                    .map_or_else(|| py::py_str_opt(d.get("bundle_id")), py::py_str);
                if r.s("actor") == "owner" {
                    current = Some((name.clone(), at));
                    app_ids.entry(&name).push(("trail".into(), r.s("id")));
                    hour(at.hour, &name, &mut hourly);
                }
            }
        }
        let mut sites: Ordered<Vec<usize>> = Ordered::default();
        let mut folders: Ordered<Ordered<i64>> = Ordered::default();
        let mut folder_ids: Ordered<Vec<super::stored::Source>> = Ordered::default();
        let mut titles: Ordered<i64> = Ordered::default();
        let mut title_ids: Ordered<Vec<super::stored::Source>> = Ordered::default();
        let (mut idle_s, mut idle_from) = (0.0f64, None);
        let owner: Vec<usize> = (0..rows.len())
            .filter(|&i| rows[i].0.s("actor") == "owner")
            .collect();
        for &i in &owner {
            let (r, at) = (&rows[i].0, py::local(&rows[i].1, zone));
            let d = &data[i];
            let source = r.s("source");
            if source == "browser_tab" && truthy(d.get("host")) {
                let host = py::py_str(&d["host"]);
                sites.entry(&host).push(i);
                hour(at.hour, &host, &mut hourly);
            } else if source == "file" {
                let path = d.get("path").map_or_else(String::new, py::py_str);
                let folder = py::path_parent(&path);
                *folders.entry(&folder).entry(&get_str(d, "change")) += 1;
                folder_ids.entry(&folder).push(("trail".into(), r.s("id")));
            } else if source == "window" && truthy(d.get("title")) {
                let key = format!("{}: {}", get_str(d, "app"), py::py_str(&d["title"]));
                *titles.entry(&key) += 1;
                title_ids.entry(&key).push(("trail".into(), r.s("id")));
            } else if source == "idle" {
                if d.get("state") == Some(&json!("idle")) {
                    idle_from = Some(at);
                } else if let Some(from) = idle_from.take() {
                    idle_s += seconds(&at, &from)?;
                }
            }
        }
        let mut top_apps = app_s.0.clone();
        top_apps.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (name, secs) in &top_apps {
            let ids = app_ids.entry(name).clone();
            ev.add(
                "E",
                &format!(
                    "the owner used {name} for about {} min",
                    py::round_int(secs / 60.0)
                ),
                ids,
            );
        }
        let mut by_sites = sites.0.clone();
        by_sites.sort_by_key(|a| std::cmp::Reverse(a.1.len()));
        for (host, rs) in &by_sites {
            let profiles: BTreeSet<String> = rs
                .iter()
                .map(|&i| {
                    format!(
                        "{}/{}",
                        get_str(&data[i], "browser"),
                        get_str(&data[i], "profile")
                    )
                })
                .collect();
            let profiles: Vec<String> = profiles.into_iter().collect();
            ev.add(
                "E",
                &format!(
                    "the owner visited {host} {} time(s) ({})",
                    rs.len(),
                    profiles.join(", ")
                ),
                rs.iter()
                    .map(|&i| ("trail".to_owned(), rows[i].0.s("id")))
                    .collect(),
            );
        }
        let mut by_folders = folders.0.clone();
        by_folders.sort_by_key(|a| std::cmp::Reverse(a.1.total()));
        for (folder, c) in &by_folders {
            let ids = folder_ids.entry(folder).clone();
            ev.add(
                "E",
                &format!("files changed in {folder}: {}", py::dict_repr(&c.0)),
                ids,
            );
        }
        let common = titles.most_common();
        for (t, n) in &common {
            let ids = title_ids.entry(t).clone();
            ev.add("E", &format!("window '{t}' ({n}x)"), ids);
        }
        let mut ferb_counts: Ordered<i64> = Ordered::default();
        for (r, _) in &rows {
            if r.s("actor") == "ferb" {
                *ferb_counts.entry(&r.s("source")) += 1;
            }
        }
        if !ferb_counts.0.is_empty() {
            ev.add(
                "F",
                &format!("FERB's own activity: {}", py::dict_repr(&ferb_counts.0)),
                rows.iter()
                    .filter(|(r, _)| r.s("actor") == "ferb")
                    .map(|(r, _)| ("trail".to_owned(), r.s("id")))
                    .collect(),
            );
        }
        let (mut chats, mut tasks) = (0i64, Vec::new());
        for j in journal_rows(store, seams, zone, day)? {
            let d = ferb_py::json::loads(&j.s("data_json")).map_err(crate::Error::Json)?;
            let jid = j.s("id");
            if j.s("kind") == "chat.message" {
                chats += 1;
                let text = d.get("text").map_or_else(String::new, py::py_str);
                let own = pystr::strip(&replace_untrusted(&text, "[pasted]")).to_owned();
                if !own.is_empty() {
                    let handle = ev.add(
                        "C",
                        &format!("the owner said: {own}"),
                        vec![("journal".into(), jid.clone())],
                    );
                    if !matches!(d.get("untrusted_parts"), None | Some(Value::Null)) {
                        ev.owner_words.push((
                            handle,
                            (jid.clone(), ferb_sanitize::untrusted::typed_lines_of(&text)),
                        ));
                    }
                }
                for (_, _, pasted) in untrusted_parts(&text) {
                    ev.add(
                        "P",
                        &format!("text the owner pasted (third-party): {pasted}"),
                        vec![("journal".into(), jid.clone())],
                    );
                }
            } else {
                let summary = d.get("summary").map_or_else(String::new, py::py_str);
                tasks.push(json!({"status": d.get("status").cloned().unwrap_or(Value::Null), "summary": summary}));
                ev.add(
                    "F",
                    &format!("FERB ran a task ({}): {summary}", get_str(&d, "status")),
                    vec![("journal".into(), jid)],
                );
            }
        }
        hourly.sort_by_key(|(h, _)| *h);
        let mut hours = Map::new();
        for (h, c) in &hourly {
            hours.insert(
                format!("{h:02}"),
                Value::Array(
                    c.most_common()
                        .into_iter()
                        .map(|(k, _)| Value::String(k))
                        .collect(),
                ),
            );
        }
        let mut apps_min = Map::new();
        for (k, v) in &top_apps {
            apps_min.insert(k.clone(), json!(py::round_int(v / 60.0)));
        }
        let mut sites_map = Map::new();
        for (h, rs) in &by_sites {
            sites_map.insert(h.clone(), json!(rs.len()));
        }
        let mut folders_map = Map::new();
        for (f, c) in &by_folders {
            let inner: Map<String, Value> =
                c.0.iter().map(|(k, n)| (k.clone(), json!(n))).collect();
            folders_map.insert(f.clone(), Value::Object(inner));
        }
        let events: Map<String, Value> = ferb_counts
            .0
            .iter()
            .map(|(k, n)| (k.clone(), json!(n)))
            .collect();
        ev.digest = json!({
            "security": seams.security_digest(store, day)?,
            "day": day,
            "hours": hours,
            "apps_min": apps_min,
            "sites": sites_map,
            "folders": folders_map,
            "windows": common.iter().map(|(t, _)| Value::String(t.clone())).collect::<Vec<_>>(),
            "duration_estimate": {"method": "focus-event intervals", "maximum_gap_seconds": 3600},
            "idle_min": py::round_int(idle_s / 60.0),
            "events": owner.len(),
            "chats": chats,
            "ferb": {"events": events, "tasks": tasks},
        });
        Ok(ev)
    }

    /// summary.py:187 `summarized`.
    pub fn summarized(store: &Store, day: &str) -> Result<bool> {
        store.exists(
            "SELECT 1 FROM journal WHERE kind = ? AND json_extract(data_json, '$.day') = ?",
            sql![SUMMARY_KIND, day],
        )
    }

    /// summary.py:192 `rebuild`: after a forget, the day's summary again from the rows that remain (its narrative
    /// is deleted, not regenerated). False when no raw rows are left.
    pub fn rebuild(
        store: &Store,
        journal: &Journal,
        seams: &dyn Seams,
        zone: &dyn LocalZone,
        day: &str,
    ) -> Result<bool> {
        if !store.exists("SELECT 1 FROM trail WHERE day = ? LIMIT 1", sql![day])? {
            return Ok(false);
        }
        store.exec(
            "DELETE FROM journal WHERE kind IN (?, ?) AND json_extract(data_json, '$.day') = ?",
            sql![SUMMARY_KIND, NARRATIVE_KIND, day],
        )?;
        let mut digest = build(store, seams, zone, day)?.digest;
        if let Value::Object(m) = &mut digest {
            m.insert("rebuilt".into(), json!(true));
            m.insert("made_by".into(), json!(SUMMARY_MADE_BY));
        }
        journal.append(SUMMARY_KIND, Some(digest), None)?;
        Ok(true)
    }
}
