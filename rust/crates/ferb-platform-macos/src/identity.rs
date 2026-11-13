//! The Mac's part of FERB's identity storage: the launcher of the pinned, signed secret helper
//! (`ferb-secrets`, which keeps values in the login Keychain), and where Chrome keeps its profiles. The
//! protocol, the refusals' meaning and the owner-secrets rules are `ferb-identity`'s; this is
//! `ferb/identity/helper.py:29-47` only.
//!
//! - The helper is `<home>/bin/ferb-secrets`, and before every use `/usr/bin/codesign --verify
//!   --strict -R=<pin>` must accept it, where the pin is the requirement text setup wrote to
//!   `<home>/bin/ferb-secrets.requirement` (`ferb/device/install.py:849-923`: a certificate, or
//!   the exact code hash). No pin, no file or a refused signature means no run; there is no
//!   unsigned fallback and no other way to choose the program (`helper.py:29-38`).
//! - A value travels only on the child's stdin and stdout pipes: never in argv, never in the
//!   environment. stderr is discarded. The child gets a scrubbed environment (see [`PASSED_ENV`]).

use std::{
    ffi::OsString,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
};

use ferb_identity::{
    helper::{Helper, Launcher},
    owner_secrets::{OwnerSecrets, Places},
    py, Error, Result,
};
use zeroize::Zeroizing;

/// `helper.py:34`. An absolute path: `PATH` is never searched for the verifier.
const CODESIGN: &str = "/usr/bin/codesign";

/// The only environment the helper and `codesign` get: where the login keychain is found and
/// what Foundation reads at start. Python's `subprocess.run` hands the daemon's whole environment
/// on (`helper.py:42`); nothing FERB needs from the helper depends on the rest, and a provider
/// key in the daemon's environment has no business in a child (decision: GATE-548).
pub const PASSED_ENV: [&str; 5] = [
    "HOME",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "__CF_USER_TEXT_ENCODING",
];

/// The Mac's [`Launcher`]: `codesign` against the pin, then the helper as a child process.
#[derive(Clone, Copy, Debug, Default)]
pub struct SignedHelper;

/// The helper of the FERB home `home`, verified and run as the Mac does.
pub fn helper(home: impl Into<PathBuf>) -> Helper {
    Helper::new(home, Arc::new(SignedHelper))
}

/// The store Python's `OwnerSecrets(home)` is: values through the signed helper of `home`.
pub fn owner_secrets(home: &Path, places: Arc<dyn Places>) -> OwnerSecrets {
    OwnerSecrets::with_keychain(home, Arc::new(helper(home)), places)
}

/// `seed.py:9`: `~/Library/Application Support/Google/Chrome`.
pub fn chrome_data() -> PathBuf {
    std::env::home_dir()
        .unwrap_or_default()
        .join("Library/Application Support/Google/Chrome")
}

impl Launcher for SignedHelper {
    /// `helper.py:29-38`: the helper's path, only once `codesign` accepted it against the pin.
    fn verified(&self, home: &Path) -> Result<PathBuf> {
        let binary = home.join("bin").join("ferb-secrets");
        let requirement = home.join("bin").join("ferb-secrets.requirement");
        if !binary.is_file() || !requirement.is_file() {
            return Err(Error::Refused(
                "the signed secret helper is unavailable; owner setup is required".into(),
            ));
        }
        let pin = fs::read_to_string(&requirement)?;
        let mut check = Command::new(CODESIGN);
        check
            .args(codesign_args(&pin, &binary))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        scrub_env(&mut check);
        if !check.status()?.success() {
            return Err(Error::Refused(
                "the secret helper signature does not match the installed identity".into(),
            ));
        }
        Ok(binary)
    }

    fn run(&self, program: &Path, body: &[u8]) -> io::Result<(bool, Zeroizing<Vec<u8>>)> {
        run(program, body)
    }
}

/// `codesign`'s arguments (`helper.py:34`): the pin as one argument, never through a shell.
fn codesign_args(pin: &str, binary: &Path) -> Vec<OsString> {
    vec![
        "--verify".into(),
        "--strict".into(),
        format!("-R={}", py::strip(pin)).into(),
        binary.as_os_str().to_owned(),
    ]
}

fn scrub_env(command: &mut Command) {
    command.env_clear();
    for name in PASSED_ENV {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
}

/// `helper.py:41-47`: run the helper with the request on its stdin; whether it failed (any exit
/// but 0, a signal included) and what it wrote on stdout. No argument, private pipes, stderr
/// discarded. (Python's `close_fds=True` is Rust's default for its own descriptors: each is
/// opened close-on-exec.)
fn run(binary: &Path, body: &[u8]) -> io::Result<(bool, Zeroizing<Vec<u8>>)> {
    let mut command = Command::new(binary);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    scrub_env(&mut command);
    let mut child = command.spawn()?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let reply = std::thread::scope(|scope| {
        scope.spawn(move || {
            // A helper that exits without reading closes the pipe: as in Python's
            // `communicate`, that is not an error of its own; the exit code says it.
            let _ = stdin.write_all(body);
        });
        read_all(&mut stdout)
    });
    let status = child.wait()?;
    Ok((!status.success(), reply?))
}

/// Everything the child writes, in a buffer that is zeroed whenever it is left behind.
fn read_all(from: &mut impl Read) -> io::Result<Zeroizing<Vec<u8>>> {
    let mut out = Zeroizing::new(Vec::with_capacity(4096));
    let mut chunk = Zeroizing::new([0u8; 4096]);
    loop {
        let n = match from.read(&mut chunk[..]) {
            Ok(0) => return Ok(out),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if out.len() + n > out.capacity() {
            let mut bigger =
                Zeroizing::new(Vec::with_capacity((out.capacity() * 2).max(out.len() + n)));
            bigger.extend_from_slice(&out);
            out = bigger; // the smaller buffer is zeroed as it drops
        }
        out.extend_from_slice(&chunk[..n]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn fixture() -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/identity/cases.json");
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    /// The verifier's argv is Python's (`helper.py:34`), for the pin file as setup writes it
    /// (with its newline). The fixture's first helper case starts with Python's codesign call.
    #[test]
    fn codesign_is_asked_as_python_asks_it() {
        let fixture = fixture();
        let call = &fixture["helper"][0]["calls"][0];
        let want: Vec<String> = call["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap().to_owned())
            .collect();
        let pin = format!("{}\n", fixture["requirement"].as_str().unwrap());
        let mut got = vec![CODESIGN.to_owned()];
        got.extend(
            codesign_args(&pin, Path::new("$HOME/bin/ferb-secrets"))
                .into_iter()
                .map(|a| a.into_string().unwrap()),
        );
        assert_eq!(got, want);
        assert_eq!(call["stdout"], "null");
        assert_eq!(call["stderr"], "null");
    }

    #[test]
    fn a_reply_buffer_grows_without_losing_bytes() {
        let data: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
        assert_eq!(*read_all(&mut &data[..]).unwrap(), data);
    }
}
