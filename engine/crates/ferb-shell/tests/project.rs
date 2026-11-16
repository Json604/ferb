//! Project mode (HAND-620..HAND-623) under the real sandbox-exec, in a temporary home with injected protection lists:
//! - a project folder is readable and writable by commands, a visible folder the task doesn't name is not, and the
//!   toolchain folders (~/go) open only for a task that works in a project;
//! - protected entries inside a project stay closed; the hidden caches stay writable and the credentials files in
//!   them closed;
//! - `project.publish`'s push reaches a local git server that asks for a login, with a credential helper whose file
//!   sits in a never-read folder: the sandboxed shell can't read it, the unsandboxed push can;
//! - every hostile setting a command can plant (hooks, core.hooksPath, core.sshCommand, credential.helper,
//!   core.fsmonitor, receivepack, include.path, the same in ~/.gitconfig) runs nothing outside the sandbox; an
//!   insteadOf or pushurl redirect is what the plan shows and exactly where the push goes; ext::, file and local
//!   addresses are refused; nothing is forced.
//!
//! Skipped, with a message, where sandbox-exec or Apple's git is missing. Network: 127.0.0.1 only.

#![cfg(target_os = "macos")]

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_shell::fetch::FetchAsk;
use ferb_shell::protection::{Entry, LocalPort, Protection};
use ferb_shell::publish::{self, Ask, Publisher};
use ferb_shell::pyos::{dedupe, realpath};
use ferb_shell::sandbox::{Sandbox, SANDBOX_EXEC};
use serde_json::Value;

fn skip() -> bool {
    if !Path::new(SANDBOX_EXEC).exists() {
        eprintln!("skipped: {SANDBOX_EXEC} is not available here");
        return true;
    }
    if publish::apple_git().is_none() {
        eprintln!("skipped: Apple's git is not installed here");
        return true;
    }
    false
}

struct Lists {
    home: PathBuf,
    ferb: PathBuf,
    workspace: PathBuf,
    uploads: PathBuf,
    reads: Vec<Entry>,
    writes: Vec<Entry>,
}

fn entries(home: &Path, kind: &str, items: &[&str]) -> Vec<Entry> {
    items
        .iter()
        .map(|i| {
            let typed = home.join(i.trim_start_matches("~/"));
            Entry {
                id: format!("{kind}:{i}"),
                path: realpath(&typed),
                typed,
            }
        })
        .collect()
}

impl Lists {
    fn new(home: &Path, reads: &[&str], writes: &[&str]) -> Arc<Lists> {
        let ferb = realpath(&home.join(".ferb"));
        Arc::new(Lists {
            home: home.to_path_buf(),
            workspace: ferb.join("workspace"),
            uploads: ferb.join("workspace").join("uploads"),
            ferb,
            reads: entries(home, "read", reads),
            writes: entries(home, "write", writes),
        })
    }
}

impl Protection for Lists {
    fn home(&self) -> &Path {
        &self.home
    }
    fn ferb_home(&self) -> &Path {
        &self.ferb
    }
    fn workspace(&self) -> &Path {
        &self.workspace
    }
    fn uploads(&self) -> &Path {
        &self.uploads
    }
    fn reads(&self) -> &[Entry] {
        &self.reads
    }
    fn writes(&self) -> &[Entry] {
        &self.writes
    }
    fn app_entries(&self) -> Vec<PathBuf> {
        vec![]
    }
    fn service_files(&self) -> Vec<PathBuf> {
        vec![]
    }
    fn guarded_parents(&self) -> Vec<PathBuf> {
        let mut out: BTreeSet<PathBuf> = BTreeSet::new();
        for e in self.reads.iter().chain(&self.writes) {
            for p in [&e.path, &e.typed] {
                out.extend(
                    p.ancestors()
                        .skip(1)
                        .filter(|a| a.parent().is_some())
                        .map(Path::to_path_buf),
                );
            }
        }
        out.into_iter().collect()
    }
    fn forms_now(&self, path: &Path) -> Vec<PathBuf> {
        dedupe([path.to_path_buf(), realpath(path)])
    }
    fn app_data(&self, _: &Path) -> bool {
        false
    }
    fn local_ports(&self) -> &[LocalPort] {
        &[]
    }
    fn rules(&self) -> Vec<Value> {
        vec![]
    }
}

struct Home {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    sandbox: Arc<Sandbox>,
}

const NEVER_READ: [&str; 5] = [
    "~/Secrets",
    "~/.cargo/credentials.toml",
    "~/.npmrc",
    "~/.ssh",
    "~/code/site/private",
];
const NEVER_WRITE: [&str; 2] = ["~/Locked", "~/code/site/vendor-locked"];

fn home() -> Home {
    let tmp = tempfile::Builder::new()
        .prefix("ferbproject")
        .tempdir_in("/tmp")
        .unwrap();
    let root = realpath(tmp.path());
    let home = root.join("home");
    for d in [
        ".ferb/workspace",
        "Documents",
        "Secrets",
        "Locked",
        "code/site/private",
        "code/site/vendor-locked",
        ".cargo/registry",
        ".ssh",
    ] {
        std::fs::create_dir_all(home.join(d)).unwrap();
    }
    std::fs::write(home.join("code/site/private/key.txt"), "k").unwrap();
    std::fs::write(home.join(".cargo/credentials.toml"), "token = \"x\"").unwrap();
    std::fs::write(home.join(".npmrc"), "//registry/:_authToken=x").unwrap();
    let lists = Lists::new(&home, &NEVER_READ, &NEVER_WRITE);
    let sandbox = Arc::new(
        Sandbox::new(lists, root.join("profiles"), Arc::new(|| None), None)
            .with_scan(Arc::new(|| Some(Default::default()))),
    );
    Home {
        _tmp: tmp,
        root,
        home,
        sandbox,
    }
}

impl Home {
    /// A command under the shell's profile with these write folders: (exit code, stderr).
    fn sh(&self, command: &str, folders: &[String]) -> (i32, String) {
        let argv = self.sandbox.argv(command, None, folders).unwrap();
        let out = Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(&self.home)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        Sandbox::cleanup(Sandbox::profile_path(&argv).as_deref());
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn p(&self, rel: &str) -> String {
        self.home.join(rel).to_string_lossy().into_owned()
    }

    fn publisher(&self, helper: &str) -> Publisher {
        let mut p = Publisher::new(
            self.sandbox.clone(),
            self.home.clone(),
            self.root.join("home/.ferb/run/publish"),
        );
        p.credential_helper = helper.to_string();
        p.allow_loopback_http = true;
        p.pass_env = vec![];
        p.push_timeout = Duration::from_secs(60);
        p
    }
}

// decision: HAND-620, HAND-622
#[test]
fn a_project_folder_opens_and_a_visible_folder_it_doesnt_name_stays_closed() {
    if skip() {
        return;
    }
    let h = home();
    let site = h.p("code/site");
    let projects = vec![site.clone()];
    let mut folders = vec![site.clone()];
    folders.extend(ferb_policy::projects::dev_tool_folders(
        &h.home.to_string_lossy(),
        &folders,
        &projects,
    ));
    let (code, err) = h.sh(
        "echo hi > code/site/index.html && cat code/site/index.html && rm code/site/index.html && \
         mkdir -p go/pkg/mod && touch go/pkg/mod/x",
        &folders,
    );
    assert_eq!(code, 0, "{err}");
    let (code, _) = h.sh("touch Documents/x", &folders);
    assert_ne!(
        code, 0,
        "a visible folder the task doesn't name stays closed"
    );
    // the same task without a project: ~/go is a visible folder it doesn't name (HAND-203, HAND-215)
    let plain = vec![h.p("Documents")];
    assert!(
        ferb_policy::projects::dev_tool_folders(&h.home.to_string_lossy(), &plain, &projects)
            .is_empty()
    );
    let (code, _) = h.sh("mkdir -p go/other", &plain);
    assert_ne!(code, 0, "~/go stays closed outside a project");
}

// decision: HAND-621
#[test]
fn protected_entries_inside_a_project_stay_closed() {
    if skip() {
        return;
    }
    let h = home();
    let folders = vec![h.p("code/site")];
    let (code, _) = h.sh("cat code/site/private/key.txt", &folders);
    assert_ne!(code, 0, "a never-read entry inside the project is not read");
    let (code, _) = h.sh("touch code/site/private/new", &folders);
    assert_ne!(
        code, 0,
        "a never-read entry inside the project is not written"
    );
    let (code, _) = h.sh("touch code/site/vendor-locked/x", &folders);
    assert_ne!(
        code, 0,
        "a never-write entry inside the project is not written"
    );
    let (code, err) = h.sh("touch code/site/ok", &folders);
    assert_eq!(code, 0, "{err}");
}

// decision: HAND-622
#[test]
fn hidden_caches_stay_writable_and_their_credentials_closed() {
    if skip() {
        return;
    }
    let h = home();
    let (code, err) = h.sh(
        "touch .cargo/registry/x && mkdir -p .npm/_cacache Library/Caches/pip && touch .npm/_cacache/y",
        &[],
    );
    assert_eq!(code, 0, "{err}");
    for f in [".cargo/credentials.toml", ".npmrc"] {
        let (code, _) = h.sh(&format!("cat {f}"), &[]);
        assert_ne!(code, 0, "{f} stays closed");
    }
}

// ---------- a local git server that asks for a login ----------

struct Server {
    port: u16,
    /// Each request's path, as served.
    seen: Arc<Mutex<Vec<String>>>,
}

/// Smart HTTP over `git http-backend` (CGI), Basic auth `tester:s3cret` required for every request.
fn serve(root: &Path) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (root, s) = (root.to_path_buf(), seen.clone());
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let (root, s) = (root.clone(), s.clone());
            std::thread::spawn(move || {
                let _ = handle(conn, &root, &s);
            });
        }
    });
    Server { port, seen }
}

fn handle(conn: TcpStream, root: &Path, seen: &Mutex<Vec<String>>) -> std::io::Result<()> {
    conn.set_read_timeout(Some(Duration::from_secs(20)))?;
    let mut reader = BufReader::new(conn.try_clone()?);
    let mut out = conn;
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let mut headers: Vec<(String, String)> = Vec::new();
    loop {
        let mut h = String::new();
        reader.read_line(&mut h)?;
        let h = h.trim_end().to_string();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    let get = |k: &str| headers.iter().find(|(h, _)| h == k).map(|(_, v)| v.clone());
    // "tester:s3cret"
    // "tester:s3cret", or the GitHub CLI's login handed to git as x-access-token:<token>
    let auth = get("authorization").unwrap_or_default();
    if auth != "Basic dGVzdGVyOnMzY3JldA==" && auth != gh_basic() {
        out.write_all(
            b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"t\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )?;
        return Ok(());
    }
    seen.lock().unwrap().push(target.clone());
    if get("expect").is_some_and(|e| e.eq_ignore_ascii_case("100-continue")) {
        out.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
    }
    let mut body = Vec::new();
    if let Some(n) = get("content-length").and_then(|n| n.parse::<usize>().ok()) {
        body.resize(n, 0);
        reader.read_exact(&mut body)?;
    } else if get("transfer-encoding").is_some_and(|t| t.contains("chunked")) {
        loop {
            let mut size = String::new();
            reader.read_line(&mut size)?;
            let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
            if n == 0 {
                let mut end = String::new();
                reader.read_line(&mut end)?;
                break;
            }
            let mut chunk = vec![0u8; n + 2];
            reader.read_exact(&mut chunk)?;
            body.extend_from_slice(&chunk[..n]);
        }
    }
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let mut cgi = Command::new("/usr/bin/git");
    cgi.arg("http-backend")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", root)
        .env("GIT_PROJECT_ROOT", root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("REQUEST_METHOD", &method)
        .env("PATH_INFO", path)
        .env("QUERY_STRING", query)
        .env("REMOTE_USER", "tester")
        .env("REMOTE_ADDR", "127.0.0.1")
        .env("CONTENT_LENGTH", body.len().to_string())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped());
    if let Some(t) = get("content-type") {
        cgi.env("CONTENT_TYPE", t);
    }
    if let Some(e) = get("content-encoding") {
        cgi.env("HTTP_CONTENT_ENCODING", e);
    }
    let mut child = cgi.spawn()?;
    child.stdin.take().unwrap().write_all(&body)?;
    let result = child.wait_with_output()?;
    let raw = result.stdout;
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| (i, 4))
        .or_else(|| raw.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2)));
    let (head, rest) = match split {
        Some((i, n)) => (
            String::from_utf8_lossy(&raw[..i]).into_owned(),
            &raw[i + n..],
        ),
        None => (String::new(), &raw[..]),
    };
    let mut status = "200 OK".to_string();
    let mut lines = Vec::new();
    for l in head.lines() {
        if let Some(s) = l.strip_prefix("Status:") {
            status = s.trim().to_string();
        } else if !l.trim().is_empty() {
            lines.push(l.trim().to_string());
        }
    }
    let mut resp = format!("HTTP/1.1 {status}\r\n");
    for l in lines {
        resp.push_str(&l);
        resp.push_str("\r\n");
    }
    resp.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        rest.len()
    ));
    out.write_all(resp.as_bytes())?;
    out.write_all(rest)?;
    Ok(())
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("/usr/bin/git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Repo {
    h: Home,
    server: Server,
    served: PathBuf,
    site: PathBuf,
    /// The credential helper's file, in a never-read folder.
    creds: PathBuf,
    /// Where a hostile setting would write if it ran outside the sandbox (a never-write folder: the sandbox can't).
    marker: PathBuf,
}

fn repo() -> Repo {
    let h = home();
    let served = h.root.join("served");
    std::fs::create_dir_all(&served).unwrap();
    git(&served, &["init", "-q", "--bare", "-b", "main", "site.git"]);
    git(
        &served,
        &["init", "-q", "--bare", "-b", "main", "other.git"],
    );
    let server = serve(&served);
    let site = h.home.join("code/site");
    git(&site, &["init", "-q", "-b", "main"]);
    std::fs::write(site.join("index.html"), "<h1>hi</h1>").unwrap();
    git(&site, &["add", "index.html"]);
    git(&site, &["commit", "-q", "-m", "first"]);
    let url = format!("http://127.0.0.1:{}/site.git", server.port);
    git(&site, &["remote", "add", "origin", &url]);
    let creds = h.home.join("Secrets/git-credentials");
    std::fs::write(
        &creds,
        format!("http://tester:s3cret@127.0.0.1:{}\n", server.port),
    )
    .unwrap();
    let marker = h.home.join("Locked/ran");
    Repo {
        h,
        server,
        served,
        site,
        creds,
        marker,
    }
}

impl Repo {
    fn helper(&self) -> String {
        format!("store --file={}", self.creds.display())
    }

    fn ask(&self) -> Ask {
        Ask {
            folder: self.site.to_string_lossy().into_owned(),
            remote: "origin".into(),
            branch: None,
            set_upstream: true,
            force: false,
            gh: false,
        }
    }

    fn served_head(&self, repo: &str) -> Option<String> {
        let out = Command::new("/usr/bin/git")
            .args(["rev-parse", "-q", "--verify", "refs/heads/main"])
            .current_dir(self.served.join(repo))
            .output()
            .unwrap();
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

// decision: HAND-623
#[test]
fn publish_pushes_with_a_login_the_sandboxed_shell_cannot_read() {
    if skip() {
        return;
    }
    let r = repo();
    // the shell can't read the login (a never-read folder)
    let (code, _) = r.h.sh(&format!("cat {}", r.creds.display()), &[]);
    assert_ne!(code, 0, "the sandboxed shell read the credential file");
    let p = r.h.publisher(&r.helper());
    let plan = block_on(p.prepare(&r.ask())).unwrap();
    assert_eq!(plan.branch, "main");
    assert_eq!(plan.host, "127.0.0.1");
    assert_eq!(
        plan.url,
        format!("http://127.0.0.1:{}/site.git", r.server.port)
    );
    let head = git(&r.site, &["rev-parse", "HEAD"]);
    assert_eq!(plan.commit, head);
    let facts = block_on(p.push(&plan, true, None)).unwrap();
    assert_eq!(facts["pushed"], true, "{facts:?}");
    assert_eq!(facts["forced"], false);
    assert_eq!(facts["result"], "[new branch]");
    assert_eq!(facts["upstream_set"], true, "{facts:?}");
    assert_eq!(r.served_head("site.git").as_deref(), Some(head.as_str()));
    // the project's own record of the remote branch
    assert_eq!(git(&r.site, &["rev-parse", "origin/main"]), head);
    assert_eq!(git(&r.site, &["config", "branch.main.remote"]), "origin");
    // FERB's scratch folder is gone
    let left: Vec<_> = std::fs::read_dir(r.h.root.join("home/.ferb/run/publish"))
        .map(|d| d.flatten().collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn without_the_login_the_push_fails_and_never_prompts() {
    if skip() {
        return;
    }
    let r = repo();
    let p = r.h.publisher("");
    let plan = block_on(p.prepare(&r.ask())).unwrap();
    let facts = block_on(p.push(&plan, false, None)).unwrap();
    assert_eq!(facts["pushed"], false, "{facts:?}");
    assert!(r.served_head("site.git").is_none());
}

// decision: HAND-623
#[test]
fn nothing_is_forced_a_diverged_branch_is_rejected() {
    if skip() {
        return;
    }
    let r = repo();
    let p = r.h.publisher(&r.helper());
    let plan = block_on(p.prepare(&r.ask())).unwrap();
    assert_eq!(
        block_on(p.push(&plan, false, None)).unwrap()["pushed"],
        true
    );
    let first = r.served_head("site.git").unwrap();
    git(&r.site, &["commit", "-q", "--amend", "-m", "rewritten"]);
    let plan = block_on(p.prepare(&r.ask())).unwrap();
    let facts = block_on(p.push(&plan, false, None)).unwrap();
    assert_eq!(facts["pushed"], false, "{facts:?}");
    assert!(
        facts["result"].as_str().unwrap().contains("rejected"),
        "{facts:?}"
    );
    assert_eq!(r.served_head("site.git").unwrap(), first);
}

/// A hook file that, if it ever runs, writes the marker (inside the sandbox it can't: a never-write folder).
fn hostile_script(r: &Repo, name: &str) -> PathBuf {
    let dir = r.site.join(".git/evil");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\necho {name} >> '{}'\nexit 0\n",
            r.marker.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

// decision: HAND-623
#[test]
fn hostile_settings_a_command_can_plant_run_nothing_outside_the_sandbox() {
    if skip() {
        return;
    }
    let r = repo();
    let hook = hostile_script(&r, "hook");
    // the repository's own hooks folder
    for name in [
        "pre-push",
        "reference-transaction",
        "post-update",
        "push-to-checkout",
    ] {
        std::fs::copy(&hook, r.site.join(".git/hooks").join(name)).unwrap();
    }
    let s = hook.display().to_string();
    let included = r.site.join(".git/evil/included");
    std::fs::write(
        &included,
        format!(
            "[core]\n\thooksPath = {}\n[credential]\n\thelper = !{s}\n",
            r.site.join(".git/evil").display()
        ),
    )
    .unwrap();
    for (k, v) in [
        (
            "core.hooksPath",
            r.site.join(".git/evil").display().to_string(),
        ),
        ("core.sshCommand", s.clone()),
        ("core.fsmonitor", s.clone()),
        ("core.askPass", s.clone()),
        ("credential.helper", format!("!{s}")),
        ("remote.origin.receivepack", s.clone()),
        ("remote.origin.uploadpack", s.clone()),
        ("core.gitProxy", s.clone()),
        ("gpg.program", s.clone()),
        ("push.gpgSign", "true".into()),
        ("include.path", included.display().to_string()),
        ("protocol.allow", "always".into()),
    ] {
        git(&r.site, &["config", "--add", k, &v]);
    }
    // the same in the owner's ~/.gitconfig, which commands can write (a hidden file)
    std::fs::write(
        r.h.home.join(".gitconfig"),
        format!(
            "[core]\n\thooksPath = {}\n\tsshCommand = {s}\n\tfsmonitor = {s}\n[credential]\n\thelper = !{s}\n\
             [trace2]\n\tnormalTarget = {}\n",
            r.site.join(".git/evil").display(),
            r.h.home.join("Locked/trace").display()
        ),
    )
    .unwrap();
    // the hook folder named by core.hooksPath gets the hooks too
    std::fs::copy(&hook, r.site.join(".git/evil/pre-push")).unwrap();
    std::fs::copy(&hook, r.site.join(".git/evil/reference-transaction")).unwrap();
    let p = r.h.publisher(&r.helper());
    let plan = block_on(p.prepare(&r.ask())).unwrap();
    let facts = block_on(p.push(&plan, true, None)).unwrap();
    assert_eq!(facts["pushed"], true, "{facts:?}");
    assert!(
        !r.marker.exists(),
        "a planted setting ran outside the sandbox: {}",
        std::fs::read_to_string(&r.marker).unwrap_or_default()
    );
    assert!(!r.h.home.join("Locked/trace").exists());
    assert_eq!(
        r.served_head("site.git"),
        Some(git(&r.site, &["rev-parse", "HEAD"]))
    );
    // the control: an ordinary push from the project, outside the sandbox, does run what was planted
    let ordinary = Command::new("/usr/bin/git")
        .args(["push", "-q", "origin", "main"])
        .current_dir(&r.site)
        .env_clear()
        .env("HOME", &r.h.home)
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    let _ = ordinary;
    assert!(
        r.marker.exists(),
        "the control: the planted settings would have run outside FERB's push"
    );
}

// decision: HAND-623, HAND-624
#[test]
fn a_redirect_a_command_planted_is_what_the_plan_shows_and_where_the_push_goes() {
    if skip() {
        return;
    }
    let r = repo();
    let base = format!("http://127.0.0.1:{}/", r.server.port);
    git(
        &r.site,
        &[
            "config",
            &format!("url.{base}other.git.pushInsteadOf"),
            &format!("{base}site.git"),
        ],
    );
    let p = r.h.publisher(&r.helper());
    let plan = block_on(p.prepare(&r.ask())).unwrap();
    assert_eq!(
        plan.url,
        format!("{base}other.git"),
        "the plan shows the rewritten address"
    );
    let facts = block_on(p.push(&plan, false, None)).unwrap();
    assert_eq!(facts["pushed"], true, "{facts:?}");
    assert!(r.served_head("other.git").is_some());
    assert!(r.served_head("site.git").is_none());
    assert!(r
        .server
        .seen
        .lock()
        .unwrap()
        .iter()
        .all(|s| s.starts_with("/other.git/")));
    // a second push address: refused, nothing guessed
    git(
        &r.site,
        &[
            "config",
            "--add",
            "remote.origin.pushurl",
            &format!("{base}site.git"),
        ],
    );
    git(
        &r.site,
        &[
            "config",
            "--add",
            "remote.origin.pushurl",
            &format!("{base}other.git"),
        ],
    );
    let err = block_on(p.prepare(&r.ask())).unwrap_err();
    assert!(err.contains("2 push addresses"), "{err}");
}

// decision: HAND-623
#[test]
fn local_file_and_helper_addresses_are_refused_before_anything_runs() {
    if skip() {
        return;
    }
    let r = repo();
    let p = r.h.publisher(&r.helper());
    for url in [
        r.served.join("site.git").display().to_string(),
        format!("file://{}", r.served.join("site.git").display()),
        format!("ext::sh -c 'echo x >> {}'", r.marker.display()),
        "https://tester:s3cret@example.invalid/x.git".to_string(),
    ] {
        git(&r.site, &["remote", "set-url", "origin", &url]);
        let err = block_on(p.prepare(&r.ask())).unwrap_err();
        assert!(err.starts_with("FERB doesn't publish to"), "{url}: {err}");
        assert!(!err.contains("s3cret"), "{err}");
    }
    assert!(!r.marker.exists());
}

// decision: HAND-623
#[test]
fn a_repository_that_borrows_objects_or_links_them_is_refused() {
    if skip() {
        return;
    }
    let r = repo();
    let p = r.h.publisher(&r.helper());
    let alt = r.site.join(".git/objects/info/alternates");
    std::fs::write(&alt, format!("{}\n", r.h.home.join("Secrets").display())).unwrap();
    let err = block_on(p.prepare(&r.ask())).unwrap_err();
    assert!(err.contains("borrows objects"), "{err}");
    std::fs::remove_file(&alt).unwrap();
    std::os::unix::fs::symlink(
        r.h.home.join("Secrets/git-credentials"),
        r.site.join(".git/objects/pack/pack-x.pack"),
    )
    .unwrap();
    let err = block_on(p.prepare(&r.ask())).unwrap_err();
    assert!(err.contains("is a link"), "{err}");
}

#[test]
fn the_folder_must_be_the_repository_top() {
    if skip() {
        return;
    }
    let r = repo();
    std::fs::create_dir_all(r.site.join("web")).unwrap();
    let p = r.h.publisher(&r.helper());
    let mut ask = r.ask();
    ask.folder = r.site.join("web").display().to_string();
    let err = block_on(p.prepare(&ask)).unwrap_err();
    assert!(err.contains("top folder"), "{err}");
}

// decision: HAND-623
#[test]
fn ssh_settings_that_include_a_file_commands_can_change_are_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::fs::write(
        home.join(".ssh/config"),
        "Host *\n  IdentityAgent ~/.agent\n",
    )
    .unwrap();
    assert!(publish::ssh_config_closed(home).is_ok());
    std::fs::write(home.join(".ssh/extra"), "Include ~/.orbstack/ssh/config\n").unwrap();
    std::fs::write(home.join(".ssh/config"), "Include extra\nHost x\n").unwrap();
    let err = publish::ssh_config_closed(home).unwrap_err();
    assert!(err.contains(".orbstack"), "{err}");
    std::fs::write(home.join(".ssh/extra"), "Include /etc/ssh/ssh_config.d/*\n").unwrap();
    assert!(publish::ssh_config_closed(home).is_ok());
}

// ---------- force, fetch, clone, the GitHub CLI's login (HAND-641..643) ----------

/// The GitHub CLI's test login (invented) as git sends it.
const GH_TOKEN: &str = "gho_inventedTestToken123";

fn gh_basic() -> String {
    use base64::Engine;
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{GH_TOKEN}"))
    )
}

/// A commit made straight in the served repository on top of `parent` (someone else's push).
fn served_commit(served: &Path, parent: &str, message: &str, branch: &str) -> String {
    let tree = git(served, &["rev-parse", &format!("{parent}^{{tree}}")]);
    let c = git(served, &["commit-tree", &tree, "-p", parent, "-m", message]);
    git(served, &["update-ref", &format!("refs/heads/{branch}"), &c]);
    c
}

// decision: HAND-641
#[test]
fn a_forced_push_replaces_exactly_what_the_owner_was_shown() {
    if skip() {
        return;
    }
    let r = repo();
    let p = r.h.publisher(&r.helper());
    let plan = block_on(p.prepare(&r.ask())).unwrap();
    assert_eq!(
        block_on(p.push(&plan, false, None)).unwrap()["pushed"],
        true
    );
    let first = r.served_head("site.git").unwrap();
    git(&r.site, &["commit", "-q", "--amend", "-m", "rewritten"]);
    let mut ask = r.ask();
    ask.force = true;
    let plan = block_on(p.prepare(&ask)).unwrap();
    let f = plan.force.clone().expect("something would be dropped");
    assert_eq!(f.remote_commit, first);
    assert_eq!(f.dropped, 1);
    assert_eq!(f.subject, "first");
    assert_eq!(f.author, "t");
    assert!(!f.date.is_empty());
    let facts = block_on(p.push(&plan, false, Some(&f.remote_commit))).unwrap();
    assert_eq!(facts["pushed"], true, "{facts:?}");
    assert_eq!(facts["forced"], true);
    assert_eq!(
        r.served_head("site.git"),
        Some(git(&r.site, &["rev-parse", "HEAD"]))
    );
}

// decision: HAND-641
#[test]
fn a_forced_push_stops_when_the_branch_moved_after_the_question() {
    if skip() {
        return;
    }
    let r = repo();
    let p = r.h.publisher(&r.helper());
    let plan = block_on(p.prepare(&r.ask())).unwrap();
    assert_eq!(
        block_on(p.push(&plan, false, None)).unwrap()["pushed"],
        true
    );
    git(&r.site, &["commit", "-q", "--amend", "-m", "rewritten"]);
    let mut ask = r.ask();
    ask.force = true;
    let plan = block_on(p.prepare(&ask)).unwrap();
    let shown = plan.force.clone().unwrap().remote_commit;
    // someone else pushes after the owner was asked
    let theirs = served_commit(&r.served.join("site.git"), &shown, "theirs", "main");
    let facts = block_on(p.push(&plan, false, Some(&shown))).unwrap();
    assert_eq!(facts["pushed"], false, "{facts:?}");
    assert!(
        facts["why"].as_str().unwrap().contains("changed after"),
        "{facts:?}"
    );
    assert_eq!(r.served_head("site.git").as_deref(), Some(theirs.as_str()));
}

#[test]
fn force_on_a_branch_that_only_moves_forward_is_an_ordinary_push() {
    if skip() {
        return;
    }
    let r = repo();
    let p = r.h.publisher(&r.helper());
    let mut ask = r.ask();
    ask.force = true;
    // nothing there yet
    let plan = block_on(p.prepare(&ask)).unwrap();
    assert!(plan.force.is_none());
    assert_eq!(
        block_on(p.push(&plan, false, None)).unwrap()["pushed"],
        true
    );
    git(&r.site, &["commit", "-q", "--allow-empty", "-m", "second"]);
    let plan = block_on(p.prepare(&ask)).unwrap();
    assert!(plan.force.is_none(), "a fast-forward drops nothing");
}

fn fetch_ask(r: &Repo) -> FetchAsk {
    FetchAsk {
        folder: r.site.to_string_lossy().into_owned(),
        remote: "origin".into(),
        clone_from: None,
        gh: false,
    }
}

// decision: HAND-642
#[test]
fn a_fetch_gets_new_commits_with_the_login_and_runs_nothing_of_the_repository() {
    if skip() {
        return;
    }
    let r = repo();
    let p = r.h.publisher(&r.helper());
    let plan = block_on(p.prepare(&r.ask())).unwrap();
    assert_eq!(
        block_on(p.push(&plan, false, None)).unwrap()["pushed"],
        true
    );
    let head = r.served_head("site.git").unwrap();
    let served = r.served.join("site.git");
    let theirs = served_commit(&served, &head, "theirs", "main");
    let feature = served_commit(&served, &head, "feature work", "feature");
    // planted settings that would run programs if git read them with the login
    let hook = hostile_script(&r, "hook");
    for name in ["reference-transaction", "post-merge", "post-checkout"] {
        std::fs::copy(&hook, r.site.join(".git/hooks").join(name)).unwrap();
    }
    let s = hook.display().to_string();
    for (k, v) in [
        ("core.fsmonitor", s.clone()),
        ("core.sshCommand", s.clone()),
        ("credential.helper", format!("!{s}")),
        ("remote.origin.uploadpack", s.clone()),
        (
            "core.hooksPath",
            r.site.join(".git/evil").display().to_string(),
        ),
    ] {
        git(&r.site, &["config", "--add", k, &v]);
    }
    std::fs::copy(&hook, r.site.join(".git/evil/reference-transaction")).unwrap();
    let plan = block_on(p.prepare_fetch(&fetch_ask(&r))).unwrap();
    assert!(!plan.clone);
    let facts = block_on(p.fetch(&plan)).unwrap();
    assert_eq!(facts["fetched"], true, "{facts:?}");
    let branches = facts["branches"].to_string();
    assert!(
        branches.contains(&theirs) && branches.contains(&feature),
        "{branches}"
    );
    assert_eq!(git(&r.site, &["rev-parse", "origin/main"]), theirs);
    assert_eq!(git(&r.site, &["rev-parse", "origin/feature"]), feature);
    git(
        &r.site,
        &["cat-file", "-e", &format!("{theirs}^{{commit}}")],
    );
    assert!(
        !r.marker.exists(),
        "a planted setting ran: {}",
        std::fs::read_to_string(&r.marker).unwrap_or_default()
    );
    // the control: an ordinary fetch from the project, outside the sandbox, runs what was planted
    let _ = Command::new("/usr/bin/git")
        .args(["fetch", "-q", "origin"])
        .current_dir(&r.site)
        .env_clear()
        .env("HOME", &r.h.home)
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        r.marker.exists(),
        "the control: the planted settings would have run"
    );
}

// decision: HAND-642
#[test]
fn a_clone_goes_into_a_new_folder_with_its_default_branch_checked_out() {
    if skip() {
        return;
    }
    let r = repo();
    let p = r.h.publisher(&r.helper());
    let plan = block_on(p.prepare(&r.ask())).unwrap();
    assert_eq!(
        block_on(p.push(&plan, false, None)).unwrap()["pushed"],
        true
    );
    let copy = r.h.home.join("code/copy");
    let ask = FetchAsk {
        folder: copy.to_string_lossy().into_owned(),
        remote: "origin".into(),
        clone_from: Some(format!("http://127.0.0.1:{}/site.git", r.server.port)),
        gh: false,
    };
    let plan = block_on(p.prepare_fetch(&ask)).unwrap();
    assert!(plan.clone);
    let facts = block_on(p.fetch(&plan)).unwrap();
    assert_eq!(facts["fetched"], true, "{facts:?}");
    assert_eq!(facts["checked_out"], true, "{facts:?}");
    assert_eq!(
        std::fs::read_to_string(copy.join("index.html")).unwrap(),
        "<h1>hi</h1>"
    );
    assert_eq!(
        git(&copy, &["rev-parse", "--abbrev-ref", "@{u}"]),
        "origin/main"
    );
    // a folder that isn't empty is refused
    let err = block_on(p.prepare_fetch(&ask)).unwrap_err();
    assert!(err.contains("isn't an empty folder"), "{err}");
}

#[test]
fn submodules_are_named_with_their_address_not_fetched() {
    if skip() {
        return;
    }
    let r = repo();
    std::fs::write(
        r.site.join(".gitmodules"),
        "[submodule \"lib\"]\n\tpath = lib\n\turl = https://git.example.invalid/o/lib.git\n",
    )
    .unwrap();
    let p = r.h.publisher(&r.helper());
    let plan = block_on(p.prepare(&r.ask())).unwrap();
    assert_eq!(
        block_on(p.push(&plan, false, None)).unwrap()["pushed"],
        true
    );
    let plan = block_on(p.prepare_fetch(&fetch_ask(&r))).unwrap();
    let facts = block_on(p.fetch(&plan)).unwrap();
    let subs = &facts["submodules_not_fetched"];
    assert_eq!(subs["submodules"][0]["name"], "lib", "{facts:?}");
    assert_eq!(
        subs["submodules"][0]["url"],
        "https://git.example.invalid/o/lib.git"
    );
}

// decision: HAND-643
#[test]
fn the_github_cli_login_is_used_only_when_asked_and_never_shown() {
    if skip() {
        return;
    }
    let r = repo();
    let mut p = r.h.publisher("");
    // gh's hosts.yml in the test home, read by FERB's own code; a planted `gh` program that must never run
    let gh_dir = r.h.home.join(".config/gh");
    std::fs::create_dir_all(&gh_dir).unwrap();
    std::fs::write(
        gh_dir.join("hosts.yml"),
        format!(
            "127.0.0.1:\n    user: tester\n    oauth_token: {GH_TOKEN}\n    git_protocol: https\n"
        ),
    )
    .unwrap();
    let fake = hostile_script(&r, "gh");
    for dir in [".local/bin", "bin"] {
        std::fs::create_dir_all(r.h.home.join(dir)).unwrap();
        std::fs::copy(&fake, r.h.home.join(dir).join("gh")).unwrap();
    }
    let home = r.h.home.clone();
    p.gh_login = Arc::new(move |host: &str| publish::gh_hosts_yml(&home, host));
    let plan = block_on(p.prepare(&r.ask())).unwrap();
    let facts = block_on(p.push(&plan, false, None)).unwrap();
    assert_eq!(facts["pushed"], false, "no login without gh: {facts:?}");
    let mut ask = r.ask();
    ask.gh = true;
    let plan = block_on(p.prepare(&ask)).unwrap();
    let facts = block_on(p.push(&plan, false, None)).unwrap();
    assert_eq!(facts["pushed"], true, "{facts:?}");
    assert!(!facts.to_string_contains(GH_TOKEN));
    assert!(!r.marker.exists(), "a planted gh ran");
    // a fetch with it too
    let mut fa = fetch_ask(&r);
    fa.gh = true;
    let plan = block_on(p.prepare_fetch(&fa)).unwrap();
    let facts = block_on(p.fetch(&plan)).unwrap();
    assert_eq!(facts["fetched"], true, "{facts:?}");
    // FERB's scratch folder, which held the login for git, is gone
    let left: Vec<_> = std::fs::read_dir(r.h.root.join("home/.ferb/run/publish"))
        .map(|d| d.flatten().collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn gh_keyring_values_decode() {
    assert_eq!(publish::decode_keyring("gho_x").as_deref(), Some("gho_x"));
    assert_eq!(
        publish::decode_keyring("go-keyring-base64:Z2hvX3h5eg==").as_deref(),
        Some("gho_xyz")
    );
    assert_eq!(
        publish::decode_keyring("go-keyring-encoded:67686f5f71").as_deref(),
        Some("gho_q")
    );
}

trait ContainsText {
    fn to_string_contains(&self, needle: &str) -> bool;
}

impl ContainsText for serde_json::Map<String, Value> {
    fn to_string_contains(&self, needle: &str) -> bool {
        serde_json::to_string(self).unwrap().contains(needle)
    }
}
