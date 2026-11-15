//! Python's POSIX path text as `pathlib.PurePosixPath` and `posixpath` handle it (`parts`, `/`, `parent`, `name`,
//! `relative_to`, `is_relative_to`, ordering; `normpath`, `join`, `split`), and `os.path.realpath`,
//! `Path.resolve`, `Path.exists` and `Path.is_dir` over the file-identity syscalls of a [`Files`] seam. Paths are
//! text here, as Python's are.

use std::io;

/// What `lstat`/`stat` report that the policy reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stat {
    pub dev: u64,
    pub ino: u64,
    pub kind: FileKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Dir,
    File,
    Link,
    Other,
}

/// The file-identity syscalls the protection lists rest on (a seam: the POSIX ones in [`PosixFiles`], the
/// name-only open with `F_GETPATH` on macOS).
pub trait Files {
    fn lstat(&self, path: &str) -> io::Result<Stat>;
    fn stat(&self, path: &str) -> io::Result<Stat>;
    fn readlink(&self, path: &str) -> io::Result<String>;
    fn cwd(&self) -> io::Result<String>;
    /// `os.open(path, O_EVTONLY | O_NONBLOCK | O_CLOEXEC | O_NOFOLLOW | (O_DIRECTORY if folder))`, `fstat` and
    /// `fcntl(F_GETPATH)`: what opened, and the kernel's name for it. Err when it can't be opened or named.
    fn opened_name(&self, path: &str, folder: bool) -> io::Result<(Stat, String)>;
}

/// `PurePosixPath(text).parts`: the root ("/" or exactly "//") then each component, "." and empty ones dropped.
pub fn parts(p: &str) -> Vec<String> {
    let (root, rest) = if p.starts_with("//") && !p.starts_with("///") {
        ("//", &p[2..])
    } else if p.starts_with('/') {
        ("/", p.trim_start_matches('/'))
    } else {
        ("", p)
    };
    let mut out: Vec<String> = Vec::new();
    if !root.is_empty() {
        out.push(root.to_string());
    }
    out.extend(
        rest.split('/')
            .filter(|c| !c.is_empty() && *c != ".")
            .map(str::to_string),
    );
    out
}

/// `str(PurePosixPath(*parts))`.
pub fn from_parts(parts: &[String]) -> String {
    match parts.first() {
        None => ".".to_string(),
        Some(root) if root.starts_with('/') => format!("{root}{}", parts[1..].join("/")),
        Some(_) => parts.join("/"),
    }
}

/// `str(PurePosixPath(text))`.
pub fn clean(p: &str) -> String {
    from_parts(&parts(p))
}

pub fn is_absolute(p: &str) -> bool {
    p.starts_with('/')
}

/// `PurePosixPath(a) / b`.
pub fn div(a: &str, b: &str) -> String {
    if is_absolute(b) {
        return clean(b);
    }
    let mut ps = parts(a);
    ps.extend(parts(b));
    from_parts(&ps)
}

/// `.parent`.
pub fn parent(p: &str) -> String {
    let mut ps = parts(p);
    if ps.len() > 1 || (ps.len() == 1 && !ps[0].starts_with('/')) {
        ps.pop();
    }
    from_parts(&ps)
}

/// `.name`.
pub fn name(p: &str) -> String {
    let ps = parts(p);
    match ps.last() {
        Some(last) if !last.starts_with('/') => last.clone(),
        _ => String::new(),
    }
}

/// `.parents`, nearest first.
pub fn parents(p: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = clean(p);
    loop {
        let up = parent(&cur);
        if up == cur {
            break;
        }
        out.push(up.clone());
        cur = up;
    }
    out
}

/// `.is_relative_to(root)` (text, case-sensitive).
pub fn is_relative_to(p: &str, root: &str) -> bool {
    let (a, b) = (parts(p), parts(root));
    a.len() >= b.len() && a[..b.len()] == b[..]
}

/// `.relative_to(root)` when `is_relative_to`.
pub fn relative_to(p: &str, root: &str) -> String {
    let (a, b) = (parts(p), parts(root));
    from_parts(&a[b.len().min(a.len())..])
}

/// The sort key of a PurePosixPath (its text split on "/").
pub fn sort_key(p: &str) -> Vec<String> {
    clean(p).split('/').map(str::to_string).collect()
}

/// `posixpath.normpath`.
pub fn normpath(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let (initial, rest) = if !path.starts_with('/') {
        ("", path)
    } else if !path[1..].starts_with('/') || path[2..].starts_with('/') {
        ("/", &path[1..])
    } else {
        ("//", &path[2..])
    };
    let mut comps: Vec<&str> = Vec::new();
    for comp in rest.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp != ".." || (initial.is_empty() && comps.is_empty()) || comps.last() == Some(&"..") {
            comps.push(comp);
        } else if !comps.is_empty() {
            comps.pop();
        }
    }
    let out = format!("{initial}{}", comps.join("/"));
    if out.is_empty() {
        ".".to_string()
    } else {
        out
    }
}

/// `posixpath.join(a, b)`.
pub fn join(a: &str, b: &str) -> String {
    if b.starts_with('/') {
        b.to_string()
    } else if a.is_empty() || a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

/// `posixpath.split(p)`.
pub fn split(p: &str) -> (String, String) {
    let i = p.rfind('/').map_or(0, |i| i + 1);
    let (head, tail) = (&p[..i], &p[i..]);
    let trimmed = head.trim_end_matches('/');
    let head = if !head.is_empty() && trimmed.is_empty() {
        head
    } else {
        trimmed
    };
    (head.to_string(), tail.to_string())
}

/// `os.path.abspath`.
pub fn abspath(files: &dyn Files, p: &str) -> io::Result<String> {
    if is_absolute(p) {
        Ok(normpath(p))
    } else {
        Ok(normpath(&join(&files.cwd()?, p)))
    }
}

/// `os.path.realpath(p)` (non-strict): links followed by name; missing parts kept as written.
pub fn realpath(files: &dyn Files, p: &str) -> io::Result<String> {
    let mut seen: Vec<(String, Option<String>)> = Vec::new();
    let (path, _) = join_real(files, "", p, &mut seen, 0)?;
    abspath(files, &path)
}

fn join_real(
    files: &dyn Files,
    path: &str,
    rest: &str,
    seen: &mut Vec<(String, Option<String>)>,
    depth: usize,
) -> io::Result<(String, bool)> {
    if depth > 1000 {
        return Err(io::Error::other("maximum recursion depth exceeded"));
    }
    let mut path = path.to_string();
    let mut rest = rest.to_string();
    if is_absolute(&rest) {
        rest = rest[1..].to_string();
        path = "/".to_string();
    }
    while !rest.is_empty() {
        let (name, r) = match rest.find('/') {
            Some(i) => (rest[..i].to_string(), rest[i + 1..].to_string()),
            None => (rest.clone(), String::new()),
        };
        rest = r;
        if name.is_empty() || name == "." {
            continue;
        }
        if name == ".." {
            if !path.is_empty() {
                let (head, tail) = split(&path);
                path = head;
                if tail == ".." {
                    path = join(&join(&path, ".."), "..");
                }
            } else {
                path = "..".to_string();
            }
            continue;
        }
        let newpath = join(&path, &name);
        let is_link = matches!(files.lstat(&newpath), Ok(st) if st.kind == FileKind::Link);
        if !is_link {
            path = newpath;
            continue;
        }
        if let Some((_, cached)) = seen.iter().find(|(k, _)| *k == newpath) {
            if let Some(c) = cached {
                path = c.clone();
                continue;
            }
            return Ok((join(&newpath, &rest), false));
        }
        seen.push((newpath.clone(), None));
        let target = files.readlink(&newpath)?;
        let (p, ok) = join_real(files, &path, &target, seen, depth + 1)?;
        path = p;
        if !ok {
            return Ok((join(&path, &rest), false));
        }
        if let Some(slot) = seen.iter_mut().find(|(k, _)| *k == newpath) {
            slot.1 = Some(path.clone());
        }
    }
    Ok((path, true))
}

const ELOOP: i32 = 62;

fn eloop(e: &io::Error) -> bool {
    e.raw_os_error() == Some(ELOOP) || e.raw_os_error() == Some(libc_eloop())
}

#[cfg(unix)]
fn libc_eloop() -> i32 {
    libc::ELOOP
}

#[cfg(not(unix))]
fn libc_eloop() -> i32 {
    ELOOP
}

/// Why `Path.resolve()` failed: an OSError, or Python's RuntimeError for a symlink loop.
#[derive(Debug)]
pub enum ResolveError {
    Os(io::Error),
    Loop,
}

/// `Path(p).resolve()` (non-strict): realpath, then a symlink loop found by stat is an error.
pub fn resolve(files: &dyn Files, p: &str) -> Result<String, ResolveError> {
    let s = realpath(files, p).map_err(|e| {
        if eloop(&e) {
            ResolveError::Loop
        } else {
            ResolveError::Os(e)
        }
    })?;
    if let Err(e) = files.stat(&s) {
        if eloop(&e) {
            return Err(ResolveError::Loop);
        }
    }
    Ok(clean(&s))
}

fn ignorable(e: &io::Error) -> bool {
    // pathlib's _ignore_error: ENOENT, ENOTDIR, EBADF, ELOOP
    #[cfg(unix)]
    {
        matches!(
            e.raw_os_error(),
            Some(libc::ENOENT) | Some(libc::ENOTDIR) | Some(libc::EBADF) | Some(libc::ELOOP)
        ) || e.kind() == io::ErrorKind::NotFound
    }
    #[cfg(not(unix))]
    {
        e.kind() == io::ErrorKind::NotFound
    }
}

/// `Path(p).exists()`: Err where pathlib raises.
pub fn exists(files: &dyn Files, p: &str) -> io::Result<bool> {
    match files.stat(p) {
        Ok(_) => Ok(true),
        Err(e) if ignorable(&e) => Ok(false),
        Err(e) => Err(e),
    }
}

/// `Path(p).is_dir()`: Err where pathlib raises.
pub fn is_dir(files: &dyn Files, p: &str) -> io::Result<bool> {
    match files.stat(p) {
        Ok(st) => Ok(st.kind == FileKind::Dir),
        Err(e) if ignorable(&e) => Ok(false),
        Err(e) => Err(e),
    }
}

/// `Path(p).is_symlink()`: Err where pathlib raises.
pub fn is_symlink(files: &dyn Files, p: &str) -> io::Result<bool> {
    match files.lstat(p) {
        Ok(st) => Ok(st.kind == FileKind::Link),
        Err(e) if ignorable(&e) => Ok(false),
        Err(e) => Err(e),
    }
}

/// The POSIX file-identity syscalls (`lstat`, `stat`, `readlink`, `getcwd`); the name-only open is macOS's
/// (`O_EVTONLY` and `F_GETPATH`, in `mac`), elsewhere unavailable.
pub struct PosixFiles;

#[cfg(unix)]
fn stat_of(m: &std::fs::Metadata) -> Stat {
    use std::os::unix::fs::MetadataExt;
    let ft = m.file_type();
    let kind = if ft.is_symlink() {
        FileKind::Link
    } else if ft.is_dir() {
        FileKind::Dir
    } else if ft.is_file() {
        FileKind::File
    } else {
        FileKind::Other
    };
    Stat {
        dev: m.dev(),
        ino: m.ino(),
        kind,
    }
}

#[cfg(unix)]
impl Files for PosixFiles {
    fn lstat(&self, path: &str) -> io::Result<Stat> {
        std::fs::symlink_metadata(path).map(|m| stat_of(&m))
    }

    fn stat(&self, path: &str) -> io::Result<Stat> {
        std::fs::metadata(path).map(|m| stat_of(&m))
    }

    fn readlink(&self, path: &str) -> io::Result<String> {
        let target = std::fs::read_link(path)?;
        target.into_os_string().into_string().map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "a link target that isn't UTF-8")
        })
    }

    fn cwd(&self) -> io::Result<String> {
        std::env::current_dir()?
            .into_os_string()
            .into_string()
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "a working folder that isn't UTF-8",
                )
            })
    }

    fn opened_name(&self, path: &str, folder: bool) -> io::Result<(Stat, String)> {
        mac::opened_name(path, folder)
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::CString;
    use std::io;

    use super::{FileKind, Stat};

    const O_EVTONLY: libc::c_int = 0x8000;

    struct Fd(libc::c_int);

    impl Drop for Fd {
        fn drop(&mut self) {
            // SAFETY: the descriptor was returned by open() below and is closed exactly once, here.
            unsafe {
                libc::close(self.0);
            }
        }
    }

    /// Open for a name only (no data read), never through a link; fstat; F_GETPATH.
    pub(super) fn opened_name(path: &str, folder: bool) -> io::Result<(Stat, String)> {
        let c = CString::new(path)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a NUL in the path"))?;
        let flags = O_EVTONLY
            | libc::O_NONBLOCK
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | if folder { libc::O_DIRECTORY } else { 0 };
        // SAFETY: `c` is a valid NUL-terminated string that outlives the call; open() only reads it.
        let fd = unsafe { libc::open(c.as_ptr(), flags) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = Fd(fd);
        // SAFETY: an all-zero `stat` is a valid value of this plain C struct; fstat() overwrites it.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: `fd.0` is an open descriptor and `st` is a valid, writable stat struct.
        if unsafe { libc::fstat(fd.0, &mut st) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buf = vec![0u8; libc::PATH_MAX as usize];
        // SAFETY: F_GETPATH writes at most PATH_MAX bytes (NUL included) into `buf`, which has that many.
        if unsafe { libc::fcntl(fd.0, libc::F_GETPATH, buf.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        let name = String::from_utf8(buf[..end].to_vec()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "a kernel path that isn't UTF-8")
        })?;
        let kind = match st.st_mode & libc::S_IFMT {
            libc::S_IFDIR => FileKind::Dir,
            libc::S_IFREG => FileKind::File,
            libc::S_IFLNK => FileKind::Link,
            _ => FileKind::Other,
        };
        Ok((
            Stat {
                dev: st.st_dev as u64,
                ino: st.st_ino,
                kind,
            },
            name,
        ))
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod mac {
    use std::io;

    use super::Stat;

    /// No name-only open with a kernel path off macOS: nothing is named this way.
    pub(super) fn opened_name(_path: &str, _folder: bool) -> io::Result<(Stat, String)> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "F_GETPATH is macOS's",
        ))
    }
}
