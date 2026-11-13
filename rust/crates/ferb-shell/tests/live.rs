//! Harmless commands (/bin/echo, /bin/sleep, /bin/cat, /bin/sh) under the real sandbox-exec in a temporary folder,
//! asserting what Python's tests assert (tests/test_shell_sandbox.py, test_shell_scope.py, test_escape_hatch.py): a
//! write outside the allowed folders is denied, a denied executable does not run, FERB's own state stays closed,
//! signals reach only the command's own processes, stop kills the whole group, a group whose leader ended is signalled
//! only after its members are confirmed ours, long lines and lifted commands refuse input. Skipped, with a message,
//! where sandbox-exec is missing. Only processes these tests start are ever signalled.
// decision: HAND-203, HAND-205, HAND-217, HAND-429, HAND-243, GATE-1

#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use ferb_sanitize::{RedactionStream, Redactor};
use ferb_shell::manager::{ManagerConfig, ShellError, ShellManager};
use ferb_shell::protection::{Entry, LocalPort, Protection, ADMIN};
use ferb_shell::pty::PtyProc;
use ferb_shell::pyos::{dedupe, realpath};
use ferb_shell::sandbox::{Options, Sandbox, SANDBOX_EXEC};
use ferb_shell::sockets::group_members;
use serde_json::Value;

fn skip() -> bool {
    if !Path::new(SANDBOX_EXEC).exists() {
        eprintln!("skipped: {SANDBOX_EXEC} is not available here");
        return true;
    }
    false
}

/// A small protection list over a fake home (the policy crate's job; enough for these tests): entries resolved as
/// written and now, every folder above them guarded, FERB's service definitions in the fake home.
struct Lists {
    home: PathBuf,
    ferb: PathBuf,
    workspace: PathBuf,
    uploads: PathBuf,
    reads: Vec<Entry>,
    writes: Vec<Entry>,
    app: Option<PathBuf>,
}

fn entries(home: &Path, prefix: &str, items: &[&str]) -> Vec<Entry> {
    items
        .iter()
        .map(|i| {
            let typed = home.join(i.trim_start_matches("~/"));
            Entry {
                id: format!("{prefix}:{i}"),
                path: realpath(&typed),
                typed,
            }
        })
        .collect()
}

impl Lists {
    fn new(home: &Path, reads: &[&str], writes: &[&str], app: Option<PathBuf>) -> Arc<Lists> {
        let ferb = realpath(&home.join(".ferb"));
        Arc::new(Lists {
            home: home.to_path_buf(),
            workspace: ferb.join("workspace"),
            uploads: ferb.join("workspace").join("uploads"),
            ferb,
            reads: entries(home, "read", reads),
            writes: entries(home, "write", writes),
            app,
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
        self.app
            .iter()
            .flat_map(|a| dedupe([a.clone(), realpath(a)]))
            .collect()
    }
    fn service_files(&self) -> Vec<PathBuf> {
        let agents = self.home.join("Library/LaunchAgents");
        vec![
            agents.join("com.ferb.daemon.plist"),
            agents.join("com.ferb.FERB.login.plist"),
        ]
    }
    fn guarded_parents(&self) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = self
            .reads
            .iter()
            .chain(&self.writes)
            .flat_map(|e| [e.path.clone(), e.typed.clone()])
            .collect();
        roots.push(self.uploads.clone());
        roots.extend(self.app_entries());
        roots.extend(self.service_files());
        let mut out: Vec<PathBuf> = roots
            .iter()
            .flat_map(|r| {
                r.ancestors()
                    .skip(1)
                    .filter(|p| p.parent().is_some())
                    .map(Path::to_path_buf)
                    .collect::<Vec<_>>()
            })
            .collect();
        out.sort();
        out.dedup();
        out
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

/// A fake home in a temporary folder, as test_shell_scope.py's `_home`.
fn home() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::Builder::new()
        .prefix("ferbshlive")
        .tempdir_in("/tmp")
        .unwrap();
    let home = tmp.path().join("home");
    for d in [
        "Documents/repo",
        "Projects",
        "Library/Caches",
        ".cache",
        ".ferb/workspace",
    ] {
        std::fs::create_dir_all(home.join(d)).unwrap();
    }
    std::fs::write(home.join("Documents/report.txt"), "the owner's report").unwrap();
    (tmp, home)
}

fn sandbox(tmp: &Path, lists: Arc<Lists>) -> Sandbox {
    Sandbox::new(lists, tmp.join("profiles"), Arc::new(|| None), None)
        .with_scan(Arc::new(|| Some(Default::default())))
}

/// `sh -c command` under the profile; (exit code, stdout, stderr). The profile file is removed after.
fn run(
    sb: &Sandbox,
    command: &str,
    lift: Option<&str>,
    folders: &[String],
) -> (i32, String, String) {
    let argv = sb.argv(command, lift, folders).unwrap();
    let out = Command::new(&argv[0]).args(&argv[1..]).output().unwrap();
    Sandbox::cleanup(Sandbox::profile_path(&argv).as_deref());
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn the_sandbox_is_available_and_its_profile_runs() {
    if skip() {
        return;
    }
    let (tmp, home) = home();
    let sb = sandbox(tmp.path(), Lists::new(&home, &[], &[], None));
    assert_eq!(sb.available(), (true, "ok".to_string()));
    assert!(
        std::fs::read_dir(tmp.path().join("profiles"))
            .unwrap()
            .next()
            .is_none(),
        "the probe's profile is gone"
    );
}

// decision: HAND-203, HAND-205
#[test]
fn a_write_outside_the_allowed_folders_is_denied_and_programs_keep_working() {
    if skip() {
        return;
    }
    let (tmp, home) = home();
    let sb = sandbox(tmp.path(), Lists::new(&home, &[], &[], None));
    let docs = home.join("Documents");
    let d = docs.display();
    for refused in [
        format!("echo x > '{d}/new.txt'"),
        format!("echo more >> '{d}/report.txt'"),
        format!("rm '{d}/report.txt'"),
        format!("mv '{d}/report.txt' '{d}/renamed.txt'"),
        format!("echo x > '{}/top.txt'", home.display()),
    ] {
        let (code, _, err) = run(&sb, &refused, None, &[]);
        assert!(
            code != 0 && err.contains("Operation not permitted"),
            "{refused}: {err}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(docs.join("report.txt")).unwrap(),
        "the owner's report"
    );
    let ws = home.join(".ferb/workspace");
    let (code, _, err) = run(
        &sb,
        &format!("echo c > '{}/.cache/c' && echo l > '{}/Library/Caches/l' && echo w > '{}/w' && /bin/echo ok", home.display(), home.display(), ws.display()),
        None,
        &[],
    );
    assert_eq!(code, 0, "{err}");
    // the task's folders, and only those, are writable (HAND-204)
    let named = vec![home.join("Projects").to_string_lossy().into_owned()];
    assert_eq!(
        run(
            &sb,
            &format!("echo x > '{}/Projects/a.txt'", home.display()),
            None,
            &named
        )
        .0,
        0
    );
    assert_ne!(
        run(&sb, &format!("echo x > '{d}/a.txt'"), None, &named).0,
        0
    );
}

// decision: HAND-429
#[test]
fn a_denied_executable_does_not_run_and_a_bundles_helper_does() {
    if skip() {
        return;
    }
    let (tmp, home) = home();
    let program = |at: PathBuf| {
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(&at, "#!/bin/sh\n/bin/echo ran\n").unwrap();
        std::fs::set_permissions(&at, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        at
    };
    let app = program(home.join("Applications/Fake Browser.app/Contents/MacOS/Fake Browser"));
    let helper =
        program(home.join("Applications/Host.app/Contents/Helpers/Inner.app/Contents/MacOS/inner"));
    let sb = sandbox(tmp.path(), Lists::new(&home, &[], &[], None));
    let (code, out, _) = run(&sb, &format!("'{}' --headless", app.display()), None, &[]);
    assert!(code != 0 && !out.contains("ran"));
    assert_eq!(
        run(&sb, &format!("'{}'", helper.display()), None, &[])
            .1
            .trim(),
        "ran"
    );
}

#[test]
fn ferbs_state_and_a_never_read_store_stay_closed_and_a_lift_opens_only_its_rule() {
    if skip() {
        return;
    }
    let (tmp, home) = home();
    std::fs::create_dir_all(home.join(".ferb/run")).unwrap();
    std::fs::write(home.join(".ferb/run/token"), "secret-token").unwrap();
    std::fs::create_dir_all(home.join(".aws")).unwrap();
    std::fs::write(home.join(".aws/config"), "region = invented-1").unwrap();
    let sb = sandbox(tmp.path(), Lists::new(&home, &["~/.aws"], &[], None));
    let (_, out, _) = run(
        &sb,
        &format!("/bin/cat '{}/.ferb/run/token'", home.display()),
        None,
        &[],
    );
    assert!(!out.contains("secret-token"));
    let (code, out, _) = run(
        &sb,
        &format!("/bin/cat '{}/.aws/config'", home.display()),
        None,
        &[],
    );
    assert!(code != 0 && !out.contains("invented-1"));
    let (code, out, _) = run(
        &sb,
        &format!("/bin/cat '{}/.aws/config'", home.display()),
        Some("read:~/.aws"),
        &[],
    );
    assert!(code == 0 && out.contains("invented-1"));
    let (_, out, _) = run(
        &sb,
        &format!("/bin/cat '{}/.ferb/run/token'", home.display()),
        Some("read:~/.aws"),
        &[],
    );
    assert!(
        !out.contains("secret-token"),
        "a lift never reopens FERB's own state"
    );
    for never in ["ferb_state", "network", "ferb_app", "ferb_service"] {
        assert!(sb
            .profile(&Options {
                lift: Some(never),
                ..Default::default()
            })
            .is_err());
    }
}

// decision: HAND-217
#[test]
fn a_command_signals_only_its_own_processes() {
    if skip() {
        return;
    }
    let (tmp, home) = home();
    let sb = sandbox(tmp.path(), Lists::new(&home, &[], &[], None));
    let mut outside = Command::new("/bin/sleep").arg("30").spawn().unwrap(); // ours: started by this test
    let (_, out, err) = run(
        &sb,
        &format!(
            "kill {}; echo outside=$?; /bin/sleep 30 & k=$!; kill $k; echo own=$?",
            outside.id()
        ),
        None,
        &[],
    );
    assert!(
        out.contains("outside=1")
            && out.contains("own=0")
            && err.contains("Operation not permitted"),
        "{out} {err}"
    );
    assert!(
        outside.try_wait().unwrap().is_none(),
        "the outside process is still alive"
    );
    outside.kill().unwrap();
    outside.wait().unwrap();
}

fn manager(tmp: &Path, lists: Arc<Lists>) -> Arc<ShellManager> {
    let redactor = Arc::new(Redactor::new(&[], &[]));
    redactor.add_secret_value("sk-invented-registered-value-123456");
    let sb = Arc::new(sandbox(tmp, lists));
    let r = redactor.clone();
    let cfg = ManagerConfig::new(
        tmp.join("logs"),
        Arc::new(|t: &str| t.to_string()),
        Arc::new(move || RedactionStream::new(r.clone())),
        sb,
    );
    ShellManager::new(cfg)
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_kills_the_whole_group_even_after_the_command_exited() {
    if skip() {
        return;
    }
    let (tmp, home) = home();
    let lists = Lists::new(&home, &[], &[], None);
    let ws = lists.workspace.clone();
    let m = manager(tmp.path(), lists);
    let h = m
        .start(
            "/bin/sleep 30 >/dev/null 2>&1 & /bin/sleep 0.5; /bin/echo started",
            &ws,
            "task-a",
            false,
            None,
            None,
            &[],
        )
        .await
        .unwrap();
    assert_eq!(m.wait(&h, 10.0).await.unwrap(), Some(0)); // its own process ended at once
    let pgid = m.process(&h.id).unwrap().pid().unwrap();
    assert!(
        !group_members(pgid).is_empty(),
        "the child lives on in the group"
    );
    let (text, _, _) = m.read(&h, 0, None).await.unwrap();
    assert!(text.contains("started"));
    assert_eq!(m.end_task("task-a").await, 0);
    assert!(
        group_members(pgid).is_empty(),
        "the contained child survived its task"
    );
    assert!(!m.tracked(&h.id), "the entry went");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_group_whose_id_could_be_reused_is_never_signalled_without_a_check() {
    if skip() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let redactor = Arc::new(Redactor::new(&[], &[]));
    let p = PtyProc::new(
        vec![
            "/bin/sh".into(),
            "-c".into(),
            "/bin/sleep 30 >/dev/null 2>&1 & /bin/sleep 0.5; exit 0".into(),
        ],
        tmp.path().to_path_buf(),
        vec![("PATH".into(), "/usr/bin:/bin".into())],
        tmp.path().join("l.log"),
        Arc::new(|t: &str| t.to_string()),
        Arc::new(move || RedactionStream::new(redactor.clone())),
        None,
        None,
    );
    p.start().await.unwrap();
    assert_eq!(p.wait(10.0).await, Some(0));
    let pgid = p.pid().unwrap();
    p.kill(Some(Arc::new(|_: &[i32]| false))).await; // members not confirmed: nothing sent
    assert!(!group_members(pgid).is_empty());
    p.kill(None).await; // no check at all: nothing sent either
    assert!(!group_members(pgid).is_empty());
    p.kill(Some(Arc::new(|_: &[i32]| true))).await;
    assert!(group_members(pgid).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_long_line_is_refused_a_late_send_is_reported_and_output_is_redacted() {
    if skip() {
        return;
    }
    let (tmp, home) = home();
    let lists = Lists::new(&home, &[], &[], None);
    let ws = lists.workspace.clone();
    let m = manager(tmp.path(), lists);
    let h = m
        .start(
            "IFS= read -r line; printf 'got %s\\n' \"${#line}\"",
            &ws,
            "t",
            false,
            None,
            None,
            &[],
        )
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let refused = m
        .send(&h, &format!("{}\n", "a".repeat(3001)), 2.0)
        .await
        .unwrap_err();
    assert!(
        matches!(&refused, ShellError::Refused(s) if s.contains("1023 bytes") && s.contains("Nothing was sent")),
        "{refused}"
    );
    assert_eq!(
        m.send(&h, &format!("{}\n", "b".repeat(1023)), 2.0)
            .await
            .unwrap(),
        (1024, 1024)
    );
    m.wait(&h, 10.0).await.unwrap();
    assert!(m.read(&h, 0, None).await.unwrap().0.contains("got 1023"));
    assert_eq!(m.send(&h, "too late\n", 0.1).await.unwrap(), (0, 9));
    let h2 = m
        .start(
            "/bin/echo key sk-invented-registered-value-123456 end",
            &ws,
            "t",
            false,
            None,
            None,
            &[],
        )
        .await
        .unwrap();
    m.wait(&h2, 10.0).await.unwrap();
    let (text, offset, total) = m.read(&h2, 0, None).await.unwrap();
    assert!(
        !text.contains("sk-invented-registered-value") && text.contains("key") && offset == total,
        "{text}"
    );
    // another task never sees it
    assert!(m.read(&m.handle(&h2.id, "other"), 0, None).await.is_err());
    m.kill_all().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lifted_command_takes_no_input_and_admin_needs_its_password() {
    if skip() {
        return;
    }
    let (tmp, home) = home();
    std::fs::create_dir_all(home.join(".aws")).unwrap();
    let lists = Lists::new(&home, &["~/.aws"], &[], None);
    let ws = lists.workspace.clone();
    let m = manager(tmp.path(), lists);
    let grant = ferb_core::types::EscapeGrant {
        rule: "read:~/.aws".into(),
        confirmation_id: "c-1".into(),
        command: "/bin/cat".into(),
        cwd: realpath(&ws).to_string_lossy().into_owned(),
        secret: None,
    };
    let h = m
        .start(
            "/bin/cat",
            &ws,
            "t",
            false,
            Some("read:~/.aws"),
            Some(&grant),
            &[],
        )
        .await
        .unwrap();
    let refused = m.send(&h, "x\n", 1.0).await.unwrap_err();
    assert!(
        matches!(refused, ShellError::Refused(s) if s.contains("one-time lift of read:~/.aws"))
    );
    let other = m
        .start(
            "/bin/cat x",
            &ws,
            "t",
            false,
            Some("read:~/.aws"),
            Some(&grant),
            &[],
        )
        .await
        .unwrap_err();
    assert!(
        matches!(other, ShellError::Refused(s) if s.contains("does not cover this exact command"))
    );
    let admin = m
        .start("id -u", &ws, "t", false, Some(ADMIN), None, &[])
        .await
        .unwrap_err();
    assert!(matches!(admin, ShellError::Refused(s) if s.contains("hidden password")));
    m.kill_all().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_file_lives_until_its_process_ends() {
    if skip() {
        return;
    }
    let (tmp, home) = home();
    let lists = Lists::new(&home, &[], &[], None);
    let ws = lists.workspace.clone();
    let m = manager(tmp.path(), lists);
    let h = m
        .start("/bin/sleep 5", &ws, "t", false, None, None, &[])
        .await
        .unwrap();
    let profiles = || {
        std::fs::read_dir(tmp.path().join("profiles"))
            .map(|d| d.count())
            .unwrap_or(0)
    };
    assert_eq!(profiles(), 1);
    m.kill(&h).await.unwrap();
    m.wait(&h, 5.0).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(profiles(), 0);
}

// decision: HAND-214, HAND-232, HAND-245
#[tokio::test(flavor = "multi_thread")]
async fn network_goes_only_through_the_proxy_is_recorded_and_a_download_past_its_budget_is_stopped()
{
    use std::io::{Read, Write};
    use std::sync::Mutex;
    if skip() || !Path::new("/usr/bin/curl").exists() {
        eprintln!("skipped: needs sandbox-exec and /usr/bin/curl");
        return;
    }
    // an origin on this Mac's loopback, reached by a name only the proxy knows
    let origin = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let oport = origin.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for c in origin.incoming().flatten().take(4) {
            let mut c = c;
            let mut buf = [0u8; 4096];
            let n = c.read(&mut buf).unwrap_or(0);
            let big = String::from_utf8_lossy(&buf[..n]).contains("/big");
            let body = if big {
                "x".repeat(4_000_000)
            } else {
                "hello".to_string()
            };
            let _ = c.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        }
    });
    let mut cfg = ferb_shell::proxy::ProxyConfig::new(Arc::new(|_, _, _| (true, String::new())));
    cfg.resolve = Arc::new(move |host: &str, _| {
        if host == "origin.test" {
            Ok(vec![std::net::SocketAddr::from(([127, 0, 0, 1], oport))])
        } else {
            Err(ferb_shell::proxy::Unreachable(
                "couldn't resolve the name (gaierror)".into(),
            ))
        }
    });
    let proxy = Arc::new(ferb_shell::proxy::EgressProxy::new(cfg).unwrap());
    let pport = proxy.port();
    let (tmp, home) = home();
    let lists = Lists::new(&home, &[], &[], None);
    let ws = lists.workspace.clone();
    let sb = Arc::new(
        Sandbox::new(
            lists,
            tmp.path().join("profiles"),
            Arc::new(move || Some(pport)),
            None,
        )
        .with_scan(Arc::new(|| Some(Default::default())))
        .with_direct_ports(Arc::new(|_| vec![])),
    );
    let redactor = Arc::new(Redactor::new(&[], &[]));
    let recorded: Arc<Mutex<Vec<Value>>> = Arc::default();
    let rec = recorded.clone();
    let mut mc = ManagerConfig::new(
        tmp.path().join("logs"),
        Arc::new(|t: &str| t.to_string()),
        Arc::new(move || RedactionStream::new(redactor.clone())),
        sb,
    );
    mc.proxy = Some(proxy.clone());
    mc.on_network = Some(Arc::new(move |_info, facts| {
        rec.lock().unwrap().push(Value::Object(facts))
    }));
    let m = ShellManager::new(mc);
    let h = m
        .start(
            "/usr/bin/curl -s http://origin.test/x",
            &ws,
            "t",
            false,
            None,
            None,
            &[],
        )
        .await
        .unwrap();
    assert_eq!(m.wait(&h, 20.0).await.unwrap(), Some(0));
    assert_eq!(m.read(&h, 0, None).await.unwrap().0, "hello");
    let net = m.network(&h).unwrap();
    assert_eq!(net["destinations"][0]["host"], "origin.test");
    // around the proxy: nothing
    let direct = m
        .start(
            &format!("/usr/bin/curl -s --noproxy '*' http://127.0.0.1:{oport}/x"),
            &ws,
            "t",
            false,
            None,
            None,
            &[],
        )
        .await
        .unwrap();
    assert_ne!(m.wait(&direct, 20.0).await.unwrap(), Some(0));
    // a download past its budget is stopped, and says why (HAND-232)
    m.set_on_received(Some(Arc::new(|_task: &str, _n: i64| {
        Some("the task's download budget is used up".to_string())
    })));
    let big = m
        .start(
            "/usr/bin/curl -s --limit-rate 200k http://origin.test/big -o /dev/null; /bin/sleep 30",
            &ws,
            "t",
            false,
            None,
            None,
            &[],
        )
        .await
        .unwrap();
    let code = m.wait(&big, 10.0).await.unwrap();
    assert!(
        code.is_some_and(|c| c != 0),
        "the downloading command was stopped: {code:?}"
    );
    assert_eq!(
        m.info(&big).unwrap().stopped.as_deref(),
        Some("the task's download budget is used up")
    );
    m.end_task("t").await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while recorded.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(recorded
        .lock()
        .unwrap()
        .iter()
        .any(|f| f["destinations"][0]["host"] == "origin.test"));
    proxy.close();
}
