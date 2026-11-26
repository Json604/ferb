//! The real `ferbd daemon` program on a temporary FERB home under /tmp (a socket path must be short), with the test
//! suite's stand-ins (`FERBD_STAND_INS`, honored only by the test build), its control socket, and raw line clients.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

pub const BIN: &str = env!("CARGO_BIN_EXE_ferbd");

pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

pub fn fixture(rel: &str) -> Value {
    let text =
        std::fs::read_to_string(repo().join("engine/fixtures").join(rel)).expect("the fixture");
    serde_json::from_str(&text).expect("JSON")
}

pub fn scenario(file: &Value, name: &str) -> Value {
    file["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == name)
        .cloned()
        .unwrap_or_else(|| panic!("no scenario {name}"))
}

/// A temporary root under /tmp.
pub fn root() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("fxd")
        .tempdir_in("/tmp")
        .unwrap()
}

/// One daemon process on `<root>/<nn>/home`.
pub struct Run {
    pub root: PathBuf,
    pub home: PathBuf,
    pub user: PathBuf,
    pub child: Option<Child>,
    pub stand: PathBuf,
    /// More of the program's environment (a test's own TMPDIR).
    pub env: Vec<(String, String)>,
    /// Where the program's stderr goes (`<root>/<nn>-stderr.log`, written afresh by each start): a file, never a pipe
    /// the test doesn't read, whose buffer the system may cut to 512 bytes when its pipe memory runs short (seen
    /// 2026-11-27: every start then blocked on its fifth line of stderr).
    pub err_log: PathBuf,
}

impl Run {
    pub fn paths(root: &Path, nn: &str) -> (PathBuf, PathBuf) {
        let base = root.join(nn);
        (base.join("home"), base.join("user"))
    }

    /// Starts `ferbd daemon` and waits until it answers `ping` and its start is done.
    pub fn start(root: &Path, nn: &str, config: Value) -> Run {
        Run::start_env(root, nn, config, Vec::new())
    }

    /// As `start`, with more of the program's environment.
    pub fn start_env(root: &Path, nn: &str, config: Value, env: Vec<(String, String)>) -> Run {
        let (home, user) = Run::paths(root, nn);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut cfg = config;
        cfg["user_home"] = json!(user.to_string_lossy());
        let stand = root.join(format!("{nn}-stand.json"));
        std::fs::write(&stand, cfg.to_string()).unwrap();
        let mut run = Run {
            root: root.to_path_buf(),
            home,
            user,
            child: None,
            stand,
            env,
            err_log: root.join(format!("{nn}-stderr.log")),
        };
        run.spawn();
        run
    }

    pub fn spawn(&mut self) {
        let _ = std::fs::remove_file(self.home.join("run/stand-ins.sock"));
        let child = Command::new(BIN)
            .arg("daemon")
            .env_clear()
            .env("FERB_HOME", &self.home)
            .env("HOME", &self.user)
            .env("PATH", "/usr/bin:/bin")
            .env("FERBD_STAND_INS", &self.stand)
            .envs(self.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&self.err_log).expect("the stderr file"))
            .spawn()
            .expect("ferbd starts");
        self.child = Some(child);
        self.wait_ready();
    }

    pub fn socket(&self) -> PathBuf {
        self.home.join("run/ferb.sock")
    }

    /// `ping` answered and every other call too (a call that waits for the start returns once it's done).
    pub fn wait_ready(&self) {
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(mut c) = Raw::connect(&self.socket()) {
                if c.hello("cli").is_ok() {
                    c.send(
                        r#"{"jsonrpc": "2.0", "id": 900, "method": "memory.list", "params": {}}"#,
                    );
                    // the control socket takes a connection: its file exists from bind(), a moment before
                    // listen(), and a connection in between is refused
                    if c.recv().is_some()
                        && UnixStream::connect(self.home.join("run/stand-ins.sock")).is_ok()
                    {
                        return;
                    }
                }
            }
            if Instant::now() >= until {
                panic!(
                    "the daemon didn't start (home {}): {}",
                    self.home.display(),
                    self.stderr_now()
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// What a daemon that didn't start wrote on stderr (it is stopped first, so the read ends).
    fn stderr_now(&self) -> String {
        let Some(child) = &self.child else {
            return String::new();
        };
        unsafe {
            // SAFETY: a plain signal to the child this test started.
            libc::kill(child.id() as i32, libc::SIGKILL);
        }
        std::fs::read_to_string(&self.err_log).unwrap_or_default()
    }

    /// One request on the control socket.
    pub fn control(&self, req: Value) -> Value {
        let s = match UnixStream::connect(self.home.join("run/stand-ins.sock")) {
            Ok(s) => s,
            Err(e) => panic!(
                "control {req}: {e}; the daemon wrote: {}",
                self.stderr_now()
            ),
        };
        s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        let mut w = s.try_clone().unwrap();
        writeln!(w, "{req}").unwrap();
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).expect("a control answer");
        if let Some(e) = v.get("error") {
            panic!("control {req}: {e}");
        }
        v["ok"].clone()
    }

    /// FERB's own stop (ferb-device `Installer::stop`, DMN-619): the stop marker names the daemon, then SIGTERM; the
    /// exit status and what it wrote on stderr.
    pub fn stop(&mut self) -> (ExitStatus, String) {
        let pid = self.child.as_ref().expect("running").id();
        ferb_device::install::mark_stop(&self.home, i64::from(pid)).unwrap();
        self.signal_stop(libc::SIGTERM)
    }

    /// A stop FERB didn't make (a `kill`, Activity Monitor): `signal` with no stop marker.
    pub fn signal_stop(&mut self, signal: i32) -> (ExitStatus, String) {
        let mut child = self.child.take().expect("running");
        // SAFETY: the pid is our own child's, still unreaped (we hold it).
        unsafe {
            libc::kill(child.id() as i32, signal);
        }
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                let err = std::fs::read_to_string(&self.err_log).unwrap_or_default();
                return (status, err);
            }
            assert!(Instant::now() < until, "the daemon didn't stop");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// A table of the store, read with sqlite through the control socket.
    pub fn table(&self, name: &str) -> Value {
        self.control(json!({"do": "table", "name": name}))
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// A raw connection: lines out, lines in.
pub struct Raw {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Raw {
    pub fn connect(path: &Path) -> std::io::Result<Raw> {
        let s = UnixStream::connect(path)?;
        s.set_read_timeout(Some(Duration::from_secs(30)))?;
        Raw::from_stream(s)
    }

    /// A connection already made (its read timeout as the caller set it).
    pub fn from_stream(s: UnixStream) -> std::io::Result<Raw> {
        Ok(Raw {
            reader: BufReader::new(s.try_clone()?),
            writer: s,
        })
    }

    pub fn hello(&mut self, role: &str) -> Result<Value, String> {
        self.send(&format!(
            r#"{{"jsonrpc": "2.0", "id": 0, "method": "auth.hello", "params": {{"role": "{role}"}}}}"#
        ));
        let line = self.recv().ok_or("closed")?;
        let v: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
        if v.get("error").is_some() {
            return Err(line);
        }
        Ok(v)
    }

    pub fn send(&mut self, line: &str) {
        let _ = self.writer.write_all(line.as_bytes());
        let _ = self.writer.write_all(b"\n");
    }

    /// The next line without its newline; None when closed.
    pub fn recv(&mut self) -> Option<String> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim_end_matches('\n').to_string()),
        }
    }
}

/// The fixture's `/FX` for this run's root.
pub fn fx(text: &str, root: &Path) -> String {
    text.replace(&root.to_string_lossy().to_string(), "/FX")
}

/// Every text stored once by its SHA-256 (`bodies.json` of the daemon fixtures).
pub fn line_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Object(o) if o.contains_key("sha256") => {
            let bodies = fixture("daemon/bodies.json");
            bodies[o["sha256"].as_str().unwrap()]
                .as_str()
                .expect("a body")
                .to_string()
        }
        other => panic!("not a line: {other}"),
    }
}

pub fn method_of(line: &str) -> (String, Option<i64>) {
    let v: Value = serde_json::from_str(line).unwrap_or(Value::Null);
    (
        v["method"].as_str().unwrap_or("").to_owned(),
        v["id"].as_i64(),
    )
}
