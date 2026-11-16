//! Text recognition in a child process of FERB's own program (HAND-531, HAND-627): the last attempt before an image
//! is withheld, for when Vision fails inside the long-running daemon while a fresh process reads the same image.
//!
//! HAND-627: the child is kept running ([`Helper`], `ferb read-image-text --serve`): Vision loads its model once in it
//! (6-65 s on the owner's Mac, 2026-11-16), then each image takes tens of milliseconds. Images go to its standard
//! input and the regions come back on its standard output, each framed by its length ([`serve_loop`]); nothing is
//! written to disk and its error output is discarded. One image at a time: a picture that comes while it starts waits
//! for it. When it fails it starts again at once, then after a growing wait ([`backoff`]). The daemon's check thread
//! ([`Readers::tick`]) reads a small test picture with this process's own Vision at the start and every
//! [`SELF_TEST_EVERY`]: when that fails, the child is started and warmed before a task needs a picture; when it reads
//! again, the child is stopped. It ends with the daemon (its input closes).
//!
//! The one-shot child is [`serve`] ([`COMMAND`]), kept for the by-hand test's program.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::ocr::{domain, Attempt, Fault, Recognize, Redacted, TextRegion, Withheld};

/// The internal command of FERB's `ferb` program that reads an image's text (`ferb read-image-text`).
pub const COMMAND: &str = "read-image-text";
/// `ferb read-image-text --serve`: the kept child, one framed image after another ([`serve_loop`]).
pub const SERVE: &str = "--serve";
/// HAND-627 (method constants, not owner caps): after its first failure in a row the helper starts again at once;
/// after the next ones it waits [`RESTART_BASE`], doubled each time, at most [`RESTART_CEILING`]. A success resets it.
pub const RESTART_BASE: Duration = Duration::from_secs(5);
pub const RESTART_CEILING: Duration = Duration::from_secs(300);
/// HAND-627: how often the daemon's check thread runs [`Readers::tick`], and how often a tick reads the small test
/// picture with this process's own Vision.
pub const TICK: Duration = Duration::from_secs(30);
pub const SELF_TEST_EVERY: Duration = Duration::from_secs(300);

/// The wait before the helper's next start after `failures` failures in a row (HAND-627).
pub fn backoff(failures: u32) -> Duration {
    if failures <= 1 {
        return Duration::ZERO;
    }
    let doubled = RESTART_BASE.saturating_mul(1u32 << (failures - 2).min(20));
    doubled.min(RESTART_CEILING)
}

/// The longest image the child takes, and the longest answer the parent reads.
pub const MAX_IMAGE: usize = 64 * 1024 * 1024;
pub const MAX_REPLY: usize = 16 * 1024 * 1024;

/// How long a child may take: Vision's first load in a fresh process took 26 s on the owner's Mac (2026-11-15) and
/// 50-100 s in the past (DMN-520), so the limit is above that.
pub const CHILD_TIMEOUT: Duration = Duration::from_secs(150);

/// The child's answer: its regions, or the fault its own attempts ended with.
pub fn reply(result: &Result<Vec<TextRegion>, Fault>) -> Vec<u8> {
    let v = match result {
        Ok(regions) => json!({"regions": regions.iter().map(|r| json!({
            "text": r.text, "box": [r.bbox.0, r.bbox.1, r.bbox.2, r.bbox.3],
        })).collect::<Vec<_>>()}),
        Err(f) => json!({"fault": {
            "step": f.step, "domain": f.domain, "code": f.code,
            "under": f.under.as_ref().map(|(d, c)| json!([d, c])),
        }}),
    };
    serde_json::to_vec(&v).unwrap_or_default()
}

/// The parent's reading of [`reply`]: the regions, the child's fault (its domain and code), or a reply fault.
pub fn parse_reply(bytes: &[u8]) -> Result<Vec<TextRegion>, Fault> {
    let bad = || Fault::new(domain::REPLY, 0);
    let v: Value = serde_json::from_slice(bytes).map_err(|_| bad())?;
    if let Some(f) = v.get("fault") {
        let mut fault = Fault::new(
            f.get("domain").and_then(Value::as_str).ok_or_else(bad)?,
            f.get("code").and_then(Value::as_i64).ok_or_else(bad)?,
        );
        fault.under = f
            .get("under")
            .and_then(|u| Some((u.get(0)?.as_str()?.to_string(), u.get(1)?.as_i64()?)));
        return Err(fault);
    }
    let mut out = Vec::new();
    for r in v.get("regions").and_then(Value::as_array).ok_or_else(bad)? {
        let text = r.get("text").and_then(Value::as_str).ok_or_else(bad)?;
        let b = r.get("box").and_then(Value::as_array).ok_or_else(bad)?;
        let n = |i: usize| b.get(i).and_then(Value::as_i64).ok_or_else(bad);
        if b.len() != 4 {
            return Err(bad());
        }
        out.push(TextRegion {
            text: text.to_string(),
            bbox: (n(0)?, n(1)?, n(2)?, n(3)?),
        });
    }
    Ok(out)
}

/// The child's side: the image from `input` (at most [`MAX_IMAGE`] bytes), read by each recognizer in turn until one
/// does, the answer to `output`. Exit status 0 when an answer was written, 65 for an input it refused, 74 when the
/// answer couldn't be written.
pub fn serve(input: &mut dyn Read, output: &mut dyn Write, recognizers: &[&dyn Recognize]) -> i32 {
    let mut image = Vec::new();
    if input
        .take(MAX_IMAGE as u64 + 1)
        .read_to_end(&mut image)
        .is_err()
        || image.is_empty()
        || image.len() > MAX_IMAGE
    {
        return 65;
    }
    let mut result = Err(Fault::new(domain::REPLY, 0));
    for r in recognizers {
        result = r.recognize(&image);
        if result.is_ok() {
            break;
        }
    }
    let out = reply(&result);
    if output.write_all(&out).and_then(|_| output.flush()).is_err() {
        return 74;
    }
    0
}

/// The kept child's side (`ferb read-image-text --serve`): images from `input`, each a 4-byte big-endian length and
/// that many bytes (at most [`MAX_IMAGE`]), each read by the recognizers in turn; for each, a 4-byte length and the
/// [`reply`] on `output`. Exit status 0 at the end of the input, 65 for a frame it refused, 74 when an answer couldn't
/// be written.
pub fn serve_loop(
    input: &mut dyn Read,
    output: &mut dyn Write,
    recognizers: &[&dyn Recognize],
) -> i32 {
    loop {
        let mut len = [0u8; 4];
        match input.read_exact(&mut len) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return 0,
            Err(_) => return 65,
        }
        let n = u32::from_be_bytes(len) as usize;
        if n == 0 || n > MAX_IMAGE {
            return 65;
        }
        let mut image = vec![0u8; n];
        if input.read_exact(&mut image).is_err() {
            return 65;
        }
        let mut result = Err(Fault::new(domain::REPLY, 0));
        for r in recognizers {
            result = r.recognize(&image);
            if result.is_ok() {
                break;
            }
        }
        let out = reply(&result);
        let framed = (out.len() as u32)
            .to_be_bytes()
            .into_iter()
            .chain(out)
            .collect::<Vec<u8>>();
        if output
            .write_all(&framed)
            .and_then(|_| output.flush())
            .is_err()
        {
            return 74;
        }
    }
}

/// Monotonic time as the helper reads it (tests move it by hand).
pub type Clock = Arc<dyn Fn() -> Duration + Send + Sync>;

/// The real clock: time since the first reading.
pub fn real_clock() -> Clock {
    let origin = std::time::Instant::now();
    Arc::new(move || origin.elapsed())
}

/// The kept child that reads images' text (HAND-627): `program` with `args` (FERB's `ferb` with [`COMMAND`] and
/// [`SERVE`]). One image at a time; a caller waits for the one before it.
pub struct Helper {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// How long one image may take, the child's first load of Vision included ([`CHILD_TIMEOUT`]).
    pub timeout: Duration,
    clock: Clock,
    state: Mutex<HelperState>,
}

#[derive(Default)]
struct HelperState {
    running: Option<Running>,
    /// Failures in a row (a start, an end, a fault, a timeout); a read resets it.
    failures: u32,
    /// No start before this (the backoff).
    next_start: Duration,
}

impl HelperState {
    fn failed(&mut self, now: Duration) {
        self.failures = self.failures.saturating_add(1);
        self.next_start = now + backoff(self.failures);
    }
}

/// A started child: its process, the image writer's queue, and the replies its reader passes on (None: it ended).
struct Running {
    child: Child,
    to_child: mpsc::Sender<Vec<u8>>,
    from_child: mpsc::Receiver<Option<Vec<u8>>>,
}

impl Running {
    /// Ends the child: its input closes, it is killed and reaped. Returns its exit status's code (-1 for none).
    fn end(mut self) -> i64 {
        drop(self.to_child);
        let _ = self.child.kill();
        match self.child.wait() {
            Ok(s) => s.code().map_or(-1, i64::from),
            Err(_) => -1,
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Helper {
    pub fn new(program: PathBuf, args: Vec<String>, timeout: Duration) -> Helper {
        Helper::with_clock(program, args, timeout, real_clock())
    }

    pub fn with_clock(
        program: PathBuf,
        args: Vec<String>,
        timeout: Duration,
        clock: Clock,
    ) -> Helper {
        Helper {
            program,
            args,
            timeout,
            clock,
            state: Mutex::new(HelperState::default()),
        }
    }

    /// Whether a child is running now.
    pub fn running(&self) -> bool {
        lock(&self.state).running.is_some()
    }

    /// Ends the child, if one runs (the daemon's own Vision reads again, or the daemon stops).
    pub fn stop(&self) {
        let running = lock(&self.state).running.take();
        if let Some(r) = running {
            r.end();
        }
    }

    fn start(&self) -> Result<Running, Fault> {
        let mut child = Command::new(&self.program)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| Fault::new(domain::START, e.raw_os_error().map_or(-1, i64::from)))?;
        let (to_child, images) = mpsc::channel::<Vec<u8>>();
        let (replies, from_child) = mpsc::channel::<Option<Vec<u8>>>();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let writer = std::thread::Builder::new()
            .name("ferb-image-helper-in".into())
            .spawn(move || {
                let Some(mut stdin) = stdin else { return };
                for image in images {
                    let framed = (image.len() as u32).to_be_bytes();
                    if stdin
                        .write_all(&framed)
                        .and_then(|_| stdin.write_all(&image))
                        .and_then(|_| stdin.flush())
                        .is_err()
                    {
                        return;
                    }
                }
            });
        let reader = std::thread::Builder::new()
            .name("ferb-image-helper-out".into())
            .spawn(move || {
                let Some(mut stdout) = stdout else {
                    let _ = replies.send(None);
                    return;
                };
                loop {
                    let mut len = [0u8; 4];
                    if stdout.read_exact(&mut len).is_err() {
                        let _ = replies.send(None);
                        return;
                    }
                    let n = u32::from_be_bytes(len) as usize;
                    let mut body = vec![0u8; n.min(MAX_REPLY + 1)];
                    if n > MAX_REPLY || stdout.read_exact(&mut body).is_err() {
                        let _ = replies.send(None);
                        return;
                    }
                    if replies.send(Some(body)).is_err() {
                        return;
                    }
                }
            });
        let running = Running {
            child,
            to_child,
            from_child,
        };
        if writer.is_err() || reader.is_err() {
            running.end();
            return Err(Fault::new(domain::START, -1));
        }
        Ok(running)
    }

    /// One image's regions, from the kept child (started first when none runs, within the start limit).
    pub fn read(&self, image: &[u8]) -> Result<Vec<TextRegion>, Fault> {
        let mut st = lock(&self.state);
        let ended = st
            .running
            .as_mut()
            .is_some_and(|r| r.child.try_wait().ok().flatten().is_some());
        if ended {
            if let Some(r) = st.running.take() {
                r.end();
            }
        }
        let now = (self.clock)();
        if st.running.is_none() {
            // HAND-627: within the backoff it waits; a picture that comes while a start is under way waits on the lock
            if now < st.next_start {
                let left = (st.next_start - now).as_secs_f64().ceil() as i64;
                return Err(Fault::new(domain::BACKOFF, left));
            }
            match self.start() {
                Ok(r) => st.running = Some(r),
                Err(f) => {
                    st.failed(now);
                    return Err(f);
                }
            }
        }
        let running = st.running.as_mut().expect("a running helper");
        let sent = running.to_child.send(image.to_vec()).is_ok();
        let got = if sent {
            running.from_child.recv_timeout(self.timeout)
        } else {
            Ok(None)
        };
        let fault = match got {
            Ok(Some(body)) => match parse_reply(&body) {
                Ok(regions) => {
                    st.failures = 0;
                    st.next_start = Duration::ZERO;
                    return Ok(regions);
                }
                // the image's own (it couldn't be decoded): the child stays
                Err(f) if f.domain == domain::DECODE => return Err(f),
                Err(f) => f,
            },
            Ok(None) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                let code = st.running.take().map_or(-1, Running::end);
                st.failed((self.clock)());
                return Err(Fault::new(domain::EXIT, code));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                Fault::new(domain::TIMEOUT, self.timeout.as_secs() as i64)
            }
        };
        // a child that faulted or didn't answer in time is ended: the next image starts a fresh one (after the backoff)
        if let Some(r) = st.running.take() {
            r.end();
        }
        st.failed((self.clock)());
        Err(fault)
    }

    /// Failures in a row now, and when the next start may be (HAND-627).
    pub fn backoff_state(&self) -> (u32, Duration) {
        let st = lock(&self.state);
        (st.failures, st.next_start)
    }

    /// No child runs and the backoff allows a start now: a tick may warm it.
    pub fn due(&self) -> bool {
        let st = lock(&self.state);
        st.running.is_none() && (self.clock)() >= st.next_start
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Recognize for Helper {
    fn recognize(&self, image: &[u8]) -> Result<Vec<TextRegion>, Fault> {
        self.read(image)
    }
}

/// The readers an image goes through (HAND-531, HAND-627): this process's own Vision (`in_process`: as it chooses its
/// devices, then on the CPU), then the kept child. While this process's Vision is found failing (by an image or by the
/// self-test), images go straight to the child; when the self-test or an image finds it reading again, the child is
/// stopped. [`tick`](Self::tick) finds a failure before a task needs a picture and warms the child then.
pub struct Readers {
    pub in_process: Vec<Attempt>,
    pub helper: Option<Arc<Helper>>,
    /// The small picture the self-test and the warm-up read (a blank image on the Mac).
    probe: Vec<u8>,
    clock: Clock,
    failed: std::sync::atomic::AtomicBool,
    next_test: Mutex<Duration>,
}

/// What one [`Readers::tick`] did (for the daemon's journal and tests).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tick {
    /// The self-test ran: Some(true) this process's Vision read the test picture.
    pub tested: Option<bool>,
    /// The child was started (or found running) and read the test picture: Some(false) it failed.
    pub warmed: Option<bool>,
    /// Images went back to this process's Vision and the child was stopped.
    pub switched_back: bool,
    /// The self-test found this process's Vision failing where it read before: images go to the child from now.
    pub went_to_helper: bool,
}

impl Readers {
    pub fn new(
        in_process: Vec<Attempt>,
        helper: Option<Arc<Helper>>,
        probe: Vec<u8>,
        clock: Clock,
    ) -> Readers {
        Readers {
            in_process,
            helper,
            probe,
            clock,
            failed: std::sync::atomic::AtomicBool::new(false),
            next_test: Mutex::new(Duration::ZERO),
        }
    }

    /// This process's Vision is found failing now: images go to the child.
    pub fn in_process_failed(&self) -> bool {
        self.failed.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Records whether this process's Vision fails; (switched back, went to the child). Back: the child is stopped.
    fn set_failed(&self, failed: bool) -> (bool, bool) {
        let was = self
            .failed
            .swap(failed, std::sync::atomic::Ordering::SeqCst);
        let back = was && !failed;
        if back {
            if let Some(h) = &self.helper {
                h.stop();
            }
        }
        (back, !was && failed)
    }

    /// One run of the daemon's check thread (every [`TICK`]): the self-test when it is due (at the first tick, then
    /// every [`SELF_TEST_EVERY`]), and while this process's Vision fails, the child warmed when no child runs and its
    /// backoff allows a start. Blocking (a warm-up waits for the child's first load): run it on its own thread.
    pub fn tick(&self) -> Tick {
        let mut out = Tick::default();
        let now = (self.clock)();
        let due = {
            let mut next = lock(&self.next_test);
            let due = now >= *next;
            if due {
                *next = now + SELF_TEST_EVERY;
            }
            due
        };
        if due && !self.in_process.is_empty() {
            let reads = self
                .in_process
                .iter()
                .any(|a| a.recognize.recognize(&self.probe).is_ok());
            out.tested = Some(reads);
            (out.switched_back, out.went_to_helper) = self.set_failed(!reads);
        }
        if self.in_process_failed() {
            if let Some(h) = self.helper.as_ref().filter(|h| h.running() || h.due()) {
                out.warmed = Some(h.read(&self.probe).is_ok());
            }
        }
        out
    }

    /// The attempts for the next image; `helper`: whether the child may be used (a warm-up's blank image doesn't).
    pub fn attempts(&self, helper: bool) -> Vec<Attempt> {
        // skipped only when the child is there to read it instead
        let skip = helper && self.helper.is_some() && self.in_process_failed();
        let mut out = if skip {
            vec![]
        } else {
            self.in_process.clone()
        };
        if helper {
            if let Some(h) = &self.helper {
                out.push(Attempt {
                    step: "child",
                    recognize: h.clone(),
                });
            }
        }
        out
    }

    /// What one image's reading says about this process's own Vision: it read it (the child is stopped), or every
    /// in-process attempt failed (images go to the child until the self-test finds it reading again).
    pub fn settle(&self, got: &Result<Redacted, Withheld>) {
        let steps: Vec<&str> = self.in_process.iter().map(|a| a.step).collect();
        let faults = match got {
            Ok(r) if steps.contains(&r.read_by) => {
                self.set_failed(false);
                if let Some(h) = &self.helper {
                    h.stop();
                }
                return;
            }
            Ok(r) => &r.faults,
            Err(w) => &w.faults,
        };
        let all_failed =
            !steps.is_empty() && steps.iter().all(|s| faults.iter().any(|f| f.step == *s));
        if all_failed {
            self.set_failed(true);
        }
    }
}
