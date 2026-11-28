//! Front door step 8 (kb/plans/front-door.md "The card and the library"; ORCH-719 item 6, ORCH-727): the library's
//! index and contents as one block, on an invented five-year store: its size stays bounded at any age (14 days of at
//! most 6 lines, one line per closed month of the last twelve and per closed year, 8 names a kind), it is the same bytes
//! all day (an item saved today joins tomorrow), and a forget takes out the lines written from what it forgot. The
//! Librarian's input never shows outside content and never names an imported line. Temporary folders, invented data.

use ferb_learning::library::{self, CONTENTS_NAMES, DAY_KIND, DAY_LINES, MONTH_KIND, YEAR_KIND};
use ferb_store::Db;
use rusqlite::params;
use serde_json::{json, Value};

const TODAY: &str = "2026-11-27";

fn store() -> (tempfile::TempDir, Db) {
    let tmp = tempfile::tempdir().unwrap();
    let db = Db::open(tmp.path().join("ferb.db")).unwrap();
    db.migrate().unwrap();
    (tmp, db)
}

/// The stamps here are UTC and the Mac's zone is UTC: a stamp's day is its first ten characters.
fn day_of(at: &str) -> Option<String> {
    at.get(..10).map(str::to_owned)
}

fn clock_of(at: &str) -> Option<String> {
    at.get(11..16).map(str::to_owned)
}

fn journal(db: &Db, id: &str, at: &str, kind: &str, data: Value, made: &str) {
    db.conn()
        .execute(
            "INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, ?, ?, ?, ?)",
            params![id, at, kind, data.to_string(), made],
        )
        .unwrap();
}

fn item(db: &Db, id: &str, kind: &str, subject: &str, key: &str, origin: &str, at: &str) {
    db.conn()
        .execute(
            "INSERT INTO memory_items (id, kind, subject, key, content, origin, status, confidence, observed_at, \
             valid_from, created_at) VALUES (?, ?, ?, ?, ?, ?, 'active', 1.0, ?, ?, ?)",
            params![id, kind, subject, key, format!("invented content {id}"), origin, at, at, at],
        )
        .unwrap();
}

/// 15 invented words.
fn words(n: usize) -> String {
    (0..15)
        .map(|i| format!("topic{n}w{i}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Five years of the Librarian's rows before TODAY: a day row for every day (six lines of 15 words, each citing one
/// chat entry), a month row for every closed month, a year row for every closed year; 3,000 memory items.
fn five_years(db: &Db) {
    let mut day = library::shift_day(TODAY, -5 * 365);
    let mut n = 0;
    db.conn().execute_batch("BEGIN").unwrap();
    while day.as_str() < TODAY {
        let lines: Vec<Value> = (0..DAY_LINES)
            .map(|_| {
                n += 1;
                json!({"text": words(n), "refs": [format!("journal:chat{n}")]})
            })
            .collect();
        for l in &lines {
            let id = l["refs"][0]
                .as_str()
                .unwrap()
                .trim_start_matches("journal:")
                .to_owned();
            journal(
                db,
                &id,
                &format!("{day}T10:00:00+00:00"),
                "chat.message",
                json!({"text": "invented"}),
                "owner",
            );
        }
        journal(
            db,
            &format!("day-{day}"),
            &format!("{}T01:00:00+00:00", library::shift_day(&day, 1)),
            DAY_KIND,
            json!({"day": day, "lines": lines, "made_by": "test"}),
            "inferred,owner",
        );
        day = library::shift_day(&day, 1);
    }
    for y in 2021..=2026 {
        for m in 1..=12 {
            let month = format!("{y}-{m:02}");
            if month.as_str() >= &TODAY[..7] || month.as_str() < "2021-11" {
                continue;
            }
            journal(
                db,
                &format!("month-{month}"),
                &format!("{month}-28T01:00:00+00:00"),
                MONTH_KIND,
                json!({"month": month, "text": (0..40).map(|i| format!("m{i}")).collect::<Vec<_>>().join(" "),
                       "refs": [], "made_by": "test"}),
                "inferred",
            );
        }
        if y < 2026 {
            journal(
                db,
                &format!("year-{y}"),
                &format!("{y}-12-31T23:00:00+00:00"),
                YEAR_KIND,
                json!({"year": y.to_string(), "text": (0..40).map(|i| format!("y{i}")).collect::<Vec<_>>().join(" "),
                       "refs": [], "made_by": "test"}),
                "inferred",
            );
        }
    }
    for i in 0..3000 {
        let kind = ["fact", "preference", "person", "project", "env"][i % 5];
        let subject = match kind {
            "person" => format!("person:someone-{i}"),
            "project" => format!("project:thing-{i}"),
            _ => "owner".to_owned(),
        };
        let at = format!(
            "{}T08:00:00+00:00",
            library::shift_day(TODAY, -((i as i64) % 1800) - 1)
        );
        item(
            db,
            &format!("item{i:05}"),
            kind,
            &subject,
            &format!("key_{i}"),
            "owner_stated",
            &at,
        );
    }
    db.conn().execute_batch("COMMIT").unwrap();
}

// decision: ORCH-719, ORCH-746
// decision: ORCH-727
#[test]
fn the_index_stays_bounded_on_a_five_year_store() {
    let (_tmp, db) = store();
    five_years(&db);
    let (text, origins) = library::index_text(db.conn(), TODAY, &day_of).unwrap();
    let chars = text.chars().count();
    let tokens = chars.div_ceil(4);
    eprintln!("the library block on a five-year store: {chars} characters, about {tokens} tokens");
    // 14 days of 6 lines, 12 closed months, 5 closed years, five kinds of names
    let day_heads = text.lines().filter(|l| l.ends_with(" 2026:")).count();
    assert_eq!(day_heads, 14, "{text}");
    assert_eq!(
        text.lines().filter(|l| l.starts_with("- topic")).count(),
        14 * DAY_LINES
    );
    let months = text
        .lines()
        .filter(|l| l.starts_with("- ") && l.contains(": m0"))
        .count();
    // ORCH-759: the twelve closed months before this one
    assert_eq!(months, 12, "November 2025 to October 2026");
    assert!(
        text.contains("- Nov 2025: m0") && !text.contains("- Oct 2025: m0"),
        "{text}"
    );
    let years = text.lines().filter(|l| l.contains(": y0")).count();
    assert_eq!(years, 5, "2021 to 2025");
    for kind in [
        "people",
        "projects",
        "preferences",
        "facts",
        "setup and accounts",
    ] {
        let line = text
            .lines()
            .find(|l| l.starts_with(&format!("- {kind} (600)")))
            .unwrap_or_else(|| panic!("{kind}: {text}"));
        assert_eq!(line.matches(", ").count() + 1, CONTENTS_NAMES, "{line}");
    }
    // the same at any age: these invented words are long (about 180 characters a line); on everyday words the block
    // is about 2.5k tokens (ferb-supervisor tests/front_door_input.rs measures it with FERB's o200k estimate)
    assert!(chars < 24_000, "{chars} characters, {tokens} tokens");
    assert!(
        origins.contains("ferb") && origins.contains("owner"),
        "{origins:?}"
    );
}

// decision: ORCH-719, ORCH-746
#[test]
fn the_index_is_the_same_bytes_all_day() {
    let (_tmp, db) = store();
    five_years(&db);
    let (morning, _) = library::index_text(db.conn(), TODAY, &day_of).unwrap();
    // during the day: a message, a remembered item, a forgotten one's journal row; none of it joins until tomorrow
    journal(
        &db,
        "today-msg",
        &format!("{TODAY}T09:30:00+00:00"),
        "chat.message",
        json!({"text": "hi"}),
        "owner",
    );
    item(
        &db,
        "item-today",
        "person",
        "person:new-friend",
        "who",
        "owner_stated",
        &format!("{TODAY}T10:00:00+00:00"),
    );
    let (evening, _) = library::index_text(db.conn(), TODAY, &day_of).unwrap();
    assert_eq!(morning, evening);
    // tomorrow the window moves and today's item is named
    let tomorrow = library::shift_day(TODAY, 1);
    let (next, _) = library::index_text(db.conn(), &tomorrow, &day_of).unwrap();
    assert_ne!(morning, next);
    assert!(next.contains("new friend"), "{next}");
}

// decision: ORCH-719, ORCH-746
#[test]
fn a_forget_takes_out_the_lines_written_from_it() {
    let (_tmp, db) = store();
    let yesterday = library::shift_day(TODAY, -1);
    item(
        &db,
        "robin-birthday",
        "fact",
        "person:robin",
        "birthday",
        "owner_stated",
        "2026-11-01T08:00:00+00:00",
    );
    journal(
        &db,
        "e1",
        &format!("{yesterday}T10:00:00+00:00"),
        "chat.message",
        json!({"text": "x"}),
        "owner",
    );
    journal(
        &db,
        "day-y",
        &format!("{TODAY}T01:00:00+00:00"),
        DAY_KIND,
        json!({"day": yesterday, "lines": [
            {"text": "INVENTED-LINE-ROBIN talked about Robin's birthday", "refs": ["journal:e1", "memory:robin-birthday"]},
            {"text": "INVENTED-LINE-OTHER planned the week", "refs": ["journal:e1"]}]}),
        "owner",
    );
    journal(
        &db,
        "month-o",
        "2026-11-01T01:00:00+00:00",
        MONTH_KIND,
        json!({"month": "2026-10", "text": "INVENTED-MONTH-ROBIN", "refs": ["memory:robin-birthday"]}),
        "inferred",
    );
    let (before, _) = library::index_text(db.conn(), TODAY, &day_of).unwrap();
    assert!(
        before.contains("INVENTED-LINE-ROBIN") && before.contains("INVENTED-MONTH-ROBIN"),
        "{before}"
    );
    // the forget deletes the item (items.rs `forget`)
    db.conn()
        .execute("DELETE FROM memory_items WHERE id = 'robin-birthday'", [])
        .unwrap();
    let (after, _) = library::index_text(db.conn(), TODAY, &day_of).unwrap();
    assert!(!after.contains("INVENTED-LINE-ROBIN"), "{after}");
    assert!(!after.contains("INVENTED-MONTH-ROBIN"), "{after}");
    assert!(after.contains("INVENTED-LINE-OTHER"), "{after}");
    // a deleted turn's entry takes its lines too
    db.conn()
        .execute("DELETE FROM journal WHERE id = 'e1'", [])
        .unwrap();
    let (gone, _) = library::index_text(db.conn(), TODAY, &day_of).unwrap();
    assert!(!gone.contains("INVENTED-LINE-OTHER"), "{gone}");
}

// decision: MEM-489
// decision: MEM-466
#[test]
fn the_librarian_reads_no_outside_content_and_names_no_imported_line() {
    let (_tmp, db) = store();
    let day = library::shift_day(TODAY, -1);
    let at = |h: u32| format!("{day}T{h:02}:00:00+00:00");
    journal(
        &db,
        "m1",
        &at(9),
        "chat.message",
        json!({"session_id": "s1", "text": "INVENTED-OWNER-WORDS"}),
        "owner",
    );
    journal(
        &db,
        "a1",
        &at(10),
        "task.answer",
        json!({"task_id": "t1", "text": "INVENTED-WEB-RESULT"}),
        "external:web:example.com,ferb",
    );
    journal(
        &db,
        "m2",
        &at(11),
        "chat.message",
        json!({"session_id": "temporary:abc", "text": "INVENTED-TEMPORARY"}),
        "owner",
    );
    // an import's line and the owner's own words
    journal(&db, "imp", &at(8), "memory.import", json!({}), "ferb");
    item(
        &db,
        "imported1",
        "preference",
        "owner",
        "INVENTED_IMPORTED_KEY",
        "inferred",
        &at(8),
    );
    db.conn()
        .execute(
            "INSERT INTO memory_sources (item_id, kind, source_id) VALUES ('imported1', 'journal', 'imp')",
            [],
        )
        .unwrap();
    item(
        &db,
        "own1",
        "preference",
        "owner",
        "INVENTED_OWN_KEY",
        "owner_stated",
        &at(9),
    );
    let input = library::day_input(db.conn(), &day, &day_of, &clock_of)
        .unwrap()
        .expect("a day with a message");
    assert!(
        input.text.contains("INVENTED-OWNER-WORDS"),
        "{}",
        input.text
    );
    assert!(
        !input.text.contains("INVENTED-WEB-RESULT"),
        "{}",
        input.text
    );
    assert!(
        input.text.contains("holds outside content: not shown"),
        "{}",
        input.text
    );
    assert!(!input.text.contains("INVENTED-TEMPORARY"), "{}", input.text);
    assert!(
        !input.text.contains("INVENTED IMPORTED KEY"),
        "{}",
        input.text
    );
    assert!(input.text.contains("INVENTED OWN KEY"), "{}", input.text);
    assert!(
        !input.origins.iter().any(|o| o.starts_with("external")),
        "{:?}",
        input.origins
    );
    // the contents count the imported line and never name it
    let (contents, _) = library::contents(db.conn(), TODAY, &day_of).unwrap();
    assert!(
        contents.contains("preferences (2): INVENTED OWN KEY"),
        "{contents}"
    );
    assert!(!contents.contains("IMPORTED"), "{contents}");
    // a day with no message of his in his chats gets no input (no call)
    let silent = library::shift_day(TODAY, -2);
    assert!(library::day_input(db.conn(), &silent, &day_of, &clock_of)
        .unwrap()
        .is_none());
    // the answer's lines: at most six, cut, their tags turned into the records
    let answer = json!({"lines": (0..9).map(|i| json!({"text": format!("line {i}"), "tags": ["e1", "[m1]", "zz"]}))
                                       .collect::<Vec<_>>()});
    let lines = library::day_lines(&answer, &input);
    assert_eq!(lines.len(), DAY_LINES);
    assert_eq!(lines[0]["refs"], json!(["journal:m1", input.tags["m1"]]));
}

// decision: ORCH-759
#[test]
fn a_line_citing_a_message_goes_when_what_was_saved_from_it_is_forgotten() {
    let (_tmp, db) = store();
    let day = library::shift_day(TODAY, -1);
    journal(
        &db,
        "said",
        &format!("{day}T09:00:00+00:00"),
        "chat.message",
        json!({"session_id": "s1", "text": "INVENTED my office moved"}),
        "owner",
    );
    item(
        &db,
        "office",
        "fact",
        "owner",
        "office_place",
        "owner_stated",
        "2026-10-01T08:00:00+00:00",
    );
    db.conn()
        .execute(
            "INSERT INTO memory_sources (item_id, kind, source_id) VALUES ('office', 'journal', 'said')",
            [],
        )
        .unwrap();
    let input = library::day_input(db.conn(), &day, &day_of, &clock_of)
        .unwrap()
        .expect("a day with a message");
    let tag = input
        .tags
        .iter()
        .find(|(_, r)| r.as_str() == "journal:said")
        .map(|(t, _)| t.clone())
        .unwrap();
    // the Librarian cites only the message; the line keeps what was saved from it too
    let lines = library::day_lines(
        &json!({"lines": [{"text": "INVENTED-LINE talked about the office", "tags": [tag]}]}),
        &input,
    );
    assert_eq!(lines[0]["refs"], json!(["journal:said", "memory:office"]));
    journal(
        &db,
        "day-row",
        &format!("{TODAY}T01:00:00+00:00"),
        DAY_KIND,
        json!({"day": day, "lines": lines}),
        "owner",
    );
    let (before, _) = library::index_text(db.conn(), TODAY, &day_of).unwrap();
    assert!(before.contains("INVENTED-LINE"), "{before}");
    db.conn()
        .execute("DELETE FROM memory_items WHERE id = 'office'", [])
        .unwrap();
    let (after, _) = library::index_text(db.conn(), TODAY, &day_of).unwrap();
    assert!(!after.contains("INVENTED-LINE"), "{after}");
}

// decision: ORCH-759
#[test]
fn in_january_last_years_months_stay_beside_its_year_line() {
    let (_tmp, db) = store();
    for (m, text) in [
        ("2026-12", "INVENTED-DEC"),
        ("2026-01", "INVENTED-JAN"),
        ("2025-12", "INVENTED-OLD"),
    ] {
        journal(
            &db,
            &format!("month-{m}"),
            "2027-01-02T01:00:00+00:00",
            MONTH_KIND,
            json!({"month": m, "text": text, "refs": []}),
            "inferred",
        );
    }
    journal(
        &db,
        "year-2026",
        "2027-01-03T01:00:00+00:00",
        YEAR_KIND,
        json!({"year": "2026", "text": "INVENTED-YEAR", "refs": []}),
        "inferred",
    );
    let (text, _) = library::index_text(db.conn(), "2027-01-10", &day_of).unwrap();
    assert!(text.contains("- Dec 2026: INVENTED-DEC"), "{text}");
    assert!(text.contains("- Jan 2026: INVENTED-JAN"), "{text}");
    assert!(
        !text.contains("INVENTED-OLD"),
        "thirteen months back: {text}"
    );
    assert!(text.contains("- 2026: INVENTED-YEAR"), "{text}");
}
