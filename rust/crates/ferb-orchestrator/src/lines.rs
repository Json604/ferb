//! The two helpers of ferb/trace/lines.py these modules use (`shorten`, `choices`); `shorten` is ferb-py's one copy,
//! which ferb-trace re-exports too.

/// lines.py:17 `shorten`: a short form that never cuts a word (ORCH-601). The one copy is ferb-py's.
pub use ferb_py::text::shorten;

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
