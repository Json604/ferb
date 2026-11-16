//! Project mode (owner, 2026-11-16: HAND-620): a folder anywhere on the Mac that the owner said Yes to as a code
//! project. What a project is and isn't, as code enforces it:
//!
//! - The folder's files: FERB's commands already read anywhere outside the never-read list and write in the folders
//!   a task names (HAND-203, HAND-204); a project adds nothing there. Protected entries inside a project stay closed:
//!   the shell profile writes the never-read and never-write denies after every folder it opens (HAND-621).
//! - The toolchains' own folders that live in plain sight in the home folder ([`DEV_TOOL_FOLDERS`]) are writable for a
//!   task that works in a project (HAND-622). Hidden folders and ~/Library already are (HAND-203, HAND-205).
//! - `project.publish` pushes the folder's git repository with the owner's own git login, from FERB's code outside
//!   the shell sandbox, to an address the owner approved for that folder (HAND-623, HAND-624).
//!
//! What can never be a project ([`refusal`]): FERB's own state (its workspace's folders may), FERB.app, its service
//! definitions, a folder that holds one of them (the home folder, ~/Library, /), and anything inside a never-read or
//! never-write entry. Decided from the protection lists only (rule 3), never from the folder's name or contents.
//!
//! - `project.fetch` gets a repository's branches (or a clone) with the owner's login the same way (HAND-642);
//!   `force` replaces a remote branch after its own question, every time (HAND-641); the GitHub CLI's saved login is
//!   used for a project once the owner says so (HAND-643). The owner's rule (HAND-640): what FERB's commands can't do
//!   by default, FERB asks plainly and does on his Yes.
//!
//! The records are the owner's answers, kept in the journal (append-only, never operational: kept until the owner
//! removes the project in Settings): `project.granted`, `project.publish_approved`, `project.fetch_approved`,
//! `project.gh_approved`, `project.removed`, and each forced push's single-use `project.force_approved` /
//! `project.force_used`.

use crate::protection::{Protection, FERB_APP, FERB_SERVICE, FERB_STATE, REMOVE, WRITE};

/// The tool that publishes a project's git repository.
pub const TOOL: &str = "project.publish";
/// Its action kind: not a send of the owner's words (no new-recipient or sink rule reads it), never reversible (a
/// ledger row keeps an unknown push from being repeated, P19). Its one question is [`PUBLISH_RULE`].
pub const KIND: &str = "publish";
/// The rule of the publish question. Never routine: the 30-minute window doesn't answer it (GATE-207).
pub const PUBLISH_RULE: &str = "project:publish";
/// HAND-641: a forced push's question: asked every time, never remembered, never routine.
pub const FORCE_RULE: &str = "project:force";
/// HAND-642: getting from an address not yet approved for the project (or a clone).
pub const FETCH_RULE: &str = "project:fetch";
/// HAND-643: the GitHub CLI's saved login for a project, asked once per project and host.
pub const GH_RULE: &str = "project:gh";
/// The fetch tool and its kind (it writes objects and remote-branch records into the project).
pub const FETCH_TOOL: &str = "project.fetch";
pub const FETCH_KIND: &str = "fetch";
pub const GRANTED: &str = "project.granted";
pub const APPROVED: &str = "project.publish_approved";
pub const FETCH_APPROVED: &str = "project.fetch_approved";
pub const GH_APPROVED: &str = "project.gh_approved";
pub const FORCE_APPROVED: &str = "project.force_approved";
pub const FORCE_USED: &str = "project.force_used";
pub const REMOVED: &str = "project.removed";

/// HAND-622: the toolchains' default working folders that sit in plain sight in the home folder, so HAND-203 closes
/// them to writes: Go's GOPATH (its module cache and installed programs) and conda's base installs (environments and
/// packages). Every other toolchain cache measured (cargo, rustup, npm, pnpm, yarn, bun, deno, pip, uv, poetry,
/// pyenv, gem, rbenv, gradle, maven, CocoaPods, SwiftPM, Xcode's DerivedData, Homebrew's cache) is a hidden folder or
/// in ~/Library, writable already; their credentials files (~/.cargo/credentials.toml, ~/.npmrc, ~/.docker, ~/.pypirc,
/// ~/.git-credentials) stay on the never-read list.
pub const DEV_TOOL_FOLDERS: &[&str] =
    &["go", "miniconda3", "anaconda3", "miniforge3", "mambaforge"];

/// Why `folder` (absolute) can't be a project, or None. Structural only: the protection lists' answers for the folder
/// itself and for what it holds.
// decision: HAND-621
pub fn refusal(p: &Protection<'_>, folder: &str) -> Option<String> {
    if !folder.starts_with('/') {
        return Some("a project folder must be an absolute path".into());
    }
    // what the folder holds: FERB's own state, FERB.app or its service definitions inside it
    let held = p.fs_rules(folder, REMOVE);
    for own in [FERB_STATE, FERB_APP, FERB_SERVICE] {
        if held.iter().any(|r| r == own) {
            return Some(format!(
                "{} can't be a project: it is or holds FERB's own files (rule {own}), which FERB never changes",
                shown(&p.home, folder)
            ));
        }
    }
    // the folder itself inside a protected entry
    if let Some(rule) = p.fs_rules(folder, WRITE).into_iter().next() {
        return Some(format!(
            "{} can't be a project: it is inside a place on your protection list (rule {rule})",
            shown(&p.home, folder)
        ));
    }
    None
}

/// Whether `path` is `folder` or inside it, by whole path components (both absolute, already resolved).
pub fn within(path: &str, folder: &str) -> bool {
    let folder = folder.trim_end_matches('/');
    if folder.is_empty() {
        return path.starts_with('/');
    }
    path == folder || path.starts_with(&format!("{folder}/"))
}

/// A path as the owner reads it: the home folder as ~.
pub fn shown(home: &str, path: &str) -> String {
    let home = home.trim_end_matches('/');
    let real = std::fs::canonicalize(home)
        .ok()
        .and_then(|p| p.to_str().map(str::to_string));
    for h in std::iter::once(home.to_string()).chain(real) {
        if !h.is_empty() && within(path, &h) {
            return format!("~{}", &path[h.len()..]);
        }
    }
    path.to_string()
}

/// The granted project that holds `path`, if any (the innermost).
pub fn holding<'a>(projects: &'a [String], path: &str) -> Option<&'a str> {
    projects
        .iter()
        .filter(|f| within(path, f))
        .max_by_key(|f| f.len())
        .map(String::as_str)
}

/// HAND-622: the toolchain folders a task's shell may also change, when one of its folders is in a project: each of
/// [`DEV_TOOL_FOLDERS`] under `home` (existing or not: a first build makes ~/go).
pub fn dev_tool_folders(home: &str, task_folders: &[String], projects: &[String]) -> Vec<String> {
    if !task_folders.iter().any(|f| holding(projects, f).is_some()) {
        return Vec::new();
    }
    let home = home.trim_end_matches('/');
    DEV_TOOL_FOLDERS
        .iter()
        .map(|d| format!("{home}/{d}"))
        .filter(|d| !task_folders.contains(d))
        .collect()
}

/// HAND-625: the worker's fact line when the owner has projects (none: no line, the context as before).
pub fn fact_line(home: &str, projects: &[String]) -> Option<String> {
    if projects.is_empty() {
        return None;
    }
    let shown: Vec<String> = projects.iter().map(|f| shown(home, f)).collect();
    Some(format!(
        "The owner's project folders (each one the owner said Yes to): {}. To publish one's git repository use \
         project.publish (git push with the owner's own login, run by FERB, never by a command: commands can't reach \
         saved logins or SSH keys), and to get from its remote with that login project.fetch; FERB asks before the \
         first use of each address. A task that works in a project may also change the toolchain folders {}.",
        shown.join(", "),
        DEV_TOOL_FOLDERS
            .iter()
            .map(|d| format!("~/{d}"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// The owner's projects as their answers left them: each granted folder not removed since, with the push addresses
/// approved for it since it was granted, in the order they came.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub folder: String,
    pub since: String,
    /// Addresses approved for pushing.
    pub destinations: Vec<String>,
    /// Addresses approved for getting from (a push address may be got from too).
    pub sources: Vec<String>,
    /// Hosts whose GitHub CLI login the owner allowed for this project.
    pub gh_hosts: Vec<String>,
}

impl Project {
    /// Whether FERB may get from `url` for this project unasked.
    pub fn gets_from(&self, url: &str) -> bool {
        self.sources
            .iter()
            .chain(&self.destinations)
            .any(|u| u == url)
    }
}

/// Folds the journal's project rows (kind, at, data), oldest first, into the projects that stand.
pub fn fold(rows: &[(String, String, serde_json::Value)]) -> Vec<Project> {
    let mut out: Vec<Project> = Vec::new();
    for (kind, at, data) in rows {
        let Some(folder) = data.get("folder").and_then(|v| v.as_str()) else {
            continue;
        };
        match kind.as_str() {
            GRANTED => {
                if !out.iter().any(|p| p.folder == folder) {
                    out.push(Project {
                        folder: folder.to_string(),
                        since: at.clone(),
                        destinations: Vec::new(),
                        sources: Vec::new(),
                        gh_hosts: Vec::new(),
                    });
                }
            }
            APPROVED | FETCH_APPROVED | GH_APPROVED => {
                let field = if kind == GH_APPROVED { "host" } else { "url" };
                let Some(value) = data.get(field).and_then(|v| v.as_str()) else {
                    continue;
                };
                if let Some(p) = out.iter_mut().find(|p| p.folder == folder) {
                    let list = match kind.as_str() {
                        APPROVED => &mut p.destinations,
                        FETCH_APPROVED => &mut p.sources,
                        _ => &mut p.gh_hosts,
                    };
                    if !list.iter().any(|d| d == value) {
                        list.push(value.to_string());
                    }
                }
            }
            REMOVED => out.retain(|p| p.folder != folder),
            _ => {}
        }
    }
    out
}

/// The journal's project records (Mac-side, on ferb-store's `Db`). Written only on the owner's Yes to the publish
/// question (the gateway) and the owner's Remove in FERB.app (an app-only method); never by a tool or a model.
#[cfg(target_os = "macos")]
pub mod store {
    use ferb_core::interfaces::{Clock, IdSource};
    use ferb_store::Db;
    use rusqlite::params;
    use serde_json::{json, Value};

    use super::{
        fold, Project, APPROVED, FETCH_APPROVED, FORCE_APPROVED, FORCE_USED, GH_APPROVED, GRANTED,
        REMOVED,
    };

    pub type Result<T> = std::result::Result<T, rusqlite::Error>;

    /// Every project that stands, oldest first.
    pub fn list(db: &Db) -> Result<Vec<Project>> {
        let mut stmt = db.conn().prepare(
            "SELECT kind, at, data_json FROM journal WHERE kind IN (?1, ?2, ?3, ?4, ?5) ORDER BY seq",
        )?;
        let rows = stmt
            .query_map(
                params![GRANTED, APPROVED, FETCH_APPROVED, GH_APPROVED, REMOVED],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let rows: Vec<(String, String, Value)> = rows
            .into_iter()
            .map(|(k, at, d)| (k, at, serde_json::from_str(&d).unwrap_or(Value::Null)))
            .collect();
        Ok(fold(&rows))
    }

    pub fn folders(db: &Db) -> Result<Vec<String>> {
        Ok(list(db)?.into_iter().map(|p| p.folder).collect())
    }

    /// Whether the owner approved pushing `folder`'s repository to exactly `url` (and hasn't removed the project).
    pub fn approved(db: &Db, folder: &str, url: &str) -> Result<bool> {
        Ok(list(db)?
            .iter()
            .any(|p| p.folder == folder && p.destinations.iter().any(|d| d == url)))
    }

    fn append(
        db: &Db,
        clock: &dyn Clock,
        ids: &dyn IdSource,
        kind: &str,
        data: Value,
    ) -> Result<()> {
        db.conn().execute(
            "INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, ?, ?, ?, 'owner')",
            params![
                ids.new_id(),
                ferb_core::when::isoformat(&clock.now()),
                kind,
                ferb_py::json::dumps_utf8(&data)
            ],
        )?;
        Ok(())
    }

    /// The owner's Yes to the publish question: the folder becomes a project (if it isn't one) and the address is
    /// approved for it.
    pub fn grant(
        db: &Db,
        clock: &dyn Clock,
        ids: &dyn IdSource,
        folder: &str,
        url: &str,
        task_id: &str,
    ) -> Result<()> {
        let standing = list(db)?;
        if !standing.iter().any(|p| p.folder == folder) {
            append(
                db,
                clock,
                ids,
                GRANTED,
                json!({"folder": folder, "task_id": task_id}),
            )?;
        }
        let has = standing
            .iter()
            .any(|p| p.folder == folder && p.destinations.iter().any(|d| d == url));
        if !has {
            append(
                db,
                clock,
                ids,
                APPROVED,
                json!({"folder": folder, "url": url, "task_id": task_id}),
            )?;
        }
        Ok(())
    }

    fn ensure_project(
        db: &Db,
        clock: &dyn Clock,
        ids: &dyn IdSource,
        folder: &str,
        task_id: &str,
    ) -> Result<Vec<Project>> {
        let standing = list(db)?;
        if !standing.iter().any(|p| p.folder == folder) {
            append(
                db,
                clock,
                ids,
                GRANTED,
                json!({"folder": folder, "task_id": task_id}),
            )?;
            return list(db);
        }
        Ok(standing)
    }

    /// HAND-642: the owner's Yes to getting from `url`: the folder becomes a project and the address is one FERB may
    /// get from for it.
    pub fn grant_fetch(
        db: &Db,
        clock: &dyn Clock,
        ids: &dyn IdSource,
        folder: &str,
        url: &str,
        task_id: &str,
    ) -> Result<()> {
        let standing = ensure_project(db, clock, ids, folder, task_id)?;
        if !standing
            .iter()
            .any(|p| p.folder == folder && p.gets_from(url))
        {
            append(
                db,
                clock,
                ids,
                FETCH_APPROVED,
                json!({"folder": folder, "url": url, "task_id": task_id}),
            )?;
        }
        Ok(())
    }

    /// HAND-643: the owner's Yes to using the GitHub CLI's login for this project and host.
    pub fn grant_gh(
        db: &Db,
        clock: &dyn Clock,
        ids: &dyn IdSource,
        folder: &str,
        host: &str,
        task_id: &str,
    ) -> Result<()> {
        let standing = ensure_project(db, clock, ids, folder, task_id)?;
        if !standing
            .iter()
            .any(|p| p.folder == folder && p.gh_hosts.iter().any(|h| h == host))
        {
            append(
                db,
                clock,
                ids,
                GH_APPROVED,
                json!({"folder": folder, "host": host, "task_id": task_id}),
            )?;
        }
        Ok(())
    }

    /// HAND-641: the owner's Yes to one forced push: what it may replace, for this task's next push of exactly this
    /// commit to exactly this branch and address.
    #[allow(clippy::too_many_arguments)]
    pub fn approve_force(
        db: &Db,
        clock: &dyn Clock,
        ids: &dyn IdSource,
        task_id: &str,
        folder: &str,
        url: &str,
        branch: &str,
        commit: &str,
        remote_commit: &str,
    ) -> Result<()> {
        append(
            db,
            clock,
            ids,
            FORCE_APPROVED,
            json!({"task_id": task_id, "folder": folder, "url": url, "branch": branch,
                   "commit": commit, "remote_commit": remote_commit}),
        )
    }

    /// HAND-641: takes the newest unused Yes for this push, if any: the remote commit the owner agreed to replace.
    /// Taking it uses it up (`project.force_used`): a later forced push asks again.
    #[allow(clippy::too_many_arguments)]
    pub fn take_force(
        db: &Db,
        clock: &dyn Clock,
        ids: &dyn IdSource,
        task_id: &str,
        folder: &str,
        url: &str,
        branch: &str,
        commit: &str,
    ) -> Result<Option<String>> {
        let mut stmt = db.conn().prepare(
            "SELECT id, kind, data_json FROM journal WHERE kind IN (?1, ?2) AND \
             json_extract(data_json, '$.task_id') = ?3 ORDER BY seq",
        )?;
        let rows = stmt
            .query_map(params![FORCE_APPROVED, FORCE_USED, task_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let used: Vec<String> = rows
            .iter()
            .filter(|(_, k, _)| k == FORCE_USED)
            .filter_map(|(_, _, d)| serde_json::from_str::<Value>(d).ok())
            .filter_map(|d| {
                d.get("approval")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect();
        let found = rows.iter().rev().find_map(|(id, k, d)| {
            if k != FORCE_APPROVED || used.contains(id) {
                return None;
            }
            let d: Value = serde_json::from_str(d).ok()?;
            let is = |f: &str, v: &str| d.get(f).and_then(Value::as_str) == Some(v);
            (is("folder", folder) && is("url", url) && is("branch", branch) && is("commit", commit))
                .then(|| {
                    (
                        id.clone(),
                        d.get("remote_commit")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
        });
        let Some((id, remote)) = found else {
            return Ok(None);
        };
        append(
            db,
            clock,
            ids,
            FORCE_USED,
            json!({"task_id": task_id, "approval": id}),
        )?;
        Ok(Some(remote))
    }

    /// The owner's Remove in Settings: false when no such project stands.
    pub fn remove(db: &Db, clock: &dyn Clock, ids: &dyn IdSource, folder: &str) -> Result<bool> {
        if !list(db)?.iter().any(|p| p.folder == folder) {
            return Ok(false);
        }
        append(db, clock, ids, REMOVED, json!({"folder": folder}))?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn within_is_by_whole_components() {
        assert!(within("/a/b", "/a/b"));
        assert!(within("/a/b/c", "/a/b"));
        assert!(!within("/a/bc", "/a/b"));
        assert!(within("/a/b/c", "/a/b/"));
    }

    // decision: HAND-622
    #[test]
    fn dev_tool_folders_only_for_a_task_in_a_project() {
        let projects = vec!["/u/code/site".to_string()];
        assert!(dev_tool_folders("/u", &["/u/Documents".into()], &projects).is_empty());
        let got = dev_tool_folders("/u", &["/u/code/site/web".into()], &projects);
        assert_eq!(got[0], "/u/go");
        assert_eq!(got.len(), DEV_TOOL_FOLDERS.len());
    }

    // decision: HAND-620
    #[test]
    fn the_fold_keeps_what_the_owner_left() {
        let r = |k: &str, at: &str, d: serde_json::Value| (k.to_string(), at.to_string(), d);
        let rows = vec![
            r(GRANTED, "t1", json!({"folder": "/p"})),
            r(
                APPROVED,
                "t1",
                json!({"folder": "/p", "url": "https://h/a.git"}),
            ),
            r(
                APPROVED,
                "t2",
                json!({"folder": "/p", "url": "https://h/a.git"}),
            ),
            r(GRANTED, "t3", json!({"folder": "/q"})),
            r(REMOVED, "t4", json!({"folder": "/p"})),
            r(GRANTED, "t5", json!({"folder": "/p"})),
        ];
        let got = fold(&rows);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].folder, "/q");
        // removed then granted again: the old approvals are gone
        assert_eq!(got[1].folder, "/p");
        assert_eq!(got[1].since, "t5");
        assert!(got[1].destinations.is_empty());
    }

    /// The 30-minute "don't ask" never answers the publish question: it sends the owner's files with their login.
    // decision: GATE-207, HAND-624, HAND-641, HAND-642, HAND-643
    #[test]
    fn the_publish_question_is_never_routine() {
        for rule in [PUBLISH_RULE, FORCE_RULE, FETCH_RULE, GH_RULE] {
            let n = crate::confirm::Need::new(rule, "k".into(), crate::confirm::Values::new());
            assert!(!crate::confirm::routine(&n), "{rule}");
        }
    }

    #[test]
    fn no_projects_no_fact_line() {
        assert_eq!(fact_line("/u", &[]), None);
        let l = fact_line("/u", &["/u/code/site".into()]).unwrap();
        assert!(l.contains("~/code/site"), "{l}");
    }
}
