//! usage.py (P24): metadata-only disk usage. Never opens a regular file, follows a link or returns contents.

use std::os::fd::RawFd;
use std::path::Path;

use ferb_core::types::{Dict, ToolContext, ToolResult};
use serde::Deserialize;
use serde_json::json;

use crate::files::{self, FsEnv, Out, Raised};
use crate::pypath as pp;
use crate::sys::{self, At, OsError};

pub const NAME: &str = "fs.disk_usage";
pub const DESCRIPTION: &str =
    "Count files and logical/allocated bytes under an exact path, using metadata only; no contents or symlink traversal.";

/// Python's name for an `OSError` of this errno (`type(e).__name__`).
pub fn error_class(e: &OsError) -> &'static str {
    match e.errno {
        libc::ENOENT => "FileNotFoundError",
        libc::EACCES | libc::EPERM => "PermissionError",
        libc::EEXIST => "FileExistsError",
        libc::ENOTDIR => "NotADirectoryError",
        libc::EISDIR => "IsADirectoryError",
        libc::EINTR => "InterruptedError",
        libc::ECHILD => "ChildProcessError",
        libc::EAGAIN | libc::EALREADY | libc::EINPROGRESS => "BlockingIOError",
        libc::EPIPE | libc::ESHUTDOWN => "BrokenPipeError",
        libc::ECONNABORTED => "ConnectionAbortedError",
        libc::ECONNREFUSED => "ConnectionRefusedError",
        libc::ECONNRESET => "ConnectionResetError",
        libc::ESRCH => "ProcessLookupError",
        libc::ETIMEDOUT => "TimeoutError",
        _ => "OSError",
    }
}

/// The protection lists' check (path, access), as `ToolContext.path_closed`.
pub type Closed<'a> = Option<&'a dyn Fn(&str, &str) -> Option<String>>;

struct Count<'a> {
    files: i64,
    directories: i64,
    bytes: i64,
    allocated: i64,
    errors: Vec<String>,
    paths: Vec<String>,
    details: bool,
    closed: Closed<'a>,
}

impl Count<'_> {
    fn folder(&mut self, folder: RawFd) -> Result<(), OsError> {
        let where_ = sys::opened_path(folder)?;
        if let Some(closed) = self.closed {
            if closed(&where_, "metadata").is_some_and(|r| !r.is_empty()) {
                self.errors
                    .push(format!("{where_}: protected state omitted"));
                return Ok(());
            }
        }
        self.directories += 1;
        if self.details {
            self.paths.push(format!("{where_}/"));
        }
        for entry in sys::scandir(folder)? {
            let name = entry.name;
            let step = (|| -> Result<(), OsError> {
                let item = sys::stat_at(At::Dir(folder), &name, false)?;
                if item.is_dir() {
                    let child = sys::open(
                        At::Dir(folder),
                        &name,
                        sys::O_RDONLY | sys::O_DIRECTORY | sys::O_NOFOLLOW,
                        0,
                    )?;
                    self.folder(child.raw())?;
                } else {
                    if self.details {
                        self.paths.push(pp::join(&where_, &name));
                    }
                    self.files += 1;
                    self.bytes += item.size;
                    self.allocated += item.blocks * 512;
                }
                Ok(())
            })();
            if let Err(e) = step {
                self.errors
                    .push(format!("{}: {}", pp::join(&where_, &name), error_class(&e)));
            }
        }
        Ok(())
    }
}

/// `usage(path, closed, details)`.
pub fn usage(path: &str, closed: Closed, details: bool) -> Result<Dict, OsError> {
    let mut c = Count {
        files: 0,
        directories: 0,
        bytes: 0,
        allocated: 0,
        errors: Vec::new(),
        paths: Vec::new(),
        details,
        closed,
    };
    let info = sys::lstat(path)?;
    if info.is_dir() {
        let fd = sys::open(
            At::Cwd,
            path,
            sys::O_RDONLY | sys::O_DIRECTORY | sys::O_NOFOLLOW,
            0,
        )?;
        c.folder(fd.raw())?;
    } else {
        (c.files, c.bytes, c.allocated) = (1, info.size, info.blocks * 512);
        if details {
            c.paths.push(pp::norm(path));
        }
    }
    let mut d = Dict::new();
    d.insert("path".into(), json!(pp::norm(path)));
    d.insert("files".into(), json!(c.files));
    d.insert("directories".into(), json!(c.directories));
    d.insert("bytes".into(), json!(c.bytes));
    d.insert("allocated_bytes".into(), json!(c.allocated));
    d.insert("complete".into(), json!(c.errors.is_empty()));
    d.insert("errors".into(), json!(c.errors));
    d.insert(
        "symlinks".into(),
        json!("count link metadata; never follow"),
    );
    if details {
        d.insert("paths".into(), json!(c.paths));
    }
    Ok(d)
}

#[derive(Clone, Debug, Deserialize)]
pub struct Args {
    pub path: String,
}

/// `file_paths`: the path as given, for metadata.
pub fn file_paths(a: &Args) -> Vec<(String, &'static str)> {
    vec![(a.path.clone(), "metadata")]
}

/// fs.disk_usage: the lists checked on the real path (a refusal raises `Refused`), then the walk. An OS error from the
/// walk's start leaves the tool, as in Python.
pub fn fs_disk_usage(a: &Args, ctx: &ToolContext, env: &FsEnv) -> Out {
    let path = match files::real_path(&a.path, &ctx.workspace, &env.home) {
        Ok(p) => p,
        Err(files::Fail::Os(e)) => return Err(Raised::Os(e)),
        Err(files::Fail::Raised(m) | files::Fail::Refused(m)) => return Err(Raised::Error(m)),
        Err(files::Fail::Decode(_)) => unreachable!("no text is decoded"),
    };
    let closed = ctx
        .path_closed
        .as_ref()
        .map(|c| move |p: &str, access: &str| (c.0)(Path::new(p), access));
    if let Some(c) = &closed {
        if let Some(why) = c(&path, "metadata").filter(|w| !w.is_empty()) {
            return Err(Raised::Refused(why));
        }
    }
    let closed_ref = closed
        .as_ref()
        .map(|c| c as &dyn Fn(&str, &str) -> Option<String>);
    let content = usage(&path, closed_ref, false).map_err(Raised::Os)?;
    Ok(ToolResult {
        content,
        read_sources: vec![("file".into(), path)],
        ..Default::default()
    })
}
