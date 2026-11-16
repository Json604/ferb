//! What a command could not read (HAND-595, HAND-596): shell.run under the real sandbox-exec, in a temporary home
//! with an injected never-read list. A command that walks into a never-read folder gets its output plus a `not_read`
//! fact naming the folder, its rule and why; a command that reaches none gets no such key (Python's bytes); the
//! approved escape for that folder reads it and the fact leaves it out; the list is capped with the rest counted.
//! Skipped, with a message, where sandbox-exec is missing.
// decision: HAND-595, HAND-596

#![cfg(target_os = "macos")]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ferb_core::types::{EscapeGrant, Identity, ToolContext};
use ferb_sanitize::{RedactionStream, Redactor};
use ferb_shell::manager::{ManagerConfig, ShellManager};
use ferb_shell::not_read;
use ferb_shell::protection::{Entry, LocalPort, Protection};
use ferb_shell::pyos::{dedupe, realpath};
use ferb_shell::sandbox::{Sandbox, SANDBOX_EXEC};
use ferb_shell::tools::{self as st, ResolvePath};
use serde_json::{json, Value};

const CHROME: &str = "~/Library/Application Support/Google/Chrome";

fn skip() -> bool {
    if !Path::new(SANDBOX_EXEC).exists() {
        eprintln!("skipped: {SANDBOX_EXEC} is not available here");
        return true;
    }
    false
}

/// A never-read list over a fake home (the policy crate's job; enough for the profile and the fact).
struct Lists {
    home: PathBuf,
    ferb: PathBuf,
    workspace: PathBuf,
    uploads: PathBuf,
    reads: Vec<Entry>,
}

impl Lists {
    fn new(home: &Path, reads: &[String]) -> Arc<Lists> {
        let ferb = realpath(&home.join(".ferb"));
        Arc::new(Lists {
            home: home.to_path_buf(),
            workspace: ferb.join("workspace"),
            uploads: ferb.join("workspace").join("uploads"),
            ferb,
            reads: reads
                .iter()
                .map(|i| {
                    let typed = home.join(i.trim_start_matches("~/"));
                    Entry {
                        id: format!("read:{i}"),
                        path: realpath(&typed),
                        typed,
                    }
                })
                .collect(),
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
        &[]
    }
    fn app_entries(&self) -> Vec<PathBuf> {
        vec![]
    }
    fn service_files(&self) -> Vec<PathBuf> {
        vec![]
    }
    fn guarded_parents(&self) -> Vec<PathBuf> {
        let mut out: BTreeSet<PathBuf> = BTreeSet::new();
        for e in &self.reads {
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

/// A fake home: Chrome's data folder (a protected store) beside an ordinary app's folder.
fn home() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::Builder::new()
        .prefix("ferbnotread")
        .tempdir_in("/tmp")
        .unwrap();
    let home = tmp.path().join("home");
    let support = home.join("Library/Application Support");
    for d in ["Google/Chrome/Default", "Other App"] {
        std::fs::create_dir_all(support.join(d)).unwrap();
    }
    std::fs::write(
        support.join("Google/Chrome/Default/Login Data"),
        vec![b'x'; 40_000],
    )
    .unwrap();
    std::fs::write(support.join("Other App/cache.bin"), vec![b'y'; 8_000]).unwrap();
    std::fs::create_dir_all(home.join(".ferb/workspace")).unwrap();
    std::fs::create_dir_all(home.join("Documents")).unwrap();
    (tmp, home)
}

fn manager(tmp: &Path, lists: Arc<Lists>) -> Arc<ShellManager> {
    let redactor = Arc::new(Redactor::new(&[], &[]));
    let sb = Arc::new(
        Sandbox::new(lists, tmp.join("profiles"), Arc::new(|| None), None)
            .with_scan(Arc::new(|| Some(Default::default()))),
    );
    let cfg = ManagerConfig::new(
        tmp.join("logs"),
        Arc::new(|t: &str| t.to_string()),
        Arc::new(move || RedactionStream::new(redactor.clone())),
        sb,
    );
    ShellManager::new(cfg)
}

struct Resolve;

impl ResolvePath for Resolve {
    fn resolve(&self, path: &str, _: &ToolContext, _: bool) -> Result<PathBuf, String> {
        Ok(realpath(Path::new(path)))
    }
}

fn ctx(home: &Path, escape: Option<EscapeGrant>) -> ToolContext {
    ToolContext {
        task_id: "t1".into(),
        worker_id: "w1".into(),
        request_id: "r1".into(),
        identity: Identity::Owner,
        scopes: ["shell".to_string()].into_iter().collect(),
        device_id: "dev".into(),
        span_id: None,
        workspace: realpath(&home.join(".ferb/workspace"))
            .to_string_lossy()
            .into_owned(),
        owner_roots: vec![],
        untrusted_roots: vec![],
        intent_id: None,
        observe_only: false,
        probe: false,
        path_closed: None,
        escape,
        terminal: None,
        folders: vec![],
    }
}

async fn run(
    shells: &ShellManager,
    home: &Path,
    command: &str,
    escape: Option<&str>,
) -> serde_json::Map<String, Value> {
    let ws = realpath(&home.join(".ferb/workspace"))
        .to_string_lossy()
        .into_owned();
    let grant = escape.map(|rule| EscapeGrant {
        rule: rule.into(),
        confirmation_id: "c1".into(),
        command: command.into(),
        cwd: ws.clone(),
        secret: None,
    });
    let args = st::RunArgs {
        start: st::StartArgs {
            command: command.into(),
            cwd: None,
            background: false,
            external_source: None,
            escape: escape.map(str::to_string),
        },
        timeout_s: 30.0,
    };
    st::shell_run(shells, &args, &ctx(home, grant), &Resolve)
        .await
        .unwrap()
        .content
}

/// A storage scan with du over a tree holding a never-read folder: du's own output (the refusal included), and the
/// fact naming the folder, its rule and why, with how the owner can open it.
#[tokio::test(flavor = "multi_thread")]
async fn du_over_a_tree_with_a_never_read_folder_names_it_and_its_rule() {
    if skip() {
        return;
    }
    let (tmp, home) = home();
    let shells = manager(tmp.path(), Lists::new(&home, &[CHROME.to_string()]));
    let support = home.join("Library/Application Support");
    let out = run(
        &shells,
        &home,
        &format!("/usr/bin/du -sk '{}'/*", support.display()),
        None,
    )
    .await;
    let text = out["output"].as_str().unwrap();
    assert!(
        text.contains("Other App") && text.contains("Operation not permitted"),
        "{text}"
    );
    let fact = &out["not_read"];
    assert_eq!(
        fact["folders"],
        json!([{"path": CHROME, "rule": format!("read:{CHROME}"),
                "why": "a browser's data (saved passwords, cookies, history, sign-ins)"}])
    );
    assert_eq!(fact["fact"], json!(not_read::FACT));
    assert_eq!(fact["allow"], json!(not_read::ALLOW));
    assert!(fact.get("more").is_none());
    // every refusal was this protection's: the note about refused writes in visible folders would misstate it
    assert!(out.get("sandbox_note").is_none());
    eprintln!(
        "du result: {}",
        serde_json::to_string_pretty(&Value::Object(out.clone())).unwrap()
    );

    // the owner's Yes for that folder (the escape grant): du reads it, and the fact leaves it out
    let command = format!("/usr/bin/du -sk '{}'", support.join("Google").display());
    let out = run(&shells, &home, &command, Some(&format!("read:{CHROME}"))).await;
    assert_eq!(out["exit_code"], json!(0), "{:?}", out["output"]);
    assert!(out.get("not_read").is_none(), "{out:?}");
}

/// A command that reaches no never-read folder gets no `not_read` key: its result is what Python composed.
#[tokio::test(flavor = "multi_thread")]
async fn a_command_that_reaches_no_protected_folder_is_unchanged() {
    if skip() {
        return;
    }
    let (tmp, home) = home();
    let shells = manager(tmp.path(), Lists::new(&home, &[CHROME.to_string()]));
    let other = home.join("Library/Application Support/Other App");
    let out = run(
        &shells,
        &home,
        &format!("/usr/bin/du -sk '{}'; /bin/echo done", other.display()),
        None,
    )
    .await;
    assert_eq!(out["exit_code"], json!(0));
    assert!(out.get("not_read").is_none());
    let keys: Vec<&str> = out.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "id",
            "running",
            "exit_code",
            "output",
            "offset",
            "total",
            "capture_error",
            "sandbox_ms",
            "network"
        ]
    );
}

/// A refusal in the output names a protected folder the command's words never named (a path built at run time).
#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_in_the_output_names_the_folder_the_words_hid() {
    if skip() {
        return;
    }
    let (tmp, home) = home();
    let shells = manager(tmp.path(), Lists::new(&home, &[CHROME.to_string()]));
    let half = home.join("Library/Application Supp");
    let out = run(
        &shells,
        &home,
        &format!(
            "p='{}'; /bin/cat \"${{p}}ort/Google/Chrome/Default/Login Data\"",
            half.display()
        ),
        None,
    )
    .await;
    assert_ne!(out["exit_code"], json!(0));
    assert_eq!(
        out["not_read"]["folders"][0]["rule"],
        json!(format!("read:{CHROME}"))
    );

    // a refused write in a visible folder beside it keeps the folder-scope note (HAND-203)
    let out = run(
        &shells,
        &home,
        &format!(
            "/usr/bin/du -sk '{}'/*; /bin/echo x > '{}/note.txt'",
            home.join("Library/Application Support").display(),
            home.join("Documents").display()
        ),
        None,
    )
    .await;
    assert!(
        out.get("not_read").is_some() && out.get("sandbox_note").is_some(),
        "{out:?}"
    );
}

/// Over the cap, the first twelve folders are listed in the list's order and the rest counted.
#[tokio::test(flavor = "multi_thread")]
async fn many_protected_folders_are_capped_and_the_rest_counted() {
    if skip() {
        return;
    }
    let (tmp, home) = home();
    let names: Vec<String> = (0..15)
        .map(|i| format!("~/Library/Application Support/Store {i:02}"))
        .collect();
    for n in &names {
        std::fs::create_dir_all(home.join(n.trim_start_matches("~/"))).unwrap();
    }
    let shells = manager(tmp.path(), Lists::new(&home, &names));
    let out = run(
        &shells,
        &home,
        &format!(
            "/usr/bin/du -sk '{}'",
            home.join("Library/Application Support").display()
        ),
        None,
    )
    .await;
    let fact = &out["not_read"];
    let listed: Vec<&str> = fact["folders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        listed,
        names[..12].iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert_eq!(fact["more"], json!(3));
    assert_eq!(
        fact["folders"][0]["why"],
        json!("a folder on the owner's never-read list")
    );
}

/// A listed folder that isn't on this Mac hid nothing: not reported.
#[test]
fn a_listed_folder_missing_from_this_mac_is_not_reported() {
    let (_tmp, home) = home();
    let lists = Lists::new(
        &home,
        &[
            "~/Library/Application Support/Firefox".to_string(),
            CHROME.to_string(),
        ],
    );
    let fact = not_read::fact(
        &*lists,
        "/usr/bin/du -sk ~/Library",
        &home.join(".ferb/workspace"),
        None,
        "",
    )
    .unwrap();
    assert_eq!(fact["folders"].as_array().unwrap().len(), 1);
    assert_eq!(fact["folders"][0]["path"], json!(CHROME));
}
