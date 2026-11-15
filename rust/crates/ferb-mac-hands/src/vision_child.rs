//! Text recognition in a short-lived child process of FERB's own program (HAND-531): the last attempt before an image
//! is withheld, for when Vision fails inside the long-running daemon while a fresh process reads the same image.
//!
//! The image goes to the child's standard input and the regions come back on its standard output, as JSON; nothing
//! is written to disk, the child's error output is discarded, and a child that doesn't answer within its limit is
//! killed. One child runs at a time ([`ChildVision::gate`]): an image that may wait (a tool's, the front window's, a
//! recording's) waits for the one running; a live preview's frame doesn't queue behind it.
//!
//! The child's side is [`serve`]: FERB's `ferb` program runs it as [`COMMAND`] (`native::vision::child_main`).

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::ocr::{domain, Fault, Recognize, TextRegion};

/// The internal command of FERB's `ferb` program that reads an image's text (`ferb read-image-text`).
pub const COMMAND: &str = "read-image-text";

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

/// Reading an image's text in a child process: `program` with `args` (FERB's `ferb` with [`COMMAND`]).
#[derive(Clone)]
pub struct ChildVision {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub timeout: Duration,
    /// One child at a time, across every image of the process.
    pub gate: Arc<Mutex<()>>,
    /// Whether this image waits for a child already running; when not, it fails at once ([`domain::BUSY`]).
    pub wait: bool,
}

impl ChildVision {
    fn run(&self, image: &[u8]) -> Result<Vec<TextRegion>, Fault> {
        let mut child = Command::new(&self.program)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| Fault::new(domain::START, e.raw_os_error().map_or(-1, i64::from)))?;
        // the image in a thread of its own, so a child that answers before reading it all never blocks the parent;
        // a child that ended early only breaks the pipe
        let mut stdin = child.stdin.take();
        let image = image.to_vec();
        let writer = std::thread::Builder::new()
            .name("ferb-image-child-in".into())
            .spawn(move || {
                if let Some(s) = stdin.as_mut() {
                    let _ = s.write_all(&image);
                }
                drop(stdin);
            });
        let mut stdout = child.stdout.take();
        let (tx, rx) = std::sync::mpsc::channel();
        let reader = std::thread::Builder::new()
            .name("ferb-image-child-out".into())
            .spawn(move || {
                let mut out = Vec::new();
                let read = stdout
                    .as_mut()
                    .map(|s| s.take(MAX_REPLY as u64 + 1).read_to_end(&mut out));
                let _ = tx.send((read.is_some_and(|r| r.is_ok()), out));
            });
        if writer.is_err() || reader.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Fault::new(domain::START, -1));
        }
        let got = rx.recv_timeout(self.timeout);
        let Ok((read_all, out)) = got else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Fault::new(domain::TIMEOUT, self.timeout.as_secs() as i64));
        };
        // the answer is complete once its output closed; the child ends right after
        let status = child.wait();
        if read_all && out.len() <= MAX_REPLY && !out.is_empty() {
            return parse_reply(&out);
        }
        let code = match status {
            Ok(s) => s.code().map_or(-1, i64::from),
            Err(_) => -1,
        };
        Err(Fault::new(domain::EXIT, code))
    }
}

impl Recognize for ChildVision {
    fn recognize(&self, image: &[u8]) -> Result<Vec<TextRegion>, Fault> {
        let _held = if self.wait {
            self.gate.lock().unwrap_or_else(|e| e.into_inner())
        } else {
            match self.gate.try_lock() {
                Ok(g) => g,
                Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => {
                    return Err(Fault::new(domain::BUSY, 0))
                }
            }
        };
        self.run(image)
    }
}
