//! The system calls the file tools make, as Python's `os` module makes them: each failure is an [`OsError`] carrying
//! the errno, the C library's text for it and the name it was about, so a tool's error reads as Python's did
//! (`f"{e.strerror or e} ({e.filename})"`). Calls that exist only on macOS (F_GETPATH, clonefileat, fclonefileat,
//! renameatx_np, fchflags, the fd xattr calls) are in [`mac`]; elsewhere they fail as Python's fallbacks did.

use std::ffi::{CStr, CString};
use std::fmt;
use std::os::fd::RawFd;

pub use libc::{
    O_CLOEXEC, O_CREAT, O_DIRECTORY, O_EXCL, O_NOFOLLOW, O_NONBLOCK, O_RDONLY, O_TRUNC, O_WRONLY,
};

/// `os.O_SEARCH` where the system has it, else `O_RDONLY` (files.py:818): a folder opened only to work inside it.
#[cfg(target_os = "macos")]
pub const O_SEARCH: i32 = 0x4010_0000; // O_EXEC | O_DIRECTORY on macOS (<sys/fcntl.h>: O_SEARCH)
#[cfg(not(target_os = "macos"))]
pub const O_SEARCH: i32 = libc::O_RDONLY;

/// `os.O_EVTONLY` (macOS), else Python's fallback 0x8000 (protection.py:167).
#[cfg(target_os = "macos")]
pub const O_EVTONLY: i32 = libc::O_EVTONLY;
#[cfg(not(target_os = "macos"))]
pub const O_EVTONLY: i32 = 0x8000;

/// Python's `OSError`: errno (0 when it had none), strerror and the names it was about.
#[derive(Clone, PartialEq, Eq)]
pub struct OsError {
    pub errno: i32,
    pub strerror: Option<String>,
    pub filename: Option<String>,
    pub filename2: Option<String>,
}

impl OsError {
    /// `OSError(errno, os.strerror(errno))` about `filename`.
    pub fn new(errno: i32, filename: Option<&str>) -> OsError {
        OsError {
            errno,
            strerror: Some(strerror(errno)),
            filename: filename.map(str::to_string),
            filename2: None,
        }
    }

    /// The last call's errno.
    pub fn last(filename: Option<&str>) -> OsError {
        OsError::new(errno(), filename)
    }

    /// `OSError(errno, text)`: Python's own message, no file.
    pub fn with_text(errno: i32, text: &str) -> OsError {
        OsError {
            errno,
            strerror: Some(text.to_string()),
            filename: None,
            filename2: None,
        }
    }

    /// `OSError(text)`: no errno, `str(e)` is the text.
    pub fn plain(text: &str) -> OsError {
        OsError {
            errno: 0,
            strerror: None,
            filename: Some(text.to_string()),
            filename2: None,
        }
        .plain_marker()
    }

    fn plain_marker(mut self) -> OsError {
        // a plain OSError(text) keeps its text as args[0]; `filename` stays None in Python
        self.strerror = None;
        self.errno = -1;
        self
    }

    pub fn is(&self, code: i32) -> bool {
        self.errno == code
    }

    /// `e.strerror or e`.
    pub fn strerror_or_str(&self) -> String {
        match &self.strerror {
            Some(s) if !s.is_empty() => s.clone(),
            _ => self.to_string(),
        }
    }

    /// `e.filename` as `_os_error` names it (`None` for a plain error).
    pub fn file(&self) -> Option<&str> {
        if self.errno == -1 {
            None
        } else {
            self.filename.as_deref()
        }
    }
}

/// `str(OSError)`.
impl fmt::Display for OsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.errno == -1 {
            return f.write_str(self.filename.as_deref().unwrap_or(""));
        }
        let text = self.strerror.clone().unwrap_or_default();
        match (&self.filename, &self.filename2) {
            (Some(a), Some(b)) => write!(
                f,
                "[Errno {}] {}: {} -> {}",
                self.errno,
                text,
                crate::py_repr(a),
                crate::py_repr(b)
            ),
            (Some(a), None) => write!(f, "[Errno {}] {}: {}", self.errno, text, crate::py_repr(a)),
            _ => write!(f, "[Errno {}] {}", self.errno, text),
        }
    }
}

impl fmt::Debug for OsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "OsError({self})")
    }
}

impl std::error::Error for OsError {}

pub type Res<T> = Result<T, OsError>;

pub fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `os.strerror`.
pub fn strerror(code: i32) -> String {
    // SAFETY: strerror returns a pointer to a NUL-terminated string valid until the next call on this thread; it is
    // copied at once.
    unsafe {
        let p = libc::strerror(code);
        if p.is_null() {
            format!("Unknown error: {code}")
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }
}

fn cstr(s: &str) -> Res<CString> {
    CString::new(s).map_err(|_| OsError::plain("embedded null byte"))
}

/// An owned file descriptor, closed on drop.
#[derive(Debug)]
pub struct Fd(RawFd);

impl Fd {
    pub fn raw(&self) -> RawFd {
        self.0
    }
}

impl Drop for Fd {
    fn drop(&mut self) {
        // SAFETY: the fd is owned by this value and closed once.
        unsafe {
            libc::close(self.0);
        }
    }
}

/// Where a name is looked up: a held folder or the current folder (`dir_fd=None`).
#[derive(Clone, Copy, Debug)]
pub enum At {
    Cwd,
    Dir(RawFd),
}

impl At {
    fn raw(self) -> RawFd {
        match self {
            At::Cwd => libc::AT_FDCWD,
            At::Dir(fd) => fd,
        }
    }
}

/// `os.open(name, flags, mode, dir_fd=...)`.
pub fn open(at: At, name: &str, flags: i32, mode: u32) -> Res<Fd> {
    let c = cstr(name)?;
    // SAFETY: a valid C string and flags; the returned fd is owned by the `Fd`.
    let fd = unsafe { libc::openat(at.raw(), c.as_ptr(), flags, mode as libc::c_uint) };
    if fd < 0 {
        Err(OsError::last(Some(name)))
    } else {
        Ok(Fd(fd))
    }
}

/// The facts of `os.stat_result` the tools read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct St {
    pub dev: i64,
    pub ino: u64,
    pub mode: u32,
    pub nlink: u64,
    pub uid: u32,
    pub gid: u32,
    pub size: i64,
    pub atime_sec: i64,
    pub atime_nsec: i64,
    pub mtime_sec: i64,
    pub mtime_nsec: i64,
    pub blocks: i64,
    pub flags: u32,
}

impl St {
    #[allow(clippy::unnecessary_cast)] // the field types differ by platform
    fn from(s: &libc::stat) -> St {
        #[cfg(target_os = "macos")]
        let flags = s.st_flags;
        #[cfg(not(target_os = "macos"))]
        let flags = 0u32;
        St {
            dev: s.st_dev as i64,
            ino: s.st_ino as u64,
            mode: s.st_mode as u32,
            nlink: s.st_nlink as u64,
            uid: s.st_uid,
            gid: s.st_gid,
            size: s.st_size,
            atime_sec: s.st_atime,
            atime_nsec: s.st_atime_nsec,
            mtime_sec: s.st_mtime,
            mtime_nsec: s.st_mtime_nsec,
            blocks: s.st_blocks,
            flags,
        }
    }

    pub fn is_dir(&self) -> bool {
        self.mode & libc::S_IFMT as u32 == libc::S_IFDIR as u32
    }
    pub fn is_reg(&self) -> bool {
        self.mode & libc::S_IFMT as u32 == libc::S_IFREG as u32
    }
    pub fn is_lnk(&self) -> bool {
        self.mode & libc::S_IFMT as u32 == libc::S_IFLNK as u32
    }
    /// `stat.S_IMODE`.
    pub fn imode(&self) -> u32 {
        self.mode & 0o7777
    }
    /// `st_mtime_ns`.
    pub fn mtime_ns(&self) -> i128 {
        i128::from(self.mtime_sec) * 1_000_000_000 + i128::from(self.mtime_nsec)
    }
    pub fn atime_ns(&self) -> i128 {
        i128::from(self.atime_sec) * 1_000_000_000 + i128::from(self.atime_nsec)
    }
    /// `st_mtime` (a float, as Python computes it: `sec + nsec * 1e-9`).
    pub fn mtime(&self) -> f64 {
        self.mtime_sec as f64 + self.mtime_nsec as f64 * 1e-9
    }
    pub fn id(&self) -> (i64, u64) {
        (self.dev, self.ino)
    }
}

/// `os.fstat`.
pub fn fstat(fd: RawFd) -> Res<St> {
    let mut s: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `s` is a valid out-pointer.
    if unsafe { libc::fstat(fd, &mut s) } != 0 {
        return Err(OsError::last(None));
    }
    Ok(St::from(&s))
}

/// `os.stat(name, dir_fd=..., follow_symlinks=...)`.
pub fn stat_at(at: At, name: &str, follow: bool) -> Res<St> {
    let c = cstr(name)?;
    let mut s: libc::stat = unsafe { std::mem::zeroed() };
    let flag = if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
    // SAFETY: valid C string and out-pointer.
    if unsafe { libc::fstatat(at.raw(), c.as_ptr(), &mut s, flag) } != 0 {
        return Err(OsError::last(Some(name)));
    }
    Ok(St::from(&s))
}

/// `os.stat(path)`.
pub fn stat(path: &str) -> Res<St> {
    stat_at(At::Cwd, path, true)
}

/// `os.lstat(path)`.
pub fn lstat(path: &str) -> Res<St> {
    stat_at(At::Cwd, path, false)
}

/// `os.mkdir(name, dir_fd=...)` (mode 0o777, as Python's default).
pub fn mkdir_at(at: At, name: &str, mode: u32) -> Res<()> {
    let c = cstr(name)?;
    // SAFETY: valid C string.
    if unsafe { libc::mkdirat(at.raw(), c.as_ptr(), mode as libc::mode_t) } != 0 {
        return Err(OsError::last(Some(name)));
    }
    Ok(())
}

/// `os.unlink(name, dir_fd=...)`.
pub fn unlink_at(at: At, name: &str) -> Res<()> {
    let c = cstr(name)?;
    // SAFETY: valid C string.
    if unsafe { libc::unlinkat(at.raw(), c.as_ptr(), 0) } != 0 {
        return Err(OsError::last(Some(name)));
    }
    Ok(())
}

/// `os.rmdir(path)`.
pub fn rmdir(path: &str) -> Res<()> {
    let c = cstr(path)?;
    // SAFETY: valid C string.
    if unsafe { libc::rmdir(c.as_ptr()) } != 0 {
        return Err(OsError::last(Some(path)));
    }
    Ok(())
}

/// `os.rename(src, dst, src_dir_fd=..., dst_dir_fd=...)` / `os.replace`.
pub fn rename_at(src_at: At, src: &str, dst_at: At, dst: &str) -> Res<()> {
    let (a, b) = (cstr(src)?, cstr(dst)?);
    // SAFETY: valid C strings.
    if unsafe { libc::renameat(src_at.raw(), a.as_ptr(), dst_at.raw(), b.as_ptr()) } != 0 {
        let mut e = OsError::last(Some(src));
        e.filename2 = Some(dst.to_string());
        return Err(e);
    }
    Ok(())
}

/// `os.fchmod`.
pub fn fchmod(fd: RawFd, mode: u32) -> Res<()> {
    // SAFETY: plain syscall.
    if unsafe { libc::fchmod(fd, mode as libc::mode_t) } != 0 {
        return Err(OsError::last(None));
    }
    Ok(())
}

/// `os.chmod(name, mode, dir_fd=..., follow_symlinks=False)`.
pub fn chmod_at_nofollow(at: At, name: &str, mode: u32) -> Res<()> {
    let c = cstr(name)?;
    // SAFETY: valid C string.
    if unsafe {
        libc::fchmodat(
            at.raw(),
            c.as_ptr(),
            mode as libc::mode_t,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(OsError::last(Some(name)));
    }
    Ok(())
}

/// `os.ftruncate`.
pub fn ftruncate(fd: RawFd, len: i64) -> Res<()> {
    // SAFETY: plain syscall.
    if unsafe { libc::ftruncate(fd, len as libc::off_t) } != 0 {
        return Err(OsError::last(None));
    }
    Ok(())
}

/// `os.write` until all is written (files.py `_write_all`).
pub fn write_all(fd: RawFd, data: &[u8]) -> Res<()> {
    let mut view = data;
    while !view.is_empty() {
        // SAFETY: the slice is valid for its length.
        let n = unsafe { libc::write(fd, view.as_ptr().cast(), view.len()) };
        if n < 0 {
            let e = errno();
            if e == libc::EINTR {
                continue;
            }
            return Err(OsError::new(e, None));
        }
        view = &view[n as usize..];
    }
    Ok(())
}

/// `os.read(fd, n)`.
pub fn read(fd: RawFd, n: usize) -> Res<Vec<u8>> {
    let mut buf = vec![0u8; n];
    loop {
        // SAFETY: the buffer is valid for its length.
        let got = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), n) };
        if got < 0 {
            if errno() == libc::EINTR {
                continue;
            }
            return Err(OsError::last(None));
        }
        buf.truncate(got as usize);
        return Ok(buf);
    }
}

/// A buffered file's `read(count)`: up to `count` bytes, fewer only at the end.
pub fn read_up_to(fd: RawFd, count: usize) -> Res<Vec<u8>> {
    let mut out = Vec::new();
    while out.len() < count {
        let block = read(fd, (count - out.len()).min(1 << 20))?;
        if block.is_empty() {
            break;
        }
        out.extend_from_slice(&block);
    }
    Ok(out)
}

/// Everything left to read.
pub fn read_to_end(fd: RawFd) -> Res<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let block = read(fd, 1 << 20)?;
        if block.is_empty() {
            return Ok(out);
        }
        out.extend_from_slice(&block);
    }
}

/// `os.pread`.
pub fn pread(fd: RawFd, n: usize, offset: i64) -> Res<Vec<u8>> {
    let mut buf = vec![0u8; n];
    // SAFETY: the buffer is valid for its length.
    let got = unsafe { libc::pread(fd, buf.as_mut_ptr().cast(), n, offset as libc::off_t) };
    if got < 0 {
        return Err(OsError::last(None));
    }
    buf.truncate(got as usize);
    Ok(buf)
}

/// `stream.seek(offset)`.
pub fn seek(fd: RawFd, offset: i64) -> Res<()> {
    // SAFETY: plain syscall.
    if unsafe { libc::lseek(fd, offset as libc::off_t, libc::SEEK_SET) } < 0 {
        return Err(OsError::last(None));
    }
    Ok(())
}

/// `os.utime(fd or name, ns=(atime, mtime))` on an open file.
pub fn futimens(fd: RawFd, atime_ns: i128, mtime_ns: i128) -> Res<()> {
    let ts = |ns: i128| libc::timespec {
        tv_sec: ns.div_euclid(1_000_000_000) as libc::time_t,
        tv_nsec: ns.rem_euclid(1_000_000_000) as libc::c_long,
    };
    let times = [ts(atime_ns), ts(mtime_ns)];
    // SAFETY: `times` holds two timespecs.
    if unsafe { libc::futimens(fd, times.as_ptr()) } != 0 {
        return Err(OsError::last(None));
    }
    Ok(())
}

/// One entry of `os.scandir(fd)`: its name and its d_type (`DT_*`), as `DirEntry` keeps them.
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub dtype: u8,
    pub ino: u64,
}

/// `os.listdir(fd)` / `os.scandir(fd)`: a folder's names (no `.` or `..`) in the order the system gives them, read
/// through a duplicate of the open folder (Python dups the fd and rewinds it).
pub fn scandir(fd: RawFd) -> Res<Vec<Entry>> {
    // SAFETY: dup of a valid fd; fdopendir takes ownership of the duplicate.
    let dup = unsafe { libc::dup(fd) };
    if dup < 0 {
        return Err(OsError::last(None));
    }
    // SAFETY: `dup` is a valid fd we own.
    let dir = unsafe { libc::fdopendir(dup) };
    if dir.is_null() {
        let e = OsError::last(None);
        // SAFETY: closing our own duplicate.
        unsafe { libc::close(dup) };
        return Err(e);
    }
    // SAFETY: a valid DIR.
    unsafe { libc::rewinddir(dir) };
    let mut out = Vec::new();
    loop {
        // SAFETY: reset errno to tell the end from an error.
        unsafe { *errno_location() = 0 };
        // SAFETY: a valid DIR.
        let ent = unsafe { libc::readdir(dir) };
        if ent.is_null() {
            let e = errno();
            // SAFETY: closes the DIR and the duplicate.
            unsafe { libc::closedir(dir) };
            if e != 0 {
                return Err(OsError::new(e, None));
            }
            return Ok(out);
        }
        // SAFETY: readdir's entry is valid until the next call.
        let (name, dtype, ino) = unsafe {
            let e = &*ent;
            (
                CStr::from_ptr(e.d_name.as_ptr())
                    .to_string_lossy()
                    .into_owned(),
                e.d_type,
                e.d_ino as u64,
            )
        };
        if name == "." || name == ".." {
            continue;
        }
        out.push(Entry { name, dtype, ino });
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
unsafe fn errno_location() -> *mut i32 {
    // SAFETY: the thread's errno.
    unsafe { libc::__error() }
}
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
unsafe fn errno_location() -> *mut i32 {
    // SAFETY: the thread's errno.
    unsafe { libc::__errno_location() }
}

/// `os.access(path, mode, follow_symlinks=...)`.
pub fn access(path: &str, mode: i32, follow: bool) -> bool {
    let Ok(c) = CString::new(path) else {
        return false;
    };
    let flag = if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
    // SAFETY: valid C string.
    unsafe { libc::faccessat(libc::AT_FDCWD, c.as_ptr(), mode, flag) == 0 }
}

/// `os.readlink`.
pub fn readlink(path: &str) -> Res<String> {
    std::fs::read_link(path)
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| OsError::new(e.raw_os_error().unwrap_or(0), Some(path)))
}

/// What an open file or folder really is (`files.opened_path`, macOS F_GETPATH): every link followed, the volume's
/// own spelling.
pub fn opened_path(fd: RawFd) -> Res<String> {
    mac::getpath(fd)
}

pub use mac::{clonefile, clonefileat, fchflags, fclonefileat, lchflags, renameatx_excl, xattrs};
pub use mac::{fsetxattr, CLONE_NOFOLLOW};

#[cfg(target_os = "macos")]
pub mod mac {
    //! macOS's own calls, made through libc's declarations or, where libc has none, declared here.
    use super::*;

    pub const CLONE_NOFOLLOW: u32 = 0x0001;
    const RENAME_EXCL: u32 = 0x0000_0004;

    mod ffi {
        extern "C" {
            pub fn clonefileat(
                src_dirfd: libc::c_int,
                src: *const libc::c_char,
                dst_dirfd: libc::c_int,
                dst: *const libc::c_char,
                flags: u32,
            ) -> libc::c_int;
            pub fn fclonefileat(
                srcfd: libc::c_int,
                dst_dirfd: libc::c_int,
                dst: *const libc::c_char,
                flags: u32,
            ) -> libc::c_int;
            pub fn clonefile(
                src: *const libc::c_char,
                dst: *const libc::c_char,
                flags: u32,
            ) -> libc::c_int;
            pub fn renameatx_np(
                fromfd: libc::c_int,
                from: *const libc::c_char,
                tofd: libc::c_int,
                to: *const libc::c_char,
                flags: libc::c_uint,
            ) -> libc::c_int;
        }
    }

    /// F_GETPATH into a MAXPATHLEN buffer, cut at its first NUL (files.py:54-56).
    pub fn getpath(fd: RawFd) -> Res<String> {
        let mut buf = [0u8; 1024];
        // SAFETY: F_GETPATH writes at most MAXPATHLEN (1024) bytes into the buffer.
        if unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) } < 0 {
            return Err(OsError::last(None));
        }
        let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
        Ok(String::from_utf8_lossy(&buf[..end]).into_owned())
    }

    /// `clonefileat(src_dir, name, dst_dir, new, flags)`: the errno on failure.
    pub fn clonefileat_raw(
        src_dir: RawFd,
        name: &str,
        dst_dir: RawFd,
        new: &str,
        flags: u32,
    ) -> Result<(), i32> {
        let (a, b) = (
            CString::new(name).map_err(|_| libc::EINVAL)?,
            CString::new(new).map_err(|_| libc::EINVAL)?,
        );
        // SAFETY: valid C strings and fds owned by the caller.
        if unsafe { ffi::clonefileat(src_dir, a.as_ptr(), dst_dir, b.as_ptr(), flags) } != 0 {
            return Err(errno());
        }
        Ok(())
    }

    pub fn clonefileat_avail() -> bool {
        true
    }

    /// `fclonefileat(src_fd, dir_fd, name, flags)` as an OSError (recovery.py `_fclone`).
    pub fn fclonefileat(src: RawFd, dir: RawFd, name: &str, flags: u32) -> Res<()> {
        let c = cstr(name)?;
        // SAFETY: valid C string and fds owned by the caller.
        if unsafe { ffi::fclonefileat(src, dir, c.as_ptr(), flags) } != 0 {
            return Err(OsError::last(None));
        }
        Ok(())
    }

    /// `clonefile(src, dst, 0)` as an OSError (recovery.py `_clone`).
    pub fn clonefile(src: &str, dst: &str) -> Res<()> {
        let (a, b) = (cstr(src)?, cstr(dst)?);
        // SAFETY: valid C strings.
        if unsafe { ffi::clonefile(a.as_ptr(), b.as_ptr(), 0) } != 0 {
            return Err(OsError::last(None));
        }
        Ok(())
    }

    pub fn clonefileat(
        src_dir: RawFd,
        name: &str,
        dst_dir: RawFd,
        new: &str,
        flags: u32,
    ) -> Result<(), i32> {
        clonefileat_raw(src_dir, name, dst_dir, new, flags)
    }

    /// renameatx_np with RENAME_EXCL (files.py `move_excl`): EEXIST when anything stands at `dst`.
    pub fn renameatx_excl(src_dir: RawFd, src: &str, dst_dir: RawFd, dst: &str) -> Res<()> {
        let (a, b) = (cstr(src)?, cstr(dst)?);
        // SAFETY: valid C strings and fds owned by the caller.
        if unsafe { ffi::renameatx_np(src_dir, a.as_ptr(), dst_dir, b.as_ptr(), RENAME_EXCL) } != 0
        {
            let code = errno();
            return Err(OsError::with_text(code, &strerror(code)));
        }
        Ok(())
    }

    /// `fchflags(fd, flags)`: the errno on failure.
    pub fn fchflags(fd: RawFd, flags: u32) -> Result<(), i32> {
        // SAFETY: plain syscall.
        if unsafe { libc::fchflags(fd, flags) } != 0 {
            return Err(errno());
        }
        Ok(())
    }

    /// `os.chflags(path, flags, follow_symlinks=False)`.
    pub fn lchflags(path: &str, flags: u32) -> Res<()> {
        let c = cstr(path)?;
        // SAFETY: valid C string.
        if unsafe { libc::lchflags(c.as_ptr(), flags) } != 0 {
            return Err(OsError::last(Some(path)));
        }
        Ok(())
    }

    /// Every extended attribute of an open file, in the order the system lists them (files.py `_xattrs`: best
    /// effort, one that can't be read is left out).
    pub fn xattrs(fd: RawFd) -> Vec<(Vec<u8>, Vec<u8>)> {
        // SAFETY: a size query with a null buffer.
        let size = unsafe { libc::flistxattr(fd, std::ptr::null_mut(), 0, 0) };
        if size <= 0 {
            return Vec::new();
        }
        let mut names = vec![0u8; size as usize];
        // SAFETY: the buffer is `size` bytes.
        let size = unsafe { libc::flistxattr(fd, names.as_mut_ptr().cast(), names.len(), 0) };
        let mut out = Vec::new();
        let size = size.max(0) as usize;
        for name in names[..size.min(names.len())].split(|b| *b == 0) {
            if name.is_empty() {
                continue;
            }
            let Ok(c) = CString::new(name) else { continue };
            // SAFETY: a size query with a null buffer.
            let len = unsafe { libc::fgetxattr(fd, c.as_ptr(), std::ptr::null_mut(), 0, 0, 0) };
            if len < 0 {
                continue;
            }
            let mut value = vec![0u8; (len as usize).max(1)];
            // SAFETY: the buffer is at least `len` bytes.
            let got = unsafe {
                libc::fgetxattr(
                    fd,
                    c.as_ptr(),
                    value.as_mut_ptr().cast(),
                    len as usize,
                    0,
                    0,
                )
            };
            if got >= 0 {
                value.truncate(got as usize);
                out.push((name.to_vec(), value));
            }
        }
        out
    }

    /// `fsetxattr(fd, name, value, 0, 0)`; its result is ignored by the callers (best effort).
    pub fn fsetxattr(fd: RawFd, name: &[u8], value: &[u8]) -> i32 {
        let Ok(c) = CString::new(name) else { return -1 };
        // SAFETY: valid C string and value buffer.
        unsafe { libc::fsetxattr(fd, c.as_ptr(), value.as_ptr().cast(), value.len(), 0, 0) }
    }
}

#[cfg(not(target_os = "macos"))]
pub mod mac {
    //! Not macOS: as Python's fallbacks (no clone, no Trash move, no fd xattrs).
    use super::*;

    pub const CLONE_NOFOLLOW: u32 = 0x0001;

    pub fn getpath(fd: RawFd) -> Res<String> {
        std::fs::read_link(format!("/proc/self/fd/{fd}"))
            .map(|p| p.to_string_lossy().into_owned())
            .map_err(|e| OsError::new(e.raw_os_error().unwrap_or(libc::ENOTSUP), None))
    }
    pub fn clonefileat_avail() -> bool {
        false
    }
    pub fn clonefileat(_: RawFd, _: &str, _: RawFd, _: &str, _: u32) -> Result<(), i32> {
        Err(libc::ENOTSUP)
    }
    pub fn fclonefileat(_: RawFd, _: RawFd, _: &str, _: u32) -> Res<()> {
        Err(OsError::with_text(
            libc::ENOTSUP,
            "fclonefileat is unavailable",
        ))
    }
    pub fn clonefile(_: &str, _: &str) -> Res<()> {
        Err(OsError::with_text(
            libc::ENOTSUP,
            "clonefile is unavailable",
        ))
    }
    pub fn renameatx_excl(_: RawFd, _: &str, _: RawFd, _: &str) -> Res<()> {
        Err(OsError::with_text(
            libc::ENOTSUP,
            "renameatx_np is unavailable",
        ))
    }
    pub fn fchflags(_: RawFd, _: u32) -> Result<(), i32> {
        Err(libc::ENOTSUP)
    }
    pub fn lchflags(path: &str, _: u32) -> Res<()> {
        Err(OsError::new(libc::ENOTSUP, Some(path)))
    }
    pub fn xattrs(_: RawFd) -> Vec<(Vec<u8>, Vec<u8>)> {
        Vec::new()
    }
    pub fn fsetxattr(_: RawFd, _: &[u8], _: &[u8]) -> i32 {
        -1
    }
}

/// `UF_IMMUTABLE | UF_APPEND` (recovery.py `_LOCKED`).
pub const LOCKED: u32 = 0x0000_0002 | 0x0000_0004;
