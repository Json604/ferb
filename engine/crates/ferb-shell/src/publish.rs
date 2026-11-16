//! `project.publish` (HAND-623): FERB pushes a project folder's git repository with the owner's own git login, from
//! FERB's code outside the shell sandbox, with arguments code fixes. No model-written command line runs outside the
//! sandbox, and no model ever holds the login: git's credential helper (the Keychain's) or the SSH agent answers git
//! itself, inside this process tree.
//!
//! The push never reads anything a model-written command could have changed and that runs programs or picks the
//! destination:
//! - it runs from a scratch git folder FERB makes in its own state (closed to every command), which borrows the
//!   project's objects (`objects/info/alternates`) and holds FERB's own empty config: the project's `.git/config`,
//!   hooks and attributes are never read by the pushing git, so a planted `core.hooksPath`, pre-push hook,
//!   `core.sshCommand`, `credential.helper`, `url.*.insteadOf`, `remote.*.pushurl` or `receivepack`,
//!   `core.fsmonitor`, `protocol.*.allow`, `include.path` or `http.*` setting has no effect;
//! - `GIT_CONFIG_NOSYSTEM` and `GIT_CONFIG_GLOBAL=/dev/null`: the owner's ~/.gitconfig and ~/.config/git (hidden, so
//!   writable by commands, HAND-205) are not read either; the credential helper is FERB's fixed choice;
//! - a cleared environment (no GIT_*, SSH_*, PATH from the daemon) with PATH the system's own folders; the git
//!   program is Apple's, root-owned, in a folder the shell can't write (/Library/Developer, /Applications);
//! - the destination is the exact address git resolved in the project (insteadOf applied), shown to the owner and
//!   approved for this folder; only https and ssh addresses, never one with a password in it; redirects are not
//!   followed; the refspec is `<commit>:refs/heads/<branch>`, never forced, never more than one branch;
//! - for ssh, the owner's ~/.ssh/config may include only files in ~/.ssh or /etc/ssh (both closed to commands);
//!   BatchMode, so nothing prompts.
//!
//! What the project's own git says (its folder, branch, commit and push address) is read by git *inside* the shell
//! sandbox, so whatever the repository's config runs there is contained like any command.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::sandbox::{Options, Sandbox};

/// The folders the pushing git's PATH holds (system-owned).
pub const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";
/// The ssh FERB's push uses (system-owned) and its fixed options.
pub const SSH_COMMAND: &str = "/usr/bin/ssh -o BatchMode=yes";
/// The folders an Apple git may live in: both closed to the shell's writes (/Library and /Applications are on the
/// default never-write list).
pub const GIT_HOMES: [&str; 2] = ["/Library/Developer/", "/Applications/"];
const COMMAND_LINE_TOOLS_GIT: &str = "/Library/Developer/CommandLineTools/usr/bin/git";
/// The longest git output kept for the worker.
const OUTPUT_CHARS: usize = 4000;

/// Why a publish can't go ahead: a plain fact for the worker (nothing ran outside the sandbox).
pub type Refused = String;

/// What the worker asks for (fixed shape: nothing else is passed to git).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub folder: String,
    pub remote: String,
    pub branch: Option<String>,
    pub set_upstream: bool,
    /// HAND-641: replace the remote branch (with a lease on the commit the owner was shown).
    pub force: bool,
    /// HAND-643: use the GitHub CLI's saved login when the Keychain has none.
    pub gh: bool,
}

impl Ask {
    /// From the tool's arguments (already checked against its schema).
    pub fn from_args(args: &Map<String, Value>) -> Ask {
        let s = |k: &str| {
            args.get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        Ask {
            folder: s("folder").unwrap_or_default(),
            remote: s("remote").unwrap_or_else(|| "origin".into()),
            branch: s("branch"),
            set_upstream: flag(args, "set_upstream"),
            force: flag(args, "force"),
            gh: flag(args, "gh_login"),
        }
    }
}

/// A true boolean argument.
pub fn flag(args: &Map<String, Value>, k: &str) -> bool {
    args.get(k).and_then(Value::as_bool).unwrap_or(false)
}

/// HAND-641: what a forced push would replace, read from the remote branch just fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Force {
    pub remote_commit: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    pub date: String,
    /// Commits on the remote branch that the folder's branch doesn't have.
    pub dropped: u64,
}

/// A push worked out before any question (rule 5): what the project's git says, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The repository's top folder, resolved: the project folder.
    pub folder: String,
    pub common_dir: String,
    pub object_format: String,
    pub shallow: bool,
    pub remote: String,
    pub branch: String,
    pub commit: String,
    /// The exact address git pushes to (insteadOf applied).
    pub url: String,
    pub scheme: String,
    pub host: String,
    pub path: String,
    /// Set when the push replaces commits on the remote branch (asked for with `force`).
    pub force: Option<Force>,
    pub gh: bool,
}

impl Plan {
    pub fn to_json(&self) -> Map<String, Value> {
        let mut v = json!({
            "action": "push",
            "folder": self.folder,
            "remote": self.remote,
            "branch": self.branch,
            "commit": self.commit,
            "url": self.url,
            "host": self.host,
            "path": self.path,
        });
        if let Some(f) = &self.force {
            v["force"] = json!({
                "remote_commit": f.remote_commit, "short": f.short, "subject": f.subject,
                "author": f.author, "date": f.date, "dropped": f.dropped,
                "local_short": self.commit.chars().take(7).collect::<String>(),
            });
        }
        if self.gh {
            v["gh"] = json!(true);
        }
        match v {
            Value::Object(m) => m,
            _ => Map::new(),
        }
    }
}

/// Where a saved GitHub CLI login is read (HAND-643): the host in, the token out. Never printed, never in argv.
pub type GhLogin = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// How the program runs git and where it keeps its scratch folders.
pub struct Publisher {
    pub sandbox: Arc<Sandbox>,
    pub git: PathBuf,
    /// The owner's home folder (HOME for git and ssh).
    pub home: PathBuf,
    /// Where scratch git folders are made: inside FERB's own state, closed to every command.
    pub scratch_root: PathBuf,
    /// The one credential helper git may run (`osxkeychain`, the Keychain's, from git's own folder).
    pub credential_helper: String,
    /// Tests only: plain http to this Mac's loopback address (a local test server); never set by the daemon.
    pub allow_loopback_http: bool,
    pub inspect_timeout: Duration,
    pub push_timeout: Duration,
    /// Variables from the daemon's environment the push keeps (the SSH agent, the temporary folder, the user).
    pub pass_env: Vec<(String, String)>,
    /// The GitHub CLI's saved login for a host (HAND-643).
    pub gh_login: GhLogin,
}

/// The variables of the daemon's own environment a push keeps.
pub const PASSED_ENV: [&str; 4] = ["SSH_AUTH_SOCK", "TMPDIR", "USER", "LOGNAME"];

impl Publisher {
    /// The daemon's publisher: Apple's git, the Keychain's helper, the daemon's SSH agent.
    pub fn new(sandbox: Arc<Sandbox>, home: PathBuf, scratch_root: PathBuf) -> Publisher {
        let git = apple_git().unwrap_or_else(|| PathBuf::from(COMMAND_LINE_TOOLS_GIT));
        let pass_env = PASSED_ENV
            .iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
            .collect();
        let h = home.clone();
        Publisher {
            sandbox,
            git,
            gh_login: Arc::new(move |host: &str| gh_login(&h, host)),
            home,
            scratch_root,
            credential_helper: "osxkeychain".into(),
            allow_loopback_http: false,
            inspect_timeout: Duration::from_secs(20),
            push_timeout: Duration::from_secs(300),
            pass_env,
        }
    }

    /// Whether the git program is one FERB runs outside the sandbox: Apple's, root-owned, in a folder the shell can't
    /// write. A test publisher may name another.
    pub fn git_trusted(&self) -> bool {
        trusted_git(&self.git)
    }

    /// What the project's own git says about its top folder (inside the sandbox): (folder, git common folder, object
    /// format, shallow), its layout checked.
    pub async fn inspect(&self, folder: &str) -> Result<(String, String, String, bool), Refused> {
        if !folder.starts_with('/') {
            return Err("folder must be an absolute path (the repository's top folder)".into());
        }
        let real = std::fs::canonicalize(folder)
            .map_err(|_| format!("{folder} isn't a folder FERB can find"))?;
        if !real.is_dir() {
            return Err(format!("{folder} isn't a folder"));
        }
        let folder_s = text(&real)?;
        let out = self
            .git_inside(
                &folder_s,
                &[
                    "rev-parse",
                    "--path-format=absolute",
                    "--show-toplevel",
                    "--git-common-dir",
                    "--show-object-format",
                    "--is-shallow-repository",
                ],
                &[],
            )
            .await?;
        let lines: Vec<&str> = out.lines().collect();
        if lines.len() != 4 {
            return Err(format!(
                "{folder_s} isn't a git repository's working folder"
            ));
        }
        let top = canonical(lines[0])?;
        if top != folder_s {
            return Err(format!(
                "name the repository's top folder: {} (not {folder_s})",
                lines[0]
            ));
        }
        let common_dir = canonical(lines[1])?;
        if !within(&common_dir, &folder_s) {
            return Err(format!(
                "the repository's git folder ({common_dir}) is outside {folder_s}; FERB works only with a repository \
                 whose git folder is inside its project folder"
            ));
        }
        let object_format = lines[2].trim().to_string();
        if object_format != "sha1" && object_format != "sha256" {
            return Err(format!("unknown object format {object_format:?}"));
        }
        let shallow = lines[3].trim() == "true";
        layout(Path::new(&common_dir), &folder_s, shallow)?;
        Ok((folder_s, common_dir, object_format, shallow))
    }

    /// Works out the push (inside the sandbox) and checks the repository's layout. Nothing runs outside the sandbox,
    /// except for `force`: the remote branch is fetched first (with the owner's login, isolated as a push is) so the
    /// question can say what would be replaced.
    pub async fn prepare(&self, ask: &Ask) -> Result<Plan, Refused> {
        let mut plan = self.plan(ask).await?;
        if ask.force {
            plan.force = self.force_facts(&plan).await?;
        }
        Ok(plan)
    }

    /// The push without the remote's state: what `run` checks again against the owner's Yes.
    pub async fn plan(&self, ask: &Ask) -> Result<Plan, Refused> {
        if !valid_remote(&ask.remote) {
            return Err(format!(
                "{:?} isn't a remote name FERB pushes to (letters, digits, '.', '_' and '-')",
                ask.remote
            ));
        }
        if let Some(b) = &ask.branch {
            if !valid_branch(b) {
                return Err(format!("{b:?} isn't a branch name git accepts"));
            }
        }
        let (folder_s, common_dir, object_format, shallow) = self.inspect(&ask.folder).await?;
        let branch = match &ask.branch {
            Some(b) => b.clone(),
            None => {
                let head = self
                    .git_inside(&folder_s, &["symbolic-ref", "-q", "HEAD"], &[])
                    .await
                    .map_err(|_| {
                        "HEAD isn't on a branch here: say which branch to publish".to_string()
                    })?;
                let b = head
                    .trim()
                    .strip_prefix("refs/heads/")
                    .ok_or("HEAD isn't on a branch here: say which branch to publish")?
                    .to_string();
                if !valid_branch(&b) {
                    return Err(format!("{b:?} isn't a branch name FERB pushes"));
                }
                b
            }
        };
        let commit = self
            .git_inside(
                &folder_s,
                &[
                    "rev-parse",
                    "-q",
                    "--verify",
                    &format!("refs/heads/{branch}^{{commit}}"),
                ],
                &[],
            )
            .await
            .map_err(|_| format!("there is no branch {branch} with a commit here"))?
            .trim()
            .to_string();
        if !is_object_id(&commit, &object_format) {
            return Err(format!("git gave no commit for {branch}"));
        }
        let urls = self
            .git_inside(
                &folder_s,
                &["remote", "get-url", "--push", "--all", &ask.remote],
                &[],
            )
            .await
            .map_err(|_| format!("this repository has no remote named {}", ask.remote))?;
        let url = one_address(&urls, &ask.remote, "push")?;
        let (scheme, host, path) = address(&url, self.allow_loopback_http)?;
        if scheme == "ssh" {
            ssh_config_closed(&self.home)?;
        }
        Ok(Plan {
            folder: folder_s,
            common_dir,
            object_format,
            shallow,
            remote: ask.remote.clone(),
            branch,
            commit,
            url,
            scheme,
            host,
            path,
            force: None,
            gh: ask.gh,
        })
    }

    /// HAND-641: the remote branch fetched now, and what replacing it would drop. None when nothing would be dropped
    /// (the branch isn't there yet, or the folder's branch already holds it: an ordinary push).
    async fn force_facts(&self, plan: &Plan) -> Result<Option<Force>, Refused> {
        let refs = self
            .fetch_into(
                &plan.folder,
                &plan.common_dir,
                &plan.object_format,
                &plan.url,
                &plan.remote,
                Some(&plan.branch),
                plan.gh,
            )
            .await?;
        let Some((_, remote_commit)) = refs.into_iter().find(|(b, _)| *b == plan.branch) else {
            return Ok(None);
        };
        if remote_commit == plan.commit {
            return Ok(None);
        }
        let dropped: u64 = self
            .git_inside(
                &plan.folder,
                &[
                    "rev-list",
                    "--count",
                    &format!("{}..{remote_commit}", plan.commit),
                ],
                &[],
            )
            .await?
            .trim()
            .parse()
            .unwrap_or(0);
        if dropped == 0 {
            return Ok(None);
        }
        let shown = self
            .git_inside(
                &plan.folder,
                &[
                    "log",
                    "-1",
                    "--no-show-signature",
                    "--date=short",
                    "--format=%h%x00%s%x00%an%x00%ad",
                    &remote_commit,
                ],
                &[],
            )
            .await
            .unwrap_or_default();
        let mut parts = shown.trim_end_matches('\n').split('\0').map(clip);
        let short = parts
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| remote_commit.chars().take(7).collect());
        Ok(Some(Force {
            remote_commit,
            short,
            subject: parts.next().unwrap_or_default(),
            author: parts.next().unwrap_or_default(),
            date: parts.next().unwrap_or_default(),
            dropped,
        }))
    }

    /// git inside the shell sandbox, in `folder` (`folders`: the visible folders it may change); its standard
    /// output, or why it failed.
    pub(crate) async fn git_inside(
        &self,
        folder: &str,
        args: &[&str],
        folders: &[String],
    ) -> Result<String, Refused> {
        self.git_inside_with(folder, args, folders, None).await
    }

    /// `git_inside` with bytes on its standard input (a pack, a list of ref updates).
    pub(crate) async fn git_inside_with(
        &self,
        folder: &str,
        args: &[&str],
        folders: &[String],
        input: Option<Vec<u8>>,
    ) -> Result<String, Refused> {
        // hooks off here too: a reference-transaction hook would run (contained) on update-ref
        let mut argv = vec![
            text(&self.git)?,
            "-c".to_string(),
            "core.hooksPath=/dev/null".to_string(),
            "-C".to_string(),
            folder.to_string(),
        ];
        argv.extend(args.iter().map(|a| a.to_string()));
        let wrapped = self
            .sandbox
            .wrap(
                &argv,
                &Options {
                    folders,
                    ..Options::default()
                },
            )
            .map_err(|e| format!("the sandbox couldn't start git: {e}"))?;
        let profile = Sandbox::profile_path(&wrapped);
        let mut cmd = tokio::process::Command::new(&wrapped[0]);
        cmd.args(&wrapped[1..])
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", SYSTEM_PATH)
            .env("LC_ALL", "C")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_PAGER", "cat");
        let ran = run_with(cmd, self.push_timeout, input).await;
        Sandbox::cleanup(profile.as_deref());
        let (code, stdout, stderr) = ran?;
        if code != 0 {
            return Err(one_line(&stderr));
        }
        Ok(stdout)
    }

    /// The push itself, outside the sandbox, from a scratch git folder; then (inside the sandbox) the project's own
    /// record of the remote branch, and its upstream when asked. `lease`: the remote commit the owner agreed to
    /// replace (HAND-641): git refuses if the branch there is anything else now. The facts for the worker.
    pub async fn push(
        &self,
        plan: &Plan,
        set_upstream: bool,
        lease: Option<&str>,
    ) -> Result<Map<String, Value>, Refused> {
        if !self.git_trusted() {
            return Err(format!(
                "FERB runs only Apple's git outside its sandbox, and {} isn't it",
                self.git.display()
            ));
        }
        if let Some(l) = lease {
            if !is_object_id(l, &plan.object_format) {
                return Err("the remote commit to replace isn't a commit id".into());
            }
        }
        // the plan's checks again, just before git reads the objects
        layout(Path::new(&plan.common_dir), &plan.folder, plan.shallow)?;
        let scratch = self.scratch()?;
        let pushed = self.push_from(&scratch, plan, lease).await;
        let _ = std::fs::remove_dir_all(&scratch);
        let (code, stdout, stderr) = pushed?;
        let (flag, summary) = porcelain(&stdout, &plan.branch);
        let allowed: &[&str] = if lease.is_some() {
            &[" ", "*", "=", "+"]
        } else {
            &[" ", "*", "="]
        };
        let ok = code == 0 && flag.as_deref().is_some_and(|f| allowed.contains(&f));
        let mut facts = plan.to_json();
        facts.remove("action");
        facts.insert("pushed".into(), json!(ok));
        facts.insert("result".into(), json!(summary));
        facts.insert("forced".into(), json!(ok && lease.is_some()));
        let said = cut(&strip_userinfo(&format!("{stdout}{stderr}")));
        facts.insert("git_output".into(), json!(said));
        if ok {
            let tracking = format!("refs/remotes/{}/{}", plan.remote, plan.branch);
            let updated = self
                .git_inside(&plan.folder, &["update-ref", &tracking, &plan.commit], &[])
                .await
                .is_ok();
            facts.insert("remote_branch_recorded".into(), json!(updated));
            if set_upstream {
                let up = format!("--set-upstream-to={}/{}", plan.remote, plan.branch);
                let set = self
                    .git_inside(&plan.folder, &["branch", &up, &plan.branch], &[])
                    .await
                    .is_ok();
                facts.insert("upstream_set".into(), json!(set));
            }
        } else if lease.is_some() && summary.contains("stale info") {
            facts.insert(
                "why".into(),
                json!(
                    "the branch there changed after the owner's Yes, so FERB didn't overwrite it"
                ),
            );
        }
        Ok(facts)
    }

    pub(crate) fn scratch(&self) -> Result<PathBuf, Refused> {
        use std::os::unix::fs::DirBuilderExt;
        let io = |e: std::io::Error| format!("FERB couldn't make its scratch folder: {e}");
        std::fs::create_dir_all(&self.scratch_root).map_err(io)?;
        let dir = self
            .scratch_root
            .join(format!("publish-{}", crate::token_hex(8)));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(io)?;
        Ok(dir)
    }

    fn clean_env(&self, cmd: &mut tokio::process::Command) {
        cmd.env_clear();
        for (k, v) in &self.pass_env {
            cmd.env(k, v);
        }
        cmd.env("HOME", &self.home)
            .env("PATH", SYSTEM_PATH)
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_SSH_COMMAND", SSH_COMMAND)
            .env("SSH_ASKPASS_REQUIRE", "never")
            .env("GIT_PAGER", "cat");
    }

    /// A bare scratch git folder in `scratch` (FERB's own config, no templates), borrowing `objects` when given.
    pub(crate) async fn scratch_repo(
        &self,
        scratch: &Path,
        object_format: &str,
        common_dir: Option<&str>,
        shallow: bool,
    ) -> Result<PathBuf, Refused> {
        let git_dir = scratch.join("git");
        let mut init = tokio::process::Command::new(&self.git);
        self.clean_env(&mut init);
        init.arg("init")
            .arg("--bare")
            .arg("-q")
            .arg("--template=")
            .arg(format!("--object-format={object_format}"))
            .arg(&git_dir);
        let (code, _, err) = run(init, self.inspect_timeout).await?;
        if code != 0 {
            return Err(format!(
                "FERB couldn't make its scratch git folder: {}",
                one_line(&err)
            ));
        }
        if let Some(common) = common_dir {
            let objects = Path::new(common).join("objects");
            let info = git_dir.join("objects/info");
            std::fs::create_dir_all(&info).map_err(|e| e.to_string())?;
            std::fs::write(info.join("alternates"), format!("{}\n", text(&objects)?))
                .map_err(|e| e.to_string())?;
            if shallow {
                let bytes = std::fs::read(Path::new(common).join("shallow"))
                    .map_err(|e| format!("the repository's shallow list couldn't be read: {e}"))?;
                std::fs::write(git_dir.join("shallow"), bytes).map_err(|e| e.to_string())?;
            }
        }
        Ok(git_dir)
    }

    /// The fixed settings every git that uses the owner's login runs with: no hooks, no program a repository or
    /// a command could name, the one credential helper (and, with `gh`, the GitHub CLI's login in a FERB-private file
    /// that git's own store helper reads), https and ssh only, no redirects, objects checked on the way in.
    fn isolation(&self, scratch: &Path, url: &str, gh: bool) -> Result<Vec<String>, Refused> {
        let mut configs: Vec<String> = vec![
            "core.hooksPath=/dev/null".into(),
            "core.fsmonitor=false".into(),
            "core.askPass=".into(),
            "credential.helper=".into(),
            format!("credential.helper={}", self.credential_helper),
            "credential.interactive=false".into(),
            "http.followRedirects=false".into(),
            "push.recurseSubmodules=no".into(),
            "push.followTags=false".into(),
            "push.gpgSign=false".into(),
            "fetch.recurseSubmodules=no".into(),
            "transfer.fsckObjects=true".into(),
            "transfer.unpackLimit=1".into(),
            "fetch.unpackLimit=1".into(),
            "gc.auto=0".into(),
            "maintenance.auto=false".into(),
            "protocol.allow=never".into(),
            "protocol.https.allow=always".into(),
            "protocol.ssh.allow=always".into(),
        ];
        if self.allow_loopback_http {
            configs.push("protocol.http.allow=always".into());
        }
        if gh {
            let (scheme, authority) = scheme_authority(url);
            if scheme != "https" && !(self.allow_loopback_http && scheme == "http") {
                return Err("the GitHub CLI's login works only for https addresses".into());
            }
            let host = authority.split(':').next().unwrap_or_default();
            let token = (self.gh_login)(host).ok_or_else(|| {
                format!("the GitHub CLI has no saved login for {host} that FERB can read")
            })?;
            if token.len() < 8
                || token.len() > 512
                || !token
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
            {
                return Err(
                    "the GitHub CLI's saved login isn't in a form FERB can hand to git".into(),
                );
            }
            let file = scratch.join("gh-login");
            write_private(
                &file,
                format!("{scheme}://x-access-token:{token}@{authority}\n").as_bytes(),
            )?;
            configs.push(format!("credential.helper=store --file={}", text(&file)?));
        }
        Ok(configs)
    }

    /// git with the owner's login (isolated: clean environment, FERB's fixed settings), on the scratch `git_dir`.
    pub(crate) async fn git_net(
        &self,
        scratch: &Path,
        git_dir: Option<&Path>,
        url: &str,
        gh: bool,
        args: &[&str],
    ) -> Result<(i32, String, String), Refused> {
        let mut cmd = tokio::process::Command::new(&self.git);
        self.clean_env(&mut cmd);
        if let Some(d) = git_dir {
            cmd.arg(format!("--git-dir={}", text(d)?));
        }
        for c in self.isolation(scratch, url, gh)? {
            cmd.arg("-c").arg(c);
        }
        cmd.args(args);
        run(cmd, self.push_timeout).await
    }

    /// git on FERB's scratch folder alone (no network, no login).
    async fn git_scratch(
        &self,
        git_dir: &Path,
        args: &[&str],
        input: Option<Vec<u8>>,
    ) -> Result<String, Refused> {
        let mut cmd = tokio::process::Command::new(&self.git);
        self.clean_env(&mut cmd);
        cmd.arg(format!("--git-dir={}", text(git_dir)?))
            .arg("-c")
            .arg("core.hooksPath=/dev/null")
            .args(args);
        let (code, out, err) = run_with(cmd, self.inspect_timeout, input).await?;
        if code != 0 {
            return Err(one_line(&err));
        }
        Ok(out)
    }

    async fn push_from(
        &self,
        scratch: &Path,
        plan: &Plan,
        lease: Option<&str>,
    ) -> Result<(i32, String, String), Refused> {
        let git_dir = self
            .scratch_repo(
                scratch,
                &plan.object_format,
                Some(&plan.common_dir),
                plan.shallow,
            )
            .await?;
        let refspec = format!("{}:refs/heads/{}", plan.commit, plan.branch);
        let lease_arg = lease.map(|l| format!("--force-with-lease=refs/heads/{}:{l}", plan.branch));
        let mut args: Vec<&str> = vec!["push", "--porcelain", "--no-verify"];
        if let Some(l) = &lease_arg {
            args.push(l);
        }
        args.push(&plan.url);
        args.push(&refspec);
        self.git_net(scratch, Some(&git_dir), &plan.url, plan.gh, &args)
            .await
    }

    /// HAND-642: the remote's branches (or one) fetched with the owner's login into the project: the network
    /// transfer runs in a scratch git folder (FERB's config only, the project's objects borrowed so only what's new
    /// comes down, objects checked); the packs it got are handed to `git index-pack` and the remote branches'
    /// records to `git update-ref`, both inside the sandbox. Nothing of the repository's runs with the login.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn fetch_into(
        &self,
        folder: &str,
        common_dir: &str,
        object_format: &str,
        url: &str,
        remote: &str,
        branch: Option<&str>,
        gh: bool,
    ) -> Result<Vec<(String, String)>, Refused> {
        if !self.git_trusted() {
            return Err(format!(
                "FERB runs only Apple's git outside its sandbox, and {} isn't it",
                self.git.display()
            ));
        }
        layout(Path::new(common_dir), folder, false)?;
        let scratch = self.scratch()?;
        let got = self
            .fetch_in(
                &scratch,
                folder,
                common_dir,
                object_format,
                url,
                remote,
                branch,
                gh,
            )
            .await;
        let _ = std::fs::remove_dir_all(&scratch);
        got
    }

    #[allow(clippy::too_many_arguments)]
    async fn fetch_in(
        &self,
        scratch: &Path,
        folder: &str,
        common_dir: &str,
        object_format: &str,
        url: &str,
        remote: &str,
        branch: Option<&str>,
        gh: bool,
    ) -> Result<Vec<(String, String)>, Refused> {
        let git_dir = self
            .scratch_repo(scratch, object_format, Some(common_dir), false)
            .await?;
        // what the project already has, so only what's new comes down
        let haves = self
            .git_inside(folder, &["for-each-ref", "--format=%(objectname)"], &[])
            .await
            .unwrap_or_default();
        let mut updates = String::new();
        for (i, id) in haves
            .lines()
            .map(str::trim)
            .filter(|id| is_object_id(id, object_format))
            .take(10_000)
            .enumerate()
        {
            updates.push_str(&format!("create refs/haves/{i} {id}\n"));
        }
        if !updates.is_empty() {
            let _ = self
                .git_scratch(
                    &git_dir,
                    &["update-ref", "--stdin"],
                    Some(updates.into_bytes()),
                )
                .await;
        }
        let spec = match branch {
            Some(b) => format!("+refs/heads/{b}:refs/remotes/{remote}/{b}"),
            None => format!("+refs/heads/*:refs/remotes/{remote}/*"),
        };
        let (code, _, err) = self
            .git_net(
                scratch,
                Some(&git_dir),
                url,
                gh,
                &[
                    "fetch",
                    "--no-tags",
                    "--no-recurse-submodules",
                    "--no-write-fetch-head",
                    url,
                    &spec,
                ],
            )
            .await?;
        if code != 0 {
            if branch.is_some() && err.contains("couldn't find remote ref") {
                return Ok(Vec::new());
            }
            return Err(format!(
                "git couldn't get {}: {}",
                strip_userinfo(url),
                one_line(&strip_userinfo(&err))
            ));
        }
        // the packs it got, into the project's objects, inside the sandbox
        if let Ok(entries) = std::fs::read_dir(git_dir.join("objects/pack")) {
            let mut packs: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "pack"))
                .collect();
            packs.sort();
            for pack in packs {
                let bytes = std::fs::read(&pack).map_err(|e| e.to_string())?;
                self.git_inside_with(folder, &["index-pack", "--stdin"], &[], Some(bytes))
                    .await
                    .map_err(|e| {
                        format!("the objects FERB got couldn't be stored in the project: {e}")
                    })?;
            }
        }
        let listed = self
            .git_scratch(
                &git_dir,
                &[
                    "for-each-ref",
                    "--format=%(objectname) %(refname)",
                    &format!("refs/remotes/{remote}/"),
                ],
                None,
            )
            .await?;
        let prefix = format!("refs/remotes/{remote}/");
        let mut got = Vec::new();
        let mut updates = String::new();
        for line in listed.lines() {
            let Some((id, name)) = line.split_once(' ') else {
                continue;
            };
            let Some(b) = name.strip_prefix(&prefix) else {
                continue;
            };
            if !is_object_id(id, object_format) || !valid_branch(b) {
                continue;
            }
            updates.push_str(&format!("update {name} {id}\n"));
            got.push((b.to_string(), id.to_string()));
        }
        if !updates.is_empty() {
            self.git_inside_with(
                folder,
                &["update-ref", "--stdin"],
                &[],
                Some(updates.into_bytes()),
            )
            .await
            .map_err(|e| {
                format!("the project's record of the remote branches couldn't be written: {e}")
            })?;
        }
        Ok(got)
    }
}

/// The tool's result: the facts, an error when `done` (pushed, fetched) isn't true.
pub fn result(facts: Map<String, Value>, done: &str) -> ferb_core::types::ToolResult {
    let ok = facts.get(done) == Some(&Value::Bool(true));
    ferb_core::types::ToolResult {
        content: facts,
        is_error: !ok,
        ..Default::default()
    }
}

/// Runs a command with no input, its output captured; past `timeout` its process group is killed.
async fn run(
    cmd: tokio::process::Command,
    timeout: Duration,
) -> Result<(i32, String, String), Refused> {
    run_with(cmd, timeout, None).await
}

/// `run`, with `input` written to its standard input when given.
async fn run_with(
    mut cmd: tokio::process::Command,
    timeout: Duration,
    input: Option<Vec<u8>>,
) -> Result<(i32, String, String), Refused> {
    cmd.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true)
    .process_group(0);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("git couldn't start: {e}"))?;
    if let (Some(bytes), Some(mut stdin)) = (input, child.stdin.take()) {
        use tokio::io::AsyncWriteExt;
        tokio::spawn(async move {
            let _ = stdin.write_all(&bytes).await;
            let _ = stdin.shutdown().await;
        });
    }
    let pid = child.id();
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(out)) => Ok((
            crate::exit_code(out.status),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )),
        Ok(Err(e)) => Err(format!("git failed: {e}")),
        Err(_) => {
            if let Some(pid) = pid {
                // SAFETY: kill(2) on the process group this call started (process_group(0) made it its leader).
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
            Err(format!(
                "git didn't finish within {} seconds and was stopped",
                timeout.as_secs()
            ))
        }
    }
}

fn text(p: &Path) -> Result<String, Refused> {
    p.to_str()
        .map(str::to_string)
        .ok_or_else(|| "a path here isn't UTF-8 text".to_string())
}

fn canonical(p: &str) -> Result<String, Refused> {
    let c = std::fs::canonicalize(p.trim()).map_err(|_| format!("{} can't be found", p.trim()))?;
    text(&c)
}

fn within(path: &str, folder: &str) -> bool {
    let folder = folder.trim_end_matches('/');
    path == folder || path.starts_with(&format!("{folder}/"))
}

fn one_line(s: &str) -> String {
    let l = s
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("git failed");
    l.chars().take(300).collect()
}

fn cut(s: &str) -> String {
    let n = s.chars().count();
    if n <= OUTPUT_CHARS {
        s.to_string()
    } else {
        s.chars().skip(n - OUTPUT_CHARS).collect()
    }
}

/// A short line of git's output for a question: no control characters, at most 120 characters.
fn clip(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect::<String>()
        .trim()
        .to_string()
}

/// A full object id of the repository's format.
pub fn is_object_id(id: &str, object_format: &str) -> bool {
    let len = if object_format == "sha256" { 64 } else { 40 };
    id.len() == len && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The one address git gives for a remote (`kind`: push or fetch).
fn one_address(out: &str, remote: &str, kind: &str) -> Result<String, Refused> {
    let urls: Vec<&str> = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if urls.len() != 1 {
        return Err(format!(
            "the remote {remote} has {} {kind} addresses; FERB uses exactly one",
            urls.len()
        ));
    }
    Ok(urls[0].to_string())
}

/// (scheme, host[:port]) of an address already checked by [`address`].
fn scheme_authority(url: &str) -> (String, String) {
    match url.split_once("://") {
        Some((s, rest)) => {
            let authority = rest.split('/').next().unwrap_or_default();
            let hostport = authority.rsplit('@').next().unwrap_or_default();
            (s.to_ascii_lowercase(), hostport.to_ascii_lowercase())
        }
        None => ("ssh".into(), String::new()),
    }
}

/// A new owner-only file (it may hold a login: inside FERB's scratch folder, removed with it).
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), Refused> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("FERB couldn't hand the login to git: {e}"))?;
    f.write_all(bytes)
        .map_err(|e| format!("FERB couldn't hand the login to git: {e}"))
}

/// HAND-643: the GitHub CLI's saved login for `host`, read by FERB's own code, never by running `gh` (Homebrew's gh
/// is unsigned and in a folder FERB's commands can write, so running it with the login would run whatever a command
/// put there). Where gh keeps it: the login Keychain, item `gh:<host>`, read with Apple's /usr/bin/security (gh's
/// keyring library writes it that way, plain or as `go-keyring-base64:` / `go-keyring-encoded:`), else the
/// `oauth_token` of `<host>` in ~/.config/gh/hosts.yml (gh without a keyring).
pub fn gh_login(home: &Path, host: &str) -> Option<String> {
    keychain_gh(host).or_else(|| gh_hosts_yml(home, host))
}

fn keychain_gh(host: &str) -> Option<String> {
    if host.is_empty()
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
    {
        return None;
    }
    let out = std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", &format!("gh:{host}"), "-w"])
        .env_clear()
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8(out.stdout).ok()?.trim().to_string();
    decode_keyring(&raw)
}

/// gh's keyring values: plain, `go-keyring-base64:<base64>` or `go-keyring-encoded:<hex>`.
pub fn decode_keyring(raw: &str) -> Option<String> {
    use base64::Engine;
    let text = if let Some(b) = raw.strip_prefix("go-keyring-base64:") {
        String::from_utf8(base64::engine::general_purpose::STANDARD.decode(b).ok()?).ok()?
    } else if let Some(h) = raw.strip_prefix("go-keyring-encoded:") {
        String::from_utf8(hex::decode(h).ok()?).ok()?
    } else {
        raw.to_string()
    };
    let t = text.trim().to_string();
    (!t.is_empty()).then_some(t)
}

/// `oauth_token` under `<host>:` in gh's hosts.yml.
pub fn gh_hosts_yml(home: &Path, host: &str) -> Option<String> {
    let body = std::fs::read_to_string(home.join(".config/gh/hosts.yml")).ok()?;
    let mut inside = false;
    for line in body.lines() {
        if !line.starts_with([' ', '\t']) && !line.trim().is_empty() {
            inside = line.trim_end().trim_end_matches(':') == host;
            continue;
        }
        if inside {
            if let Some(v) = line.trim().strip_prefix("oauth_token:") {
                let v = v.trim().trim_matches(['"', '\'']);
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// The objects the push may read are the project's own: the objects folder is inside the project and no link, it
/// borrows from no other repository (alternates), and its packs and info files are no links (a link could hand the
/// unsandboxed git another repository's objects that commands can't read).
// decision: HAND-623
pub fn layout(common_dir: &Path, folder: &str, shallow: bool) -> Result<(), Refused> {
    let objects = common_dir.join("objects");
    let meta = std::fs::symlink_metadata(&objects)
        .map_err(|_| "the repository has no objects folder".to_string())?;
    if !meta.is_dir() {
        return Err("the repository's objects folder is a link or not a folder".into());
    }
    let real = canonical(&text(&objects)?)?;
    if !within(&real, folder) {
        return Err("the repository's objects are outside its project folder".into());
    }
    for alt in ["alternates", "http-alternates"] {
        if let Ok(bytes) = std::fs::read(objects.join("info").join(alt)) {
            if bytes.iter().any(|b| !b.is_ascii_whitespace()) {
                return Err(
                    "the repository borrows objects from another folder (objects/info/alternates); FERB publishes \
                     only a repository with its own objects (git repack -a -d, then remove that file)"
                        .into(),
                );
            }
        }
    }
    for sub in ["pack", "info"] {
        if let Ok(entries) = std::fs::read_dir(objects.join(sub)) {
            for e in entries.flatten() {
                if e.file_type().map(|t| t.is_symlink()).unwrap_or(true) {
                    return Err(format!(
                        "objects/{sub}/{} is a link; FERB publishes only a repository whose objects are its own files",
                        e.file_name().to_string_lossy()
                    ));
                }
            }
        }
    }
    if shallow {
        let m = std::fs::symlink_metadata(common_dir.join("shallow"))
            .map_err(|_| "the repository's shallow list is missing".to_string())?;
        if !m.is_file() {
            return Err("the repository's shallow list is a link".into());
        }
    }
    Ok(())
}

/// A remote name FERB passes to git: letters, digits, '.', '_', '-', not starting with '-' or '.'.
pub fn valid_remote(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        && !name.starts_with(['-', '.'])
}

/// git's rules for a branch name (`git check-ref-format --branch`), stricter where a name could read as an option or
/// a refspec: no leading '-', no ':', '+', '^', '~', '?', '*', '[', '\\', space or control character, no "..", no
/// "@{", no component starting with '.' or ending in ".lock", no empty component, not "@".
pub fn valid_branch(name: &str) -> bool {
    if name.is_empty()
        || name.len() > 250
        || name == "@"
        || name.starts_with('-')
        || name.starts_with('/')
        || name.ends_with('/')
        || name.ends_with('.')
        || name.contains("..")
        || name.contains("@{")
        || name.contains("//")
    {
        return false;
    }
    if name
        .chars()
        .any(|c| c.is_control() || c == ' ' || ":+^~?*[\\".contains(c))
    {
        return false;
    }
    name.split('/')
        .all(|c| !c.is_empty() && !c.starts_with('.') && !c.ends_with(".lock"))
}

/// (scheme, host, path) of a push address FERB pushes to: https (or ssh, scp-like included). Refused: any other
/// transport (a local path, file://, git://, ext::, a `<helper>::` address), a password inside the address, an empty
/// or odd host. `loopback_http`: tests only, plain http to 127.0.0.1.
// decision: HAND-623
pub fn address(url: &str, loopback_http: bool) -> Result<(String, String, String), Refused> {
    let bad = |why: &str| format!("FERB doesn't publish to {}: {why}", strip_userinfo(url));
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(bad("the address holds spaces or control characters"));
    }
    let (scheme, rest) = if let Some((s, r)) = url.split_once("://") {
        (s.to_ascii_lowercase(), r.to_string())
    } else if url.contains("::") {
        return Err(bad("it names a transport helper"));
    } else if let Some((userhost, path)) = url.split_once(':') {
        // scp-like ssh: [user@]host:path (no slash before the colon)
        if userhost.contains('/') || userhost.is_empty() || path.is_empty() {
            return Err(bad(
                "it is a local path; FERB publishes only to https or ssh addresses",
            ));
        }
        ("ssh".to_string(), format!("{userhost}/{path}"))
    } else {
        return Err(bad(
            "it is a local path; FERB publishes only to https or ssh addresses",
        ));
    };
    let allowed = scheme == "https" || scheme == "ssh" || (loopback_http && scheme == "http");
    if !allowed {
        return Err(bad("FERB publishes only to https or ssh addresses"));
    }
    let (authority, path) = rest.split_once('/').unwrap_or((rest.as_str(), ""));
    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, authority),
    };
    if userinfo.is_some_and(|u| u.contains(':')) {
        return Err(bad(
            "the address holds a password; FERB pushes with your saved git login instead",
        ));
    }
    let host = if let Some(h) = hostport.strip_prefix('[') {
        h.split_once(']').map(|(h, _)| h).unwrap_or("")
    } else {
        hostport.split(':').next().unwrap_or("")
    };
    let host = host.to_ascii_lowercase();
    if host.is_empty()
        || host.starts_with('-')
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-:".contains(&b))
    {
        return Err(bad("its host isn't a plain host name"));
    }
    if scheme == "http" && host != "127.0.0.1" && host != "localhost" && host != "::1" {
        return Err(bad("FERB publishes only to https or ssh addresses"));
    }
    if path.trim_matches('/').is_empty() {
        return Err(bad("it names no repository"));
    }
    Ok((scheme, host, path.to_string()))
}

/// Text with any `user:password@` inside an address cut to `user@` (git's own output can echo an address).
pub fn strip_userinfo(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let (head, tail) = rest.split_at(at + 3);
        out.push_str(head);
        let end = tail
            .find(|c: char| c.is_whitespace() || c == '/' || c == '\'' || c == '"')
            .unwrap_or(tail.len());
        let authority = &tail[..end];
        match authority.rsplit_once('@') {
            Some((user, host)) => {
                let user = user.split(':').next().unwrap_or("");
                out.push_str(user);
                out.push('@');
                out.push_str(host);
            }
            None => out.push_str(authority),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// The status of the one ref pushed, from `git push --porcelain`: (flag, summary).
fn porcelain(stdout: &str, branch: &str) -> (Option<String>, String) {
    let want = format!(":refs/heads/{branch}");
    for line in stdout.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(flag), Some(refs), summary) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        if flag.chars().count() == 1 && refs.ends_with(&want) {
            return (Some(flag.to_string()), summary.unwrap_or("").to_string());
        }
    }
    (None, String::new())
}

/// The git program FERB runs: Apple's (xcode-select's folder, else the Command Line Tools'), when trusted.
pub fn apple_git() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(out) = std::process::Command::new("/usr/bin/xcode-select")
        .arg("-p")
        .env_clear()
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    {
        let dev = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !dev.is_empty() {
            candidates.push(PathBuf::from(dev).join("usr/bin/git"));
        }
    }
    candidates.push(PathBuf::from(COMMAND_LINE_TOOLS_GIT));
    candidates.into_iter().find(|c| trusted_git(c))
}

/// Root-owned, not writable by group or others, in one of [`GIT_HOMES`].
pub fn trusted_git(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(real) = std::fs::canonicalize(path) else {
        return false;
    };
    let Some(s) = real.to_str() else {
        return false;
    };
    if !GIT_HOMES.iter().any(|h| s.starts_with(h)) {
        return false;
    }
    std::fs::metadata(&real)
        .map(|m| m.is_file() && m.uid() == 0 && m.mode() & 0o022 == 0)
        .unwrap_or(false)
}

/// For an ssh push: the owner's ~/.ssh/config (and the files it includes) may include only files in ~/.ssh or
/// /etc/ssh, both closed to commands. A file elsewhere (a hidden tool folder, writable by commands) could run a
/// program through ProxyCommand or Match exec when FERB's push reads it, outside the sandbox.
// decision: HAND-623
pub fn ssh_config_closed(home: &Path) -> Result<(), Refused> {
    let ssh_dir = home.join(".ssh");
    let mut queue = vec![ssh_dir.join("config")];
    let mut seen: Vec<PathBuf> = Vec::new();
    while let Some(file) = queue.pop() {
        if seen.contains(&file) || seen.len() > 64 {
            continue;
        }
        seen.push(file.clone());
        let Ok(body) = std::fs::read_to_string(&file) else {
            continue;
        };
        for line in body.lines() {
            let line = line.trim();
            let (key, rest) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
                Some((k, r)) => (
                    k,
                    r.trim_start_matches(|c: char| c.is_whitespace() || c == '='),
                ),
                None => continue,
            };
            if !key.eq_ignore_ascii_case("include") {
                continue;
            }
            for pattern in rest.split_whitespace() {
                let pattern = pattern.trim_matches('"');
                let path = if let Some(r) = pattern.strip_prefix("~/") {
                    home.join(r)
                } else if pattern.starts_with('/') {
                    PathBuf::from(pattern)
                } else {
                    ssh_dir.join(pattern)
                };
                let shown = path.display().to_string();
                let closed = path.starts_with(&ssh_dir) || path.starts_with("/etc/ssh");
                if !closed {
                    return Err(format!(
                        "your SSH settings (~/.ssh/config) include {shown}, which FERB's commands can change; FERB \
                         doesn't push over ssh while they do (an https address works, or move that file into ~/.ssh)"
                    ));
                }
                if path.starts_with(&ssh_dir) {
                    expand(&path, &mut queue)?;
                }
            }
        }
    }
    Ok(())
}

/// The files an Include pattern inside ~/.ssh names: wildcards only in its last part.
fn expand(path: &Path, queue: &mut Vec<PathBuf>) -> Result<(), Refused> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let parent = path.parent().unwrap_or(Path::new("/"));
    if parent.to_string_lossy().contains(['*', '?', '[']) {
        return Err(format!(
            "your SSH settings include {} with a wildcard in a folder name; FERB doesn't push over ssh with it",
            path.display()
        ));
    }
    if !name.contains(['*', '?', '[']) {
        queue.push(path.to_path_buf());
        return Ok(());
    }
    if let Ok(entries) = std::fs::read_dir(parent) {
        for e in entries.flatten() {
            if glob(name, &e.file_name().to_string_lossy()) {
                queue.push(e.path());
            }
        }
    }
    Ok(())
}

/// A shell-style match with '*' and '?' ('[' matched as itself: ssh's sets are rare and only widen what is read).
fn glob(pattern: &str, name: &str) -> bool {
    fn go(p: &[char], n: &[char]) -> bool {
        match p.first() {
            None => n.is_empty(),
            Some('*') => (0..=n.len()).any(|i| go(&p[1..], &n[i..])),
            Some('?') => !n.is_empty() && go(&p[1..], &n[1..]),
            Some(c) => n.first() == Some(c) && go(&p[1..], &n[1..]),
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    go(&p, &n)
}

#[cfg(test)]
mod tests {
    use super::*;

    // decision: HAND-623
    #[test]
    fn only_https_and_ssh_addresses() {
        assert_eq!(
            address("https://github.com/o/r.git", false).unwrap(),
            ("https".into(), "github.com".into(), "o/r.git".into())
        );
        assert_eq!(
            address("git@github.com:o/r.git", false).unwrap(),
            ("ssh".into(), "github.com".into(), "o/r.git".into())
        );
        assert_eq!(address("ssh://git@host:2222/o/r", false).unwrap().1, "host");
        for refused in [
            "/tmp/r.git",
            "./r",
            "file:///tmp/r.git",
            "git://host/r",
            "ext::sh -c touch% /tmp/x",
            "fd::3",
            "http://host/r.git",
            "https://user:token@github.com/o/r.git",
            "https://github.com/",
            "https://-oProxyCommand=x/r",
        ] {
            assert!(address(refused, false).is_err(), "{refused}");
        }
        assert!(address("http://127.0.0.1:9/r.git", true).is_ok());
        assert!(address("http://example.com/r.git", true).is_err());
    }

    #[test]
    fn names_that_could_read_as_options_or_refspecs_are_refused() {
        for ok in ["main", "feature/x-1", "release_2.0"] {
            assert!(valid_branch(ok), "{ok}");
        }
        for bad in [
            "-f", "+main", "a:b", "a..b", "a b", "x.lock", ".hidden", "a//b", "@", "a@{1}", "",
        ] {
            assert!(!valid_branch(bad), "{bad}");
        }
        assert!(valid_remote("origin"));
        for bad in ["-o", "a b", "a/b", ".x", ""] {
            assert!(!valid_remote(bad), "{bad}");
        }
    }

    #[test]
    fn passwords_never_leave_in_output() {
        assert_eq!(
            strip_userinfo("To https://u:secret@h/x.git\n"),
            "To https://u@h/x.git\n"
        );
        assert_eq!(strip_userinfo("no address"), "no address");
    }

    #[test]
    fn porcelain_reads_the_one_ref() {
        let out = "To https://h/r.git\n \tabc:refs/heads/main\taaa..bbb\nDone\n";
        assert_eq!(
            porcelain(out, "main"),
            (Some(" ".into()), "aaa..bbb".into())
        );
        let rej = "To x\n!\tabc:refs/heads/main\t[rejected] (non-fast-forward)\nDone\n";
        assert_eq!(porcelain(rej, "main").0.as_deref(), Some("!"));
    }
}
