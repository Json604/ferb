//! Stage 3 S3b (T-1203): a phone's files both ways through a real ferb-rpc server, as the phone link's helper reaches
//! the daemon (`phone.files`, `phone.tree`, `phone.read`, `phone.put_*`). The lab's attacks on file access
//! (ferb-phone-link@57b3f34 rust/tests/attacks.rs, attacks_home.rs, hardening.rs: their file parts) each have a test
//! here, named in its comment, against FERB's real protection lists in a temporary home laid out like a Mac's; the
//! never-read list is walked entry by entry. Nothing outside the test's temporary folder is read or written but a
//! path's metadata (an absolute list entry, `/dev/null`).

mod phone_support;

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine;
use ferb_core::interfaces::Clock;
use ferb_daemon::phone::files::{FileLimits, FileSeams, Judge, PHONE_KEY_PLACES};
use ferb_policy::ppath::PosixFiles;
use ferb_policy::protection::{self, Lists, Protection};
use ferb_rpc::phone_gate as code;
use p256::ecdsa::SigningKey;
use phone_support::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const P1: &str = "endpoint-one";
const P2: &str = "endpoint-two";
const SECRET: &[u8] = b"SECRET-KEY-MATERIAL";

static POSIX: PosixFiles = PosixFiles;

/// FERB's lists as the daemon builds them, on the system's stateless file calls.
struct Lists0(Protection<'static>);
// SAFETY: `Protection<'static>` holds owned data and the `&'static PosixFiles` above, whose calls go straight to libc;
// nothing in it has interior mutability.
unsafe impl Send for Lists0 {}
// SAFETY: as above.
unsafe impl Sync for Lists0 {}

impl Judge for Lists0 {
    fn home(&self) -> String {
        self.0.home.clone()
    }
    fn ferb_home(&self) -> String {
        self.0.ferb_home.clone()
    }
    fn rules(&self, path: &str, access: &str) -> Vec<String> {
        self.0.fs_rules(path, access)
    }
}

type HookFn = Box<dyn Fn(&str, &str) + Send + Sync>;

struct Fx {
    w: World,
    home: PathBuf,
    inbox: PathBuf,
    free: Arc<AtomicU64>,
    hook: Arc<Mutex<Option<HookFn>>>,
    keys: Keys,
}

/// The never-read entries that are files on a Mac (the rest are folders).
const FILE_ENTRIES: &[&str] = &[
    "~/.netrc",
    "~/.npmrc",
    "~/.git-credentials",
    "~/.pypirc",
    "~/.cargo/credentials.toml",
    "~/.zsh_history",
    "~/.bash_history",
];

fn at(home: &Path, entry: &str) -> PathBuf {
    home.join(entry.trim_start_matches("~/"))
}

/// A home laid out like a Mac's: every never-read entry and every phone key place (a file with the secret inside, or
/// the file itself), FERB's own folder with its workspace and uploads, and ordinary documents.
fn lay_out(home: &Path) {
    for entry in protection::default_never_read()
        .iter()
        .chain(PHONE_KEY_PLACES)
        .filter(|e| e.starts_with("~/"))
    {
        let p = at(home, entry);
        if FILE_ENTRIES.contains(entry) {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, SECRET).unwrap();
        } else {
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("k.txt"), SECRET).unwrap();
        }
    }
    for (rel, body) in [
        (".ferb/ferb.db", "STATE"),
        (".ferb/config.toml", "STATE"),
        (".ferb/workspace/w.txt", "work"),
        (".ferb/workspace/uploads/u.txt", "STATE"),
        ("docs/plain.txt", "plain"),
    ] {
        let p = home.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
}

async fn fx(strong: bool, inbox: Option<&str>, added: Vec<String>, limits: FileLimits) -> Fx {
    let w = world(None).await;
    let home = std::fs::canonicalize(w.dir.path()).unwrap().join("home");
    std::fs::create_dir_all(&home).unwrap();
    lay_out(&home);
    let added: Vec<String> = added
        .into_iter()
        .map(|a| at(&home, &a).to_string_lossy().into_owned())
        .collect();
    let (nr, nw, ta, lp) = (
        json!(protection::default_never_read()),
        json!(protection::DEFAULT_NEVER_WRITE),
        json!(protection::DEFAULT_TERMINAL_APPS),
        json!([]),
    );
    let lists = Lists {
        never_read: &nr,
        never_write: &nw,
        terminal_apps: &ta,
        local_ports: &lp,
    };
    let p = Protection::new(
        &home.to_string_lossy(),
        &home.join(".ferb").to_string_lossy(),
        &lists,
        None,
        &POSIX,
    )
    .unwrap();
    let inbox = home.join(inbox.unwrap_or("Downloads/FERB from phone"));
    let free = Arc::new(AtomicU64::new(u64::MAX / 4));
    let hook: Arc<Mutex<Option<HookFn>>> = Arc::new(Mutex::new(None));
    let (f, h) = (free.clone(), hook.clone());
    w.phones.register_files(
        &w.server,
        FileSeams {
            judge: Arc::new(Lists0(p)),
            inbox: inbox.clone(),
            free_bytes: Arc::new(move |_p: &Path| Ok(f.load(Ordering::SeqCst))),
            added_places: added,
            task_file: None,
            hook: Some(Arc::new(move |stage: &str, path: &str| {
                if let Some(hook) = h.lock().unwrap().as_ref() {
                    hook(stage, path);
                }
            })),
        },
        limits,
    );
    let keys = Keys::new(1);
    pair(&w, P1, &keys, strong, false).await;
    Fx {
        w,
        home,
        inbox,
        free,
        hook,
        keys,
    }
}

async fn plain_fx() -> Fx {
    fx(true, None, Vec::new(), FileLimits::default()).await
}

impl Fx {
    fn s(&self, rel: &str) -> String {
        self.home.join(rel).to_string_lossy().into_owned()
    }

    async fn open(&self, session: &str) -> Conn {
        let mut c = Conn::phone(&self.w, P1, session).await;
        c.unlock(&self.keys).await;
        c
    }

    fn set_hook(&self, f: impl Fn(&str, &str) + Send + Sync + 'static) {
        *self.hook.lock().unwrap() = Some(Box::new(f));
    }

    fn inbox_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![self.inbox.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                if e.file_type().unwrap().is_dir() {
                    stack.push(e.path());
                }
                out.push(e.file_name().to_string_lossy().into_owned());
            }
        }
        out
    }
}

fn b64u(raw: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
}

/// The bytes of a `phone.read` stream's events, its hash checked against what the daemon said.
fn body(events: &[Value]) -> Vec<u8> {
    assert!(events[0].get("meta").is_some(), "{events:?}");
    let sha = events[1]["sha256"].as_str().unwrap().to_owned();
    let mut out = Vec::new();
    for e in &events[2..] {
        out.extend(
            base64::engine::general_purpose::STANDARD
                .decode(e["d"].as_str().unwrap())
                .unwrap(),
        );
    }
    let offset = events[0]["meta"]["offset"].as_u64().unwrap();
    if offset == 0 {
        assert_eq!(sha, b64u(&Sha256::digest(&out)), "the hash is the bytes'");
    }
    out
}

async fn fetch(c: &mut Conn, path: &str, proof: Option<Value>) -> Result<Vec<u8>, i64> {
    match c.call("phone.read", json!({"path": path}), proof).await {
        Ok(_) => Ok(body(&c.events)),
        Err((code, _)) => Err(code),
    }
}

/// A proof for one key-place access: `{challenge, method, params: {"path": <as sent>}}` signed by `key`.
async fn place_proof(c: &mut Conn, key: &SigningKey, signed_as: &str, path: &str) -> Value {
    let ch = c.challenge().await;
    call_proof(key, &ch, signed_as, &json!({"path": path}))
}

async fn fetch_proven(c: &mut Conn, key: &SigningKey, path: &str) -> Result<Vec<u8>, i64> {
    let p = place_proof(c, key, "link.get", path).await;
    fetch(c, path, Some(p)).await
}

async fn list(c: &mut Conn, path: &str, proof: Option<Value>) -> Result<Value, i64> {
    c.call("phone.files", json!({"path": path}), proof)
        .await
        .map_err(|e| e.0)
}

/// An upload as the helper makes it: begin, 16 KiB chunks from the offset the daemon says, end.
async fn upload(c: &mut Conn, mut begin: Value, bytes: &[u8]) -> Result<Value, i64> {
    begin["size"] = json!(bytes.len());
    let b = c
        .call("phone.put_begin", begin, None)
        .await
        .map_err(|e| e.0)?;
    let id = b["upload_id"].as_str().unwrap().to_owned();
    let from = b["offset"].as_u64().unwrap() as usize;
    for chunk in bytes[from..].chunks(16 * 1024) {
        let d = base64::engine::general_purpose::STANDARD.encode(chunk);
        c.call("phone.put_chunk", json!({"upload_id": id, "d": d}), None)
            .await
            .map_err(|e| e.0)?;
    }
    c.call("phone.put_end", json!({"upload_id": id}), None)
        .await
        .map_err(|e| e.0)
}

// ---- the never-read list, entry by entry ------------------------------------------------------------------------

/// Every entry of FERB's real never-read list (and every place PHN-131 adds) is a key place for the phone: fetched or
/// listed it asks for its own fresh proof before anything is opened, the PIN-capable key is refused for it where the
/// phone has a fingerprint key, the fingerprint opens it, a folder's walk leaves it out, and uploads cannot land in
/// it. A test fails here when FERB's list grows with an entry that does not hold (lab: the_guarded_places_are_ferbs_own_list,
/// hardening.rs).
// decision: PHN-121
// decision: PHN-131
// decision: PHN-133
// decision: PHN-129
// decision: PHN-482
#[tokio::test]
async fn every_never_read_entry_and_phone_key_place_takes_its_own_fresh_fingerprint() {
    let f = plain_fx().await;
    let mut c = f.open("s1").await;
    let entries: Vec<&str> = protection::default_never_read()
        .into_iter()
        .chain(PHONE_KEY_PLACES.iter().copied())
        .collect();
    assert!(entries.len() > 60, "{}", entries.len());
    for entry in &entries {
        if !entry.starts_with("~/") {
            // an absolute entry (/Library/Keychains): refused before anything is opened, never read with a proof here
            let p = format!("{entry}/ferb-test-no-such-file");
            assert_eq!(
                fetch(&mut c, &p, None).await,
                Err(code::FRESH_UNLOCK_REQUIRED),
                "{entry}"
            );
            continue;
        }
        let is_file = FILE_ENTRIES.contains(entry);
        let file = if is_file {
            f.s(entry.trim_start_matches("~/"))
        } else {
            f.s(&format!("{}/k.txt", entry.trim_start_matches("~/")))
        };
        assert_eq!(
            fetch(&mut c, &file, None).await,
            Err(code::FRESH_UNLOCK_REQUIRED),
            "{entry}"
        );
        assert_eq!(
            fetch_proven(&mut c, &f.keys.lock, &file).await,
            Err(code::UNLOCK_INVALID),
            "{entry}: the PIN key"
        );
        assert_eq!(
            fetch_proven(&mut c, &f.keys.strong, &file).await.as_deref(),
            Ok(SECRET),
            "{entry}"
        );
        // by its ~ spelling too
        let tilde = file.replacen(&f.home.to_string_lossy().into_owned(), "~", 1);
        assert_eq!(
            fetch(&mut c, &tilde, None).await,
            Err(code::FRESH_UNLOCK_REQUIRED),
            "{entry} as ~"
        );
        if !is_file {
            let dir = f.s(entry.trim_start_matches("~/"));
            assert_eq!(
                list(&mut c, &dir, None).await,
                Err(code::FRESH_UNLOCK_REQUIRED),
                "{entry}"
            );
            let p = place_proof(&mut c, &f.keys.strong, "link.files", &dir).await;
            let listed = list(&mut c, &dir, Some(p)).await.unwrap();
            assert_eq!(listed["high_value"], true, "{entry}");
            assert_eq!(listed["entries"][0]["locked"], true, "{entry}: {listed}");
        }
    }
    // a walk of the whole home leaves every one of them out
    let tree = c.ok("phone.tree", json!({"path": "~"})).await;
    let rels: Vec<String> = tree["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["rel"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(rels, vec!["docs/plain.txt"], "{tree}");
    // and nothing of theirs is in the audit but paths
    assert!(!audit_text(&f.w).contains(std::str::from_utf8(SECRET).unwrap()));
}

/// Uploads land only in the phone's upload folder; one configured inside a never-read entry, FERB's own state or a
/// never-write place takes nothing and makes nothing there.
// decision: PHN-127
// decision: PHN-133
// decision: PHN-484
#[tokio::test]
async fn an_upload_folder_in_a_protected_place_takes_nothing() {
    let mut places: Vec<String> = protection::default_never_read()
        .into_iter()
        .filter(|e| e.starts_with("~/") && !FILE_ENTRIES.contains(e))
        .map(|e| format!("{}/inbox", e.trim_start_matches("~/")))
        .collect();
    places.extend([".ferb/inbox".into(), "Library/LaunchAgents/inbox".into()]);
    for place in &places {
        let f = fx(true, Some(place), Vec::new(), FileLimits::default()).await;
        let mut c = f.open("s1").await;
        assert_eq!(
            upload(&mut c, json!({"name": "a.txt"}), b"x").await,
            Err(code::FILE_NOT_ALLOWED),
            "{place}"
        );
        assert!(!f.inbox.exists(), "{place}: a folder was made there");
    }
}

// ---- the lab's attacks ------------------------------------------------------------------------------------------

/// Lab attacks.rs finding_2_no_spelling_of_a_path_dodges_the_fresh_proof: every spelling of a key place (case, Unicode
/// form, `..`, `/./`, doubled slashes, links of every kind, a hard link, the Data volume's name, `/.vol`, `/var` for
/// `/private/var`, `~name`, relative) is refused without its proof; with it, the honest spelling opens.
// decision: PHN-114
// decision: PHN-121
// decision: PHN-140
// decision: PHN-483
#[tokio::test]
async fn no_spelling_of_a_path_dodges_the_fresh_proof() {
    let (nfc, nfd) = ("Schl\u{fc}ssel", "Schlu\u{308}ssel");
    let f = fx(
        true,
        None,
        vec!["~/vault".into(), format!("~/{nfc}")],
        FileLimits::default(),
    )
    .await;
    let h = &f.home;
    std::fs::create_dir_all(h.join("vault/deep")).unwrap();
    std::fs::write(h.join("vault/token.json"), SECRET).unwrap();
    std::fs::create_dir_all(h.join(nfc)).unwrap();
    std::fs::write(h.join(nfc).join("k.txt"), SECRET).unwrap();
    let ln = |to: &str, from: &str| std::os::unix::fs::symlink(to, h.join(from)).unwrap();
    ln(&f.s("vault/token.json"), "docs/link.json");
    ln(&f.s("vault"), "docs/shortcut");
    ln(&f.s("docs/link.json"), "docs/chain.json");
    ln("../vault/token.json", "docs/relative-link.json");
    std::fs::hard_link(h.join("vault/token.json"), h.join("docs/hard.json")).unwrap();
    let meta = std::fs::metadata(h.join("vault/token.json")).unwrap();
    let user = std::env::var("USER").unwrap_or_else(|_| "owner".into());
    let mut c = f.open("s1").await;
    assert_eq!(
        fetch(&mut c, &f.s("docs/plain.txt"), None).await.unwrap(),
        b"plain"
    );
    let path = |rel: &str| f.s(rel);
    let mut spellings: Vec<(&str, String)> = vec![
        ("plain", path("vault/token.json")),
        ("~name", format!("~{user}/{}", "vault/token.json")),
        ("relative", "vault/token.json".into()),
        ("upper case", path("VAULT/TOKEN.JSON")),
        ("mixed case", path("Vault/Token.Json")),
        ("accent, decomposed", path(&format!("{nfd}/k.txt"))),
        ("accent, composed", path(&format!("{nfc}/k.txt"))),
        (
            "accent, upper",
            path(&format!("{}/K.TXT", nfd.to_uppercase())),
        ),
        ("trailing slash", path("vault/token.json/")),
        ("trailing /.", path("vault/token.json/.")),
        (
            "doubled slashes",
            path("vault//token.json").replacen("/home/", "//home//", 1),
        ),
        ("/./ inside", path("vault/./deep/../token.json")),
        ("..", path("docs/../vault/token.json")),
        ("a link", path("docs/link.json")),
        ("a link to a link", path("docs/chain.json")),
        ("a relative link", path("docs/relative-link.json")),
        (
            "a folder link in the middle",
            path("docs/shortcut/token.json"),
        ),
        (
            "a hard link from an ordinary folder",
            path("docs/hard.json"),
        ),
        (
            "the Data volume's name (firmlink)",
            format!("/System/Volumes/Data{}", path("vault/token.json")),
        ),
        (
            "the file by its number (/.vol)",
            format!("/.vol/{}/{}", meta.dev(), meta.ino()),
        ),
        ("a NUL", format!("{}\u{0}x", path("vault/token.json"))),
    ];
    let home = f.home.to_string_lossy().into_owned();
    if let Some(rest) = home.strip_prefix("/private") {
        spellings.push(("/var for /private/var", format!("{rest}/vault/token.json")));
    }
    let mut leaks = Vec::new();
    for (name, spelled) in &spellings {
        let got = fetch(&mut c, spelled, None).await;
        if got.as_deref() == Ok(SECRET) {
            leaks.push(*name);
        }
        // what stopped it: a spelling FERB cannot use at all, a file by number, else the key place's own proof
        let want = match *name {
            "~name" | "relative" | "a NUL" => code::BAD_REQUEST,
            "the file by its number (/.vol)" => code::FILE_NOT_ALLOWED,
            _ => code::FRESH_UNLOCK_REQUIRED,
        };
        assert_eq!(got, Err(want), "{name}");
    }
    for (name, spelled) in [
        ("list: a link", path("docs/shortcut")),
        ("list: upper case", path("VAULT")),
        (
            "list: the Data volume's name",
            format!("/System/Volumes/Data{}", path("vault")),
        ),
        ("list: by number", {
            let m = std::fs::metadata(h.join("vault")).unwrap();
            format!("/.vol/{}/{}", m.dev(), m.ino())
        }),
    ] {
        if list(&mut c, &spelled, None).await.is_ok() {
            leaks.push(name);
        }
    }
    assert!(
        leaks.is_empty(),
        "a key place came back without its own fresh proof through: {leaks:?}"
    );
    // with its proof it opens, under the honest spelling and under a link (the rule must not lock the owner out)
    assert_eq!(
        fetch_proven(&mut c, &f.keys.strong, &path("vault/token.json"))
            .await
            .unwrap(),
        SECRET
    );
    assert_eq!(
        fetch_proven(&mut c, &f.keys.strong, &path("docs/link.json"))
            .await
            .unwrap(),
        SECRET
    );
    assert_eq!(
        fetch_proven(&mut c, &f.keys.strong, &path(&format!("{nfd}/k.txt")))
            .await
            .unwrap(),
        SECRET
    );
}

/// Lab attacks.rs (the swap race of finding_2): a link swapped between the check and the open, for a folder in the
/// middle and for the file itself, at the very moment between them (a hook), and raced by a thread: the file that
/// opened is judged, so the key place still asks for its proof.
// decision: PHN-114
// decision: PHN-483
#[tokio::test(flavor = "multi_thread")]
async fn a_link_swapped_between_the_check_and_the_open_is_judged_as_it_opened() {
    let f = fx(true, None, vec!["~/vault".into()], FileLimits::default()).await;
    let h = f.home.clone();
    std::fs::create_dir_all(h.join("vault")).unwrap();
    std::fs::write(h.join("vault/token.json"), SECRET).unwrap();
    std::fs::create_dir_all(h.join("docs/race")).unwrap();
    std::fs::write(h.join("docs/race/token.json"), b"INNOCENT").unwrap();
    std::fs::write(h.join("docs/racefile"), b"INNOCENT").unwrap();
    std::fs::create_dir_all(h.join("docs/park")).unwrap();
    let mut c = f.open("s1").await;
    // at the exact moment: the check saw an ordinary folder / file; the open meets a link into the vault
    let h2 = h.clone();
    f.set_hook(move |stage, path| {
        if stage != "before_open" {
            return;
        }
        let d = h2.join("docs");
        if path.ends_with("docs/race/token.json") {
            std::fs::rename(d.join("race"), d.join("park/race")).unwrap();
            std::os::unix::fs::symlink(h2.join("vault"), d.join("race")).unwrap();
        } else if path.ends_with("docs/racefile") {
            std::fs::rename(d.join("racefile"), d.join("park/racefile")).unwrap();
            std::os::unix::fs::symlink(h2.join("vault/token.json"), d.join("racefile")).unwrap();
        }
    });
    assert_eq!(
        fetch(&mut c, &f.s("docs/race/token.json"), None).await,
        Err(code::FRESH_UNLOCK_REQUIRED)
    );
    assert_eq!(
        fetch(&mut c, &f.s("docs/racefile"), None).await,
        Err(code::FRESH_UNLOCK_REQUIRED)
    );
    *f.hook.lock().unwrap() = None;
    // and raced: a thread swaps the names back and forth while the phone fetches
    std::fs::remove_file(h.join("docs/race")).unwrap();
    std::fs::remove_file(h.join("docs/racefile")).unwrap();
    std::fs::rename(h.join("docs/park/race"), h.join("docs/race")).unwrap();
    std::fs::rename(h.join("docs/park/racefile"), h.join("docs/racefile")).unwrap();
    std::os::unix::fs::symlink(h.join("vault"), h.join("docs/park/dirlink")).unwrap();
    std::os::unix::fs::symlink(h.join("vault/token.json"), h.join("docs/park/filelink")).unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (h3, stop2) = (h.clone(), stop.clone());
    let swapper = std::thread::spawn(move || {
        let d = h3.join("docs");
        while !stop2.load(Ordering::Relaxed) {
            for (live, parked) in [("race", "park/dirlink"), ("racefile", "park/filelink")] {
                let _ = std::fs::rename(d.join(live), d.join("park/tmp"));
                let _ = std::fs::rename(d.join(parked), d.join(live));
                let _ = std::fs::rename(d.join("park/tmp"), d.join(parked));
            }
            std::thread::yield_now();
        }
    });
    let mut won = 0;
    for _ in 0..150 {
        for target in ["docs/race/token.json", "docs/racefile"] {
            if fetch(&mut c, &f.s(target), None).await.as_deref() == Ok(SECRET) {
                won += 1;
            }
        }
    }
    stop.store(true, Ordering::Relaxed);
    swapper.join().unwrap();
    assert_eq!(
        won, 0,
        "fetches returned the key place without a fresh proof"
    );
}

/// Lab attacks.rs finding_2_ferbs_own_data_is_never_shown_under_any_name: FERB's own state, under any name and with a
/// fresh fingerprint proof for that very spelling, is never opened or listed (rule 6: NEVER_LIFTED). Its workspace,
/// which the model's tools may read, is the owner's like any folder.
// decision: PHN-114
// decision: PHN-482
#[tokio::test]
async fn ferb_s_own_data_is_never_shown_under_any_name() {
    let f = plain_fx().await;
    let h = &f.home;
    std::os::unix::fs::symlink(h.join(".ferb/ferb.db"), h.join("docs/l.db")).unwrap();
    std::os::unix::fs::symlink(h.join(".ferb"), h.join("docs/m")).unwrap();
    std::fs::hard_link(h.join(".ferb/ferb.db"), h.join("docs/hard.db")).unwrap();
    let meta = std::fs::metadata(h.join(".ferb/ferb.db")).unwrap();
    let mut c = f.open("s1").await;
    let spellings = [
        f.s(".ferb/ferb.db"),
        f.s(".FERB/FERB.DB"),
        f.s("docs/l.db"),
        f.s("docs/m/ferb.db"),
        f.s("docs/hard.db"),
        f.s(".ferb/workspace/uploads/u.txt"),
        "~/.ferb/config.toml".into(),
        format!("/System/Volumes/Data{}", f.s(".ferb/ferb.db")),
        format!("/.vol/{}/{}", meta.dev(), meta.ino()),
    ];
    for spelled in &spellings {
        assert_eq!(
            fetch(&mut c, spelled, None).await,
            Err(code::FILE_NOT_ALLOWED),
            "{spelled}"
        );
        let proven = fetch_proven(&mut c, &f.keys.strong, spelled).await;
        assert_eq!(
            proven,
            Err(code::FILE_NOT_ALLOWED),
            "{spelled} with a fresh proof"
        );
    }
    assert_eq!(
        fetch(&mut c, &f.s(".ferb/ferb.db"), None).await,
        Err(code::FILE_NOT_ALLOWED)
    );
    assert_eq!(
        fetch_proven(&mut c, &f.keys.strong, &f.s("docs/hard.db")).await,
        Err(code::FILE_NOT_ALLOWED)
    );
    for spelled in [
        f.s(".ferb"),
        f.s(".FERB"),
        f.s("docs/m"),
        format!("/System/Volumes/Data{}", f.s(".ferb")),
    ] {
        assert_eq!(
            list(&mut c, &spelled, None).await,
            Err(code::FILE_NOT_ALLOWED),
            "{spelled}"
        );
    }
    assert_eq!(
        fetch(&mut c, &f.s(".ferb/workspace/w.txt"), None)
            .await
            .unwrap(),
        b"work"
    );
}

/// Lab attacks.rs finding_2_a_key_place_asks_for_its_proof_before_it_says_anything: a key place's file that does not
/// exist answers `fresh_unlock_required`, not `not_found`: nothing is said about it before the proof.
// decision: PHN-121
#[tokio::test]
async fn a_key_place_asks_for_its_proof_before_it_says_anything() {
    let f = plain_fx().await;
    let mut c = f.open("s1").await;
    for spelled in [
        f.s(".ssh/ferb-test-no-such-file"),
        "~/.ssh/ferb-test-no-such-file".into(),
        f.s(".aws/ferb-test-no-such-file"),
        f.s("docs/no-such.pem"),
    ] {
        assert_eq!(
            fetch(&mut c, &spelled, None).await,
            Err(code::FRESH_UNLOCK_REQUIRED),
            "{spelled}"
        );
    }
    assert_eq!(
        fetch(&mut c, &f.s("docs/no-such.txt"), None).await,
        Err(code::NOT_FOUND)
    );
}

/// Lab attacks_home.rs finding_2_a_wrong_or_empty_home_does_not_switch_the_rule_off: `~` is the home the lists were
/// built for, never this process's `HOME` (here another folder): the lists' `~/.ssh` asks for its proof.
// decision: PHN-121
#[tokio::test]
async fn the_lists_home_not_the_process_s_home_decides() {
    let f = plain_fx().await;
    assert_ne!(
        std::env::var("HOME").ok(),
        Some(f.home.to_string_lossy().into_owned())
    );
    let mut c = f.open("s1").await;
    for place in [".ssh", ".aws", ".gnupg"] {
        assert_eq!(
            fetch(&mut c, &format!("~/{place}/k.txt"), None).await,
            Err(code::FRESH_UNLOCK_REQUIRED),
            "{place}"
        );
    }
    assert_eq!(
        fetch(&mut c, "~/docs/plain.txt", None).await.unwrap(),
        b"plain"
    );
}

/// Lab attacks.rs finding_11_an_upload_needs_the_unlock_and_is_quarantined: a locked phone sends nothing and leaves
/// nothing; an unlocked phone's file lands 0600, marked as downloaded (macOS asks before a file that runs when opened
/// runs: rule 5).
// decision: PHN-127
// decision: PHN-484
#[tokio::test]
async fn an_upload_needs_the_unlock_and_is_quarantined() {
    let f = plain_fx().await;
    let mut locked = Conn::phone(&f.w, P1, "locked").await;
    let script = b"#!/bin/sh\necho hi\n";
    assert_eq!(
        upload(&mut locked, json!({"name": "run-me.command"}), script).await,
        Err(code::LOCKED)
    );
    assert!(f.inbox_names().is_empty());
    let mut c = f.open("s1").await;
    let up = upload(&mut c, json!({"name": "run-me.command"}), script)
        .await
        .unwrap();
    let saved = PathBuf::from(up["saved"].as_str().unwrap());
    assert!(
        saved.starts_with(std::fs::canonicalize(&f.inbox).unwrap()),
        "{saved:?}"
    );
    assert!(saved.to_string_lossy().ends_with("run-me.command"));
    assert_eq!(std::fs::read(&saved).unwrap(), script);
    assert_eq!(
        std::fs::metadata(&saved).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(up["sha256"], json!(b64u(&Sha256::digest(script))));
    let q = xattr(&saved, "com.apple.quarantine").expect("the upload carries no quarantine mark");
    assert!(
        q.starts_with("0081;") && q.contains(";FERB phone link;"),
        "{q}"
    );
    assert!(
        !f.inbox_names().iter().any(|n| n.ends_with(".ferb-part")),
        "a part file stayed behind"
    );
    // two files of the same name never overwrite each other
    let again = upload(&mut c, json!({"name": "run-me.command"}), b"other")
        .await
        .unwrap();
    assert_ne!(again["saved"], up["saved"]);
    assert_eq!(std::fs::read(&saved).unwrap(), script);
}

fn xattr(p: &Path, name: &str) -> Option<String> {
    let (c, n) = (
        std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).unwrap(),
        std::ffi::CString::new(name).unwrap(),
    );
    let mut buf = vec![0u8; 512];
    // SAFETY: valid NUL-terminated strings and a writable buffer of the given size.
    let n = unsafe {
        libc::getxattr(
            c.as_ptr(),
            n.as_ptr(),
            buf.as_mut_ptr().cast(),
            buf.len(),
            0,
            0,
        )
    };
    (n >= 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

/// Lab hardening.rs the_daemon_trusts_nothing_the_bridge_says_about_a_phone (its file part): no claim of "unlocked" or
/// "fresh" in the parameters opens anything; an unknown phone gets nothing.
// decision: PHN-124
#[tokio::test]
async fn no_claim_of_unlocked_or_fresh_opens_a_file() {
    let f = plain_fx().await;
    let mut locked = Conn::phone(&f.w, P1, "locked").await;
    for (m, p) in [
        (
            "phone.files",
            json!({"path": f.s("docs"), "fresh": true, "unlocked": true}),
        ),
        ("phone.tree", json!({"path": f.s("docs"), "fresh": true})),
        (
            "phone.read",
            json!({"path": f.s("docs/plain.txt"), "unlocked": true}),
        ),
        ("phone.put_group", json!({"name": "x"})),
        ("phone.put_begin", json!({"name": "x", "size": 1})),
    ] {
        assert_eq!(
            locked.call(m, p, None).await.unwrap_err().0,
            code::LOCKED,
            "{m}"
        );
    }
    assert_eq!(
        Conn::open(
            &f.w,
            json!({"role": "phone", "device": "stranger", "session": "s"})
        )
        .await
        .err()
        .unwrap()
        .0,
        code::UNKNOWN_PHONE
    );
    let mut c = f.open("s1").await;
    assert_eq!(
        fetch(&mut c, &f.s("docs/plain.txt"), None).await.unwrap(),
        b"plain"
    );
}

/// Lab hardening.rs key_places_and_sensitive_changes_take_the_fingerprint_key_only (its key-place part): where the
/// phone has a fingerprint key only its signature opens a key place; a phone without one signs with its lock key.
// decision: PHN-129
// decision: PHN-133
#[tokio::test]
async fn a_key_place_takes_the_fingerprint_key_only_where_the_phone_has_one() {
    let f = plain_fx().await;
    let mut c = f.open("s1").await;
    let k = f.s(".ssh/k.txt");
    let pin = place_proof(&mut c, &f.keys.lock, "link.get", &k).await;
    let refused = c
        .call("phone.read", json!({"path": k}), Some(pin))
        .await
        .unwrap_err();
    assert_eq!(refused.0, code::UNLOCK_INVALID);
    assert!(refused.1.contains("fingerprint"), "{}", refused.1);
    assert_eq!(
        fetch_proven(&mut c, &f.keys.strong, &k).await.unwrap(),
        SECRET
    );
    // a phone paired without a fingerprint key: its lock key
    let g = fx(false, None, Vec::new(), FileLimits::default()).await;
    let mut d = g.open("s1").await;
    let k = g.s(".ssh/k.txt");
    assert_eq!(
        fetch_proven(&mut d, &g.keys.lock, &k).await.unwrap(),
        SECRET
    );
}

/// A proof is for one access of one path: another path, a second use, another session or the wrong method name are
/// refused.
// decision: PHN-121
// decision: PHN-108
#[tokio::test]
async fn a_proof_for_another_path_or_a_second_use_is_refused() {
    let f = plain_fx().await;
    std::fs::write(f.home.join(".ssh/b.txt"), SECRET).unwrap();
    let mut c = f.open("s1").await;
    let (a, b) = (f.s(".ssh/k.txt"), f.s(".ssh/b.txt"));
    let p = place_proof(&mut c, &f.keys.strong, "link.get", &a).await;
    assert_eq!(
        fetch(&mut c, &b, Some(p.clone())).await,
        Err(code::UNLOCK_INVALID)
    );
    let p = place_proof(&mut c, &f.keys.strong, "link.get", &a).await;
    assert_eq!(fetch(&mut c, &a, Some(p.clone())).await.unwrap(), SECRET);
    assert_eq!(
        fetch(&mut c, &a, Some(p)).await,
        Err(code::UNLOCK_INVALID),
        "used twice"
    );
    let mut other = f.open("s2").await;
    let p = place_proof(&mut other, &f.keys.strong, "link.get", &a).await;
    assert_eq!(
        fetch(&mut c, &a, Some(p)).await,
        Err(code::UNLOCK_INVALID),
        "another session's"
    );
    let p = place_proof(&mut c, &f.keys.strong, "link.files", &a).await;
    assert_eq!(
        fetch(&mut c, &a, Some(p)).await,
        Err(code::UNLOCK_INVALID),
        "a listing's proof"
    );
    // the same path spelled otherwise is another call: its proof is for the path as sent
    let p = place_proof(&mut c, &f.keys.strong, "link.get", &a).await;
    let tilde = a.replacen(&f.home.to_string_lossy().into_owned(), "~", 1);
    assert_eq!(
        fetch(&mut c, &tilde, Some(p)).await,
        Err(code::UNLOCK_INVALID)
    );
}

/// A file with more than one name asks for its own fingerprint (PHN-140), even when every name is ordinary; when a
/// walk of FERB's folder cannot finish within its bound the answer is closed.
// decision: PHN-140
// decision: PHN-482
#[tokio::test]
async fn a_hard_linked_file_asks_for_its_own_fingerprint() {
    let f = plain_fx().await;
    std::fs::hard_link(f.home.join("docs/plain.txt"), f.home.join("docs/twin.txt")).unwrap();
    std::fs::hard_link(f.home.join(".ssh/k.txt"), f.home.join("docs/innocent.txt")).unwrap();
    let mut c = f.open("s1").await;
    assert_eq!(
        fetch(&mut c, &f.s("docs/plain.txt"), None).await,
        Err(code::FRESH_UNLOCK_REQUIRED)
    );
    assert_eq!(
        fetch(&mut c, &f.s("docs/innocent.txt"), None).await,
        Err(code::FRESH_UNLOCK_REQUIRED)
    );
    assert_eq!(
        fetch_proven(&mut c, &f.keys.strong, &f.s("docs/twin.txt"))
            .await
            .unwrap(),
        b"plain"
    );
    let listed = list(&mut c, &f.s("docs"), None).await.unwrap();
    let twin = listed["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "twin.txt")
        .unwrap();
    assert_eq!(twin["locked"], true);
    let g = fx(
        true,
        None,
        Vec::new(),
        FileLimits {
            own_names_max: 2,
            ..FileLimits::default()
        },
    )
    .await;
    std::fs::hard_link(g.home.join("docs/plain.txt"), g.home.join("docs/twin.txt")).unwrap();
    let mut d = g.open("s1").await;
    assert_eq!(
        fetch_proven(&mut d, &g.keys.strong, &g.s("docs/twin.txt")).await,
        Err(code::FILE_NOT_ALLOWED),
        "a walk that cannot finish is closed"
    );
}

/// Only ordinary files are sent: a pipe (it never blocks the daemon), a socket, a device and a link to a pipe are
/// refused; a folder and an app bundle are listed and walked, never fetched as one.
// decision: PHN-114
// decision: PHN-483
#[tokio::test]
async fn pipes_sockets_devices_and_folders_are_never_sent_as_files() {
    let f = plain_fx().await;
    let h = &f.home;
    let pipe = std::ffi::CString::new(f.s("docs/pipe")).unwrap();
    // SAFETY: a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(pipe.as_ptr(), 0o600) }, 0);
    std::os::unix::fs::symlink(h.join("docs/pipe"), h.join("docs/pipelink")).unwrap();
    let _sock = std::os::unix::net::UnixListener::bind(h.join("docs/s")).unwrap();
    std::fs::create_dir_all(h.join("docs/App.app/Contents")).unwrap();
    std::fs::write(h.join("docs/App.app/Contents/Info.plist"), b"plist").unwrap();
    let mut c = f.open("s1").await;
    for p in [
        f.s("docs/pipe"),
        f.s("docs/pipelink"),
        f.s("docs/s"),
        "/dev/null".into(),
    ] {
        assert_eq!(fetch(&mut c, &p, None).await, Err(code::NOT_A_FILE), "{p}");
    }
    assert_eq!(
        fetch(&mut c, &f.s("docs/App.app"), None).await,
        Err(code::NOT_A_FILE)
    );
    assert_eq!(
        list(&mut c, &f.s("docs/plain.txt"), None).await,
        Err(code::NOT_A_FILE)
    );
    let listed = list(&mut c, &f.s("docs"), None).await.unwrap();
    let app = listed["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "App.app")
        .unwrap();
    assert_eq!(app["dir"], true);
    let tree = c
        .ok("phone.tree", json!({"path": f.s("docs/App.app")}))
        .await;
    assert_eq!(
        tree["files"],
        json!([{"rel": "Contents/Info.plist", "size": 5}])
    );
}

/// Empty, sparse and huge files: the size comes from the file that opened; a huge one is described before a byte is
/// read, and a phone that leaves stops the hash within a slice.
// decision: PHN-116
#[tokio::test]
async fn empty_sparse_and_huge_files() {
    let f = plain_fx().await;
    std::fs::write(f.home.join("docs/empty"), b"").unwrap();
    let sparse = std::fs::File::create(f.home.join("docs/sparse")).unwrap();
    sparse.set_len(4 << 20).unwrap();
    let huge = std::fs::File::create(f.home.join("docs/huge")).unwrap();
    huge.set_len(5 << 30).unwrap();
    let mut c = f.open("s1").await;
    assert_eq!(fetch(&mut c, &f.s("docs/empty"), None).await.unwrap(), b"");
    assert_eq!(c.events.len(), 2, "meta and hash only");
    let got = fetch(&mut c, &f.s("docs/sparse"), None).await.unwrap();
    assert_eq!(got.len(), 4 << 20);
    assert!(got.iter().all(|b| *b == 0));
    // resumed from an offset: the rest only
    let rest = c
        .call(
            "phone.read",
            json!({"path": f.s("docs/plain.txt"), "offset": 2}),
            None,
        )
        .await;
    assert!(rest.is_ok());
    assert_eq!(body(&c.events), b"ain");
    let first = first_event(&f, "s1", json!({"path": f.s("docs/huge")})).await;
    assert_eq!(first["meta"]["size"], json!(5u64 << 30));
}

/// One `phone.read` on a raw connection of `session`: its first event, then the connection is dropped.
async fn first_event(f: &Fx, session: &str, params: Value) -> Value {
    let s = tokio::net::UnixStream::connect(f.w.socket()).await.unwrap();
    let (r, mut wr) = s.into_split();
    let mut r = BufReader::new(r);
    for (id, method, p) in [
        (
            1,
            "auth.hello",
            json!({"role": "phone", "device": P1, "session": session}),
        ),
        (2, "phone.read", params),
    ] {
        let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": p});
        wr.write_all(format!("{req}\n").as_bytes()).await.unwrap();
        let mut line = String::new();
        r.read_line(&mut line).await.unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        if id == 2 {
            return v["params"].clone();
        }
    }
    unreachable!()
}

/// A file that changes while it is sent stops with `file_changed` (its size, time and identity were pinned when it
/// opened); the audit says so.
// decision: PHN-302
// decision: PHN-483
#[tokio::test]
async fn a_file_that_changes_while_it_is_sent_stops_with_file_changed() {
    let f = plain_fx().await;
    let p = f.home.join("docs/grows.bin");
    std::fs::write(&p, vec![7u8; 200 * 1024]).unwrap();
    let n = Arc::new(AtomicUsize::new(0));
    let (p2, n2) = (p.clone(), n.clone());
    f.set_hook(move |stage, _| {
        if stage == "piece" && n2.fetch_add(1, Ordering::SeqCst) == 1 {
            use std::io::Write;
            let mut w = std::fs::OpenOptions::new().append(true).open(&p2).unwrap();
            w.write_all(b"more").unwrap();
        }
    });
    let mut c = f.open("s1").await;
    let got = c
        .call("phone.read", json!({"path": f.s("docs/grows.bin")}), None)
        .await;
    assert_eq!(got.unwrap_err().0, code::FILE_CHANGED);
    assert!(audit_text(&f.w).contains("file_changed"));
}

/// A removed phone stops mid-transfer, both ways: its fetch ends with `unknown_phone`, its upload's part is deleted at
/// once and its next chunk refused.
// decision: PHN-419
// decision: PHN-127
#[tokio::test]
async fn a_removed_phone_stops_mid_transfer_and_its_parts_go() {
    let f = plain_fx().await;
    let mut c = f.open("s1").await;
    let b = c
        .call(
            "phone.put_begin",
            json!({"name": "big.bin", "size": 100_000}),
            None,
        )
        .await
        .unwrap();
    let id = b["upload_id"].as_str().unwrap().to_owned();
    let d = base64::engine::general_purpose::STANDARD.encode([1u8; 1000]);
    c.ok("phone.put_chunk", json!({"upload_id": id, "d": d}))
        .await;
    assert!(f.inbox_names().iter().any(|n| n.ends_with(".ferb-part")));
    std::fs::write(f.home.join("docs/long.bin"), vec![3u8; 300 * 1024]).unwrap();
    let (phones, n) = (f.w.phones.clone(), Arc::new(AtomicUsize::new(0)));
    let n2 = n.clone();
    f.set_hook(move |stage, _| {
        if stage == "piece" && n2.fetch_add(1, Ordering::SeqCst) == 2 {
            let mut p = serde_json::Map::new();
            p.insert("id".into(), json!(P1));
            phones.remove(&app_session(), &p).unwrap();
        }
    });
    let got = c
        .call("phone.read", json!({"path": f.s("docs/long.bin")}), None)
        .await;
    assert_eq!(got.unwrap_err().0, code::UNKNOWN_PHONE);
    assert!(
        !f.inbox_names().iter().any(|n| n.ends_with(".ferb-part")),
        "the part stayed"
    );
    assert_eq!(
        c.call("phone.put_chunk", json!({"upload_id": id, "d": d}), None)
            .await
            .unwrap_err()
            .0,
        code::UNKNOWN_PHONE
    );
}

/// A transfer under way goes on after the phone locks (nothing interrupts it: its chunks ride the begin's unlock); a
/// new one waits for the unlock.
// decision: PHN-127
// decision: PHN-125
#[tokio::test]
async fn a_transfer_under_way_goes_on_after_the_phone_locks() {
    let f = plain_fx().await;
    let mut c = f.open("s1").await;
    let b = c
        .call("phone.put_begin", json!({"name": "a.bin", "size": 6}), None)
        .await
        .unwrap();
    let id = b["upload_id"].as_str().unwrap().to_owned();
    let d = |s: &[u8]| base64::engine::general_purpose::STANDARD.encode(s);
    c.ok("phone.put_chunk", json!({"upload_id": id, "d": d(b"abc")}))
        .await;
    c.ok("phone.lock", json!({})).await;
    c.ok("phone.put_chunk", json!({"upload_id": id, "d": d(b"def")}))
        .await;
    let done = c.ok("phone.put_end", json!({"upload_id": id})).await;
    assert_eq!(
        std::fs::read(done["saved"].as_str().unwrap()).unwrap(),
        b"abcdef"
    );
    assert_eq!(
        c.call("phone.put_begin", json!({"name": "b", "size": 1}), None)
            .await
            .unwrap_err()
            .0,
        code::LOCKED
    );
    // a phone's own session's chunks only: another phone cannot write into it
}

/// Lab hardening.rs uploads_stop_and_go_on_folders_arrive_whole_and_the_disk_keeps_its_floor, part 1: an upload stopped
/// part way goes on where it stopped (the bytes the part file holds), by the same phone only, and arrives whole with
/// its hash; a hash the phone gives that differs keeps nothing.
// decision: PHN-127
// decision: PHN-139
// decision: PHN-484
#[tokio::test]
async fn an_upload_stops_and_goes_on_by_the_same_phone_only() {
    let f = plain_fx().await;
    let big: Vec<u8> = (0..100_000u32).map(|i| (i % 249) as u8).collect();
    let mut c = f.open("s1").await;
    let b = c
        .call(
            "phone.put_begin",
            json!({"name": "big.bin", "size": big.len()}),
            None,
        )
        .await
        .unwrap();
    let id = b["upload_id"].as_str().unwrap().to_owned();
    let enc = |s: &[u8]| base64::engine::general_purpose::STANDARD.encode(s);
    c.ok(
        "phone.put_chunk",
        json!({"upload_id": id, "d": enc(&big[..16384])}),
    )
    .await;
    c.ok(
        "phone.put_chunk",
        json!({"upload_id": id, "d": enc(&big[16384..32768])}),
    )
    .await;
    let paused = c
        .ok("phone.put_end", json!({"upload_id": id, "abort": true}))
        .await;
    assert_eq!(
        (paused["kept"].clone(), paused["got"].clone()),
        (json!(true), json!(32768))
    );
    // another phone cannot go on with it, nor attach it
    pair(&f.w, P2, &Keys::new(2), true, false).await;
    let mut other = Conn::phone(&f.w, P2, "o1").await;
    other.unlock(&Keys::new(2)).await;
    assert_eq!(
        upload(&mut other, json!({"name": "big.bin", "resume": id}), &big).await,
        Err(code::NOT_FOUND)
    );
    // the same phone goes on from 32768, on another connection
    let mut again = f.open("s2").await;
    let done = upload(&mut again, json!({"name": "big.bin", "resume": id}), &big)
        .await
        .unwrap();
    assert_eq!(std::fs::read(done["saved"].as_str().unwrap()).unwrap(), big);
    assert_eq!(done["sha256"], json!(b64u(&Sha256::digest(&big))));
    assert_eq!(
        other
            .call(
                "chat.send",
                json!({"text": "look", "attachments": [id]}),
                None
            )
            .await
            .unwrap_err()
            .0,
        code::NOT_FOUND
    );
    // a hash that differs: nothing kept
    let b = c
        .call("phone.put_begin", json!({"name": "x", "size": 3}), None)
        .await
        .unwrap();
    let id = b["upload_id"].as_str().unwrap().to_owned();
    c.ok(
        "phone.put_chunk",
        json!({"upload_id": id, "d": enc(b"abc")}),
    )
    .await;
    let wrong = c
        .call(
            "phone.put_end",
            json!({"upload_id": id, "sha256": b64u(&Sha256::digest(b"abd"))}),
            None,
        )
        .await;
    assert_eq!(wrong.unwrap_err().0, code::BAD_REQUEST);
    assert!(!f
        .inbox_names()
        .iter()
        .any(|n| n.ends_with("-x") || n.ends_with(".ferb-part")));
    // more bytes than announced are refused
    let b = c
        .call("phone.put_begin", json!({"name": "y", "size": 2}), None)
        .await
        .unwrap();
    let id = b["upload_id"].as_str().unwrap().to_owned();
    assert_eq!(
        c.call(
            "phone.put_chunk",
            json!({"upload_id": id, "d": enc(b"abc")}),
            None
        )
        .await
        .unwrap_err()
        .0,
        code::BAD_REQUEST
    );
}

/// Part 2: a folder arrives as one folder in the upload folder, every file at its place; no name climbs out of it, a
/// second file of the same name overwrites nothing, and the folder rides with a message as one attachment.
// decision: PHN-127
// decision: PHN-484
// decision: PHN-485
#[tokio::test]
async fn a_folder_arrives_whole_nothing_climbs_out_and_it_rides_with_a_message() {
    let f = plain_fx().await;
    let mut c = f.open("s1").await;
    let group = c
        .ok("phone.put_group", json!({"name": "Holiday 2026"}))
        .await;
    let gid = group["group"].as_str().unwrap().to_owned();
    let folder = PathBuf::from(group["saved"].as_str().unwrap());
    assert!(folder.starts_with(std::fs::canonicalize(&f.inbox).unwrap()));
    for (rel, body) in [("a.txt", "A"), ("sub/b.txt", "B"), ("sub/deep/c.txt", "C")] {
        upload(
            &mut c,
            json!({"name": "x", "group": gid, "rel": rel}),
            body.as_bytes(),
        )
        .await
        .unwrap();
    }
    assert_eq!(std::fs::read(folder.join("sub/deep/c.txt")).unwrap(), b"C");
    for evil in [
        "../escape.txt",
        "sub/../../escape.txt",
        "/etc/escape.txt",
        "sub//x/../../../escape.txt",
        "./a",
    ] {
        let r = upload(
            &mut c,
            json!({"name": "escape.txt", "group": gid, "rel": evil}),
            b"E",
        )
        .await;
        assert!(r.is_err(), "{evil}: {r:?}");
    }
    // a link planted inside the folder is never followed out of it
    std::os::unix::fs::symlink(f.home.join("docs"), folder.join("planted")).unwrap();
    assert_eq!(
        upload(
            &mut c,
            json!({"name": "x", "group": gid, "rel": "planted/evil.txt"}),
            b"E"
        )
        .await,
        Err(code::FILE_NOT_ALLOWED)
    );
    assert!(!f.home.join("docs/evil.txt").exists() && !f.inbox.join("escape.txt").exists());
    assert_eq!(
        upload(
            &mut c,
            json!({"name": "x", "group": gid, "rel": "a.txt"}),
            b"Z"
        )
        .await,
        Err(code::BAD_REQUEST)
    );
    assert_eq!(std::fs::read(folder.join("a.txt")).unwrap(), b"A");
    // the folder rides with a message as one attachment (the phone names no path on the Mac)
    c.call(
        "chat.send",
        json!({"text": "the photos", "attachments": [gid]}),
        None,
    )
    .await
    .unwrap();
    let sent = &c.events[0]["params"];
    assert_eq!(sent["input_paths"], json!([folder.to_string_lossy()]));
    assert!(sent.get("attachments").is_none());
    assert_eq!(sent["client_id"], json!(format!("phone:{P1}")));
    // a path on the Mac never rides along
    assert!(c
        .call(
            "chat.send",
            json!({"text": "x", "input_paths": ["/etc/hosts"]}),
            None
        )
        .await
        .is_err());
}

/// Part 3 (Mac to phone, a folder): its files listed in one call, key files left out and counted.
// decision: PHN-131
#[tokio::test]
async fn a_folder_s_tree_leaves_key_files_out_and_counts_them() {
    let f = plain_fx().await;
    let d = f.home.join("docs2");
    std::fs::create_dir_all(d.join("sub")).unwrap();
    for (rel, body) in [
        ("r.txt", "R"),
        ("sub/s.txt", "S"),
        (".env", "KEY=1"),
        ("sub/id_rsa", "K"),
    ] {
        std::fs::write(d.join(rel), body).unwrap();
    }
    let mut c = f.open("s1").await;
    let tree = c.ok("phone.tree", json!({"path": f.s("docs2")})).await;
    let rels: Vec<&str> = tree["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["rel"].as_str().unwrap())
        .collect();
    assert_eq!(
        (rels, tree["skipped"].as_u64()),
        (vec!["r.txt", "sub/s.txt"], Some(2)),
        "{tree}"
    );
}

/// Part 4: the free-disk floor. An upload that would leave less than 1 GiB free (PHN-139) is refused before its first
/// byte and leaves nothing (the free space from an injected reader).
// decision: PHN-139
#[tokio::test]
async fn the_disk_keeps_its_floor() {
    let f = plain_fx().await;
    let floor = FileLimits::default().upload_free_floor;
    assert_eq!(floor, 1 << 30);
    f.free.store(floor + 10, Ordering::SeqCst);
    let mut c = f.open("s1").await;
    let refused = c
        .call(
            "phone.put_begin",
            json!({"name": "small.txt", "size": 11}),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(refused.0, code::MAC_DISK_FULL);
    assert!(refused.1.contains("free"), "{}", refused.1);
    assert!(!f.inbox_names().iter().any(|n| n.contains("small")));
    assert!(upload(&mut c, json!({"name": "small.txt"}), &[1u8; 10])
        .await
        .is_ok());
}

/// A paused upload is kept one hour (PHN-139), then goes; the part files a daemon before this one left are swept by
/// their age; a session's end pauses what it had under way.
// decision: PHN-139
// decision: PHN-484
#[tokio::test]
async fn a_paused_upload_is_kept_one_hour_then_goes() {
    let f = plain_fx().await;
    std::fs::create_dir_all(&f.inbox).unwrap();
    let old = f.inbox.join(".left-by-another.ferb-part");
    let young = f.inbox.join(".young.ferb-part");
    std::fs::write(&old, b"x").unwrap();
    std::fs::write(&young, b"x").unwrap();
    let wall = f.w.clock.now().timestamp() as u64;
    let when = |s: u64| std::time::UNIX_EPOCH + std::time::Duration::from_secs(s);
    std::fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(when(wall - 7200))
        .unwrap();
    std::fs::File::options()
        .write(true)
        .open(&young)
        .unwrap()
        .set_modified(when(wall - 60))
        .unwrap();
    let mut c = f.open("s1").await;
    let b = c
        .call("phone.put_begin", json!({"name": "p", "size": 10}), None)
        .await
        .unwrap();
    let id = b["upload_id"].as_str().unwrap().to_owned();
    assert!(!old.exists() && young.exists());
    c.ok("phone.put_chunk", json!({"upload_id": id, "d": "YWJj"}))
        .await;
    // the link went: the session's end pauses it
    c.ok("phone.session_end", json!({})).await;
    let mut c = f.open("s2").await;
    f.w.clock.advance(3599.0);
    c.unlock(&f.keys).await;
    let resumed = c
        .call(
            "phone.put_begin",
            json!({"name": "p", "size": 10, "resume": id}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(resumed["offset"], 3);
    c.ok("phone.put_end", json!({"upload_id": id, "abort": true}))
        .await;
    f.w.clock.advance(3601.0);
    c.unlock(&f.keys).await;
    upload(&mut c, json!({"name": "q"}), b"q").await.unwrap();
    assert_eq!(
        c.call(
            "phone.put_begin",
            json!({"name": "p", "size": 10, "resume": id}),
            None
        )
        .await
        .unwrap_err()
        .0,
        code::NOT_FOUND
    );
    let parts: Vec<String> = f
        .inbox_names()
        .into_iter()
        .filter(|n| n.ends_with(".ferb-part"))
        .collect();
    assert_eq!(parts, vec![".young.ferb-part".to_owned()], "{parts:?}");
}

/// Lab hardening.rs a_phone_that_takes_unusually_much_is_said_so_once_and_the_audit_is_kept_by_age (its notice):
/// more than 40 files, or more than 1 GiB, within 10 minutes is said once per burst (a bus event for FERB.app, a
/// journal row, an audit row); never a cap; after a quiet window a new burst is told again.
// decision: PHN-128
// decision: PHN-138
// decision: PHN-485
#[tokio::test]
async fn a_phone_that_takes_unusually_much_is_said_so_once_per_burst() {
    let f = plain_fx().await;
    let l = FileLimits::default();
    assert_eq!(
        (
            l.fetch_notice_files,
            l.fetch_notice_bytes,
            l.fetch_notice_window_s
        ),
        (40, 1 << 30, 600.0)
    );
    let notices = |f: &Fx| {
        f.w.published
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| k == "phone.heavy_fetch")
            .count()
    };
    let mut c = f.open("s1").await;
    for _ in 0..52 {
        fetch(&mut c, &f.s("docs/plain.txt"), None).await.unwrap();
        f.w.clock.advance(1.0);
    }
    assert_eq!(notices(&f), 1, "once per burst, not per file");
    let ev =
        f.w.published
            .lock()
            .unwrap()
            .iter()
            .find(|(k, _)| k == "phone.heavy_fetch")
            .unwrap()
            .1
            .clone();
    assert_eq!(
        (ev["files"].clone(), ev["phone"].clone()),
        (json!(41), json!(P1))
    );
    assert!(ev["message"].as_str().unwrap().contains("41 files"), "{ev}");
    assert!(f
        .w
        .journal
        .lock()
        .unwrap()
        .iter()
        .any(|(k, _)| k == "phone.heavy_fetch"));
    assert!(audit_text(&f.w).contains("phone.heavy_fetch"));
    // a quiet window, then a new burst is told again
    f.w.clock.advance(601.0);
    c.unlock(&f.keys).await;
    for _ in 0..41 {
        fetch(&mut c, &f.s("docs/plain.txt"), None).await.unwrap();
    }
    assert_eq!(notices(&f), 2);
    // by bytes: one file over 1 GiB is enough (described, not sent here)
    let g = plain_fx().await;
    let huge = std::fs::File::create(g.home.join("docs/huge")).unwrap();
    huge.set_len((1 << 30) + 1).unwrap();
    let _d = g.open("s1").await;
    first_event(&g, "s1", json!({"path": g.s("docs/huge")})).await;
    let n =
        g.w.published
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| k == "phone.heavy_fetch")
            .count();
    assert_eq!(n, 1);
}

/// Every browse, fetch and upload is one audit row with its path and bytes, never a file's content; a refused one too.
// decision: PHN-114
#[tokio::test]
async fn every_browse_fetch_and_upload_is_audited_with_its_path_never_its_content() {
    let f = plain_fx().await;
    let mut c = f.open("s1").await;
    list(&mut c, &f.s("docs"), None).await.unwrap();
    c.ok("phone.tree", json!({"path": f.s("docs")})).await;
    fetch(&mut c, &f.s("docs/plain.txt"), None).await.unwrap();
    assert!(fetch(&mut c, &f.s(".ssh/k.txt"), None).await.is_err());
    fetch_proven(&mut c, &f.keys.strong, &f.s(".ssh/k.txt"))
        .await
        .unwrap();
    let up = upload(&mut c, json!({"name": "note.txt"}), b"UPLOADED-CONTENT")
        .await
        .unwrap();
    let mut locked = Conn::phone(&f.w, P1, "locked").await;
    assert!(locked
        .call("phone.files", json!({"path": f.s("docs")}), None)
        .await
        .is_err());
    let audit = audit_text(&f.w);
    for needle in [
        "phone.files private ok",
        "phone.tree private ok",
        "phone.read private ok",
        "phone.read private fresh_unlock_required",
        "phone.put_begin plain ok",
        "phone.put_end plain ok",
        "phone.files private locked",
    ] {
        assert!(audit.contains(needle), "{needle} in\n{audit}");
    }
    assert!(audit.contains(&f.s("docs/plain.txt")));
    assert!(audit.contains(up["saved"].as_str().unwrap()));
    for secret in ["plain\"", "SECRET-KEY-MATERIAL", "UPLOADED-CONTENT"] {
        assert!(!audit.contains(secret), "{secret}");
    }
}
