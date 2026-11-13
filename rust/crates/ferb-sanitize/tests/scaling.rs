//! The guard of tests/test_redaction_scaling.py for the port: no input may make one call cost more than in
//! proportion to its size (the redactors run on every shell read, streamed reply and captured text).
//!
//! Measured by work done, never by a clock (TEST-508), in two ways:
//! - the instructions this process ran during a call, as the Mac's kernel counts them (`proc_pid_rusage`,
//!   `ri_instructions`): the same call runs about the same instructions whatever else the Mac is doing. Each
//!   pathological run is built at `SMALL` characters and at four times that; the larger may cost at most `GROWTH`
//!   times the smaller (in proportion: 4; a quadratic: 16). A call under the floor is never judged.
//!   Everything is measured from one test, one call at a time, so no other thread's work is counted.
//! - fancy-regex's own count of backtracks inside one pattern call. This test binary compiles every pattern with
//!   a limit of `BACKTRACKS` per call (the crate's is off, as Python has none); a call past it fails, and the test
//!   with it. A pattern that reads a run in more than one way backtracks with the square of the run or worse.
//!
//! The runs, prefixes and whole texts are the Python test's. Mac only: the count is the Mac kernel's.
#![cfg(target_os = "macos")]

use ferb_sanitize::{checked, known, parity, Redactor};

/// A whole text's run: `SMALL` characters, and four times that.
const SMALL: usize = 1024;
const FLOOR: u64 = 3_000_000;
/// One pattern's run: every pattern, three ways, on every run. An unoptimised build reads about 300 KB a second
/// through fancy-regex, so these runs are short: a pattern that costs the square of its run still costs sixteen
/// times more on the longer one (about 500 instructions a character, against the floor).
const PATTERN_SMALL: usize = 64;
const PATTERN_FLOOR: u64 = 300_000;
const GROWTH: u64 = 8;
/// Backtracks one pattern call may make. `FERB_SCALING_BACKTRACKS` overrides it, to find how many are needed.
const BACKTRACKS: usize = 64 * 4 * SMALL;

/// The instructions this process has run so far.
fn instructions() -> u64 {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
    // SAFETY: `info` is a zeroed rusage_info_v4, the buffer RUSAGE_INFO_V4 asks for; the call only writes it.
    let status = unsafe {
        libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V4,
            info.as_mut_ptr().cast(),
        )
    };
    assert_eq!(status, 0, "proc_pid_rusage failed");
    // SAFETY: the call returned 0, so the kernel filled the struct (and every bit pattern is a valid one).
    unsafe { info.assume_init() }.ri_instructions
}

/// Written in two parts: the commit gate's `secrets` check reads tracked files for the shape.
const KEY_LINE: &str = concat!("-----BEGIN RSA PRIV", "ATE KEY-----\n");

const RUNS: &[&str] = &[
    " ",
    "\t",
    "\u{a0}",
    " \t\u{a0}\u{2003}",
    "*",
    "_",
    "`",
    "'",
    "\"",
    "'\"",
    " *",
    "<b>",
    " <b>",
    "<b ",
    "</b> ",
    "1",
    "1 ",
    "1.5,",
    "4821 ",
    "4821 is ",
    "\r\n",
    " \n",
    ">\n",
    "> ",
    "#",
    ".",
    ". ",
    ":",
    "-",
    "(",
    ")",
    "[",
    "a.",
    "a-",
    "pin/",
    "PIN code 1234 ",
    "-----BEGIN ",
    KEY_LINE,
    "<a />",
    " <br />",
    "<a b/>",
    "<img src='x' />",
    "{\"token\":\"abc\",\"n\":1}",
    "{\"has_password\":true}",
    "password: 'x ",
    "api/",
    "time.",
    "one-",
    "my/",
    "eyJ-",
    "eyJabcdefgh.",
    "1/",
    "1*",
    "4111 1111**",
];
const PREFIXES: &[&str] = &[
    "",
    "482913",
    "one",
    "backup",
    "api",
    "my",
    "Your code:",
    "password is",
    "verification",
    "bearer",
];
/// The prefixes each pattern alone is run after (the whole texts above run every one).
const PATTERN_PREFIXES: &[&str] = &["482913"];
const SUFFIX: &str = " ab cd efgh";

const TEXTS: &[(&str, &str, &str)] = &[
    ("482913", " ", " ab cd"),
    ("482913", "\t", " ab cd"),
    ("482913", "\u{a0}", " ab cd"),
    ("482913", " \t\u{a0}\u{2003}", " ab cd"),
    ("482913 ", "*", " ab cd"),
    ("482913 ", "_", " ab cd"),
    ("482913 ", "`", " ab cd"),
    ("482913 ", "'\"", " ab cd"),
    ("482913", " *", " ab cd"),
    ("482913", " <b>", " ab cd"),
    ("482913 <span ", "a ", "b"),
    (
        "MAX_UNAUTH = 1024",
        " ",
        "# connections not yet authenticated held at once",
    ),
    ("", "1 ", "abc def"),
    ("", "1.5,", "abc def"),
    ("", "4821         ", "ab cd"),
    ("", "4821 is ", "your code"),
    ("one", " ", "x y zzz"),
    ("backup", " ", "x y zzz"),
    ("api", "_", " x yzw"),
    ("access ", "-", " x yzw"),
    ("my", " ", "wifi x y z"),
    ("password is", " ", "x y"),
    ("password:", "\r\n", "x y"),
    ("Your code:", "\r\n", "x"),
    ("", "a.", " x"),
    ("", "a-", " x"),
    ("", "pin/", " x"),
    ("", "PIN code 1234 ", "x"),
    ("", KEY_LINE, "x"),
    ("", "https://a.co password: x ", "y"),
    ("", "<b ", "482913 x"),
    ("> Your code:\n", ">\n", "> 482913 x"),
    ("", "(", "482913) x"),
    ("", " ", "482913 ab cd"),
    ("", "https://a.co 4821 ", "code"),
    ("Port ", "<code>8080</code> ", "is the default"),
    ("482913", " <br />", " x is your code"),
    (
        "<td>12345</td>",
        "<meta name=\"a\" content=\"b\"/>",
        "<p>Enter the code</p>",
    ),
    ("Order 482913", "<img src='a' />", "x"),
    ("482913\n", "<br />", "x code"),
    ("", "{\"token\":\"abc\",\"n\":1}", "\n"),
    ("", "{\"pin\":1234,\"x\":\"y\"}", "\n"),
    ("", "{\"user\":\"a\",\"has_password\":true}", "\n"),
    ("", "password: 'x ", "y z"),
    ("", "pwd: 'a' ", "x y"),
    ("", "api/", " x yz"),
    ("", "a.post(b).time=c;", " x yz"),
    ("my ", "api_", " x y zz"),
    ("", "eyJ-", " x yz"),
    ("", "eyJabcdefgh.", " x yz"),
    ("", "1/", " x yz"),
    ("", "1/", "1"),
    ("", "1 ", "1** yz"),
    ("", "4111 1111 1111 1111** ", "x yz"),
];

/// `prefix`, `unit` repeated to about `chars` characters, `suffix`.
fn make(prefix: &str, unit: &str, suffix: &str, chars: usize) -> String {
    let n = (chars / unit.chars().count()).max(1);
    format!("{prefix}{}{suffix}", unit.repeat(n))
}

/// The instructions `call` took (the least of `runs` runs); None when a pattern call in it failed (past the
/// backtrack limit).
fn cost(call: impl Fn(), runs: usize) -> Option<u64> {
    let mut least = u64::MAX;
    for _ in 0..runs {
        let before = instructions();
        checked(&call).ok()?;
        least = least.min(instructions() - before);
    }
    Some(least)
}

/// None when `call` on a run of `small` characters and on one four times as long costs about in proportion; else
/// what grew. A pair that looks out of proportion is measured again, the least of three runs each.
fn superlinear(
    call: impl Fn(&str),
    text: impl Fn(usize) -> String,
    small: usize,
    floor: u64,
) -> Option<String> {
    let (short, long) = (text(small), text(4 * small));
    let mut grew = None;
    for runs in [1, 3] {
        let Some(a) = cost(|| call(&short), runs) else {
            return Some(format!("a pattern call failed at {small} characters"));
        };
        let Some(b) = cost(|| call(&long), runs) else {
            return Some(format!(
                "a pattern call backtracked past its limit at {} characters",
                4 * small
            ));
        };
        if b <= floor || b <= GROWTH * a {
            return None;
        }
        grew = Some(format!(
            "{small} -> {} characters: {a} -> {b} instructions",
            4 * small
        ));
    }
    grew
}

fn stream_read(r: &Redactor, text: &str) {
    let data = text.as_bytes();
    let mut end = data.len().saturating_sub(2); // the read ends inside the last word
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    r.stream().feed(&data[..end], false);
}

fn whole_texts(slow: &mut Vec<String>) -> usize {
    let r = Redactor::default();
    let mut all: Vec<(&str, &str, &str)> = TEXTS.to_vec();
    for prefix in PREFIXES {
        for unit in RUNS {
            all.push((prefix, unit, SUFFIX));
        }
    }
    for &(prefix, unit, suffix) in &all {
        let text = |chars| make(prefix, unit, suffix, chars);
        if let Some(grew) = superlinear(|s| drop(r.text(s)), text, SMALL, FLOOR) {
            slow.push(format!(
                "Redactor::text on {prefix:?} + {unit:?} * n + {suffix:?}: {grew}"
            ));
        }
        if let Some(grew) = superlinear(|s| stream_read(&r, s), text, SMALL, FLOOR) {
            slow.push(format!(
                "stream read on {prefix:?} + {unit:?} * n + {suffix:?}: {grew}"
            ));
        }
    }
    2 * all.len()
}

const SOURCES: &[&str] = &[
    include_str!("../src/formats.rs"),
    include_str!("../src/known.rs"),
    include_str!("../src/labels.rs"),
    include_str!("../src/redact.rs"),
    include_str!("../src/stream.rs"),
    include_str!("../src/untrusted.rs"),
];

/// How the crate runs the pattern `name` of generated.rs, read from the crate's own source as the Python test
/// reads its modules': (searched through a text, matched from a position, full-matched). A pattern is run only
/// the ways the code runs it: one that is only ever matched from a position may cost the square of a run when
/// searched through it, in Python too. A pattern whose calls the scan cannot see is run every way.
fn ways(name: &str) -> (bool, bool, bool) {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let (mut search, mut matched, mut full) = (false, false, false);
    for source in SOURCES {
        let Some(at) = source.find(&format!("Rx::new(&g::{name})")) else {
            continue;
        };
        let Some(start) = source[..at].rfind("static ") else {
            continue;
        };
        let ident: String = source[start + 7..]
            .chars()
            .take_while(|&c| word(c))
            .collect();
        for (i, _) in source.match_indices(&ident) {
            if source[..i].chars().next_back().is_some_and(word) {
                continue;
            }
            let Some(call) = source[i + ident.len()..].trim_start().strip_prefix('.') else {
                continue;
            };
            let method: String = call.chars().take_while(|&c| word(c)).collect();
            match method.as_str() {
                "search" | "search_in" | "search_caps" | "find_all" | "find_all_in"
                | "caps_all" => {
                    search = true;
                }
                "match_at" | "match_in" | "match_caps" => matched = true,
                "fullmatch" | "fullmatch_in" => full = true,
                _ => {}
            }
        }
    }
    if name.starts_with("LABELS_VALUE_QUOTES_") {
        return (false, true, false); // chosen by the opening quote and matched after it (labels.rs)
    }
    if search || matched || full {
        (search, matched, full)
    } else {
        (true, true, true)
    }
}

/// Every pattern, on every run, the ways the crate runs it.
fn each_pattern(slow: &mut Vec<String>) -> usize {
    let names = parity::pattern_names();
    assert!(names.len() > 100, "only {} patterns", names.len());
    let all_ways: Vec<(bool, bool, bool)> = names.iter().map(|name| ways(name)).collect();
    let seen = all_ways
        .iter()
        .filter(|w| **w != (true, true, true))
        .count();
    assert!(
        seen * 2 > names.len(),
        "the source scan saw the calls of only {seen} patterns"
    );
    let mut judged = 0;
    for prefix in PATTERN_PREFIXES {
        for unit in RUNS {
            let text = |chars| make(prefix, unit, SUFFIX, chars);
            for (i, name) in names.iter().enumerate() {
                judged += 1;
                let (search, matched, full) = all_ways[i];
                let run = |s: &str| parity::run_pattern(i, s, prefix.len(), search, matched, full);
                if let Some(grew) = superlinear(run, text, PATTERN_SMALL, PATTERN_FLOOR) {
                    slow.push(format!("{name} on {prefix:?} + {unit:?} * n: {grew}"));
                }
            }
        }
    }
    judged
}

// test_sealed_values_are_linear_over_many_ids: known values inside every one of many FERB ids, among many words.
fn sealed_values(slow: &mut Vec<String>) {
    let r = Redactor::default();
    for value in ["8e95", "Zq9-pizza", "hunter2!"] {
        r.add_secret_value(value);
    }
    let unit = format!(
        "m-{} Zq9-pizza PIZZA2 pizzazz hunter2!x ",
        "8e95".repeat(16)
    );
    let text = |chars| make("", &unit, "", chars);
    let values = r.secret_values();
    slow.extend(
        superlinear(|s| drop(r.text(s)), text, SMALL, FLOOR)
            .map(|g| format!("known values, text: {g}")),
    );
    slow.extend(
        superlinear(
            |s| assert!(known::holds_sealed(s, &values)),
            text,
            SMALL,
            FLOOR,
        )
        .map(|g| format!("known values, holds_sealed: {g}")),
    );
}

/// The guard guards: a scan that restarts at every position, and a failed pattern call, are both seen.
fn the_guard_sees() {
    let restart = |s: &str| {
        let mut seen = 0;
        for (at, _) in s.char_indices() {
            seen += s[at..].bytes().filter(|&b| b == b'z').count(); // the rest of the text, from every position
        }
        assert_eq!(std::hint::black_box(seen), 0);
    };
    let grew = superlinear(restart, |chars| make("", "a ", "", chars), SMALL, FLOOR);
    assert!(
        grew.as_ref().is_some_and(|g| g.contains("instructions")),
        "a quadratic scan passed: {grew:?}"
    );
    parity::fail_from(Some(0));
    let failed = cost(
        || drop(Redactor::default().text("Your code is 482913 today")),
        1,
    );
    parity::fail_from(None);
    assert_eq!(failed, None, "a failed pattern call passed");
}

// One test: the count is the whole process's, so nothing else may run beside a measured call.
#[test]
fn no_pathological_input_costs_more_than_in_proportion_to_its_size() {
    let limit = std::env::var("FERB_SCALING_BACKTRACKS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(BACKTRACKS);
    parity::set_backtrack_limit(limit); // before any pattern is compiled
    assert!(
        instructions() > 0,
        "this Mac's kernel counts no instructions: nothing would be judged"
    );
    the_guard_sees();
    let mut slow = Vec::new();
    let texts = whole_texts(&mut slow);
    eprintln!("scaling: whole texts done, {} slow", slow.len());
    sealed_values(&mut slow);
    let patterns = each_pattern(&mut slow);
    eprintln!(
        "scaling: {texts} whole-text runs and {patterns} pattern runs judged, {} slow",
        slow.len()
    );
    assert!(slow.is_empty(), "{}", slow.join("\n"));
}
