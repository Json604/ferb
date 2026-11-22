//! A contract: every kind of FERB's own question the daemon can raise is one the real table takes
//! (`ferb_orchestrator::steering::memory_questions`, which gives the gateway its capability). The kinds are read from
//! the code itself, every `ask_owner(` and the browser area's `questions.ask(` call site in ferb-daemon and ferbd, not
//! from a hand list, so a new kind can't pass on a stand-in that skips the table again (journal 166: the memory
//! import's first two kinds did).

use std::path::{Path, PathBuf};

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// `pub const NAME: &str = "value";` anywhere in `texts`.
fn constant(texts: &[String], name: &str) -> Option<String> {
    let needle = format!("const {name}: &str = \"");
    texts.iter().find_map(|t| {
        let at = t.find(&needle)? + needle.len();
        Some(t[at..].split('"').next()?.to_owned())
    })
}

/// The kind argument of each call: the text after `marker(` up to the argument at `index` (0-based).
fn arguments(text: &str, marker: &str, index: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(marker) {
        let start = from + at + marker.len();
        from = start;
        // the definition itself, and a mention in a comment
        let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
        if text[..start].ends_with("fn ask_owner(")
            || text[line_start..start].trim_start().starts_with("//")
        {
            continue;
        }
        let args: Vec<&str> = text[start..].splitn(index + 2, ',').collect();
        // group A's ask_owner takes the state first; another function of that name (a device check's) isn't it
        if index == 1 && !matches!(args.first().map(|a| a.trim()), Some("st" | "&st")) {
            continue;
        }
        if let Some(a) = args.get(index) {
            out.push(a.trim().to_owned());
        }
    }
    out
}

// decision: MEM-447, MEM-448, MEM-444
#[test]
fn every_own_question_kind_the_daemon_raises_is_in_the_real_table() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for c in ["ferb-daemon/src", "ferbd/src", "ferb-orchestrator/src"] {
        sources(&root.join(c), &mut files);
    }
    let texts: Vec<String> = files
        .iter()
        .map(|f| std::fs::read_to_string(f).unwrap())
        .collect();
    let (mut heads, mut forwarders) = (Vec::new(), 0);
    for (f, t) in files.iter().zip(&texts) {
        if f.to_string_lossy().contains("ferb-orchestrator") {
            continue;
        }
        let mut found = arguments(t, "ask_owner(", 1);
        found.extend(arguments(t, "questions.ask(", 0));
        for arg in found {
            let arg = arg.trim_start_matches('&');
            let value = if let Some(lit) = arg.strip_prefix('"') {
                lit.split('"').next().unwrap().to_owned()
            } else if let Some(fmt) = arg.strip_prefix("format!(\"") {
                fmt.split('"').next().unwrap().to_owned()
            } else if arg == "kind" || arg.starts_with("Ask") {
                // a pass-through (the restart's re-ask, the browser area's bridge to group A, the seam's own call)
                forwarders += 1;
                continue;
            } else {
                let name = arg.rsplit("::").next().unwrap();
                constant(&texts, name)
                    .unwrap_or_else(|| panic!("{}: can't read the kind {arg}", f.display()))
            };
            heads.push(value.split(':').next().unwrap().to_owned());
        }
    }
    heads.sort();
    heads.dedup();
    for h in [
        "memory_local",
        "memory_sites",
        "memory_offers",
        "downloads",
        "forget",
        "browsing",
    ] {
        assert!(
            heads.contains(&h.to_owned()),
            "{h} not found among {heads:?}: the scan missed a call site"
        );
    }
    assert!(forwarders >= 2, "{forwarders}");
    for h in &heads {
        assert!(
            ferb_orchestrator::steering::memory_questions(h).is_some(),
            "FERB asks a question of kind {h}, which the real table doesn't take"
        );
        assert!(
            ferb_orchestrator::steering::OWN_KINDS.contains(&h.as_str()),
            "{h}"
        );
    }
}
