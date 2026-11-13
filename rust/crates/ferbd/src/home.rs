//! Where the daemon's data is and where its output goes: `ferb/paths.py`, `ui/mac/FERBAgent/main.c:49-55,108-116` and
//! `ferb/daemon/main.py` `service_streams`.
//!
//! **The home rule.** Two rules exist (`ferb_core::paths`): Python's `ferb_home()` honors any `FERB_HOME` (tests, a
//! second FERB side by side); FERB.app's agent host refuses to start unless `FERB_HOME` is unset, empty or names the
//! account's own `~/.ferb`. Which one applies is decided from where this executable is, exactly as ferb-rpc decides
//! the FERB.app check's mode (journal 5, `app_peer::mode_for`): inside an `.app` bundle (the shipping host) the
//! shipping rule; anywhere else (a checkout's `target` folder, an installed copy elsewhere) Python's rule.

use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use ferb_core::paths;
use ferb_rpc::app_peer::{mode_for, Mode};

/// The rule the home was found by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rule {
    /// Inside FERB.app: the account's own `~/.ferb` only.
    Shipping,
    /// Anywhere else: `$FERB_HOME`, else `~/.ferb`.
    Python,
}

/// The rule for an executable at `exe` (see the module's text).
pub fn rule_for(exe: &Path) -> Rule {
    // mode_for's checkout test needs a home; any home outside the executable's folder gives the same answer for the
    // bundle test, which is the only one that matters here.
    match mode_for(exe, Path::new("/nonexistent-ferb-home")) {
        Mode::Bundled(_) => Rule::Shipping,
        Mode::Installed | Mode::Checkout => Rule::Python,
    }
}

/// FERB's home for this process, or the shipping host's refusal (its words, `ForeignHome`).
pub fn resolve() -> Result<(PathBuf, Rule), String> {
    let exe = std::env::current_exe().unwrap_or_default();
    let rule = rule_for(&exe);
    let home = match rule {
        Rule::Python => paths::ferb_home(),
        Rule::Shipping => {
            let given = std::env::var("FERB_HOME").ok();
            paths::shipping_home(given.as_deref(), paths::account_home().as_deref())
                .map_err(|e| e.to_string())?
        }
    };
    Ok((home, rule))
}

/// The FERB.app the daemon runs from (`paths.app_bundle()`): the outermost `.app` around this executable.
pub fn app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    match mode_for(&exe, Path::new("/nonexistent-ferb-home")) {
        Mode::Bundled(app) => Some(app),
        _ => None,
    }
}

/// FERB's package folder (`ferb/`): the runtime Markdown the learners and roles read (prompts, memory notes,
/// abilities). Inside FERB.app: `Contents/Resources/ferb`; elsewhere the checkout's `ferb/` next to `rust/`, found
/// from the executable's folders, else from where this crate was built.
pub fn package_folder() -> PathBuf {
    if let Some(app) = app_bundle() {
        return app.join("Contents/Resources/ferb");
    }
    let marker = |p: &Path| p.join("ferb/orchestrator/prompts").is_dir();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(root) = exe.ancestors().find(|p| marker(p)) {
            return root.join("ferb");
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../ferb")
}

/// `service_streams(home)` (main.py:718): the working folder is FERB's home, and standard output and error go to
/// `logs/daemon.out.log` and `logs/daemon.err.log` (0600, appended) when launchd gave none (closed or `/dev/null`).
pub fn service_streams(home: &Path) -> std::io::Result<()> {
    let logs = home.join("logs");
    std::fs::create_dir_all(&logs)?;
    std::env::set_current_dir(home)?;
    let null = std::fs::metadata("/dev/null").ok().map(|m| {
        use std::os::unix::fs::MetadataExt;
        m.rdev()
    });
    for (fd, name) in [(1, "daemon.out.log"), (2, "daemon.err.log")] {
        if !to_null_or_closed(fd, null) {
            continue;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(logs.join(name))?;
        // SAFETY: both descriptors are open (the file just opened, and fd 1 or 2, which dup2 replaces atomically);
        // the file is closed when `file` drops, leaving the duplicate.
        if unsafe { libc::dup2(file.as_raw_fd(), fd) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

fn to_null_or_closed(fd: i32, null: Option<u64>) -> bool {
    let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `st` is a valid buffer for one `stat`; fstat writes it only on success, which is checked first.
    let rc = unsafe { libc::fstat(fd, st.as_mut_ptr()) };
    if rc != 0 {
        return true;
    }
    // SAFETY: fstat succeeded, so the buffer is initialized.
    let st = unsafe { st.assume_init() };
    (st.st_mode & libc::S_IFMT) == libc::S_IFCHR && Some(st.st_rdev as u64) == null
}

/// `_raise_file_limit` (shared.py:24): the open-file soft limit set to the hard limit clamped to
/// `kern.maxfilesperproc` (children still see a finite OPEN_MAX). Best effort.
pub fn raise_file_limit() {
    let mut lim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    let mut per_proc: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    let name = c"kern.maxfilesperproc";
    // SAFETY: `lim` is a valid rlimit for getrlimit to fill and setrlimit to read; sysctlbyname writes at most
    // `size` bytes into `per_proc`, a c_int, and reads nothing from the null new-value pointer.
    unsafe {
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) != 0 {
            return;
        }
        let rc = libc::sysctlbyname(
            name.as_ptr(),
            (&mut per_proc as *mut libc::c_int).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        );
        let infinite = lim.rlim_max == libc::RLIM_INFINITY;
        let mut target = if infinite { lim.rlim_cur } else { lim.rlim_max };
        if rc == 0 && per_proc > 0 {
            let p = per_proc as libc::rlim_t;
            target = if infinite { p } else { target.min(p) };
        }
        if target != lim.rlim_cur {
            lim.rlim_cur = target;
            libc::setrlimit(libc::RLIMIT_NOFILE, &lim);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inside_an_app_the_shipping_rule_applies() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("FERB.app/Contents/MacOS/ferbd");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"").unwrap();
        assert_eq!(rule_for(&exe), Rule::Shipping);
        let plain = dir.path().join("bin/ferbd");
        std::fs::create_dir_all(plain.parent().unwrap()).unwrap();
        std::fs::write(&plain, b"").unwrap();
        assert_eq!(rule_for(&plain), Rule::Python);
    }
}
