//! ferb/orchestrator/leftovers.py: the owner's words for what a task's cleanup couldn't release (HAND-419), as one
//! message (HAND-521). Raw errors and tab ids stay in the task.cleanup span.
//!
//! Fixed against Python (HAND-521): Python wrote one line per tab ("I couldn't close one tab I opened: … Close it if
//! it's still open." seven times for seven tabs). Here the tabs are grouped by browser profile (one short sentence per
//! profile when there are several), at most three titles are named (two per profile when there are several) and the
//! rest counted, and the whole cleanup is one message with one "close them if they're still open". A single tab keeps Python's sentence; every other kind keeps its sentence,
//! once, in the same message.

use serde_json::{Map, Value};

use ferb_py::value::py_str;

pub const UNSAID: &str =
    "I couldn't finish tidying up after this task. The details are in the task's record.";
pub const OPENED_TAB: &str =
    "I couldn't close one tab I opened: {where}. Close it if it's still open.";
pub const OWNERS_TAB: &str = "I couldn't fully let go of one of your tabs: {where}. It's still open as you left it; if {browser} says FERB started debugging it, press Cancel there.";
/// HAND-521: several tabs FERB opened, in one profile.
pub const OPENED_TABS: &str =
    "I couldn't close {count} tabs I opened in {where}. Close them if they're still open.";
/// HAND-521: several tabs FERB opened, across profiles (`{where}`: one short sentence per profile, its count and
/// titles).
pub const OPENED_TABS_SPREAD: &str =
    "I couldn't close {count} tabs I opened. {where} Close them if they're still open.";
/// HAND-521: several of the owner's borrowed tabs, in one profile.
pub const OWNERS_TABS: &str = "I couldn't fully let go of {count} of your tabs in {where}. They're still open as you left them; if {browser} says FERB started debugging them, press Cancel there.";
/// HAND-521: several of the owner's borrowed tabs, across profiles.
pub const OWNERS_TABS_SPREAD: &str = "I couldn't fully let go of {count} of your tabs. {where} They're still open as you left them; if {browser} says FERB started debugging them, press Cancel there.";

/// HAND-521: the titles named for one profile when it is the only one, and for each when there are several.
pub const TITLES_ALONE: usize = 3;
pub const TITLES_SPREAD: usize = 2;
/// A title longer than this many characters is cut at a word and ends in "…".
pub const TITLE_CHARS: usize = 48;

/// leftovers.py:20 `BY_RESOURCE`.
pub fn by_resource(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "browser" | "ferb_browser" => "I couldn't confirm that the browser tabs this task used were closed or let go of. Close any FERB tab group that's still open.",
        "app" => "I couldn't confirm that the apps this task opened were closed. Quit any you don't need.",
        "shell" => "I couldn't confirm that this task's terminal commands all stopped. Activity Monitor shows anything still running.",
        "listen" => "I couldn't confirm that listening stopped when this task ended.",
        "recording" => "A screen recording this task made didn't end cleanly. The details are in the task's record.",
        _ => return None,
    })
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(ferb_py::value::truthy)
}

/// `str(x or "")`.
fn text_or_empty(v: Option<&Value>) -> String {
    match v {
        Some(x) if ferb_py::value::truthy(x) => py_str(x),
        _ => String::new(),
    }
}

/// One sentence of the message, or where the tabs' sentences go (at the first record that named tabs).
enum Piece {
    Said(String),
    Tabs,
}

/// leftovers.py:31 `leftover_lines`: the owner's words for the cleanup's leftovers, as one message (HAND-521): empty
/// when nothing is left, else one line. `resources` are the release records the daemon returned (as the task.cleanup
/// span holds them); `errors` the cleanup's own errors (a record's error, or a cleanup that stopped before any record
/// came back). An app FERB left running has its own words (launched.left_running) and is not said again here.
pub fn leftover_lines(resources: &[Value], errors: &[String]) -> Vec<String> {
    let mut pieces: Vec<Piece> = Vec::new();
    let mut tabs: Vec<&Map<String, Value>> = Vec::new();
    let mut said: Vec<String> = Vec::new();
    for r in resources {
        let Some(r) = r.as_object() else { continue };
        if !truthy(r.get("error")) {
            continue;
        }
        said.push(py_str(&r["error"]));
        let kind = text_or_empty(r.get("resource"));
        if kind == "app" && truthy(r.get("left_running")) {
            continue;
        }
        let named: Vec<&Map<String, Value>> = match r.get("tabs") {
            Some(Value::Array(items)) => items.iter().filter_map(|t| t.as_object()).collect(),
            _ => Vec::new(),
        };
        if named.is_empty() {
            pieces.push(Piece::Said(
                by_resource(&kind).unwrap_or(UNSAID).to_string(),
            ));
        } else {
            if tabs.is_empty() {
                pieces.push(Piece::Tabs);
            }
            tabs.extend(named);
        }
    }
    if errors.iter().any(|e| !said.contains(e)) {
        pieces.push(Piece::Said(UNSAID.to_string()));
    }
    let mut sentences: Vec<String> = Vec::new();
    for piece in pieces {
        let new = match piece {
            Piece::Said(s) => vec![s],
            Piece::Tabs => tab_sentences(&tabs),
        };
        for s in new {
            if !sentences.contains(&s) {
                sentences.push(s);
            }
        }
    }
    if sentences.is_empty() {
        return Vec::new();
    }
    // a title cut with "…" at a sentence's end takes no full stop after it
    vec![sentences.join(" ").replace("\u{2026}. ", "\u{2026} ")]
}

/// One tab's facts as the release record names them.
struct Tab {
    browser: String,
    /// browser “profile” (account): where it is.
    place: String,
    title: String,
    borrowed: bool,
}

fn tab_of(tab: &Map<String, Value>) -> Tab {
    let browser = {
        let b = text_or_empty(tab.get("browser"));
        if b.is_empty() {
            "the browser".to_string()
        } else {
            b
        }
    };
    let profile = text_or_empty(tab.get("profile"));
    let account = text_or_empty(tab.get("account")); // the profile's signed-in account (HAND-424)
    let mut place = browser.clone();
    if !profile.is_empty() {
        place.push_str(&format!(" \u{201c}{profile}\u{201d}"));
    }
    if !profile.is_empty() && !account.is_empty() {
        place.push_str(&format!(" ({account})"));
    }
    Tab {
        browser,
        place,
        title: short_title(&text_or_empty(tab.get("title"))),
        borrowed: truthy(tab.get("borrowed")),
    }
}

/// The tabs' sentences: FERB's own tabs, then the owner's borrowed ones.
fn tab_sentences(tabs: &[&Map<String, Value>]) -> Vec<String> {
    let all: Vec<Tab> = tabs.iter().map(|t| tab_of(t)).collect();
    let mut out = Vec::new();
    for borrowed in [false, true] {
        let mine: Vec<&Tab> = all.iter().filter(|t| t.borrowed == borrowed).collect();
        if let Some(s) = sentence(&mine, borrowed) {
            out.push(s);
        }
    }
    out
}

fn sentence(tabs: &[&Tab], borrowed: bool) -> Option<String> {
    let first = tabs.first()?;
    if tabs.len() == 1 {
        let mut place = first.place.clone();
        if !first.title.is_empty() {
            place.push_str(&format!(", {}", first.title));
        }
        let template = if borrowed { OWNERS_TAB } else { OPENED_TAB };
        return Some(crate::py::format_named(
            template,
            &[("where", &place), ("browser", &first.browser)],
        ));
    }
    // by profile, in the order first seen
    let mut groups: Vec<(&str, Vec<&Tab>)> = Vec::new();
    for t in tabs {
        match groups.iter_mut().find(|(p, _)| *p == t.place) {
            Some((_, g)) => g.push(t),
            None => groups.push((&t.place, vec![t])),
        }
    }
    let mut browsers: Vec<&str> = Vec::new();
    for t in tabs {
        if !browsers.contains(&t.browser.as_str()) {
            browsers.push(&t.browser);
        }
    }
    let count = number(tabs.len());
    let browser = browsers.join(" or ");
    let (template, place) = if groups.len() == 1 {
        let (place, g) = &groups[0];
        let place = match titles(g, TITLES_ALONE) {
            Some(named) => format!("{place}: {named}"),
            None => place.to_string(),
        };
        (if borrowed { OWNERS_TABS } else { OPENED_TABS }, place)
    } else {
        let parts: Vec<String> = groups
            .iter()
            .map(|(place, g)| {
                let head = format!("{} in {place}", capitalized(&number(g.len())));
                match titles(g, TITLES_SPREAD) {
                    Some(named) => format!("{head}: {named}."),
                    None => format!("{head}."),
                }
            })
            .collect();
        (
            if borrowed {
                OWNERS_TABS_SPREAD
            } else {
                OPENED_TABS_SPREAD
            },
            parts.join(" "),
        )
    };
    Some(crate::py::format_named(
        template,
        &[("count", &count), ("where", &place), ("browser", &browser)],
    ))
}

/// Up to `limit` titles in quotes, the rest counted ("“A”, “B” and 3 more"); `None` when no tab has a title.
fn titles(tabs: &[&Tab], limit: usize) -> Option<String> {
    let mut named: Vec<String> = Vec::new();
    for t in tabs {
        if named.len() == limit {
            break;
        }
        if !t.title.is_empty() {
            named.push(format!("\u{201c}{}\u{201d}", t.title));
        }
    }
    if named.is_empty() {
        return None;
    }
    let rest = tabs.len() - named.len();
    if rest > 0 {
        named.push(format!("{} more", number(rest)));
    }
    Some(listed(&named))
}

/// "a", "a and b", "a, b and c".
fn listed(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [head @ .., last] => format!("{} and {last}", head.join(", ")),
    }
}

/// "One" for "one".
fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |c| {
        c.to_uppercase().chain(chars).collect::<String>()
    })
}

/// A count in words up to nine, else in digits.
fn number(n: usize) -> String {
    const WORDS: [&str; 10] = [
        "no", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    ];
    WORDS
        .get(n)
        .map_or_else(|| n.to_string(), |w| (*w).to_string())
}

/// A page title of at most [`TITLE_CHARS`] characters: a longer one is cut at the last word that fits and ends in "…".
pub fn short_title(title: &str) -> String {
    let title = title.trim();
    if title.chars().count() <= TITLE_CHARS {
        return title.to_string();
    }
    let head: String = title.chars().take(TITLE_CHARS - 1).collect();
    let cut = match head.rfind(' ') {
        Some(at) if head[..at].chars().count() >= TITLE_CHARS / 2 => &head[..at],
        _ => head.as_str(),
    };
    format!(
        "{}\u{2026}",
        cut.trim_end_matches(|c: char| c.is_whitespace() || ",;:-|–—·".contains(c))
    )
}
