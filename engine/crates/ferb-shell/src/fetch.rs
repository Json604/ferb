//! `project.fetch` (HAND-642): FERB gets a repository's branches with the owner's own git login, into a project folder
//! or a fresh clone folder the owner named, the same way `project.publish` pushes (ferb-shell `publish`): the network
//! transfer runs from a scratch git folder in FERB's own state with FERB's fixed settings, the owner's login answers
//! git itself, and nothing a repository or a command can configure runs with it.
//!
//! After the transfer, everything that touches the folder runs as an ordinary command inside the sandbox, without the
//! login: the packs go in through `git index-pack`, the remote branches through `git update-ref`, a clone's first
//! checkout through `git checkout`. So a repository's filters, merge drivers or hooks run only in the sandbox. A
//! merge or rebase after a fetch is the worker's own sandboxed command.
//!
//! Submodules are their own repositories: they are not fetched with the parent; each is reported with its address,
//! and the worker gets one with `project.fetch` and `clone_from` (FERB asks the owner about its address the same way).

use std::path::Path;

use serde_json::{json, Map, Value};

use crate::publish::{
    address, flag, is_object_id, ssh_config_closed, strip_userinfo, valid_branch, valid_remote,
    Publisher, Refused,
};

/// What the worker asks for (fixed shape).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchAsk {
    pub folder: String,
    pub remote: String,
    /// A clone: the address to get, into `folder` (new, or an empty folder).
    pub clone_from: Option<String>,
    pub gh: bool,
}

impl FetchAsk {
    pub fn from_args(args: &Map<String, Value>) -> FetchAsk {
        let s = |k: &str| {
            args.get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        FetchAsk {
            folder: s("folder").unwrap_or_default(),
            remote: s("remote").unwrap_or_else(|| "origin".into()),
            clone_from: s("clone_from"),
            gh: flag(args, "gh_login"),
        }
    }
}

/// A fetch worked out before any question: nothing ran outside the sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchPlan {
    pub folder: String,
    /// The repository's git common folder (None for a clone: made after the owner's Yes).
    pub common_dir: Option<String>,
    pub object_format: String,
    pub remote: String,
    pub url: String,
    pub scheme: String,
    pub host: String,
    pub path: String,
    pub clone: bool,
    pub gh: bool,
    /// The submodules the repository names: (name, address).
    pub submodules: Vec<(String, String)>,
}

impl FetchPlan {
    pub fn to_json(&self) -> Map<String, Value> {
        let mut v = json!({
            "action": if self.clone { "clone" } else { "fetch" },
            "folder": self.folder,
            "remote": self.remote,
            "url": self.url,
            "host": self.host,
            "path": self.path,
        });
        if self.gh {
            v["gh"] = json!(true);
        }
        match v {
            Value::Object(m) => m,
            _ => Map::new(),
        }
    }
}

const SUBMODULES_WHY: &str = "a submodule is its own repository: FERB doesn't get it with this one. Get it with \
project.fetch clone_from=<its address> folder=<its folder> (FERB asks the owner about that address).";

impl Publisher {
    /// Works out a fetch or a clone (inside the sandbox where it reads the project).
    pub async fn prepare_fetch(&self, ask: &FetchAsk) -> Result<FetchPlan, Refused> {
        if !valid_remote(&ask.remote) {
            return Err(format!("{:?} isn't a remote name FERB uses", ask.remote));
        }
        if let Some(url) = &ask.clone_from {
            return self.prepare_clone(ask, url).await;
        }
        let (folder, common_dir, object_format, _) = self.inspect(&ask.folder).await?;
        let urls = self
            .git_inside(&folder, &["remote", "get-url", "--all", &ask.remote], &[])
            .await
            .map_err(|_| format!("this repository has no remote named {}", ask.remote))?;
        let urls: Vec<&str> = urls
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        if urls.len() != 1 {
            return Err(format!(
                "the remote {} has {} addresses; FERB gets from exactly one",
                ask.remote,
                urls.len()
            ));
        }
        let url = urls[0].to_string();
        let (scheme, host, path) = address(&url, self.allow_loopback_http)?;
        if scheme == "ssh" {
            ssh_config_closed(&self.home)?;
        }
        let submodules = self.submodules(&folder).await;
        Ok(FetchPlan {
            folder,
            common_dir: Some(common_dir),
            object_format,
            remote: ask.remote.clone(),
            url,
            scheme,
            host,
            path,
            clone: false,
            gh: ask.gh,
            submodules,
        })
    }

    async fn prepare_clone(&self, ask: &FetchAsk, url: &str) -> Result<FetchPlan, Refused> {
        let (scheme, host, path) = address(url, self.allow_loopback_http)?;
        if scheme == "ssh" {
            ssh_config_closed(&self.home)?;
        }
        let target = Path::new(&ask.folder);
        if !ask.folder.starts_with('/') {
            return Err("folder must be an absolute path (the folder to clone into)".into());
        }
        let name = target
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| !n.is_empty() && *n != "." && *n != "..")
            .ok_or("folder must name the new folder to clone into")?;
        let parent = target
            .parent()
            .and_then(|p| std::fs::canonicalize(p).ok())
            .ok_or_else(|| format!("the folder {} would go in doesn't exist", ask.folder))?;
        let folder = parent.join(name);
        if let Ok(meta) = std::fs::symlink_metadata(&folder) {
            let empty =
                meta.is_dir() && std::fs::read_dir(&folder).is_ok_and(|mut d| d.next().is_none());
            if !empty {
                return Err(format!(
                    "{} already exists and isn't an empty folder; FERB clones only into a new or empty folder",
                    folder.display()
                ));
            }
        }
        let folder = folder
            .to_str()
            .ok_or("a path here isn't UTF-8 text")?
            .to_string();
        Ok(FetchPlan {
            folder,
            common_dir: None,
            object_format: "sha1".into(),
            remote: ask.remote.clone(),
            url: url.to_string(),
            scheme,
            host,
            path,
            clone: true,
            gh: ask.gh,
            submodules: Vec::new(),
        })
    }

    /// The submodules `.gitmodules` names in the working folder: (name, address); read inside the sandbox.
    async fn submodules(&self, folder: &str) -> Vec<(String, String)> {
        if !Path::new(folder).join(".gitmodules").is_file() {
            return Vec::new();
        }
        let out = self
            .git_inside(
                folder,
                &[
                    "config",
                    "-f",
                    ".gitmodules",
                    "--get-regexp",
                    r"^submodule\..*\.url$",
                ],
                &[],
            )
            .await
            .unwrap_or_default();
        out.lines()
            .filter_map(|l| l.split_once(' '))
            .filter_map(|(k, v)| {
                let name = k.strip_prefix("submodule.")?.strip_suffix(".url")?;
                Some((name.to_string(), strip_userinfo(v.trim())))
            })
            .take(50)
            .collect()
    }

    /// The fetch (or clone) itself; the facts for the worker.
    pub async fn fetch(&self, plan: &FetchPlan) -> Result<Map<String, Value>, Refused> {
        let mut facts = plan.to_json();
        facts.remove("action");
        let (common_dir, object_format) = if plan.clone {
            self.clone_start(plan).await?
        } else {
            (
                plan.common_dir.clone().unwrap_or_default(),
                plan.object_format.clone(),
            )
        };
        let refs = self
            .fetch_into(
                &plan.folder,
                &common_dir,
                &object_format,
                &plan.url,
                &plan.remote,
                None,
                plan.gh,
            )
            .await?;
        facts.insert("fetched".into(), json!(true));
        facts.insert(
            "branches".into(),
            Value::Array(
                refs.iter()
                    .map(|(b, c)| json!({"branch": format!("{}/{b}", plan.remote), "commit": c}))
                    .collect(),
            ),
        );
        if plan.clone {
            let checked = self.clone_finish(plan, &refs).await;
            facts.insert("checked_out".into(), json!(checked));
        } else {
            facts.insert(
                "next".into(),
                json!("merge or rebase with shell.run (for example git merge origin/main): it runs in the sandbox, \
                       without the owner's login"),
            );
        }
        let subs = if plan.clone {
            self.submodules(&plan.folder).await
        } else {
            plan.submodules.clone()
        };
        if !subs.is_empty() {
            facts.insert(
                "submodules_not_fetched".into(),
                json!({
                    "why": SUBMODULES_WHY,
                    "submodules": subs.iter().map(|(n, u)| json!({"name": n, "url": u})).collect::<Vec<_>>(),
                }),
            );
        }
        Ok(facts)
    }

    /// A clone's start: the remote's default branch and object format read (with the login, isolated), then an
    /// empty repository made in the folder and its remote recorded, both inside the sandbox.
    async fn clone_start(&self, plan: &FetchPlan) -> Result<(String, String), Refused> {
        let scratch = self.scratch()?;
        let listed = self
            .git_net(
                &scratch,
                None,
                &plan.url,
                plan.gh,
                &["ls-remote", "--symref", &plan.url, "HEAD"],
            )
            .await;
        let _ = std::fs::remove_dir_all(&scratch);
        let (code, out, err) = listed?;
        if code != 0 {
            return Err(format!(
                "git couldn't reach {}: {}",
                strip_userinfo(&plan.url),
                strip_userinfo(err.lines().next().unwrap_or("git failed"))
            ));
        }
        let (default, format) = head_of(&out);
        let parent = Path::new(&plan.folder)
            .parent()
            .and_then(|p| p.to_str())
            .unwrap_or("/")
            .to_string();
        let folders = vec![plan.folder.clone()];
        let branch_arg = format!("--initial-branch={}", default.as_deref().unwrap_or("main"));
        let format_arg = format!("--object-format={format}");
        self.git_inside(
            &parent,
            &["init", "-q", &format_arg, &branch_arg, &plan.folder],
            &folders,
        )
        .await
        .map_err(|e| format!("FERB couldn't make the folder's repository: {e}"))?;
        self.git_inside(
            &plan.folder,
            &["remote", "add", &plan.remote, &plan.url],
            &folders,
        )
        .await?;
        if let Some(d) = default {
            let _ = self
                .git_inside(&plan.folder, &["config", "ferb.cloneDefault", &d], &folders)
                .await;
        }
        Ok((format!("{}/.git", plan.folder), format))
    }

    /// A clone's first checkout of the remote's default branch, inside the sandbox (its filters run there).
    async fn clone_finish(&self, plan: &FetchPlan, refs: &[(String, String)]) -> bool {
        let folders = vec![plan.folder.clone()];
        let default = self
            .git_inside(&plan.folder, &["config", "ferb.cloneDefault"], &folders)
            .await
            .unwrap_or_default()
            .trim()
            .to_string();
        let _ = self
            .git_inside(
                &plan.folder,
                &["config", "--unset", "ferb.cloneDefault"],
                &folders,
            )
            .await;
        if default.is_empty() || !refs.iter().any(|(b, _)| *b == default) {
            return false;
        }
        let track = format!("{}/{default}", plan.remote);
        self.git_inside(
            &plan.folder,
            &["checkout", "-q", "-B", &default, "--track", &track],
            &folders,
        )
        .await
        .is_ok()
    }
}

/// `ls-remote --symref <url> HEAD`: (the default branch, the object format by the ids' length).
fn head_of(out: &str) -> (Option<String>, String) {
    let mut default = None;
    let mut format = "sha1".to_string();
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("ref: refs/heads/") {
            let b = rest.split('\t').next().unwrap_or_default();
            if valid_branch(b) {
                default = Some(b.to_string());
            }
        } else if let Some((id, _)) = line.split_once('\t') {
            if is_object_id(id, "sha256") {
                format = "sha256".into();
            }
        }
    }
    (default, format)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_branch_and_format_come_from_ls_remote() {
        let out = "ref: refs/heads/trunk\tHEAD\n0123456789012345678901234567890123456789\tHEAD\n";
        assert_eq!(head_of(out), (Some("trunk".into()), "sha1".into()));
        assert_eq!(head_of(""), (None, "sha1".into()));
        let bad = "ref: refs/heads/-x\tHEAD\n";
        assert_eq!(head_of(bad).0, None);
    }
}
