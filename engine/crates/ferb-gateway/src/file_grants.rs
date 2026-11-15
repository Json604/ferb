//! The file question's third answer (decision: GATE-593, refining GATE-587; GATE-594): "Send to every site in
//! this task". After outside content was read, sending one of the owner's files to a site the owner neither named nor
//! approved asks once per site (GATE-587); this answer stops FERB asking about that one file for the rest of the task
//! tree, on any site.
//!
//! What it covers, enforced here and in `Gateway::file_send_needs`:
//! - one file, by its resolved path and what the file system says about it now (size, modification time, inode): a
//!   file changed after the answer, or a copy of it at another path, is a different file and is asked about again;
//! - one task tree: held in memory under the tree's root and dropped with the tree (`forget_tasks`) or a restart
//!   (FERB then asks again);
//! - only the owner grants it: the app's click on that choice (`chat.approve` with its number) or the owner's own
//!   typed words that are exactly that choice's words (`Approval::option_for`); never a page, a file or the model.
//!
//! Every site that received a file under such an answer is listed in the task's answer (`sent_lines`).

use std::collections::{BTreeMap, BTreeSet};

use ferb_policy::confirm::Need;

/// The option recorded for the answer (what the app's number picks and what exact typed words pick).
pub const EVERY_SITE: &str = "Yes, on every site in this task";
/// Its button's words.
pub const EVERY_SITE_LABEL: &str = "Send to every site in this task";

/// The line under the buttons: what the answer covers and what it doesn't.
pub fn note(files: usize) -> String {
    let (it, this) = if files > 1 {
        ("they", "these files")
    } else {
        ("it", "this file")
    };
    format!(
        "“{EVERY_SITE_LABEL}” stops FERB asking about {this} for the rest of this task, on any site. If {it} \
         change{} or {} copied, FERB asks again.",
        if files > 1 { "" } else { "s" },
        if files > 1 { "are" } else { "is" }
    )
}

/// The file question's card: its buttons "Send", "Send to every site in this task", "Don't send" (as its options are
/// ordered, `answers::choices`) and the line under them. A card whose words couldn't be built keeps no labels (the app
/// shows the options themselves).
pub fn card(labels: &mut Vec<String>, note_line: &mut String, needs: &[Need]) {
    if labels.len() == 2 {
        labels.insert(1, EVERY_SITE_LABEL.to_string());
    }
    let files: usize = needs
        .iter()
        .filter(|n| is_file_need(n))
        .map(|n| {
            n.values
                .opt("files")
                .split('\n')
                .filter(|f| !f.is_empty())
                .count()
        })
        .sum();
    *note_line = note(files);
}

/// The file question: the form sink's question for a call that hands over the owner's files (GATE-587).
pub fn is_file_need(n: &Need) -> bool {
    n.rule == "sink:new_destination" && n.values.has("files")
}

/// One file as an answer covers it: its resolved path and what the file system says about it now.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileId {
    pub path: String,
    len: u64,
    modified_ns: i128,
    inode: u64,
}

impl FileId {
    /// The file at `real` (a resolved path) as it is now; None when it can't be read (gone, unreadable): such a file
    /// is never covered.
    pub fn of(real: &str) -> Option<FileId> {
        let m = std::fs::metadata(real).ok()?;
        let modified_ns = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i128)
            .unwrap_or(-1);
        #[cfg(unix)]
        let inode = std::os::unix::fs::MetadataExt::ino(&m);
        #[cfg(not(unix))]
        let inode = 0;
        Some(FileId {
            path: real.to_string(),
            len: m.len(),
            modified_ns,
            inode,
        })
    }
}

/// A file as the question shows it (`~/…` under `home`, or a full path) as it is now, its links resolved.
pub fn id_of(shown: &str, home: &str) -> Option<FileId> {
    let full = match shown.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", home.trim_end_matches('/')),
        None => shown.to_string(),
    };
    let real = std::fs::canonicalize(&full).ok()?;
    FileId::of(real.to_str()?)
}

/// The owner chose "Send to every site in this task" on a question asking `needs`: each file question's files are
/// granted for this tree. Returns (file as shown, site) for each file this call now sends.
pub fn grant_asked(
    grants: &mut FileGrants,
    root: &str,
    needs: &[Need],
    home: &str,
) -> Vec<(String, String)> {
    let mut sent = Vec::new();
    for n in needs.iter().filter(|n| is_file_need(n)) {
        let host = n.values.opt("destination").to_string();
        for shown in n.values.opt("files").split('\n').filter(|f| !f.is_empty()) {
            if let Some(id) = id_of(shown, home) {
                grants.grant(root, id);
            }
            sent.push((shown.to_string(), host.clone()));
        }
    }
    sent
}

/// The answers given and the sites that received a file under one, per task tree (its root's id).
#[derive(Debug, Default)]
pub struct FileGrants {
    granted: BTreeMap<String, BTreeSet<FileId>>,
    /// (the file as shown, the site), in the order sent, each pair once.
    sent: BTreeMap<String, Vec<(String, String)>>,
}

impl FileGrants {
    /// The owner's "Send to every site in this task" for this file in this tree.
    pub fn grant(&mut self, root: &str, file: FileId) {
        self.granted
            .entry(root.to_string())
            .or_default()
            .insert(file);
    }

    /// Whether an answer in this tree covers this file as it is now.
    pub fn covers(&self, root: &str, file: &FileId) -> bool {
        self.granted.get(root).is_some_and(|g| g.contains(file))
    }

    /// A call that ran sent `shown` to `host` under such an answer.
    pub fn sent(&mut self, root: &str, shown: &str, host: &str) {
        let list = self.sent.entry(root.to_string()).or_default();
        let pair = (shown.to_string(), host.to_string());
        if !list.contains(&pair) {
            list.push(pair);
        }
    }

    /// The task answer's lines, one per file: "Sent resume.pdf to: a.example, b.example." The file by its name, or by
    /// its path where two sent files share a name.
    pub fn sent_lines(&self, root: &str) -> Vec<String> {
        let Some(list) = self.sent.get(root) else {
            return Vec::new();
        };
        let mut files: Vec<(&str, Vec<&str>)> = Vec::new();
        for (file, host) in list {
            match files.iter_mut().find(|(f, _)| f == file) {
                Some((_, hosts)) => hosts.push(host),
                None => files.push((file, vec![host])),
            }
        }
        let name = |f: &str| {
            f.trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(f)
                .to_string()
        };
        let names: Vec<String> = files.iter().map(|(f, _)| name(f)).collect();
        files
            .iter()
            .zip(&names)
            .map(|((file, hosts), n)| {
                let shown = if names.iter().filter(|m| *m == n).count() > 1 {
                    file.to_string()
                } else {
                    n.clone()
                };
                format!("Sent {shown} to: {}.", hosts.join(", "))
            })
            .collect()
    }

    /// The trees ended: their answers and lists go.
    pub fn forget(&mut self, roots: &BTreeSet<String>) {
        self.granted.retain(|r, _| !roots.contains(r));
        self.sent.retain(|r, _| !roots.contains(r));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answers_line_names_each_file_once_with_its_sites_in_order() {
        let mut g = FileGrants::default();
        g.sent("t", "~/Documents/resume.pdf", "rex.zone");
        g.sent("t", "~/Documents/resume.pdf", "alignerr.com");
        g.sent("t", "~/Documents/resume.pdf", "rex.zone");
        g.sent("t", "~/Documents/resume.pdf", "mercor.com");
        assert_eq!(
            g.sent_lines("t"),
            vec!["Sent resume.pdf to: rex.zone, alignerr.com, mercor.com."]
        );
        g.sent("t", "~/Old/resume.pdf", "a.example");
        assert_eq!(
            g.sent_lines("t")[1],
            "Sent ~/Old/resume.pdf to: a.example.",
            "two files with one name are told apart"
        );
        g.forget(&BTreeSet::from(["t".to_string()]));
        assert!(g.sent_lines("t").is_empty());
    }

    #[test]
    fn the_note_counts_the_files() {
        assert_eq!(
            note(1),
            "“Send to every site in this task” stops FERB asking about this file for the rest of this task, on any \
             site. If it changes or is copied, FERB asks again."
        );
        assert!(
            note(2).contains("these files") && note(2).contains("If they change or are copied")
        );
    }
}
