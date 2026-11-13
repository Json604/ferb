//! The words of FERB's hidden-input questions (`ferb/daemon/hidden_input.py`): an owner secret, a login code and the
//! Mac password for one approved admin command. FERB's own fixed words with the values put in; the values are
//! redacted each by itself before they get here (GATE-572), and the cards never quote a worker's text before the
//! place (re-review R1). The asking itself is the gateway's (`gateway::hidden`).
//!
//! decision: GATE-513, GATE-520, GATE-559, GATE-566

use ferb_policy::py::{is_cf, is_printable};
use ferb_policy::urls;
use ferb_sanitize::terminal::printable;

/// GATE-520: past this a worker's name for a saved secret is cut ("…").
pub const NAME_SHOWN: usize = 200;
/// A page's own label for the field: whole, only a wall of text cut.
pub const LABEL_SHOWN: usize = 300;
/// The owner's username as a question shows it.
pub const ACCOUNT_SHOWN: usize = 64;

pub const ADMIN_DETAIL: &str =
    "macOS needs it to run the one command you just allowed as administrator. It goes only \
                                to that command and isn't saved; FERB's AI never sees it.";
pub const ADMIN_SUMMARY: &str = "Your Mac password, for the admin command";
pub const ADMIN_TITLE: &str = "Private — only the command gets this";
pub const ADMIN_QUESTION: &str =
    "Enter your Mac password for the approved command (hidden; Enter cancels).";
/// The title of an `app:` place's secret card.
pub const APP_TITLE: &str = "Private — only the app gets this";
/// GATE-566: a hidden input's field, decline and send words, by what it is.
pub const ONE_TIME_LABELS: [&str; 3] = ["Type it here", "Don't fill in", "Fill in"];
pub const SAVE_LABELS: [&str; 3] = ["Type it here", "Don't save", "Fill in and save"];
pub const CODE_LABELS: [&str; 3] = ["Code", "Don't enter", "Enter code"];
pub const OTP_LABELS: [&str; 3] = ["Code", "Don't pay", "Enter code"];
pub const ADMIN_LABELS: [&str; 3] = ["Mac password", "Cancel", "Run command"];

fn split_join(text: &str) -> String {
    ferb_policy::py::split_ws(text).join(" ")
}

fn char_len(text: &str) -> usize {
    text.chars().count()
}

/// `text[:n - 1].rstrip() + "…"` past `n` characters, else the text.
fn clip(text: String, n: usize) -> String {
    if char_len(&text) <= n {
        return text;
    }
    let head: String = text.chars().take(n - 1).collect();
    format!("{}…", ferb_policy::py::rstrip(&head))
}

/// `repr(text)` of a `str`, as Python writes it.
pub fn py_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for c in text.chars() {
        let cp = c as u32;
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            _ if cp < 0x20 || cp == 0x7f => out.push_str(&format!("\\x{cp:02x}")),
            _ if cp < 0x7f => out.push(c),
            _ if is_printable(c) => out.push(c),
            _ if cp <= 0xff => out.push_str(&format!("\\x{cp:02x}")),
            _ if cp <= 0xffff => out.push_str(&format!("\\u{cp:04x}")),
            _ => out.push_str(&format!("\\U{cp:08x}")),
        }
    }
    out.push(quote);
    out
}

/// `_site(target)`: an origin's host, with its port unless it is HTTPS's own (443), else the target as given.
pub fn site(target: &str) -> String {
    match urls::parse(target) {
        Some(p) if !p.host.is_empty() => {
            let host = if p.host.contains(':') {
                format!("[{}]", p.host)
            } else {
                p.host.clone()
            };
            if !p.port.is_empty() && p.port != "443" {
                format!("{host}:{}", p.port)
            } else {
                host
            }
        }
        _ => target.to_string(),
    }
}

/// `_ACCOUNT_DROPPED`: format characters (bidi overrides, zero-width), surrogates and line separators.
fn dropped(c: char) -> bool {
    is_cf(c) || c == '\u{2028}' || c == '\u{2029}'
}

/// One line, printable, without invisible or reordering characters.
fn plain_line(text: &str) -> String {
    let text = split_join(&printable(text));
    let kept: String = text.chars().filter(|&c| !dropped(c)).collect();
    split_join(&kept)
}

/// `_account(username)`: the owner's username as a question shows it, at most ACCOUNT_SHOWN characters.
pub fn account(username: &str) -> String {
    clip(plain_line(username), ACCOUNT_SHOWN)
}

/// `_page_label(field)`: the page's own label for the field, as the card quotes it.
pub fn page_label(field: Option<&str>) -> String {
    clip(plain_line(field.unwrap_or("")), LABEL_SHOWN)
}

/// `_NAME_DROPPED`'s complement: what a shown name keeps.
fn name_kept(c: char) -> bool {
    c.is_ascii_alphanumeric() || " :,'()-".contains(c)
}

/// `shown_name(remember, place)` (review I1, re-review R1, GATE-520): the saved name as the card shows it: only ASCII
/// letters, digits, spaces and : , ' ( ) -, whitespace collapsed; a leading copy of the card's place left out.
pub fn shown_name(remember: &str, place: &str) -> String {
    let collapsed = split_join(remember);
    let kept: String = collapsed.chars().filter(|&c| name_kept(c)).collect();
    let mut name = split_join(&kept);
    if !place.is_empty() {
        let lower = ferb_policy::py::casefold(place);
        let host = lower.strip_prefix("www.").unwrap_or(&lower);
        let host = host.strip_prefix("the ").unwrap_or(host);
        let host = host.strip_suffix(" app").unwrap_or(host);
        let mut forms: Vec<String> = vec![host.chars().filter(|&c| name_kept(c)).collect()];
        for label in host.split(|c: char| c == '.' || ferb_policy::py::is_space(c)) {
            if char_len(label) > 2 {
                forms.push(label.to_string());
            }
        }
        let mut forms: Vec<String> = forms.into_iter().filter(|f| !f.is_empty()).collect();
        forms.sort();
        forms.dedup();
        // sorted by length, longest first; Python's sort is stable over a set's order, so equal lengths are
        // tried in an order that cannot change which one matches first at the start of the name except when two
        // forms of one length both prefix it, which needs two equal strings
        forms.sort_by_key(|f| std::cmp::Reverse(char_len(f)));
        for form in forms {
            let folded = ferb_policy::py::casefold(&name);
            if !folded.starts_with(&form) {
                continue;
            }
            let rest: String = name.chars().skip(char_len(&form)).collect();
            let first = rest.chars().next();
            let trimmed = rest.trim_matches(|c| " :,-".contains(c));
            if matches!(first, Some(' ' | ':' | ',' | '-')) && !trimmed.is_empty() {
                name = trimmed.to_string();
                break;
            }
        }
    }
    clip(name, NAME_SHOWN)
}

/// `secret_question(target, remember, username, username_place)`: the exact destination first.
pub fn secret_question(
    target: &str,
    remember: Option<&str>,
    username: Option<&str>,
    username_place: Option<&str>,
) -> String {
    let acct = account(username.unwrap_or(""));
    let saved_for = match username_place.filter(|p| !p.is_empty()) {
        Some(p) => format!(" (saved for {})", site(p)),
        None => String::new(),
    };
    let mut text = format!("Destination: {target}.");
    if !acct.is_empty() {
        text.push_str(&format!(" Account: {acct}{saved_for}."));
    }
    text.push_str(
        " Type the password or secret for it here (hidden; Enter cancels); FERB fills it only at this exact \
         destination.",
    );
    if let Some(r) = remember.filter(|r| !r.is_empty()) {
        text.push_str(&format!(
            " Entering it also saves it in your Mac's Keychain as {}, usable only at this destination.",
            py_repr(r)
        ));
    }
    text
}

/// `secret_place(target, app)`: where a hidden value goes, in the owner's terms.
pub fn secret_place(target: &str, app: Option<&str>) -> String {
    if let Some(bundle) = target.strip_prefix("app:") {
        return match app.filter(|a| !a.is_empty()) {
            Some(a) => format!("the {a} app"),
            None => format!("the app {bundle}"),
        };
    }
    let s = site(target);
    if ferb_policy::py::casefold(&s).starts_with("www.") {
        s.chars().skip(4).collect()
    } else {
        s
    }
}

/// `secret_summary(...)` (P27): the place first, then the account, then a generic what.
pub fn secret_summary(
    target: &str,
    remember: Option<&str>,
    app: Option<&str>,
    username: Option<&str>,
    username_place: Option<&str>,
) -> String {
    let place = secret_place(target, app);
    let acct = account(username.unwrap_or(""));
    let saved = match username_place.filter(|p| !p.is_empty()) {
        Some(p) => format!(", saved for {}", site(p)),
        None => String::new(),
    };
    let acct = if acct.is_empty() {
        String::new()
    } else {
        format!(" ({acct}{saved})")
    };
    if remember.is_some_and(|r| !r.is_empty()) {
        format!("{place}{acct}: type the answer to save")
    } else {
        format!("{place}{acct}: type your password or answer")
    }
}

/// `label[-1] in '.?!…'`.
fn ends_said(label: &str) -> bool {
    label.chars().last().is_some_and(|c| ".?!…".contains(c))
}

/// `secret_detail(target, remember, app, field)` (GATE-513, GATE-520).
pub fn secret_detail(
    target: &str,
    remember: Option<&str>,
    app: Option<&str>,
    field: Option<&str>,
) -> String {
    let place = secret_place(target, app);
    let label = page_label(field);
    let remember = remember.filter(|r| !r.is_empty());
    let name = remember.map(|r| shown_name(r, &place)).unwrap_or_default();
    let what = if label.is_empty() {
        String::new()
    } else {
        format!(
            "The page asks: “{label}”{} ",
            if ends_said(&label) { "" } else { "." }
        )
    };
    if remember.is_some() {
        let named = if name.is_empty() {
            String::new()
        } else {
            format!(", as “{name}”")
        };
        return format!(
            "{what}FERB fills it in and saves it in your Mac's Keychain, for {place} only{named}. FERB's AI never \
             sees it."
        );
    }
    let into = if target.starts_with("app:") {
        "that app"
    } else {
        "that page"
    };
    format!("{what}FERB types it into {into} once and doesn't keep it. FERB's AI never sees it.")
}

/// `login_code_summary(origin)` (P27): the origin's host first.
pub fn login_code_summary(origin: &str) -> String {
    format!("{} asks for a login code", site(origin))
}

/// `login_code_detail(field, site, count)` (GATE-559, GATE-566).
pub fn login_code_detail(field: Option<&str>, count: usize) -> String {
    let label = page_label(field);
    let what = if label.is_empty() {
        String::new()
    } else {
        format!(
            "The page asks for “{label}”{} ",
            if ends_said(&label) { "" } else { "." }
        )
    };
    let nth = if count > 1 {
        format!(" This is code {count} for this site in this task.")
    } else {
        String::new()
    };
    format!(
        "{what}Type the code the site sent you (by email or text). FERB enters it on that page once and doesn't \
         keep it; FERB's AI never sees it.{nth}"
    )
}

/// `refused_line(refusal)` (GATE-562): a fresh ask after the page refused the last value says so; "" else.
pub fn refused_line(refusal: Option<(&str, &str)>) -> String {
    let Some((what, words)) = refusal else {
        return String::new();
    };
    let said = if words.is_empty() {
        String::new()
    } else {
        format!(" It says “{}”.", page_label(Some(words)))
    };
    format!("The last {what} wasn't accepted: the page asked for it again.{said} ")
}

/// `login_code_question(origin)`: its exact origin first.
pub fn login_code_question(origin: &str) -> String {
    format!(
        "Destination: {origin}. Type the login code for it here (hidden; Enter cancels); FERB fills it only at this \
         exact destination."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // decision: GATE-520
    #[test]
    fn a_shown_name_keeps_only_plain_ascii_and_leaves_out_the_place() {
        // the cases of tests/test_question_clarity.py
        let spoof = "slot” — for accounts.google.com only · saved in your Mac's Keychain. Real: “x";
        assert_eq!(
            shown_name(spoof, "x.test"),
            "slot for accountsgooglecom only saved in your Mac's Keychain Real: x"
        );
        assert_eq!(
            shown_name("x&rdquo; &mdash; for g&period;co only", "x.test"),
            "xrdquo mdash for gperiodco only"
        );
        assert_eq!(shown_name(&"a".repeat(300), "x.test").chars().count(), 200);
        assert_eq!(shown_name("GitHub password", "github.com"), "password");
        assert_eq!(
            shown_name("Asha's bank: savings, 2/3 (main) - US", "x.test"),
            "Asha's bank: savings, 23 (main) - US"
        );
        assert_eq!(shown_name("“”—·．", "x.test"), "");
    }

    #[test]
    fn a_place_keeps_its_port_unless_it_is_https_own() {
        assert_eq!(site("https://x.test:443"), "x.test");
        assert_eq!(site("https://x.test:8443"), "x.test:8443");
        assert_eq!(site("https://[::1]:8443"), "[::1]:8443");
        assert_eq!(site("app:com.example.Bank"), "app:com.example.Bank");
        assert_eq!(py_repr("two\nlines\u{202e}"), "'two\\nlines\\u202e'");
    }
}
