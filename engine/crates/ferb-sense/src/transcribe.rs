//! The daemon side of turning captured sound into text (`ferb/sense/transcribe.py`, L26). The recognition itself
//! runs in FERB.app with Apple's on-device recognizer (no cloud fallback); this side checks FERB.app's signature,
//! makes the answer file (also FERB.app's lease: deleting it stops FERB.app), starts FERB.app through Launch Services
//! in the background, waits within a time limit (decision: SENSE-400), reads the answer once and deletes it. Nothing
//! goes to stdout, stderr or a log. [`redacted`] builds what a model may see: the transcript through the redactor,
//! and sentence segments cut from that redacted text only.

use std::io;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use ferb_core::interfaces::BoxFuture;
use serde_json::{Map, Value};

use crate::paths;

/// transcribe.py:40: FERB.app's start, the recognizer's set-up and a short sound.
pub const LIMIT_S: f64 = 60.0;
/// transcribe.py:41: each second of sound.
pub const PER_AUDIO_S: f64 = 2.0;
/// transcribe.py:42.
pub const SENTENCE_END: [char; 8] = ['.', '?', '!', '…', '。', '?', '!', '\n'];

/// transcribe.py:45 `HearingUnavailable` and :103 `NotSigned`: FERB can't turn the sound into text now; the text
/// says why and what fixes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hearing {
    Unavailable(String),
    NotSigned(String),
}

impl Hearing {
    pub fn kind(&self) -> &'static str {
        match self {
            Hearing::Unavailable(_) => "HearingUnavailable",
            Hearing::NotSigned(_) => "NotSigned",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Hearing::Unavailable(m) | Hearing::NotSigned(m) => m,
        }
    }
}

/// transcribe.py:50.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

/// transcribe.py:57.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transcript {
    pub text: String,
    pub words: Vec<Word>,
    pub locale: String,
}

/// transcribe.py:63 `launch_args`: FERB_HOME tells FERB.app which audio folder its files must be in.
pub fn launch_args(app: &Path, mode: &str, args: &[String], home: Option<&Path>) -> Vec<String> {
    let home = home.map_or_else(
        || paths::text(app.parent().unwrap_or(Path::new(""))),
        paths::text,
    );
    let mut argv: Vec<String> = [
        "/usr/bin/open",
        "-g",
        "-n",
        "-W",
        "--env",
        &format!("FERB_HOME={home}"),
        &paths::text(app),
        "--args",
        mode,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    argv.extend(args.iter().cloned());
    argv
}

/// transcribe.py:71 `launch`: run `open … -W` until FERB.app exits; a cancelled wait kills the launch.
pub fn launch(argv: Vec<String>) -> BoxFuture<'static, io::Result<i32>> {
    Box::pin(async move {
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| io::Error::other("no program"))?;
        let mut child = tokio::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let status = child.wait().await?;
        Ok(status.code().unwrap_or(-1))
    })
}

/// transcribe.py:83 `hearing_pids` over `ps -axww -o pid=,command=` output: processes running FERB.app's
/// executable with `transcribe` or `status` and a path inside FERB's audio folder (never the running notch).
pub fn hearing_pids(app: &Path, folder: &Path, ps: &str, own_pid: i64) -> Vec<i64> {
    let pattern = format!(
        r"^{}/Contents/MacOS/\S+ (transcribe|status) .*{}/",
        regex::escape(&paths::text(app)),
        regex::escape(&paths::text(folder))
    );
    let Ok(re) = regex::Regex::new(&pattern) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in crate::software::py_splitlines(ps) {
        let line = ferb_py::value::strip(line);
        let (pid, command) = line.split_once(' ').unwrap_or((line, ""));
        if !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()) {
            if let Ok(pid) = pid.parse::<i64>() {
                if pid != own_pid && re.is_match(ferb_py::value::strip(command)) {
                    out.push(pid);
                }
            }
        }
    }
    out
}

/// The process table as `/bin/ps` lists it (empty when it can't be read).
pub fn hearing_pids_now(app: &Path, folder: &Path) -> Vec<i64> {
    let Ok(out) = std::process::Command::new("/bin/ps")
        .args(["-axww", "-o", "pid=,command="])
        .stdin(std::process::Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    hearing_pids(
        app,
        folder,
        &String::from_utf8_lossy(&out.stdout),
        i64::from(std::process::id()),
    )
}

/// transcribe.py:107 `signed`: FERB.app's signature meets the requirement setup recorded.
pub fn signed(app: &Path, requirement: &str) -> bool {
    std::process::Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict", &format!("-R={requirement}")])
        .arg(app)
        .stdin(std::process::Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success())
}

type Launch = Box<dyn Fn(Vec<String>) -> BoxFuture<'static, io::Result<i32>> + Send + Sync>;

/// transcribe.py:113.
pub struct Transcriber {
    pub app: PathBuf,
    requirement: Box<dyn Fn() -> Option<String> + Send + Sync>,
    launch: Launch,
    signed: Box<dyn Fn(&Path, &str) -> bool + Send + Sync>,
    /// FERB's home, whose audio folder FERB.app reads and answers in (None: the folder `app` is in).
    pub home: Option<PathBuf>,
    /// the lease file's random part (`secrets.token_hex(8)`)
    pub token: Box<dyn Fn() -> String + Send + Sync>,
    /// LIMIT_S, for tests
    pub limit_s: f64,
    in_flight: AtomicUsize,
}

fn token_hex() -> String {
    let mut b = [0u8; 8];
    let _ = getrandom::fill(&mut b);
    hex::encode(b)
}

fn unavailable(s: &str) -> Hearing {
    Hearing::Unavailable(s.to_string())
}

impl Transcriber {
    pub fn new(
        app: PathBuf,
        requirement: impl Fn() -> Option<String> + Send + Sync + 'static,
        launch: impl Fn(Vec<String>) -> BoxFuture<'static, io::Result<i32>> + Send + Sync + 'static,
        signed: impl Fn(&Path, &str) -> bool + Send + Sync + 'static,
        home: Option<PathBuf>,
    ) -> Transcriber {
        Transcriber {
            app,
            requirement: Box::new(requirement),
            launch: Box::new(launch),
            signed: Box::new(signed),
            home,
            token: Box::new(token_hex),
            limit_s: LIMIT_S,
            in_flight: AtomicUsize::new(0),
        }
    }

    /// FERB.app runs now for this many asks.
    pub fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }

    /// transcribe.py:124: an update must not end hearing while a transcription is in flight.
    pub fn busy(&self) -> bool {
        self.in_flight() > 0
    }

    fn checked(&self) -> Result<(), Hearing> {
        if !self.app.exists() {
            return Err(unavailable(
                "FERB.app isn't there, so FERB can't turn sound into text; download FERB again",
            ));
        }
        let ok = (self.requirement)()
            .filter(|r| !r.is_empty())
            .is_some_and(|r| (self.signed)(&self.app, &r));
        if !ok {
            return Err(Hearing::NotSigned(
                "FERB.app's signature isn't the one setup recorded, so FERB didn't give it the sound; set up FERB again"
                    .into(),
            ));
        }
        Ok(())
    }

    async fn ask(
        &self,
        mode: &str,
        folder: &Path,
        args: &[String],
        limit_s: f64,
    ) -> Result<Map<String, Value>, Hearing> {
        self.checked()?;
        let made = std::fs::create_dir_all(folder)
            .and_then(|_| std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700)));
        if let Err(e) = made {
            return Err(unavailable(&format!(
                "FERB couldn't prepare its audio folder ({e})"
            )));
        }
        let answer = folder.join(format!(".{mode}-{}.json", (self.token)()));
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&answer)
            .map_err(|e| unavailable(&format!("FERB couldn't make its answer file ({e})")))?;
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        let mut argv: Vec<String> = args.iter().take(1).cloned().collect();
        argv.push(paths::text(&answer));
        argv.extend(args.iter().skip(1).cloned());
        let argv = launch_args(&self.app, mode, &argv, self.home.as_deref());
        let result = async {
            let wait = std::time::Duration::from_secs_f64(limit_s.max(0.0));
            let code = match tokio::time::timeout(wait, (self.launch)(argv)).await {
                Err(_) => {
                    return Err(unavailable(&format!(
                        "FERB didn't finish turning the sound into text within {limit_s:.0} s, so it stopped; nothing was transcribed"
                    )))
                }
                Ok(Ok(code)) => code,
                Ok(Err(e)) => {
                    return Err(unavailable(&format!(
                        "FERB couldn't start FERB.app ({})",
                        crate::software::os_error_name(&e)
                    )))
                }
            };
            let raw = std::fs::read_to_string(&answer).unwrap_or_default();
            if raw.is_empty() {
                return Err(unavailable(&format!("FERB gave no answer (exit {code})")));
            }
            ferb_py::json::loads(&raw).map_err(|_| unavailable("FERB's hearing answer couldn't be read"))
        }
        .await;
        // read once and never kept; a stop deletes it first (FERB.app exits)
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        let _ = std::fs::remove_file(&answer);
        match result? {
            Value::Object(m) => Ok(m),
            _ => Err(unavailable("FERB's hearing answer couldn't be read")),
        }
    }

    /// transcribe.py:166: `audio_s` (the sound's length) sets the time limit.
    pub async fn transcribe(
        &self,
        audio: &Path,
        language: Option<&str>,
        audio_s: f64,
    ) -> Result<Transcript, Hearing> {
        let mut args = vec![paths::text(audio)];
        if let Some(l) = language.filter(|l| !l.is_empty()) {
            args.push(l.to_string());
        }
        let folder = audio.parent().unwrap_or(Path::new("."));
        let data = self
            .ask(
                "transcribe",
                folder,
                &args,
                self.limit_s + PER_AUDIO_S * audio_s.max(0.0),
            )
            .await?;
        if data.get("error").is_some_and(ferb_py::value::truthy) {
            return Err(unavailable(&why(&data)));
        }
        let num = |v: Option<&Value>| match v {
            Some(v) if ferb_py::value::truthy(v) => v.as_f64().unwrap_or(0.0),
            _ => 0.0,
        };
        let mut words = Vec::new();
        if let Some(Value::Array(segments)) = data.get("segments") {
            for s in segments {
                let start = num(s.get("start"));
                words.push(Word {
                    text: crate::observation::text_or_empty(s.get("text")),
                    start,
                    end: start + num(s.get("duration")),
                });
            }
        }
        let locale = match data.get("locale") {
            Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
            _ => language.unwrap_or("").to_string(),
        };
        Ok(Transcript {
            text: crate::observation::text_or_empty(data.get("text")),
            words,
            locale,
        })
    }

    /// transcribe.py:178: FERB.app's Speech Recognition state (speech, on_device, locale). No prompt.
    pub async fn status(&self, folder: &Path) -> Result<Map<String, Value>, Hearing> {
        let data = self.ask("status", folder, &[], self.limit_s).await?;
        Ok(["speech", "on_device", "locale"]
            .iter()
            .filter_map(|k| data.get(*k).map(|v| (k.to_string(), v.clone())))
            .collect())
    }
}

/// transcribe.py:184 `_why`.
pub fn why(data: &Map<String, Value>) -> String {
    let t = |k: &str| crate::observation::text_or_empty(data.get(k));
    let reason = t("reason");
    let locale = match t("locale") {
        l if l.is_empty() => "this language".to_string(),
        l => l,
    };
    match reason.as_str() {
        "on_device_unavailable" => format!(
            "On-device speech recognition isn't available for {locale} on this Mac, so FERB didn't transcribe it; it never sends sound to a cloud recognizer."
        ),
        "not_authorized" => {
            let speech = match t("speech") {
                s if s.is_empty() => "not allowed".to_string(),
                s => s,
            };
            format!(
                "FERB isn't allowed to use Speech Recognition ({speech}): the owner allows it in FERB's Settings → Permissions."
            )
        }
        "unsupported_language" => format!("Apple speech recognition has no recognizer for {locale}."),
        _ => format!(
            "FERB.app couldn't transcribe the sound: {}.",
            crate::observation::py_str(data.get("error"))
        ),
    }
}

/// transcribe.py:197 `redacted`: the transcript through the redactor, and its sentences with times (seconds, plus
/// `offset`) as segments. Each segment's text is a slice of the already-redacted transcript.
pub fn redacted(t: &Transcript, redact: &dyn Fn(&str) -> String, offset: f64) -> Value {
    let raw: Vec<char> = t.text.chars().collect();
    let red_text = redact(&t.text);
    let red: Vec<char> = red_text.chars().collect();
    let ops = ferb_fs::difflib::SequenceMatcher::new(&raw, &red, false).get_opcodes();
    let at = |i: usize, end: bool| -> usize {
        for &(tag, i1, i2, j1, j2) in &ops {
            let inside = if end {
                i1 < i && i <= i2
            } else {
                i1 <= i && i < i2
            };
            if inside {
                return if tag == "equal" {
                    j1 + (i - i1)
                } else if end {
                    j2
                } else {
                    j1
                };
            }
        }
        if i >= raw.len() {
            red.len()
        } else {
            0
        }
    };
    let find = |needle: &[char], from: usize| -> Option<usize> {
        if needle.len() > raw.len() {
            return None;
        }
        (from..=raw.len() - needle.len()).find(|&k| raw[k..k + needle.len()] == *needle)
    };
    let mut placed: Vec<(&Word, usize, usize)> = Vec::new();
    let mut cursor = 0;
    for w in &t.words {
        let needle: Vec<char> = w.text.chars().collect();
        if needle.is_empty() {
            continue;
        }
        let Some(found) = find(&needle, cursor) else {
            continue;
        };
        placed.push((w, found, found + needle.len()));
        cursor = found + needle.len();
    }
    let slice = |a: usize, b: usize| -> String {
        if a >= b {
            String::new()
        } else {
            raw[a.min(raw.len())..b.min(raw.len())].iter().collect()
        }
    };
    let mut segments = Vec::new();
    let mut group: Vec<(&Word, usize, usize)> = Vec::new();
    for (n, item) in placed.iter().enumerate() {
        group.push(*item);
        let next = placed.get(n + 1).map_or(raw.len(), |p| p.1);
        let last_one = n + 1 == placed.len();
        if last_one
            || slice(item.2 - 1, next)
                .chars()
                .any(|c| SENTENCE_END.contains(&c))
        {
            let (first, last) = (group[0], *group.last().expect("a word"));
            let gap = slice(last.2, next);
            let stop = last.2
                + gap
                    .chars()
                    .position(|c| SENTENCE_END.contains(&c))
                    .map_or(0, |k| k + 1);
            let (a, b) = (at(first.1, false), at(stop, true));
            let cut: String = if a < b {
                red[a..b].iter().collect()
            } else {
                String::new()
            };
            let text = redact(ferb_py::value::strip(&cut));
            if !text.is_empty() {
                let mut s = Map::new();
                s.insert(
                    "start".into(),
                    Value::from(ferb_memory::pymath::round(first.0.start + offset, 2)),
                );
                s.insert(
                    "end".into(),
                    Value::from(ferb_memory::pymath::round(last.0.end + offset, 2)),
                );
                s.insert("text".into(), Value::from(text));
                segments.push(Value::Object(s));
            }
            group.clear();
        }
    }
    let mut out = Map::new();
    out.insert("transcript".into(), Value::from(red_text));
    out.insert("segments".into(), Value::Array(segments));
    Value::Object(out)
}
