//! Uploads (decisions: HAND-500, HAND-501, HAND-502): a port of `_TabDriver._upload`,
//! `_upload_paths`, `_receipt`, `_upload_limit`, `_upload_bytes`, `_identify` and `_sources`, with
//! `uploads.py`'s `source_path` and `tab_key`.
//!
//! The browser sets files on a field through FERB's debugger only when the owner allowed FERB access
//! to file URLs (the hello's `file_access`); otherwise, and for a drop zone, an extension from 0.19.9
//! gets the bytes through the page. Each file is read through one handle that never follows a link
//! at its last step, checked by the protection lists as macOS opened it (rule 10) before a byte is
//! read. The bytes go nowhere else: no copy on disk, nothing logged.
//!
//! Seams: [`Files`] (opening and reading the files: [`unix::UnixFiles`] on the Mac), and the daemon's
//! `uploads.Uploads` staging ([`crate::ops::Staging`], rule 17's copies: not ported here).

use std::sync::Arc;

use ferb_core::types::{ActionResult, Dict, PathCheck};
use serde_json::{json, Map, Value};

use crate::driver::{Driver, Tab};
use crate::error::{Error, Result};
use crate::ops::{detached, str_or_empty, version_at_least};
use crate::protocol::{UPLOAD_BYTES_EXTENSION_VERSION, UPLOAD_PART};

/// `downloads.DEFAULTS["download_task_max_gb"]`: the stand-in upload limit without the daemon's
/// downloads (HAND-502).
pub const DEFAULT_TASK_MAX_GB: i64 = 32;

/// `tab_key`: the browser tab a copy was shown in.
pub fn tab_key(profile: &str, chrome_id: i64) -> String {
    format!("{profile}#{chrome_id}")
}

/// An open file being uploaded (its handle stays open while it is read).
pub struct Handle {
    pub file: std::fs::File,
}

impl Handle {
    #[cfg(unix)]
    pub fn raw(&self) -> i32 {
        std::os::fd::AsRawFd::as_raw_fd(&self.file)
    }
    #[cfg(not(unix))]
    pub fn raw(&self) -> i32 {
        -1
    }
}

/// One file's identity as read (`_identify`): path, size, modified_ns, sha256, owner_uid.
pub type FileIdentity = Map<String, Value>;

/// The upload files on disk (the platform's handles).
pub trait Files: Send + Sync {
    /// `source_path`: the file an upload reads, `~` expanded, links and `..` resolved.
    fn source_path(&self, given: &str) -> String;
    /// `os.path.isabs(os.path.expanduser(given))`.
    fn is_absolute(&self, given: &str) -> bool;
    /// `_identify`: each file's identity, read through one handle that doesn't follow a link at its
    /// last step, and the open handles; `closed` checks each handle as opened before a byte is read.
    fn identify(
        &self,
        paths: &[String],
        closed: Option<&PathCheck>,
    ) -> Result<(Vec<FileIdentity>, Vec<Handle>)>;
    /// `os.pread`.
    fn read_at(&self, h: &Handle, len: usize, offset: u64) -> std::io::Result<Vec<u8>>;
    /// `os.stat(path)`'s (size, mtime_ns), following links; `None` on an error.
    fn stat(&self, path: &str) -> Option<(i64, i64)>;
    /// A `mime.types` file's text (`mimetypes.knownfiles`), when present.
    fn mime_types(&self, path: &str) -> Option<String>;
}

fn sha_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `_receipt`: what an upload's reply proves.
pub fn receipt(
    files: &dyn Files,
    v: &Value,
    mut facts: Dict,
    identities: &[FileIdentity],
) -> ActionResult {
    let selected = v.get("selected_files");
    let bytes_match = match selected.and_then(Value::as_array) {
        Some(sel)
            if sel.iter().all(|a| {
                a.as_object()
                    .is_some_and(|a| a.get("sha256").is_some_and(ferb_py::value::truthy))
            }) =>
        {
            json!(
                sel.len() == identities.len()
                    && sel.iter().zip(identities).all(|(a, b)| {
                        ferb_py::value::py_eq(a.get("sha256").unwrap_or(&Value::Null), &b["sha256"])
                            && ferb_py::value::py_eq(
                                a.get("size").unwrap_or(&Value::Null),
                                &b["size"],
                            )
                    })
            )
        }
        _ => Value::Null,
    };
    facts.insert("bytes_match".into(), bytes_match.clone());
    let mut changed = vec![];
    for item in identities {
        let path = item["path"].as_str().unwrap_or("");
        let now = files.stat(path);
        let was = (item["size"].as_i64(), item["modified_ns"].as_i64());
        if now.map(|(s, m)| (Some(s), Some(m))) != Some(was) {
            changed.push(Value::from(path));
        }
    }
    if !changed.is_empty() {
        facts.insert("reason".into(), "source_changed_during_upload".into());
        facts.insert("changed".into(), Value::Array(changed));
        facts.insert("transferred".into(), Value::Null);
        return ActionResult {
            ok: false,
            detail: "source changed during upload; attached bytes are unverified".into(),
            facts,
            observed: None,
        };
    }
    let ok = v.get("ok").is_some_and(ferb_py::value::truthy);
    if ok && bytes_match == Value::Bool(false) {
        facts.insert("reason".into(), "uploaded_bytes_mismatch".into());
        return ActionResult {
            ok: false,
            detail: "file input contains different bytes; inspect before any submission".into(),
            facts,
            observed: None,
        };
    }
    ActionResult {
        ok,
        detail: str_or_empty(v, "detail"),
        facts,
        observed: None,
    }
}

fn reply_facts(v: &Value) -> Dict {
    v.as_object()
        .map(|m| {
            m.iter()
                .filter(|(k, _)| *k != "ok" && *k != "detail")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default()
}

impl Driver {
    fn files(&self) -> Result<Arc<dyn Files>> {
        self.seams().files.ok_or_else(|| {
            Error::refused("FERB can't read files to upload here; nothing was given to the browser")
        })
    }

    /// `_upload` (HAND-500): the route.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn upload(
        &self,
        t: &Tab,
        frame: i64,
        snap: i64,
        local: i64,
        given: &[String],
        intent_id: &str,
        closed: Option<PathCheck>,
        file_input: bool,
    ) -> Result<ActionResult> {
        let files = self.files()?;
        if let Some(rel) = given.iter().find(|f| !files.is_absolute(f)) {
            // BrowserAct makes them the workspace's: a relative one here is never guessed
            return Err(Error::refused(format!(
                "upload paths must be absolute ({rel}); nothing was given to the browser"
            )));
        }
        let paths: Vec<String> = given.iter().map(|f| files.source_path(f)).collect();
        if paths.is_empty() {
            return Err(Error::refused("upload needs at least one file path"));
        }
        let version = t.bridge.version();
        if !version_at_least(&version, UPLOAD_BYTES_EXTENSION_VERSION) {
            return match self
                .upload_paths(t, frame, snap, local, &paths, intent_id, closed.as_ref())
                .await
            {
                Err(e)
                    if e.is_driver_error() && e.message().contains("refused to set the file") =>
                {
                    let mut facts = e.facts().cloned().unwrap_or_default();
                    facts.insert("reason".into(), "upload_refused_by_browser".into());
                    Err(Error::driver_with(
                        format!(
                            "{e}. FERB's browser extension here is {}, which can only ask the browser to set the file, \
                             and the browser refuses that without access to file URLs; extension \
                             {UPLOAD_BYTES_EXTENSION_VERSION} sends the file through the page instead, and the browser \
                             updates it by itself from the Chrome Web Store",
                            if version.is_empty() { "an older one" } else { &version }
                        ),
                        facts,
                    ))
                }
                other => other,
            };
        }
        if file_input && t.bridge.file_access() == Some(true) {
            let mut r = self
                .upload_paths(t, frame, snap, local, &paths, intent_id, closed.as_ref())
                .await?;
            if r.facts.get("reason").and_then(Value::as_str) == Some("file_access_off") {
                // the switch went off since the hello: nothing was tried, and the next upload sends the bytes
                t.bridge.set_file_access(Some(false));
                r = ActionResult {
                    ok: false,
                    detail: format!(
                        "{}; try the upload again: FERB now sends the file through the page",
                        r.detail
                    ),
                    facts: r.facts,
                    observed: None,
                };
            }
            return Ok(r);
        }
        self.upload_bytes(t, frame, snap, local, &paths, intent_id, closed.as_ref())
            .await
    }

    /// `_upload_paths`: the debugger's route; the browser reads FERB's own copy of each file (rule 17).
    #[allow(clippy::too_many_arguments)]
    async fn upload_paths(
        &self,
        t: &Tab,
        frame: i64,
        snap: i64,
        local: i64,
        paths: &[String],
        intent_id: &str,
        closed: Option<&PathCheck>,
    ) -> Result<ActionResult> {
        let files = self.files()?;
        let (identities, handles) = files.identify(paths, closed)?;
        let given: Option<Vec<String>> = match self.seams().uploads {
            None => None,
            Some(uploads) => {
                let tree = (self.tasks.root)(t.owner.as_deref().unwrap_or(""));
                let sources: Vec<(i32, String, String)> = handles
                    .iter()
                    .zip(paths)
                    .zip(&identities)
                    .map(|((h, p), item)| {
                        (
                            h.raw(),
                            p.clone(),
                            item["sha256"].as_str().unwrap_or("").to_string(),
                        )
                    })
                    .collect();
                let staged =
                    uploads.stage_all(&tree, &sources, &tab_key(&t.bridge.name(), t.chrome_id));
                drop(handles);
                Some(staged.map_err(|e| {
                    Error::refused(format!(
                        "FERB couldn't make its own copy of the file to upload ({e}); nothing was given to the browser"
                    ))
                })?)
            }
        };
        let shown = given.clone().unwrap_or_else(|| paths.to_vec());
        let cid = (!intent_id.is_empty()).then(|| intent_id.to_string());
        let v = t
            .bridge
            .call(
                "upload",
                json!({"tab_id": t.chrome_id, "frame_id": frame, "snap": snap, "el": local, "paths": shown}),
                30.0,
                cid,
            )
            .await
            .map_err(detached)?;
        if v.get("detail").and_then(Value::as_str) == Some("stale") {
            return Err(Error::stale("the file field is gone; take a new snapshot"));
        }
        let mut facts = reply_facts(&v);
        facts.insert("source".into(), "local".into());
        facts.insert("files".into(), json!(identities));
        facts.insert(
            "transferred".into(),
            v.get("ok").is_some_and(ferb_py::value::truthy).into(),
        );
        facts.entry("route").or_insert_with(|| "debugger".into());
        if let Some(copies) = given {
            facts.insert("staged_copies".into(), json!(copies));
        }
        Ok(receipt(files.as_ref(), &v, facts, &identities))
    }

    /// `_upload_limit` (HAND-502): the task's download budget stands in until the owner sets an upload
    /// limit.
    pub fn upload_limit(&self) -> i64 {
        self.seams()
            .downloads
            .map_or(DEFAULT_TASK_MAX_GB * 1_000_000_000, |d| d.task_bytes())
    }

    /// `_upload_bytes` (HAND-501): the bytes in `UPLOAD_PART` parts to the target frame of this task's
    /// tab only, then `upload` with the transfer.
    #[allow(clippy::too_many_arguments)]
    async fn upload_bytes(
        &self,
        t: &Tab,
        frame: i64,
        snap: i64,
        local: i64,
        paths: &[String],
        intent_id: &str,
        closed: Option<&PathCheck>,
    ) -> Result<ActionResult> {
        let files = self.files()?;
        let (identities, handles) = files.identify(paths, closed)?;
        let total: i64 = identities
            .iter()
            .map(|i| i["size"].as_i64().unwrap_or(0))
            .sum();
        let limit = self.upload_limit();
        if total > limit {
            return Err(Error::refused(format!(
                "the files come to {total} bytes, more than the {limit} bytes one upload may carry through the page \
                 (browser.download_task_max_gb, the per-task download budget, stands in until the owner sets an upload \
                 limit); nothing was given to the browser"
            )));
        }
        let transfer = crate::pairing::token_hex(self.random(), 16);
        let mut meta = vec![];
        let mimes = crate::mime::MimeTypes::system(|p| files.mime_types(p));
        for (index, (h, item)) in handles.iter().zip(&identities).enumerate() {
            let size = item["size"].as_i64().unwrap_or(0);
            let mut offset: i64 = 0;
            let mut read: Vec<u8> = vec![];
            loop {
                let want = usize::try_from((size - offset).max(0))
                    .unwrap_or(0)
                    .min(UPLOAD_PART);
                let chunk = files
                    .read_at(h, want, u64::try_from(offset).unwrap_or(0))
                    .map_err(|e| Error::driver(e.to_string()))?;
                read.extend_from_slice(&chunk);
                t.bridge
                    .call(
                        "upload_part",
                        json!({"tab_id": t.chrome_id, "frame_id": frame, "transfer": transfer, "index": index,
                               "offset": offset, "data": ferb_identity::py::b64encode(&chunk)}),
                        30.0,
                        None,
                    )
                    .await?;
                offset += chunk.len() as i64;
                if offset >= size || chunk.is_empty() {
                    break;
                }
            }
            if offset != size || Some(sha_hex(&read).as_str()) != item["sha256"].as_str() {
                return Ok(ActionResult {
                    ok: false,
                    detail: "the file changed while FERB sent it to the page; nothing was set on the page".into(),
                    facts: [
                        ("source", json!("local")),
                        ("files", json!(identities)),
                        ("route", json!("page")),
                        ("reason", json!("source_changed_during_upload")),
                        ("transferred", json!(false)),
                    ]
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect(),
                    observed: None,
                });
            }
            let path = item["path"].as_str().unwrap_or("");
            let name = path.rsplit('/').next().unwrap_or(path).to_string();
            meta.push(json!({"name": name, "type": mimes.guess_type(&name).unwrap_or_default(), "size": size,
                             "last_modified": item["modified_ns"].as_i64().unwrap_or(0).div_euclid(1_000_000)}));
        }
        drop(handles);
        let cid = (!intent_id.is_empty()).then(|| intent_id.to_string());
        let v = t
            .bridge
            .call(
                "upload",
                json!({"tab_id": t.chrome_id, "frame_id": frame, "snap": snap, "el": local,
                       "transfer": transfer, "files": meta}),
                30.0,
                cid,
            )
            .await
            .map_err(detached)?;
        if v.get("detail").and_then(Value::as_str) == Some("stale") {
            return Err(Error::stale("the element is gone; take a new snapshot"));
        }
        let ok = v.get("ok").is_some_and(ferb_py::value::truthy);
        let mut facts = reply_facts(&v);
        facts.insert("source".into(), "local".into());
        facts.insert("files".into(), json!(identities));
        facts.insert("transferred".into(), ok.into());
        let route = facts.get("route").cloned();
        let mut detail = crate::rules::text_or_empty(v.get("detail"));
        if ok && detail.is_empty() {
            detail = if route.as_ref().and_then(Value::as_str) == Some("field") {
                format!(
                    "set {} file(s) on the field through the page",
                    facts
                        .get("files_set")
                        .map_or_else(|| meta.len().to_string(), ferb_py::value::py_str)
                )
            } else {
                format!(
                    "dropped {} file(s) on the element and the page took the drop",
                    meta.len()
                )
            };
            detail.push_str("; whether the site accepted the upload is what the page shows next");
        }
        let mut v = v;
        if let Some(m) = v.as_object_mut() {
            m.insert("detail".into(), detail.into());
        }
        Ok(receipt(files.as_ref(), &v, facts, &identities))
    }
}

/// The Mac's (and any Unix's) upload files: `O_NOFOLLOW` handles, `F_GETPATH` for the protection
/// check (macOS only: elsewhere the opened file can't be told, so a check refuses).
#[cfg(unix)]
pub mod unix {
    use std::ffi::CStr;
    use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
    use std::path::Path;

    use ferb_core::types::PathCheck;
    use serde_json::{json, Map, Value};

    use super::{FileIdentity, Files, Handle};
    use crate::error::{Error, Result};

    pub struct UnixFiles {
        /// `~`: the owner's home.
        pub home: String,
    }

    /// Python's `e.strerror`: the C library's words for an error number.
    pub fn strerror(e: &std::io::Error) -> String {
        match e.raw_os_error() {
            // SAFETY: strerror returns a pointer to a static, NUL-terminated message for any number.
            Some(n) => unsafe { CStr::from_ptr(libc::strerror(n)) }
                .to_string_lossy()
                .into_owned(),
            None => e.to_string(),
        }
    }

    /// `opened_path`: what an open file really is (macOS F_GETPATH).
    #[cfg(target_os = "macos")]
    pub fn opened_path(file: &std::fs::File) -> std::io::Result<String> {
        use std::os::fd::AsRawFd;
        let mut buf = vec![0u8; libc::PATH_MAX as usize];
        // SAFETY: F_GETPATH writes at most MAXPATHLEN bytes, NUL-terminated, into the buffer given.
        let r = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, buf.as_mut_ptr()) };
        if r == -1 {
            return Err(std::io::Error::last_os_error());
        }
        let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
        Ok(String::from_utf8_lossy(&buf[..end]).into_owned())
    }

    #[cfg(not(target_os = "macos"))]
    pub fn opened_path(_file: &std::fs::File) -> std::io::Result<String> {
        Err(std::io::Error::from_raw_os_error(libc::ENOTSUP))
    }

    fn expand(home: &str, given: &str) -> String {
        if given == "~" {
            return home.to_string();
        }
        match given.strip_prefix("~/") {
            Some(rest) => format!("{}/{rest}", home.trim_end_matches('/')),
            None => given.to_string(),
        }
    }

    fn sha256_file(file: &std::fs::File) -> std::io::Result<String> {
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        let mut offset = 0u64;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = file.read_at(&mut buf, offset)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            offset += n as u64;
        }
        Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
    }

    impl Files for UnixFiles {
        fn source_path(&self, given: &str) -> String {
            let p = expand(&self.home, given);
            std::fs::canonicalize(&p)
                .map(|r| r.to_string_lossy().into_owned())
                .unwrap_or(p)
        }

        fn is_absolute(&self, given: &str) -> bool {
            expand(&self.home, given).starts_with('/')
        }

        fn identify(
            &self,
            paths: &[String],
            closed: Option<&PathCheck>,
        ) -> Result<(Vec<FileIdentity>, Vec<Handle>)> {
            let (mut identities, mut handles) = (vec![], vec![]);
            for path in paths {
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
                    .open(path)
                    .map_err(|e| {
                        if e.kind() == std::io::ErrorKind::NotFound {
                            Error::refused(format!("no such file to upload: {path}"))
                        } else {
                            Error::refused(format!(
                                "FERB can't read the file to upload ({path}: {}); nothing was given to the browser",
                                strerror(&e)
                            ))
                        }
                    })?;
                if let Some(check) = closed {
                    let refusal = match opened_path(&file) {
                        Ok(real) => (check.0)(Path::new(&real), "read"),
                        Err(e) => Some(format!(
                            "FERB couldn't tell which file it opened to upload ({path}: {}); nothing was given to the browser",
                            strerror(&e)
                        )),
                    };
                    if let Some(r) = refusal {
                        return Err(Error::refused(r));
                    }
                }
                let before = file.metadata().map_err(|e| {
                    Error::refused(format!(
                        "FERB can't read the file to upload ({path}: {})",
                        strerror(&e)
                    ))
                })?;
                if !before.file_type().is_file() {
                    return Err(Error::refused(format!("no such file to upload: {path}")));
                }
                let digest = sha256_file(&file).map_err(|e| {
                    Error::refused(format!(
                        "FERB can't read the file to upload ({path}: {}); nothing was given to the browser",
                        strerror(&e)
                    ))
                })?;
                let after = file.metadata().map_err(|e| Error::refused(strerror(&e)))?;
                let stamp =
                    |m: &std::fs::Metadata| (m.size(), m.mtime() * 1_000_000_000 + m.mtime_nsec());
                if stamp(&before) != stamp(&after) {
                    return Err(Error::refused(
                        "upload source changed while reading identity; inspect the file again",
                    ));
                }
                let mut item: Map<String, Value> = Map::new();
                item.insert("path".into(), path.clone().into());
                item.insert("size".into(), json!(before.size()));
                item.insert("modified_ns".into(), json!(stamp(&before).1));
                item.insert("sha256".into(), digest.into());
                item.insert("owner_uid".into(), json!(before.uid()));
                identities.push(item);
                handles.push(Handle { file });
            }
            Ok((identities, handles))
        }

        fn read_at(&self, h: &Handle, len: usize, offset: u64) -> std::io::Result<Vec<u8>> {
            let mut buf = vec![0u8; len];
            let n = h.file.read_at(&mut buf, offset)?;
            buf.truncate(n);
            Ok(buf)
        }

        fn stat(&self, path: &str) -> Option<(i64, i64)> {
            let m = std::fs::metadata(path).ok()?;
            Some((m.size() as i64, m.mtime() * 1_000_000_000 + m.mtime_nsec()))
        }

        fn mime_types(&self, path: &str) -> Option<String> {
            Path::new(path)
                .is_file()
                .then(|| std::fs::read_to_string(path).ok())
                .flatten()
        }
    }
}
