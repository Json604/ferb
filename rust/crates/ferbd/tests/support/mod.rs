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
        std::fs::read_to_string(repo().join("rust/fixtures").join(rel)).expect("the fixture");
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
}

impl Run {
    pub fn paths(root: &Path, nn: &str) -> (PathBuf, PathBuf) {
        let base = root.join(nn);
        (base.join("home"), base.join("user"))
    }

    /// Starts `ferbd daemon` and waits until it answers `ping` and its start is done.
    pub fn start(root: &Path, nn: &str, config: Value) -> Run {
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
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
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
                    if c.recv().is_some()
                        && Path::new(&self.home.join("run/stand-ins.sock")).exists()
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
        let fd = child.stderr.as_ref().map(|e| {
            use std::os::fd::AsRawFd;
            e.as_raw_fd()
        });
        let Some(fd) = fd else {
            return String::new();
        };
        use std::os::fd::FromRawFd;
        // SAFETY: the descriptor stays owned by the child handle; ManuallyDrop never closes it here.
        let file = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd) });
        let mut out = String::new();
        let _ = std::io::Read::read_to_string(&mut &*file, &mut out);
        out
    }

    /// One request on the control socket.
    pub fn control(&self, req: Value) -> Value {
        let s = UnixStream::connect(self.home.join("run/stand-ins.sock")).expect("control");
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

    /// SIGTERM, then the exit status and what it wrote on stderr.
    pub fn stop(&mut self) -> (ExitStatus, String) {
        let mut child = self.child.take().expect("running");
        // SAFETY: the pid is our own child's, still unreaped (we hold it).
        unsafe {
            libc::kill(child.id() as i32, libc::SIGTERM);
        }
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    use std::io::Read;
                    let _ = e.read_to_string(&mut err);
                }
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
