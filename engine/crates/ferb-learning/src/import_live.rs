//! The memory import's live card (MEM-455, MEM-456): what every client shows in the chat, and FERB.app on its Memory
//! page, from the moment an import the owner is told about starts until its one notice is said. It is live state:
//! never stored, never a history entry; the notice is the only message an import leaves (MEM-445).
//!
//! One fold, here, for every client: the daemon keeps what each told run is doing from the job's own
//! `memory.import.progress` events ([`RunNow::progress`]) and reads the website watches' facts from its records
//! ([`Watch`]); [`card`] turns them into the card's words and numbers. FERB.app and both phones only draw it, so the
//! words are the same everywhere. Code facts only: parts read of parts planned, batches placed of batches, a watch
//! run's start and end and its status; no model's words are read to decide a stage.

use serde_json::{json, Map, Value};

/// What a told import run is doing now (the daemon keeps it from the job's progress events; never stored).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunNow {
    pub run: String,
    pub source: String,
    /// The source as the owner knows it ("Claude Code", "Codex", "ChatGPT", "Claude").
    pub name: String,
    /// A coding assistant on this Mac (what he typed into it), else a website's export (his chats there).
    pub local: bool,
    /// `reading` (the chunks) or `placing` (the facts into the profile).
    pub stage: String,
    /// The run's counts as the job last published them (reading).
    pub counts: Map<String, Value>,
    /// Placing: batches placed, and of how many (0, 0 before the first batch).
    pub placed: (i64, i64),
}

pub const READING: &str = "reading";
pub const PLACING: &str = "placing";
pub const ASKING: &str = "asking";
pub const WAITING_MAIL: &str = "waiting_mail";
pub const CHECKING_MAIL: &str = "checking_mail";

impl RunNow {
    pub fn new(run: &str, source: &str, name: &str, local: bool) -> RunNow {
        RunNow {
            run: run.into(),
            source: source.into(),
            name: name.into(),
            local,
            stage: READING.into(),
            ..RunNow::default()
        }
    }

    /// One `memory.import.progress` event of this run, folded in: a reading step's counts (its wait for the owner's
    /// chat turn and its end carry them as they stood), a placing batch (a wait while placing carries the batch's own
    /// counts: ignored). The daemon ends the card when the notice is said, never on an event.
    pub fn progress(&mut self, data: &Value) {
        let stage = data.get("stage").and_then(Value::as_str).unwrap_or("");
        let counts = data.get("counts").and_then(Value::as_object);
        match stage {
            PLACING => {
                self.stage = PLACING.into();
                if let Some(c) = counts {
                    self.placed = (int(c, "placed"), int(c, "to_place"));
                }
            }
            _ if self.stage == READING => {
                if let Some(c) = counts {
                    self.counts = c.clone();
                }
            }
            _ => {}
        }
    }

    /// Placing starts (before its first batch is known).
    pub fn placing(&mut self) {
        self.stage = PLACING.into();
        self.placed = (0, 0);
    }
}

/// A website's quiet watch (MEM-448, MEM-451) as its records stand, while no import of that site runs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Watch {
    pub source: String,
    pub name: String,
    /// One of its runs is under way now.
    pub running: bool,
    /// When its first run that went well ended: the export was asked for then (its request's own order: the first run
    /// asks, every later one looks in the mail). Local "YYYY-MM-DDTHH:MM".
    pub asked_at: Option<String>,
    /// Its last run that ended didn't go well (failed, or done with gaps): the owner was told (MEM-451).
    pub trouble: bool,
    /// Its next run, local "YYYY-MM-DDTHH:MM".
    pub next_run: Option<String>,
}

/// The owner's yesterday, today and tomorrow ("YYYY-MM-DD", his zone), for the times the card says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Days {
    pub yesterday: String,
    pub today: String,
    pub tomorrow: String,
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// "at 23:40", "yesterday at 23:40", "tomorrow at 00:25", "on Nov 20 at 23:40" (a local "YYYY-MM-DDTHH:MM").
pub fn at_words(at: &str, days: &Days) -> String {
    let (day, time) = (at.get(..10).unwrap_or(""), at.get(11..16).unwrap_or(""));
    if day == days.today {
        format!("at {time}")
    } else if day == days.yesterday {
        format!("yesterday at {time}")
    } else if day == days.tomorrow {
        format!("tomorrow at {time}")
    } else {
        let month = day
            .get(5..7)
            .and_then(|m| m.parse::<usize>().ok())
            .and_then(|m| MONTHS.get(m.wrapping_sub(1)))
            .copied()
            .unwrap_or("");
        let date = day.get(8..10).unwrap_or("").trim_start_matches('0');
        format!("on {month} {date} at {time}")
    }
}

fn int(m: &Map<String, Value>, k: &str) -> i64 {
    m.get(k).and_then(Value::as_i64).unwrap_or(0)
}

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// "Claude Code", "ChatGPT and Claude", "A, B and C".
pub fn names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn row(
    key: String,
    source: &str,
    name: &str,
    stage: &str,
    line: String,
    detail: String,
    (done, total): (i64, i64),
    found: i64,
    found_line: String,
    note: String,
) -> Value {
    let total = total.max(0);
    let done = done.clamp(0, total);
    json!({"key": key, "source": source, "name": name, "stage": stage, "line": line, "detail": detail,
           "done": done, "total": total, "count": if total > 0 { format!("{done} of {total}") } else { String::new() },
           "found": found, "found_line": found_line, "note": note})
}

fn run_row(r: &RunNow) -> Value {
    let c = &r.counts;
    let found = int(c, "facts_saved");
    let key = format!("run:{}", r.run);
    let failed = int(c, "failed_chunks");
    let note = if failed > 0 {
        format!(
            "{} couldn't be read just now. I'll go back to {} next time.",
            plural(failed, "part", "parts"),
            if failed == 1 { "it" } else { "them" }
        )
    } else {
        String::new()
    };
    if r.stage == PLACING {
        let found_line = if found > 0 {
            plural(found, "thing found", "things found")
        } else {
            String::new()
        };
        return row(
            key,
            &r.source,
            &r.name,
            PLACING,
            "Putting together what I found".into(),
            if found > 1 {
                "Many things come up more than once, so I'm joining them up.".into()
            } else {
                String::new()
            },
            r.placed,
            found,
            found_line,
            note,
        );
    }
    let planned = c.contains_key("chunks_left");
    let (done, total) = if planned {
        let done = int(c, "chunks_done");
        (done, done + int(c, "chunks_left"))
    } else {
        (0, 0)
    };
    let line = match (planned, r.local) {
        (false, true) => "Looking through your sessions",
        (false, false) => "Opening your chats",
        (true, true) => "Reading what you've typed",
        (true, false) => "Reading your chats",
    };
    let found_line = if found > 0 {
        plural(found, "thing found so far", "things found so far")
    } else {
        String::new()
    };
    row(
        key,
        &r.source,
        &r.name,
        READING,
        line.into(),
        String::new(),
        (done, total),
        found,
        found_line,
        note,
    )
}

fn watch_row(w: &Watch, days: &Days) -> Value {
    let name = &w.name;
    let (stage, line, detail) = match (&w.asked_at, w.running) {
        (None, true) => (
            ASKING,
            format!("Asking {name} for a copy of your data"),
            String::new(),
        ),
        (None, false) => (
            ASKING,
            match &w.next_run {
                Some(n) if !w.trouble => {
                    format!(
                        "I'll ask {name} for a copy of your data {}",
                        at_words(n, days)
                    )
                }
                _ => format!("Asking {name} for a copy of your data"),
            },
            String::new(),
        ),
        (Some(asked), true) => (
            CHECKING_MAIL,
            format!("Checking your email for {name}'s copy"),
            format!("Asked {}", at_words(asked, days)),
        ),
        (Some(asked), false) => (
            WAITING_MAIL,
            format!(
                "Waiting for {name}'s email — asked {}",
                at_words(asked, days)
            ),
            "I check your email every 15 minutes. It can take a day or two.".into(),
        ),
    };
    let note = match (&w.next_run, w.trouble && !w.running) {
        (_, false) => String::new(),
        (Some(n), true) => format!(
            "The last try didn't go through. I'll try again {}.",
            at_words(n, days)
        ),
        (None, true) => "The last try didn't go through.".into(),
    };
    row(
        format!("site:{}", w.source),
        &w.source,
        name,
        stage,
        line,
        detail,
        (0, 0),
        0,
        String::new(),
        note,
    )
}

/// The card, or None when nothing is under way: the told runs first (in the order they started), then each website
/// watch with no run of its own. `{"title", "rows": [{key, source, name, stage, line, detail, done, total, count,
/// found, found_line, note}]}`: a bar from `done` of `total` when `total` > 0 (with `count` beside it), `found_line`
/// under it, `detail` in a quieter line, `note` (a gap, a try that didn't go through, with what happens next) set
/// apart. A client shows each row's `name` only when the card has more than one row.
pub fn card(runs: &[RunNow], watches: &[Watch], days: &Days) -> Option<Value> {
    let mut rows: Vec<Value> = runs.iter().map(run_row).collect();
    let mut shown: Vec<&str> = runs.iter().map(|r| r.name.as_str()).collect();
    for w in watches {
        if runs.iter().any(|r| r.source == w.source) {
            continue;
        }
        rows.push(watch_row(w, days));
        shown.push(&w.name);
    }
    if rows.is_empty() {
        return None;
    }
    let mut unique: Vec<&str> = Vec::new();
    for n in shown {
        if !unique.contains(&n) {
            unique.push(n);
        }
    }
    let title = format!(
        "Bringing in what {} {} about you",
        names(&unique),
        if unique.len() == 1 { "knows" } else { "know" }
    );
    Some(json!({"title": title, "rows": rows}))
}
