//! FERB's words and answers as FERB.app draws them (ferb-mobile `rich`, `shown`; kb PHN-720..): FERB.app's
//! `MarkdownDocument` blocks and inline looks, the answer's layout (the sites row, the cards by kind, the lines cards
//! stand for, the note and code's lines), the boxes a later item draws, the work row's line, and bounds on any input.

mod common;

use std::time::{Duration, Instant};

use common::scenarios;

use ferb_mobile::fold::{self, model::ResultCard, AnswerView, Item};
use ferb_mobile::rich::{self, Block, Span};
use ferb_mobile::shown;
use serde_json::{json, Value};

fn span(text: &str) -> Span {
    Span {
        text: text.into(),
        ..Span::default()
    }
}

fn words(b: &Block) -> String {
    match b {
        Block::Paragraph { spans }
        | Block::Heading { spans, .. }
        | Block::Quote { spans }
        | Block::ListItem { spans, .. } => spans.iter().map(|s| s.text.as_str()).collect(),
        Block::Code { text, .. } => text.clone(),
        _ => String::new(),
    }
}

// ---- blocks: MarkdownDocument line for line ---------------------------------------------------------------------

// decision: PHN-721
#[test]
fn blocks_are_the_macs() {
    let b = rich::document(
        "# Title\n\nFirst line\nsecond line\n\n- one\n  - two\n3. three\n> quoted\n---\n```swift\nlet x = 1\n```\n| a | b |\n|:---|---:|\n| 1 | 2 |",
    );
    assert!(matches!(&b[0], Block::Heading { level: 1, .. }));
    assert_eq!(words(&b[1]), "First line\nsecond line");
    assert!(matches!(&b[2], Block::ListItem { marker, level: 0, .. } if marker == "•"));
    assert!(matches!(&b[3], Block::ListItem { marker, level: 1, .. } if marker == "•"));
    assert!(matches!(&b[4], Block::ListItem { marker, level: 0, .. } if marker == "3."));
    assert!(matches!(&b[5], Block::Quote { .. }));
    assert_eq!(words(&b[5]), "quoted");
    assert!(matches!(&b[6], Block::Divider));
    assert!(
        matches!(&b[7], Block::Code { text, language } if text == "let x = 1" && language == "swift")
    );
    match &b[8] {
        Block::Table {
            header,
            rows,
            align,
        } => {
            assert_eq!(header.len(), 2);
            assert_eq!(rows.len(), 1);
            assert_eq!(align, &vec!["leading".to_string(), "trailing".to_string()]);
        }
        other => panic!("not a table: {other:?}"),
    }
    assert_eq!(b.len(), 9);
}

#[test]
fn a_hash_without_a_space_and_a_star_line_are_text() {
    let b = rich::document("#tag\n***\n*not a list");
    assert!(matches!(&b[0], Block::Paragraph { .. }));
    assert!(matches!(&b[1], Block::Divider));
}

// ---- inline: bold, italics, code, links, the plain addresses -------------------------------------------------------

// decision: PHN-721
#[test]
fn the_research_answer_reads_as_on_the_mac() {
    let s = rich::inline("**Yes, a website can** sometimes *infer* it. See [LinkedIn User Agreement](https://www.linkedin.com/legal/user-agreement) and `code`.");
    let bold: String = s
        .iter()
        .filter(|x| x.bold)
        .map(|x| x.text.as_str())
        .collect();
    assert_eq!(bold, "Yes, a website can");
    assert!(s.iter().any(|x| x.italic && x.text == "infer"));
    let link = s.iter().find(|x| x.link.is_some()).unwrap();
    assert_eq!(link.text, "LinkedIn User Agreement");
    assert_eq!(
        link.link.as_deref(),
        Some("https://www.linkedin.com/legal/user-agreement")
    );
    assert!(s.iter().any(|x| x.code && x.text == "code"));
    let all: String = s.iter().map(|x| x.text.as_str()).collect();
    assert!(
        !all.contains("https://"),
        "the address is never written out: {all}"
    );
    assert!(!all.contains("**") && !all.contains('`'));
}

// decision: APP-589
#[test]
fn only_web_and_mail_links_are_made() {
    for (md, link) in [
        ("[a](javascript:alert(1))", None),
        ("[a](file:///etc/passwd)", None),
        ("[a](/relative)", None),
        ("[a](mailto:me@example.com)", Some("mailto:me@example.com")),
        ("[a](HTTPS://example.com)", Some("HTTPS://example.com")),
    ] {
        let s = rich::inline(md);
        assert_eq!(
            s.iter().find_map(|x| x.link.clone()).as_deref(),
            link,
            "{md}"
        );
        assert_eq!(
            s.iter().map(|x| x.text.as_str()).collect::<String>(),
            "a",
            "{md}"
        );
    }
}

#[test]
fn plain_addresses_become_links_without_their_punctuation() {
    let s =
        rich::inline("Read https://example.com/a_(b). Or www.example.org, or mail me@example.com.");
    let links: Vec<(String, String)> = s
        .iter()
        .filter_map(|x| x.link.clone().map(|l| (x.text.clone(), l)))
        .collect();
    assert_eq!(
        links,
        vec![
            (
                "https://example.com/a_(b)".into(),
                "https://example.com/a_(b)".into()
            ),
            ("www.example.org".into(), "https://www.example.org".into()),
            ("me@example.com".into(), "mailto:me@example.com".into()),
        ]
    );
    let all: String = s.iter().map(|x| x.text.as_str()).collect();
    assert_eq!(
        all,
        "Read https://example.com/a_(b). Or www.example.org, or mail me@example.com."
    );
}

#[test]
fn an_image_is_its_words_and_nothing_is_fetched() {
    let s = rich::inline("![a chart](https://example.com/x.png) after");
    assert_eq!(s, vec![span("a chart after")]);
}

#[test]
fn nested_and_unbalanced_marks() {
    let s = rich::inline("***both*** and **bold *italic* bold** and *open");
    assert!(s.iter().any(|x| x.bold && x.italic && x.text == "both"));
    assert!(s.iter().any(|x| x.bold && x.italic && x.text == "italic"));
    assert!(s.iter().any(|x| !x.bold && x.text.contains("*open")));
    assert_eq!(
        rich::inline("snake_case_name"),
        vec![span("snake_case_name")]
    );
    assert!(rich::inline("~~gone~~")[0].strike);
    assert_eq!(rich::inline("\\*not\\*"), vec![span("*not*")]);
}

// decision: PHN-722
#[test]
fn any_input_is_bounded_in_time_and_size() {
    let cases: Vec<String> = vec![
        "*".repeat(200_000),
        "[".repeat(100_000) + &"]".repeat(100_000),
        "`".repeat(50_000) + " x",
        "**a ".repeat(40_000),
        "- item\n".repeat(20_000),
        "  ".repeat(5_000) + "- deep",
        "x".repeat(1_000_000),
        "[a](".to_string() + &"(".repeat(100_000),
        "<".repeat(100_000),
        "|a|b|\n|---|---|\n".to_string() + &"|1|2|\n".repeat(10_000),
        "http://".repeat(50_000),
        "a@".repeat(100_000),
    ];
    for c in cases {
        let t = Instant::now();
        let b = rich::document(&c);
        let took = t.elapsed();
        assert!(
            took < Duration::from_secs(2),
            "{} chars took {took:?}",
            c.len()
        );
        assert!(b.len() <= rich::MAX_BLOCKS + 1, "{} blocks", b.len());
        let chars: usize = b.iter().map(|x| words(x).chars().count()).sum();
        assert!(chars <= rich::MAX_CHARS + 2, "{chars} characters out");
        for x in &b {
            if let Block::ListItem { level, .. } = x {
                assert!(*level <= rich::MAX_LEVEL);
            }
            if let Block::Table { header, rows, .. } = x {
                assert!(header.len() <= rich::MAX_COLUMNS && rows.len() <= rich::MAX_ROWS);
            }
        }
    }
}

// ---- the answer: FERB.app's AnswerView ---------------------------------------------------------------------------

fn card(v: Value) -> ResultCard {
    ResultCard::read(&v).expect("a card")
}

fn answer(text: &str, lines: &[&str], cards: Vec<ResultCard>) -> AnswerView {
    AnswerView {
        text: text.into(),
        display_text: None,
        status: None,
        verified: true,
        artifacts: vec![],
        note: None,
        lines: Some(lines.iter().map(|l| l.to_string()).collect()),
        cards: Some(cards),
        runtime_switch: None,
    }
}

// decision: PHN-723
#[test]
fn the_sites_are_one_row_and_their_line_is_said_nowhere() {
    // the owner's research answer of 2026-11-18 02:18 (chat entry 95f9ee54), its words shortened
    let line = "Sites contacted: 6 sites";
    let hosts = [
        "developer.chrome.com",
        "developer.mozilla.org",
        "developers.cloudflare.com",
        "w3c.github.io",
        "www.google.com",
        "www.linkedin.com",
    ];
    let cards = hosts
        .iter()
        .map(|h| card(json!({"type": "link", "host": h, "line": line})))
        .collect();
    let a = answer(
        &format!("**Yes.** It can.\n\nBottom line: maybe.\n\n{line}"),
        &[line],
        cards,
    );
    let s = shown::answer(&a, false, &[]);
    assert_eq!(s.sites, hosts.to_vec());
    assert!(s.bubble);
    assert!(s.lines.is_empty(), "{:?}", s.lines);
    assert!(s.below.is_empty() && s.above.is_empty() && s.files.is_empty());
    let text = rich::plain_text(&s.blocks);
    assert!(!text.contains("Sites contacted"), "{text}");
    assert!(text.ends_with("Bottom line: maybe."));
}

#[test]
fn an_answer_from_before_link_cards_takes_its_sites_from_the_line() {
    let a = answer("Done.\n\nSites contacted: a.com, b.org, a.com", &[], vec![]);
    let s = shown::answer(&a, false, &[]);
    assert_eq!(s.sites, vec!["a.com", "b.org"]);
    assert_eq!(rich::plain_text(&s.blocks), "Done.");
}

// decision: PHN-724
#[test]
fn every_card_goes_where_the_mac_draws_it_and_an_unknown_one_keeps_its_line() {
    let cards = vec![
        card(
            json!({"type": "reminder", "title": "Stretch", "timer_id": "t1", "line": "Reminder: Stretch"}),
        ),
        card(
            json!({"type": "event", "kind": "reminder", "what": "Call mum", "id": "r1", "line": "Reminder set"}),
        ),
        card(json!({"type": "note", "item_id": "m1", "text": "Likes tea", "line": "Remembered"})),
        card(json!({"type": "payment", "amount": "$5", "merchant": "Cafe"})),
        card(json!({"type": "send", "recipient": "Ana", "kind": "mailbox_send"})),
        card(
            json!({"type": "schedules", "schedules": [{"id": "s", "name": "News", "repeat": "every day"}]}),
        ),
        card(json!({"type": "file", "path": "/Users/x/Desktop/a.pdf"})),
        card(json!({"type": "page", "page": "memory"})),
    ];
    let mut a = answer(
        "Reminder: Stretch\nReminder set\nRemembered\nA new kind's line\nAll set.",
        &[],
        cards,
    );
    a.artifacts = vec![
        "/Users/x/Desktop/a.pdf".into(),
        "/Users/x/Desktop/b.txt".into(),
    ];
    let s = shown::answer(&a, false, &[]);
    assert_eq!(
        s.above
            .iter()
            .map(|c| c.kind_of.as_str())
            .collect::<Vec<_>>(),
        ["reminder"]
    );
    assert_eq!(
        s.below
            .iter()
            .map(|c| c.kind_of.as_str())
            .collect::<Vec<_>>(),
        ["event", "note", "payment", "send", "schedules"]
    );
    assert_eq!(s.pages.len(), 1);
    // the file card and the task's files, each once: the card's file carries its place among the files
    assert_eq!(
        s.files
            .iter()
            .map(|f| (f.path.as_str(), f.index))
            .collect::<Vec<_>>(),
        [
            ("/Users/x/Desktop/a.pdf", Some(0)),
            ("/Users/x/Desktop/b.txt", Some(1))
        ]
    );
    // the lines the drawn cards stand for leave the words; a line no card draws stays (ResultCard.swift:237-242)
    assert_eq!(rich::plain_text(&s.blocks), "A new kind's line\nAll set.");
}

#[test]
fn a_card_turn_draws_only_its_cards_and_files() {
    let a = answer(
        "1 of 2 done\nSites contacted: x.com",
        &["Sites contacted: x.com"],
        vec![card(
            json!({"type": "link", "host": "x.com", "line": "Sites contacted: x.com"}),
        )],
    );
    let s = shown::answer(&a, true, &[]);
    assert!(!s.bubble && s.blocks.is_empty());
    assert_eq!(s.sites, vec!["x.com"]);
}

#[test]
fn the_note_and_codes_lines_go_under_the_words() {
    let mut a = answer(
        "It's on the Desktop.\nCouldn't confirm: the file opens.\nRan 2 steps\n(ran late)",
        &["Ran 2 steps"],
        vec![],
    );
    a.note = Some("the file opens".into());
    a.text =
        "It's on the Desktop.\n\nCouldn't confirm: the file opens.\nRan 2 steps\n(ran late)".into();
    let s = shown::answer(&a, false, &[]);
    assert_eq!(s.lines, vec!["Ran 2 steps", "(ran late)"]);
    assert_eq!(s.note.as_deref(), Some("the file opens"));
    assert_eq!(rich::plain_text(&s.blocks), "It's on the Desktop.");
}

#[test]
fn a_shortened_answer_keeps_the_whole_under_details() {
    let mut a = answer("The whole long answer.", &[], vec![]);
    a.display_text = Some("Short.".into());
    let s = shown::answer(&a, false, &[]);
    assert_eq!(rich::plain_text(&s.blocks), "Short.");
    assert_eq!(rich::plain_text(&s.details), "The whole long answer.");
}

#[test]
fn a_timers_answer_that_its_card_says_draws_no_empty_bubble() {
    let a = answer(
        "Timer set",
        &[],
        vec![card(
            json!({"type": "event", "kind": "timer", "what": "Tea", "id": "t", "line": "Timer set"}),
        )],
    );
    let s = shown::answer(&a, false, &[]);
    assert!(!s.bubble);
    assert_eq!(s.below.len(), 1);
    // its box drawn further down: nothing here
    let s = shown::answer(&a, false, &["t".into()]);
    assert!(s.below.is_empty());
}

#[test]
fn status_words_are_the_macs() {
    assert_eq!(shown::status_words("done"), None);
    assert_eq!(
        shown::status_words("done_with_gaps").as_deref(),
        Some("Delivered, with parts not done")
    );
    assert_eq!(shown::status_words("failed").as_deref(), Some("Failed"));
    assert_eq!(
        shown::status_words("done_unverified").as_deref(),
        Some("Done Unverified")
    );
}

// ---- over the recorded conversations -----------------------------------------------------------------------------

// decision: PHN-724
#[test]
fn every_recorded_answer_lays_out_without_repeating_a_card_or_a_line() {
    let mut answers = 0;
    for (_, _, entries) in scenarios() {
        let t = fold::transcript(&entries, 0);
        for item in &t.items {
            let (a, card, hidden) = match item {
                Item::Turn(x) => (x.answer.clone(), x.card.is_some(), x.hidden.clone()),
                Item::Reply(x) => (x.said.clone(), false, x.hidden.clone()),
                _ => continue,
            };
            let Some(a) = a else { continue };
            answers += 1;
            let s = shown::answer(&a, card, &hidden);
            let text = rich::plain_text(&s.blocks);
            for c in a.cards.iter().flatten() {
                if let Some(line) = &c.line {
                    assert!(
                        !text.split('\n').any(|l| l == line),
                        "{line} said again in {text}"
                    );
                    assert!(!s.lines.contains(line), "{line} drawn twice");
                }
            }
            let mut paths: Vec<&str> = s.files.iter().map(|f| f.path.as_str()).collect();
            paths.sort();
            let n = paths.len();
            paths.dedup();
            assert_eq!(n, paths.len(), "a file twice");
        }
    }
    assert!(answers > 50, "{answers} answers");
}

/// APP-769 per shown day: a timer set yesterday and delivered today draws on both days, as FERB.app's day view does.
// decision: PHN-736
#[test]
fn one_box_per_timer_is_counted_within_each_day() {
    let at = |id: &str, pos: i64, kind: &str, body: Value, day: &str| {
        ChatEntryJson::make(id, pos, kind, body, day)
    };
    let set = vec![
        at(
            "m1",
            10,
            "message",
            json!({"text": "timer 10 s", "attachments": [], "hidden": []}),
            "2026-01-01",
        ),
        at(
            "a1",
            11,
            "answer",
            json!({"text": "Timer set", "status": "done", "cards": [{"type": "event", "kind": "timer", "what": "Tea", "id": "t9", "line": "Timer set"}]}),
            "2026-01-01",
        ),
        at(
            "n1",
            12,
            "post",
            json!({"text": "Your timer is up.", "timer_id": "t9", "card": {"type": "reminder", "title": "Tea", "kind": "timer", "timer_id": "t9"}}),
            "2026-01-02",
        ),
        at(
            "n2",
            13,
            "post",
            json!({"text": "Your timer is up.", "timer_id": "t9", "card": {"type": "reminder", "title": "Tea", "kind": "timer", "timer_id": "t9"}}),
            "2026-01-02",
        ),
    ];
    let t = fold::transcript(&set, 0);
    let hidden = |e: &str| -> Vec<String> {
        t.items
            .iter()
            .find(|i| i.entry() == e)
            .map(|i| match i {
                Item::Turn(x) => x.hidden.clone(),
                Item::Reply(x) => x.hidden.clone(),
                Item::Notice(x) | Item::Reminder(x) => x.hidden.clone(),
            })
            .unwrap_or_default()
    };
    assert!(hidden("m1").is_empty(), "the day it was set shows its box");
    assert_eq!(
        hidden("n1"),
        vec!["t9".to_string()],
        "on the next day only its last arrival shows it"
    );
    assert!(hidden("n2").is_empty());
}

struct ChatEntryJson;
impl ChatEntryJson {
    fn make(
        id: &str,
        pos: i64,
        kind: &str,
        body: Value,
        day: &str,
    ) -> ferb_mobile::entry::ChatEntry {
        let at = format!("{day}T09:00:{:02}+00:00", pos % 60);
        ferb_mobile::entry::ChatEntry::read(&json!({
            "id": id, "conversation_id": "C1", "pos": pos, "seq": pos, "rev": 1, "kind": kind, "state": "done",
            "open": false, "request_id": "r1", "task_id": "t1", "client_id": "phone-1",
            "answers": if kind == "answer" { json!("m1") } else { Value::Null },
            "placement": if kind == "answer" { json!("inline") } else { Value::Null },
            "created_at": at, "updated_at": at, "day": day, "body": body, "deleted": false,
        }))
        .unwrap()
    }
}
