//! The daemon host inside FERB.app: what `ui/mac/FERBAgent/main.c` guaranteed before Python started, done here before
//! anything else runs (`run::main` calls [`enter`] first, while the process has one thread).
//!
//! FERB.app's nested agent app (`Contents/Helpers/FERB.app`, bundle id `com.ferb.daemon`) has this program as its
//! executable (ferb-device `BUNDLE_AGENT`), so macOS's permissions attach to it. Code here enforces where it may be
//! used (AGENTS.md rule 3), and only when it runs from inside an `.app` ([`crate::home::Rule::Shipping`]); a checkout's
//! or test's `ferbd` is untouched (tests and a development daemon keep `FERB_HOME` and their environment).
//!
//! - **Only FERB's own entry modes** ([`AGENT_MODES`]), exactly as FERB starts them: `daemon` (the service
//!   definition), `mcp` (the model programs' tool bridge, started through [`crate::compose::runtimes::mcp_shim_command`])
//!   and `grant-probe` (the daemon's fresh reading of its grants, APP-662). No extra argument, no other command: the
//!   internal commands FERB.app runs (`setup --service`, `uninstall`, `setup --cli`...) are the separate program
//!   `Contents/Resources/bin/ferb` (`src/bin/ferb.rs`), which holds no permission. main.c's fourth mode, the card
//!   reader, is not a process in the Rust build.
//! - **Only the owner's FERB home**: [`crate::home::resolve`] (the account's own `~/.ferb` from the account database,
//!   `FERB_HOME` refused unless it is that folder by device and inode), plus the development override [`DEV_HOME`],
//!   which only an ad-hoc-signed build honors ([`dev_home`]).
//! - **A clean environment, rebuilt** ([`clean_environment`]): HOME, USER, LOGNAME and SHELL from the account
//!   database, a fixed PATH, the account's own temporary folder, FERB_HOME, and only [`CARRIED`] (plus `LC_*`) from
//!   what the process was started with. No proxy or certificate override, no DYLD_* or anything else reaches the
//!   daemon or the programs it starts. The environment block is replaced whole, as main.c did, so an entry without a
//!   value is dropped too.
//! - **The real executable path** ([`real_exe`]): `current_exe` (`_NSGetExecutablePath`, which can be a link or a
//!   relative path) resolved, as main.c's `realpath`. The daemon re-enters itself (`mcp`, `grant-probe`) by this path.
//! - **No general-purpose launcher**: this program runs no code it is given (no script, no `-c`, no module); the modes
//!   above are compiled in.

use std::ffi::{CStr, CString, OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use crate::home::{self, Rule};

/// The modes FERB.app's agent runs, each exactly `[mode]` (main.c `entry_point`).
pub const AGENT_MODES: [&str; 3] = ["daemon", "mcp", "grant-probe"];

/// main.c's refusal of anything else (exit status 64, EX_USAGE).
pub const NOT_AN_ENTRY: &str =
    "FERB's background agent runs only FERB's own entry points; open FERB.app instead.";

/// The variables carried over by exact name (main.c `carried`); `LC_*` by prefix.
pub const CARRIED: [&str; 8] = [
    "LANG",
    "TZ",
    "__CF_USER_TEXT_ENCODING",
    "XPC_SERVICE_NAME",
    "XPC_FLAGS",
    "FERB_APP_BUILD",
    "FERB_RUN_TOKEN",
    "FERB_RUN_TOKEN_FILE",
];

/// main.c's fixed PATH.
pub const FIXED_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// The development override: an absolute folder used as FERB's home by a FERB.app build signed ad hoc (a build made
/// with `SIGN_IDENTITY=-`), so the bundled programs can be tried on a temporary home without touching the owner's
/// `~/.ferb`. A build signed with any certificate (FERB's release certificate, its Developer ID, any other) refuses
/// to start when it is set: what ships accepts only the owner's `~/.ferb`.
pub const DEV_HOME: &str = "FERB_DEV_HOME";

/// This executable's real path: links and relative parts resolved (main.c:122, `realpath`).
pub fn real_exe() -> Option<PathBuf> {
    std::env::current_exe().ok()?.canonicalize().ok()
}

/// Whether `args` (without the program name) is one of the agent's modes, exactly.
pub fn agent_mode(args: &[String]) -> bool {
    args.len() == 1 && AGENT_MODES.contains(&args[0].as_str())
}

/// Whether a variable named `name` is carried into the rebuilt environment.
pub fn carried(name: &[u8]) -> bool {
    (name.len() > 3 && name.starts_with(b"LC_")) || CARRIED.iter().any(|c| c.as_bytes() == name)
}

/// The host's checks, run first in FERB.app's agent: `Ok` to go on, or the exit status and the line for stderr.
/// Outside an `.app` nothing is checked or changed.
pub fn enter(args: &[String]) -> Result<(), (i32, String)> {
    let Some(exe) = real_exe() else {
        return Err((70, "FERB: can't find its own executable".into()));
    };
    if home::rule_for(&exe) != Rule::Shipping {
        return Ok(());
    }
    if !agent_mode(args) {
        return Err((64, NOT_AN_ENTRY.into()));
    }
    let Some(account) = Account::read() else {
        return Err((70, "FERB: can't read your account's home folder".into()));
    };
    let (ferb_home, _) = home::resolve().map_err(|e| (64, e))?;
    let dev = std::env::var_os(DEV_HOME).filter(|v| !v.is_empty());
    clean_environment(&account, &ferb_home, dev.as_deref()).map_err(|e| (70, format!("FERB: {e}")))
}

/// The development override's home, when [`DEV_HOME`] is set: `Ok(None)` when it isn't, `Err` with the refusal when
/// this build may not honor it (signed with a certificate) or the value is unusable.
pub fn dev_home() -> Result<Option<PathBuf>, String> {
    let Some(value) = std::env::var_os(DEV_HOME).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if !ad_hoc_signed() {
        return Err(format!(
            "FERB runs only with your own FERB home: {DEV_HOME} is honored only by a development build signed ad hoc, \
             and this FERB.app is signed with a certificate."
        ));
    }
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(format!("{DEV_HOME} must be an absolute folder"));
    }
    match std::env::var_os("FERB_HOME").filter(|v| !v.is_empty()) {
        Some(given) if Path::new(&given) != path => Err(format!(
            "FERB_HOME and {DEV_HOME} name different folders: set only {DEV_HOME}"
        )),
        _ => Ok(Some(path)),
    }
}

/// Whether this process's code is signed ad hoc (no certificate): its signature's flags (Security's
/// `SecCodeCopySigningInformation`, `kSecCodeInfoFlags`) carry kSecCodeSignatureAdhoc, as `codesign -dv` shows
/// `flags=0x2(adhoc)`. A certificate-signed build (release, Developer ID, any other), or code whose signature can't be
/// read, is not. (The kernel's per-process status, `csops`, doesn't keep the ad-hoc bit on Apple silicon: a
/// linker-signed process there reports 0x22020201.)
pub fn ad_hoc_signed() -> bool {
    signature_flags(None).is_some_and(|f| f & ADHOC != 0)
}

/// kSecCodeSignatureAdhoc.
const ADHOC: u32 = 0x0000_0002;

#[cfg(target_os = "macos")]
mod sig {
    use std::ffi::c_void;
    pub type CFTypeRef = *const c_void;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        pub fn CFDictionaryGetValue(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        pub fn CFNumberGetValue(number: CFTypeRef, kind: isize, value: *mut c_void) -> u8;
        pub fn CFURLCreateFromFileSystemRepresentation(
            allocator: CFTypeRef,
            path: *const u8,
            length: isize,
            is_directory: u8,
        ) -> CFTypeRef;
        pub fn CFRelease(object: CFTypeRef);
    }

    #[link(name = "Security", kind = "framework")]
    extern "C" {
        pub static kSecCodeInfoFlags: CFTypeRef;
        pub fn SecCodeCopySelf(flags: u32, code: *mut CFTypeRef) -> i32;
        pub fn SecStaticCodeCreateWithPath(
            path: CFTypeRef,
            flags: u32,
            code: *mut CFTypeRef,
        ) -> i32;
        pub fn SecCodeCopySigningInformation(
            code: CFTypeRef,
            flags: u32,
            info: *mut CFTypeRef,
        ) -> i32;
    }

    /// kSecCSSigningInformation.
    pub const SIGNING_INFORMATION: u32 = 1 << 1;
    /// kCFNumberSInt64Type.
    pub const SINT64: isize = 4;
}

/// The signature flags of this process's code (`path` None) or of the code at `path`.
#[cfg(target_os = "macos")]
pub fn signature_flags(path: Option<&Path>) -> Option<u32> {
    use sig::*;
    let mut code: CFTypeRef = std::ptr::null();
    // SAFETY: each Create/Copy result is checked for success and null before use and released exactly once below;
    // CFDictionaryGetValue and CFNumberGetValue only read objects that are alive while `info` is held; the number is
    // written into a live i64 of the kind named.
    unsafe {
        let status = match path {
            None => SecCodeCopySelf(0, &mut code),
            Some(p) => {
                let bytes = p.as_os_str().as_bytes();
                let url = CFURLCreateFromFileSystemRepresentation(
                    std::ptr::null(),
                    bytes.as_ptr(),
                    bytes.len() as isize,
                    0,
                );
                if url.is_null() {
                    return None;
                }
                let s = SecStaticCodeCreateWithPath(url, 0, &mut code);
                CFRelease(url);
                s
            }
        };
        if status != 0 || code.is_null() {
            return None;
        }
        let mut info: CFTypeRef = std::ptr::null();
        let status = SecCodeCopySigningInformation(code, SIGNING_INFORMATION, &mut info);
        CFRelease(code);
        if status != 0 || info.is_null() {
            return None;
        }
        let number = CFDictionaryGetValue(info, kSecCodeInfoFlags);
        let mut value: i64 = 0;
        let ok = !number.is_null()
            && CFNumberGetValue(number, SINT64, (&mut value as *mut i64).cast()) != 0;
        CFRelease(info);
        ok.then_some(value as u32)
    }
}

#[cfg(not(target_os = "macos"))]
pub fn signature_flags(_path: Option<&Path>) -> Option<u32> {
    None
}

/// The account this process runs as (main.c: `getpwuid(getuid())`, never `$HOME`).
pub struct Account {
    pub dir: OsString,
    pub name: OsString,
    pub shell: OsString,
}

impl Account {
    pub fn read() -> Option<Account> {
        let mut buf = vec![0 as libc::c_char; 16 * 1024];
        let mut pw = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: every pointer is to a live buffer of the stated size; `found` is null or points at `pw`, whose
        // strings live in `buf`, and both outlive the reads below.
        unsafe {
            let rc = libc::getpwuid_r(
                libc::getuid(),
                pw.as_mut_ptr(),
                buf.as_mut_ptr(),
                buf.len(),
                &mut found,
            );
            if rc != 0 || found.is_null() || (*found).pw_dir.is_null() || (*found).pw_name.is_null()
            {
                return None;
            }
            let text = |p: *const libc::c_char| -> OsString {
                if p.is_null() {
                    OsString::new()
                } else {
                    OsString::from_vec(CStr::from_ptr(p).to_bytes().to_vec())
                }
            };
            let account = Account {
                dir: text((*found).pw_dir),
                name: text((*found).pw_name),
                shell: text((*found).pw_shell),
            };
            (!account.dir.is_empty() && !account.name.is_empty()).then_some(account)
        }
    }
}

/// The account's own temporary folder (`confstr(_CS_DARWIN_USER_TEMP_DIR)`), when the system gives an absolute one.
#[cfg(target_os = "macos")]
fn user_temp_dir() -> Option<OsString> {
    let mut buf = vec![0 as libc::c_char; libc::PATH_MAX as usize];
    // SAFETY: confstr writes at most `buf.len()` bytes, NUL-terminated, into the live buffer.
    let n = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, buf.as_mut_ptr(), buf.len()) };
    if n == 0 || n > buf.len() {
        return None;
    }
    // SAFETY: confstr succeeded and NUL-terminated what it wrote within the buffer.
    let bytes = unsafe { CStr::from_ptr(buf.as_ptr()) }.to_bytes().to_vec();
    bytes.starts_with(b"/").then(|| OsString::from_vec(bytes))
}

#[cfg(not(target_os = "macos"))]
fn user_temp_dir() -> Option<OsString> {
    None
}

/// The entries of the rebuilt environment, in order: the fixed ones, then those carried from `current` (each
/// `NAME=value`; an entry without `=` is dropped).
pub fn environment(
    account: &Account,
    ferb_home: &Path,
    dev: Option<&OsStr>,
    temp: Option<&OsStr>,
    current: &[Vec<u8>],
) -> Vec<Vec<u8>> {
    let pair = |name: &str, value: &OsStr| -> Vec<u8> {
        let mut v = name.as_bytes().to_vec();
        v.push(b'=');
        v.extend_from_slice(value.as_bytes());
        v
    };
    let shell = if account.shell.is_empty() {
        OsStr::new("/bin/zsh")
    } else {
        account.shell.as_os_str()
    };
    let mut out = vec![
        pair("HOME", &account.dir),
        pair("USER", &account.name),
        pair("LOGNAME", &account.name),
        pair("SHELL", shell),
        pair("PATH", OsStr::new(FIXED_PATH)),
        pair("FERB_HOME", ferb_home.as_os_str()),
    ];
    if let Some(t) = temp {
        out.push(pair("TMPDIR", t));
    }
    if let Some(d) = dev {
        out.push(pair(DEV_HOME, d));
    }
    for entry in current {
        if let Some(eq) = entry.iter().position(|b| *b == b'=') {
            if carried(&entry[..eq]) {
                out.push(entry.clone());
            }
        }
    }
    out
}

/// Replaces the process's whole environment block with [`environment`]'s (main.c `clean_environment`). Only while
/// the process has one thread: [`enter`] runs first in `main`.
fn clean_environment(
    account: &Account,
    ferb_home: &Path,
    dev: Option<&OsStr>,
) -> Result<(), String> {
    let current = current_entries();
    let temp = user_temp_dir();
    let fresh = environment(account, ferb_home, dev, temp.as_deref(), &current);
    let mut strings = Vec::with_capacity(fresh.len());
    for entry in fresh {
        strings.push(
            CString::new(entry).map_err(|_| "an environment entry holds a NUL byte".to_string())?,
        );
    }
    // the block and its strings live for the rest of the process, as the environment must
    let mut pointers: Vec<*mut libc::c_char> = strings.into_iter().map(CString::into_raw).collect();
    pointers.push(std::ptr::null_mut());
    let block = Box::leak(pointers.into_boxed_slice());
    #[cfg(target_os = "macos")]
    // SAFETY: one thread runs (see above), so nothing reads `environ` while it changes; the new block is
    // NUL-terminated and leaked, so it outlives every later read; libc's setenv copies a block it didn't allocate
    // before changing it.
    unsafe {
        *libc::_NSGetEnviron() = block.as_mut_ptr();
    }
    #[cfg(not(target_os = "macos"))]
    let _ = block;
    Ok(())
}

/// The raw environment entries, read from the block itself (std's `vars_os` skips an entry without `=`).
fn current_entries() -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    #[cfg(target_os = "macos")]
    // SAFETY: `_NSGetEnviron` returns the process's environment block: a NULL-terminated array of NUL-terminated
    // strings, read here while one thread runs.
    unsafe {
        let mut p = *libc::_NSGetEnviron();
        if !p.is_null() {
            while !(*p).is_null() {
                out.push(CStr::from_ptr(*p).to_bytes().to_vec());
                p = p.add(1);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_agents_modes_exactly() {
        for ok in [["daemon"], ["mcp"], ["grant-probe"]] {
            assert!(agent_mode(&ok.map(String::from)));
        }
        let refused: [&[&str]; 9] = [
            &[],
            &["daemon", "x"],
            &["setup", "--service"],
            &["uninstall"],
            &["-c", "print('ran')"],
            &["--version"],
            &["mcp", "--x"],
            &["Daemon"],
            &["-m", "ferb.daemon"],
        ];
        for argv in refused {
            let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
            assert!(!agent_mode(&argv), "{argv:?}");
        }
    }

    #[test]
    fn the_environment_is_rebuilt_from_the_account_and_the_carried_names() {
        let account = Account {
            dir: "/Users/o".into(),
            name: "o".into(),
            shell: "".into(),
        };
        let current: Vec<Vec<u8>> = [
            "HOME=/evil",
            "PATH=/evil",
            "HTTPS_PROXY=http://127.0.0.1:9",
            "SSL_CERT_FILE=/evil",
            "DYLD_INSERT_LIBRARIES=/evil",
            "PYTHONPATH=/evil",
            "FERB_RUN_TOKEN=t",
            "LANG=en_US.UTF-8",
            "LC_ALL=C",
            "LC_=x",
            "FERB_RUN_TOKEN_FILE",
            "TMPDIR=/evil",
            "FERB_HOME=/evil",
            "FERB_DEV_HOME=/evil",
        ]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
        let env = environment(
            &account,
            Path::new("/Users/o/.ferb"),
            None,
            Some(OsStr::new("/var/folders/x/T/")),
            &current,
        );
        let env: Vec<String> = env
            .into_iter()
            .map(|e| String::from_utf8(e).unwrap())
            .collect();
        assert_eq!(
            env,
            [
                "HOME=/Users/o",
                "USER=o",
                "LOGNAME=o",
                "SHELL=/bin/zsh",
                "PATH=/usr/bin:/bin:/usr/sbin:/sbin",
                "FERB_HOME=/Users/o/.ferb",
                "TMPDIR=/var/folders/x/T/",
                "FERB_RUN_TOKEN=t",
                "LANG=en_US.UTF-8",
                "LC_ALL=C",
            ]
        );
        let dev = environment(
            &account,
            Path::new("/tmp/h"),
            Some(OsStr::new("/tmp/h")),
            None,
            &[],
        );
        assert!(dev.contains(&b"FERB_DEV_HOME=/tmp/h".to_vec()));
        assert!(dev.contains(&b"FERB_HOME=/tmp/h".to_vec()));
    }

    #[test]
    fn a_test_binary_is_ad_hoc_and_apples_code_is_not() {
        // cargo's linker signs a test binary ad hoc on Apple silicon; /usr/bin/true carries Apple's certificate
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            assert!(ad_hoc_signed());
            let exe = std::env::current_exe().unwrap();
            assert!(signature_flags(Some(&exe)).is_some_and(|f| f & ADHOC != 0));
            assert!(
                signature_flags(Some(Path::new("/usr/bin/true"))).is_some_and(|f| f & ADHOC == 0)
            );
            assert_eq!(signature_flags(Some(Path::new("/nonexistent/ferbd"))), None);
        }
    }

    /// Run by the test below, in a process of its own: replaces this process's environment block, then prints what
    /// std reads and what a child process is given.
    #[test]
    #[ignore = "run by the_rebuilt_environment_is_what_the_process_and_its_children_see in a process of its own"]
    fn rebuild_then_print() {
        if std::env::var_os("FERB_HOST_CHILD").is_none() {
            return;
        }
        let account = Account::read().unwrap();
        clean_environment(&account, Path::new("/tmp/ferb-host-home"), None).unwrap();
        println!("STD HOME={}", std::env::var("HOME").unwrap_or_default());
        let child = std::process::Command::new("/usr/bin/env").output().unwrap();
        for line in String::from_utf8_lossy(&child.stdout).lines() {
            println!("CHILD {line}");
        }
    }

    #[test]
    fn the_rebuilt_environment_is_what_the_process_and_its_children_see() {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "host::tests::rebuild_then_print",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("FERB_HOST_CHILD", "1")
            .env("HTTPS_PROXY", "http://127.0.0.1:9")
            .env("SSL_CERT_FILE", "/evil")
            .env("PATH", "/evil:/usr/bin:/bin")
            .env("HOME", "/evil")
            .env("FERB_RUN_TOKEN", "t")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        let account = Account::read().unwrap();
        let home = account.dir.to_string_lossy().into_owned();
        assert!(text.contains(&format!("STD HOME={home}")), "{text}");
        let child: Vec<&str> = text
            .lines()
            .filter_map(|l| l.strip_prefix("CHILD "))
            .collect();
        assert!(child.contains(&format!("HOME={home}").as_str()), "{text}");
        assert!(
            child.contains(&"PATH=/usr/bin:/bin:/usr/sbin:/sbin"),
            "{text}"
        );
        assert!(child.contains(&"FERB_HOME=/tmp/ferb-host-home"), "{text}");
        assert!(child.contains(&"FERB_RUN_TOKEN=t"), "{text}");
        for gone in [
            "HTTPS_PROXY=",
            "SSL_CERT_FILE=",
            "FERB_HOST_CHILD=",
            "RUST_TEST_THREADS=",
        ] {
            assert!(!child.iter().any(|l| l.starts_with(gone)), "{gone}: {text}");
        }
    }
}
