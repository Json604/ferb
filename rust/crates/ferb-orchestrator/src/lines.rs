//! The two helpers of ferb/trace/lines.py these modules use (`shorten`, `choices`). The trace part (ferb-trace)
//! ports the same functions; they belong in one place once both crates are members.

use crate::py::{is_space, squeeze};

/// lines.py:17 `shorten`: a short form that never cuts a word (ORCH-601).
pub fn shorten(text: &str, n: usize, fold: bool) -> String {
    let s = if fold { squeeze(text) } else { text.to_owned() };
    if s.chars().count() <= n {
        return s;
    }
    let head: Vec<char> = s.chars().take(n.saturating_sub(1)).collect();
    // re.finditer(r"[.!?]\s", head): non-overlapping, so a match skips the space it took
    let mut last_end = None;
    let mut i = 0;
    while i + 1 < head.len() {
        if matches!(head[i], '.' | '!' | '?') && is_space(head[i + 1]) {
            last_end = Some(i);
            i += 2;
        } else {
            i += 1;
        }
    }
    if let Some(e) = last_end {
        if e + 1 >= n / 2 {
            let cut: String = head[..=e].iter().collect();
            return format!("{cut} …");
        }
    }
    match head.iter().rposition(|&c| is_space(c)) {
        Some(sp) if sp > 0 => {
            let cut: String = head[..sp].iter().collect();
            format!(
                "{}…",
                cut.trim_end_matches([' ', ',', ';', ':', '(', '—', '-', '\n', '\t'])
            )
        }
        _ => format!("{}…", head.iter().collect::<String>()),
    }
}

/// lines.py:37 `choices`: "Yes or No", "A, B or C".
pub fn choices<S: AsRef<str>>(options: &[S]) -> String {
    let opts: Vec<&str> = options.iter().map(|o| o.as_ref()).collect();
    if opts.len() < 2 {
        return opts.concat();
    }
    format!(
        "{} or {}",
        opts[..opts.len() - 1].join(", "),
        opts[opts.len() - 1]
    )
}
