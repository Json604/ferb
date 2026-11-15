//! Task-owned sampled window videos and decoded-frame inspection (ferb/act/recording.py). Never captures a display
//! or moves focus. Each frame comes masked from its driver (L13); a frame that can't be is withheld and skipped, and
//! the recording goes on. Frames are staged in FERB's own folder; the output folder is checked as opened and kept open
//! for the recording's life, and the receipt and video are written only through it (rule 10). ffmpeg and ffprobe are
//! found on the PATH or in Homebrew's folders, never bundled. With sound, Hearing captures the app's audio and the
//! stopped recording carries the redacted on-device transcript; the sound file is deleted once transcribed.

use std::collections::HashMap;
use std::ffi::CString;
use std::os::fd::RawFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ferb_core::types::{ToolContext, ToolResult};
use serde_json::{json, Map, Value};

use crate::error::{HandError, Result};
use crate::listen::{Hearing, Listening};
use crate::point::FrameFn;
use crate::tools::MacTools;

pub const D_STOP: &str = "Stop and finalize a task-owned target video. Reports capture gaps/errors; inspect the saved pixels before claiming the requested process was recorded.";
pub const D_INSPECT: &str = "Decode a saved video into a labelled contact sheet (default: beginning/middle/end). Supply times_s around important actions. Check actual target/content visually, not just file metadata. Does not change any app/window.";

/// recording.py:277: ffmpeg / ffprobe on the PATH, else Homebrew's or ~/.local/bin (launchd's PATH omits them).
pub fn executable(name: &str, path_var: Option<&str>, home: &str) -> Result<String> {
    let exec = |p: &Path| {
        use std::os::unix::fs::PermissionsExt;
        p.is_file()
            && std::fs::metadata(p)
                .map(|m| m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
    };
    for dir in path_var.unwrap_or("").split(':').filter(|d| !d.is_empty()) {
        let p = Path::new(dir).join(name);
        if exec(&p) {
            return Ok(p.to_string_lossy().into_owned());
        }
    }
    for dir in [
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
        format!("{home}/.local/bin"),
    ] {
        let p = Path::new(&dir).join(name);
        if exec(&p) {
            return Ok(p.to_string_lossy().into_owned());
        }
    }
    Err(HandError::Refused(format!(
        "{name} is not installed/on PATH; target recording needs ffmpeg and ffprobe"
    )))
}

fn find(name: &str) -> Result<String> {
    executable(
        name,
        std::env::var("PATH").ok().as_deref(),
        &std::env::var("HOME").unwrap_or_default(),
    )
}

/// recording.py:287: a child process's stdout; a failure is refused with its stderr. Dropping the future kills it.
pub async fn command(args: &[String]) -> Result<Vec<u8>> {
    let out = tokio::process::Command::new(&args[0])
        .args(&args[1..])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| HandError::Other(e.to_string()))?;
    if !out.status.success() {
        let name = Path::new(&args[0])
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        return Err(HandError::Refused(format!(
            "{name} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(out.stdout)
}

/// The timestamped concat list ffmpeg reads (recording.py:574-580).
pub fn timeline(times: &[f64], end: f64) -> String {
    let mut lines = vec!["ffconcat version 1.0".to_string()];
    for (i, at) in times.iter().enumerate() {
        let following = times.get(i + 1).copied().unwrap_or(end);
        lines.push(format!("file '{i:08}.jpg'"));
        lines.push(format!("duration {:.6}", (following - at).max(0.04)));
    }
    lines.push(format!("file '{:08}.jpg'", times.len().saturating_sub(1)));
    lines.join("\n") + "\n"
}

/// The encode's arguments (recording.py:582-587).
pub fn encode_args(ffmpeg: &str, frames_txt: &str, end: f64, video: &str) -> Vec<String> {
    let end_s = ferb_py::json::python_float(end);
    vec![
        ffmpeg.into(), "-v".into(), "error".into(), "-nostdin".into(), "-f".into(), "concat".into(), "-safe".into(), "1".into(), "-i".into(), frames_txt.into(), "-vf".into(),
        format!("scale=1280:720:force_original_aspect_ratio=decrease,pad=1280:720:(ow-iw)/2:(oh-ih)/2,fps=25,tpad=stop_mode=clone:stop_duration={end_s}"),
        "-c:v".into(), "libx264".into(), "-preset".into(), "veryfast".into(), "-pix_fmt".into(), "yuv420p".into(), "-fps_mode".into(), "cfr".into(), "-r".into(), "25".into(),
        "-t".into(), end_s, "-movflags".into(), "+faststart".into(), video.into(),
    ]
}

/// The protection lists and FERB's file rules (ferb/act/fs/files.py `resolve`, `destination_closed`,
/// `make_folders`, `write_check`): the files part's, through this seam.
pub trait FileGate: Send + Sync {
    /// The path the call names, absolute (relative to the workspace), resolved.
    fn resolve(&self, path: &str, ctx: &ToolContext, must_exist: bool) -> Result<PathBuf>;
    /// The protection lists' refusal for writing `path`, else None.
    fn closed(&self, path: &Path) -> Option<String>;
    /// Makes the folders, each inside one opened and checked; refused like `closed`.
    fn make_folders(&self, dir: &Path) -> Result<()>;
}

/// Where an open folder is now (F_GETPATH).
pub fn opened_path(fd: RawFd) -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let mut buf = vec![0u8; libc::PATH_MAX as usize];
        // SAFETY: F_GETPATH writes at most PATH_MAX bytes, NUL-terminated, into `buf`, which holds PATH_MAX bytes.
        let rc = unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) };
        if rc < 0 {
            return Err(HandError::Other(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
        Ok(PathBuf::from(
            String::from_utf8_lossy(&buf[..end]).into_owned(),
        ))
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::fs::read_link(format!("/proc/self/fd/{fd}"))
            .map_err(|e| HandError::Other(e.to_string()))
    }
}

fn cname(name: &str) -> Result<CString> {
    CString::new(name).map_err(|_| HandError::Refused("a name with a NUL byte".into()))
}

fn io_err(e: std::io::Error) -> HandError {
    HandError::Other(format!("OSError: {e}"))
}

const NEW: libc::c_int =
    libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;

fn open_at(
    dir: RawFd,
    name: &str,
    flags: libc::c_int,
    mode: libc::c_uint,
) -> std::io::Result<RawFd> {
    let c =
        CString::new(name).map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    // SAFETY: `c` is a valid NUL-terminated string for the call's duration; `dir` is an open folder descriptor.
    let fd = unsafe { libc::openat(dir, c.as_ptr(), flags, mode) };
    if fd < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(fd)
    }
}

fn unlink_at(dir: RawFd, name: &str) {
    if let Ok(c) = cname(name) {
        // SAFETY: a valid C string and an open folder descriptor; failure is ignored (best effort).
        unsafe { libc::unlinkat(dir, c.as_ptr(), 0) };
    }
}

fn write_fd(fd: RawFd, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::fd::FromRawFd;
    // SAFETY: `fd` was just opened by us and is owned by this File from here on (closed when it drops).
    let mut f = unsafe { std::fs::File::from_raw_fd(fd) };
    f.write_all(data)
}

/// recording.py:336: the video into the checked folder: a hard link, or an exclusive copy across volumes.
pub fn publish(video: &Path, out: RawFd, name: &str) -> std::io::Result<()> {
    let src = CString::new(video.as_os_str().as_encoded_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let dst =
        CString::new(name).map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    // SAFETY: both strings are valid C strings; AT_FDCWD resolves `src` normally; `out` is an open folder.
    let rc = unsafe { libc::linkat(libc::AT_FDCWD, src.as_ptr(), out, dst.as_ptr(), 0) };
    if rc == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() != Some(libc::EXDEV) {
        return Err(err);
    }
    let fd = open_at(out, name, NEW, 0o644)?;
    let copied = std::fs::read(video).and_then(|data| write_fd(fd, &data));
    if copied.is_err() {
        unlink_at(out, name);
    }
    copied
}

/// recording.py:354: the finished receipt, written new and renamed over `name` (never written through a link).
pub fn replace_receipt(out: RawFd, name: &str, text: &str) -> std::io::Result<()> {
    let mut r = [0u8; 4];
    let _ = getrandom::fill(&mut r);
    let temp = format!(".{name}.{}.ferb-tmp", hex::encode(r));
    let fd = open_at(out, &temp, NEW, 0o644)?;
    let done = write_fd(fd, text.as_bytes()).and_then(|_| {
        let (a, b) = (
            CString::new(temp.clone()).expect("no NUL"),
            CString::new(name)
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?,
        );
        // SAFETY: valid C strings, both relative to the open folder `out`.
        let rc = unsafe { libc::renameat(out, a.as_ptr(), out, b.as_ptr()) };
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    });
    if done.is_err() {
        unlink_at(out, &temp);
    }
    done
}

struct Rec {
    id: String,
    target: Map<String, Value>,
    path: PathBuf,
    folder: PathBuf,
    capture: FrameFn,
    fps: f64,
    started: f64,
    at: String,
    times: Vec<f64>,
    error: Option<String>,
    withheld: usize,
    withheld_reason: Option<String>,
    masking: std::collections::BTreeSet<String>,
    ended: Option<f64>,
    halted: bool,
    result: Option<Map<String, Value>>,
    listening: Option<Arc<Listening>>,
    audio_at: f64,
    language: Option<String>,
    sources: Vec<(String, String)>,
    out: RawFd,
}

impl Rec {
    fn receipt(&self) -> PathBuf {
        let mut p = self.path.clone().into_os_string();
        p.push(".json");
        PathBuf::from(p)
    }
}

/// recording.py:371.
pub struct RecordingManager {
    pub sanitize: Arc<dyn Fn(&[u8]) -> Result<Vec<u8>> + Send + Sync>,
    pub now: Arc<dyn Fn() -> f64 + Send + Sync>,
    pub wall: Arc<dyn Fn() -> String + Send + Sync>,
    pub hearing: Option<Arc<Hearing>>,
    pub staging: Mutex<Option<PathBuf>>,
    pub files: Arc<dyn FileGate>,
    records: Mutex<HashMap<String, Arc<tokio::sync::Mutex<Rec>>>>,
    owners: Mutex<HashMap<String, String>>,
    starts: tokio::sync::Mutex<()>,
    loops: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
}

fn fmt_info(r: &Rec, now: f64) -> Map<String, Value> {
    let (mut path, mut receipt) = (r.path.clone(), r.receipt());
    if r.out >= 0 {
        if let Ok(where_) = opened_path(r.out) {
            path = where_.join(r.path.file_name().unwrap_or_default());
            receipt = where_.join(r.receipt().file_name().unwrap_or_default());
        }
    }
    json!({"id": r.id, "resource": "recording", "target": r.target, "path": path.to_string_lossy(), "receipt": receipt.to_string_lossy(),
           "started_at": r.at, "requested_fps": r.fps, "frames": r.times.len(), "first_frame_s": r.times.first(),
           "last_frame_s": r.times.last(), "duration_s": (r.ended.unwrap_or(now) - r.started).max(0.0),
           "capture_complete": false, "content_reviewed": false, "error": r.error, "frames_withheld": r.withheld,
           "withheld_reason": r.withheld_reason, "masking": r.masking.iter().collect::<Vec<_>>(),
           "running": !r.halted && r.result.is_none()})
    .as_object()
    .cloned()
    .unwrap_or_default()
}

fn rand_hex(n: usize) -> String {
    let mut b = vec![0u8; n.div_ceil(2)];
    let _ = getrandom::fill(&mut b);
    hex::encode(b)[..n].to_string()
}

impl RecordingManager {
    pub fn new(
        sanitize: Arc<dyn Fn(&[u8]) -> Result<Vec<u8>> + Send + Sync>,
        files: Arc<dyn FileGate>,
        hearing: Option<Arc<Hearing>>,
        staging: Option<PathBuf>,
    ) -> Arc<Self> {
        let start = std::time::Instant::now();
        Arc::new(RecordingManager {
            sanitize,
            now: Arc::new(move || start.elapsed().as_secs_f64()),
            wall: Arc::new(|| {
                chrono::Utc::now()
                    .format("%Y-%m-%dT%H:%M:%S%.6f+00:00")
                    .to_string()
            }),
            hearing,
            staging: Mutex::new(staging),
            files,
            records: Mutex::new(HashMap::new()),
            owners: Mutex::new(HashMap::new()),
            starts: tokio::sync::Mutex::new(()),
            loops: Mutex::new(HashMap::new()),
        })
    }

    fn staging_dir(&self) -> Result<PathBuf> {
        let mut s = self.staging.lock().expect("lock");
        let dir = match s.clone() {
            Some(d) => d,
            None => {
                let d = std::env::temp_dir().join(format!("ferb-recordings-{}", rand_hex(8)));
                *s = Some(d.clone());
                d
            }
        };
        std::fs::create_dir_all(&dir).map_err(io_err)?;
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        Ok(dir)
    }

    /// At daemon start: the `rec-*` staging folders the last run left (never through a link).
    pub fn sweep(&self) -> std::io::Result<()> {
        let Some(dir) = self.staging.lock().expect("lock").clone() else {
            return Ok(());
        };
        if !self.records.lock().expect("lock").is_empty() {
            return Ok(());
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            return Ok(());
        };
        let mut failed = None;
        for e in rd.flatten() {
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if e.file_name().to_string_lossy().starts_with("rec-") && is_dir {
                if let Err(err) = std::fs::remove_dir_all(e.path()) {
                    failed.get_or_insert(err);
                }
            }
        }
        failed.map_or(Ok(()), Err)
    }

    /// recording.py:412 `shutdown`: every owner's recordings end as at its task's end.
    pub async fn shutdown(self: &Arc<Self>) {
        let owners: std::collections::BTreeSet<String> = self.owners().into_iter().collect();
        for owner in owners {
            self.end_task(&owner).await;
        }
    }

    /// The owners of the recordings running now (desktop.py `_task_paused`: a paused task's recordings stop).
    pub fn owners(&self) -> Vec<String> {
        self.owners
            .lock()
            .expect("lock")
            .values()
            .cloned()
            .collect()
    }

    fn get(&self, rid: &str, owner: &str) -> Result<Arc<tokio::sync::Mutex<Rec>>> {
        let rec = self
            .records
            .lock()
            .expect("lock")
            .get(rid)
            .cloned()
            .ok_or_else(|| HandError::Refused("unknown recording id".into()))?;
        if self
            .owners
            .lock()
            .expect("lock")
            .get(rid)
            .map(String::as_str)
            != Some(owner)
        {
            return Err(HandError::Refused(
                "recording belongs to another task".into(),
            ));
        }
        Ok(rec)
    }

    /// The path a recording publishes to (recording.stop's `file_paths`).
    pub fn path_of(&self, rid: &str) -> Option<PathBuf> {
        let rec = self.records.lock().expect("lock").get(rid).cloned()?;
        rec.try_lock().ok().map(|r| r.path.clone())
    }

    /// recording.py:435.
    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        self: &Arc<Self>,
        owner: &str,
        target: Map<String, Value>,
        path: PathBuf,
        capture: FrameFn,
        fps: f64,
        audio: Option<(String, i32)>,
        language: Option<String>,
        sources: Vec<(String, String)>,
    ) -> Result<Map<String, Value>> {
        let _starting = self.starts.lock().await;
        find("ffmpeg")?;
        find("ffprobe")?;
        if !fps.is_finite() || !(0.2..=10.0).contains(&fps) {
            return Err(HandError::Refused(
                "sample rate must be between 0.2 and 10 frames/second".into(),
            ));
        }
        let path = std::path::absolute(&path).map_err(io_err)?;
        if path
            .extension()
            .map(|e| ferb_py::text::lower(&e.to_string_lossy()))
            != Some("mp4".into())
        {
            return Err(HandError::Refused(
                "recording output must be an .mp4 path".into(),
            ));
        }
        let receipt_path = PathBuf::from(format!("{}.json", path.display()));
        let busy = {
            let recs: Vec<Arc<tokio::sync::Mutex<Rec>>> = self
                .records
                .lock()
                .expect("lock")
                .values()
                .cloned()
                .collect();
            let mut same_path = false;
            let mut same_target = false;
            for r in recs {
                let r = r.lock().await;
                same_path |= r.path == path && r.result.is_none();
                same_target |= r.target == target && r.result.is_none();
            }
            (same_path, same_target)
        };
        if path.exists() || receipt_path.exists() || busy.0 {
            return Err(HandError::Refused(
                "recording path or receipt already exists; choose a new path".into(),
            ));
        }
        if busy.1 {
            return Err(HandError::Refused(
                "this target already has an active recording".into(),
            ));
        }
        if let Some(r) = self.files.closed(&path) {
            return Err(HandError::Refused(r));
        }
        let (raw, note) = capture().await?;
        let san = self.sanitize.clone();
        let first = crate::input::off(move || san(&raw)).await?;
        let began = (self.now)();
        if audio.is_some() && self.hearing.is_none() {
            return Err(HandError::Refused(
                "FERB has no way to hear sound here; record without with_audio".into(),
            ));
        }
        let parent = path.parent().unwrap_or(Path::new("/")).to_path_buf();
        self.files.make_folders(&parent)?;
        let cparent = CString::new(parent.as_os_str().as_encoded_bytes())
            .map_err(|_| HandError::Refused("a path with a NUL byte".into()))?;
        // SAFETY: a valid C string; the descriptor is owned by the recording and closed when it is finished.
        let out = unsafe {
            libc::open(
                cparent.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if out < 0 {
            return Err(io_err(std::io::Error::last_os_error()));
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let receipt_name = format!("{name}.json");
        let mut folder: Option<PathBuf> = None;
        let mut receipt_made = false;
        let mut listening: Option<Arc<Listening>> = None;
        let made: Result<Rec> = async {
            let where_ = opened_path(out)?;
            if let Some(r) = self
                .files
                .closed(&where_.join(&name))
                .or_else(|| self.files.closed(&where_.join(&receipt_name)))
            {
                return Err(HandError::Refused(r));
            }
            let cn = cname(&name)?;
            let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
            // SAFETY: valid C string, open folder descriptor, and a buffer the size of `stat`.
            if unsafe {
                libc::fstatat(out, cn.as_ptr(), st.as_mut_ptr(), libc::AT_SYMLINK_NOFOLLOW)
            } == 0
            {
                return Err(HandError::Refused(
                    "recording path or receipt already exists; choose a new path".into(),
                ));
            }
            let rfd = match open_at(out, &receipt_name, NEW, 0o644) {
                Ok(fd) => fd,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Err(HandError::Refused(
                        "recording path or receipt already exists; choose a new path".into(),
                    ))
                }
                Err(e) => return Err(io_err(e)),
            };
            receipt_made = true;
            let f = self.staging_dir()?.join(format!("rec-{}", rand_hex(8)));
            std::fs::create_dir(&f).map_err(io_err)?;
            folder = Some(f.clone());
            if let (Some((app, pid)), Some(h)) = (&audio, &self.hearing) {
                listening = Some(h.start(owner, app, *pid).await?);
            }
            let audio_at = match (&listening, &self.hearing) {
                (Some(l), Some(h)) => ((self.now)() - began - h.lag(l)).max(0.0),
                _ => 0.0,
            };
            let mut srcs = sources.clone();
            if let Some((app, _)) = &audio {
                srcs.push(("audio".into(), app.clone()));
            } else {
                srcs.clear();
            }
            let mut rec = Rec {
                id: format!("rec-{}", rand_hex(12)),
                target: target.clone(),
                path: path.clone(),
                folder: f.clone(),
                capture: capture.clone(),
                fps,
                started: began,
                at: (self.wall)(),
                times: vec![],
                error: None,
                withheld: 0,
                withheld_reason: None,
                masking: Default::default(),
                ended: None,
                halted: false,
                result: None,
                listening: listening.clone(),
                audio_at,
                language: language.clone(),
                sources: srcs,
                out,
            };
            if !note.is_empty() {
                rec.masking.insert(note.clone());
            }
            std::fs::write(f.join("00000000.jpg"), &first).map_err(io_err)?;
            rec.times.push(0.0);
            let mut info = fmt_info(&rec, (self.now)());
            info.insert("sample_times_s".into(), json!(rec.times));
            write_fd(
                rfd,
                ferb_py::json::dumps_with(
                    &Value::Object(info),
                    ferb_py::json::Style {
                        ascii: true,
                        sort_keys: false,
                        indent: Some(2),
                    },
                )
                .as_bytes(),
            )
            .map_err(io_err)?;
            Ok(rec)
        }
        .await;
        let rec = match made {
            Ok(r) => r,
            Err(e) => {
                if let Some(f) = folder {
                    let _ = std::fs::remove_dir_all(f);
                }
                if receipt_made {
                    unlink_at(out, &receipt_name);
                }
                // SAFETY: `out` is the descriptor opened above and not yet handed to a recording.
                unsafe { libc::close(out) };
                if let (Some(l), Some(h)) = (listening, &self.hearing) {
                    h.discard(&l).await;
                }
                return Err(e);
            }
        };
        let id = rec.id.clone();
        let mut info = fmt_info(&rec, (self.now)());
        self.owners
            .lock()
            .expect("lock")
            .insert(id.clone(), owner.into());
        self.records
            .lock()
            .expect("lock")
            .insert(id.clone(), Arc::new(tokio::sync::Mutex::new(rec)));
        let me = self.clone();
        let (rid, own) = (id.clone(), owner.to_string());
        let job = tokio::spawn(async move { me.run_loop(&rid, &own, fps).await });
        self.loops.lock().expect("lock").insert(id, job);
        info.insert("next".into(), json!("Stop with recording_stop, then inspect decoded video frames before claiming coverage."));
        Ok(info)
    }

    /// recording.py:520: one frame (masked, or withheld and counted).
    pub async fn sample(&self, rid: &str, owner: &str) -> Result<()> {
        let rec = self.get(rid, owner)?;
        let mut r = rec.lock().await;
        if r.halted {
            return Ok(());
        }
        let got = (r.capture)().await;
        let res = match got {
            Ok((raw, note)) => {
                let san = self.sanitize.clone();
                crate::input::off(move || san(&raw))
                    .await
                    .map(|img| (img, note))
            }
            Err(e) => Err(e),
        };
        match res {
            Ok((image, note)) => {
                if r.halted {
                    return Ok(());
                }
                let at = (self.now)() - r.started;
                let file = r.folder.join(format!("{:08}.jpg", r.times.len()));
                std::fs::write(file, image).map_err(io_err)?;
                r.times.push(at);
                if !note.is_empty() {
                    r.masking.insert(note);
                }
            }
            Err(HandError::Withheld(why)) => {
                r.withheld += 1;
                r.withheld_reason = Some(why);
            }
            Err(e) => {
                r.error = Some(format!("{}: {e}", e.class()));
                r.ended = Some((self.now)());
                r.halted = true;
            }
        }
        Ok(())
    }

    async fn run_loop(self: &Arc<Self>, rid: &str, owner: &str, fps: f64) {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs_f64(1.0 / fps)).await;
            let Ok(rec) = self.get(rid, owner) else {
                return;
            };
            if rec.lock().await.halted {
                return;
            }
            let _ = self.sample(rid, owner).await;
        }
    }

    /// recording.py:549: however it ends, the recording ends once; a later stop returns the same result.
    pub async fn stop(
        self: &Arc<Self>,
        rid: &str,
        owner: &str,
        reason: Option<&str>,
        transcribe: bool,
    ) -> Result<Map<String, Value>> {
        let rec = self.get(rid, owner)?;
        let job = self.loops.lock().expect("lock").remove(rid);
        if let Some(job) = job {
            job.abort();
            let _ = job.await;
        }
        let mut r = rec.lock().await;
        r.halted = true;
        if let Some(res) = &r.result {
            return Ok(res.clone());
        }
        let now = (self.now)();
        r.ended = Some(r.ended.unwrap_or(now));
        if r.error.is_none() {
            r.error = reason.map(String::from);
        }
        let mut result = fmt_info(&r, now);
        result.insert("running".into(), json!(false));
        result.insert("released".into(), json!(true));
        let heard = match (&r.listening, &self.hearing) {
            (Some(l), Some(h)) => Some(h.finish(l).await),
            _ => None,
        };
        let encode: Result<u64> = async {
            let ffmpeg = find("ffmpeg")?;
            let end = (r.ended.unwrap_or(now) - r.started)
                .max(r.times.last().copied().unwrap_or(0.0) + 0.04);
            std::fs::write(r.folder.join("frames.txt"), timeline(&r.times, end)).map_err(io_err)?;
            let video = r.folder.join("video.mp4");
            command(&encode_args(
                &ffmpeg,
                &r.folder.join("frames.txt").to_string_lossy(),
                end,
                &video.to_string_lossy(),
            ))
            .await?;
            let name = r
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            publish(&video, r.out, &name).map_err(io_err)?;
            let cn = cname(&name)?;
            let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
            // SAFETY: valid C string, open folder, a `stat`-sized buffer; read only when the call succeeded.
            let rc = unsafe {
                libc::fstatat(
                    r.out,
                    cn.as_ptr(),
                    st.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            if rc != 0 {
                return Err(io_err(std::io::Error::last_os_error()));
            }
            // SAFETY: fstatat succeeded, so the buffer is initialized.
            Ok(unsafe { st.assume_init() }.st_size as u64)
        }
        .await;
        match encode {
            Ok(bytes) => {
                let heard_err = heard
                    .as_ref()
                    .is_some_and(|h| h.get("error").is_some_and(ferb_py::value::truthy));
                result.insert(
                    "capture_complete".into(),
                    json!(r.error.is_none() && !heard_err),
                );
                result.insert("bytes".into(), json!(bytes));
            }
            Err(e) => {
                let prefix = r
                    .error
                    .as_ref()
                    .map(|x| format!("{x}; "))
                    .unwrap_or_default();
                result.insert("error".into(), json!(format!("{prefix}{}: {e}", e.class())));
                result.insert("capture_complete".into(), json!(false));
            }
        }
        let _ = std::fs::remove_dir_all(&r.folder);
        if let Some(heard) = heard {
            if transcribe {
                result.extend(self.sound(&r, &heard).await);
            } else {
                result.insert("audio_track".into(), json!(false));
                result.insert("audio_note".into(), json!("not transcribed: the recording ended with its task, and its sound was deleted"));
            }
        }
        r.result = Some(result.clone());
        let mut final_ = result.clone();
        final_.insert("sample_times_s".into(), json!(r.times));
        let receipt_name = r
            .receipt()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Err(e) = replace_receipt(
            r.out,
            &receipt_name,
            &ferb_py::json::dumps_with(
                &Value::Object(final_),
                ferb_py::json::Style {
                    ascii: true,
                    sort_keys: false,
                    indent: Some(2),
                },
            ),
        ) {
            result.insert(
                "error".into(),
                json!(format!("recording receipt could not be finalized: {e}")),
            );
            result.insert("capture_complete".into(), json!(false));
        }
        // SAFETY: the recording's own folder descriptor, closed once (`out` is set to -1 right after).
        unsafe { libc::close(r.out) };
        r.out = -1;
        if let Some(l) = &r.listening {
            let _ = std::fs::remove_file(&l.path);
        }
        r.result = Some(result.clone());
        Ok(result)
    }

    async fn sound(&self, r: &Rec, heard: &Map<String, Value>) -> Map<String, Value> {
        let audio_s = heard.get("audio_s").and_then(Value::as_f64).unwrap_or(0.0);
        let mut out = json!({"audio_track": false, "audio_s": crate::wav::py_round(audio_s, 2), "audio_peak": heard.get("peak").cloned().unwrap_or(json!(0.0)),
                             "audio_offset_s": crate::wav::py_round(r.audio_at, 3)})
        .as_object()
        .cloned()
        .unwrap_or_default();
        let Some(h) = &self.hearing else { return out };
        if let Some(e) = heard.get("error").filter(|e| ferb_py::value::truthy(e)) {
            out.insert(
                "audio_error".into(),
                json!((h.redact)(&ferb_py::value::py_str(e))),
            );
        }
        if let Some(l) = &r.listening {
            match h
                .transcript(&l.path, heard, r.language.as_deref(), r.audio_at)
                .await
            {
                Ok(t) => out.extend(t),
                Err(HandError::Refused(m)) => {
                    out.insert("transcript_error".into(), json!(m));
                }
                Err(e) => {
                    out.insert("transcript_error".into(), json!(e.to_string()));
                }
            }
        }
        out
    }

    pub async fn end_task(self: &Arc<Self>, owner: &str) -> Vec<Map<String, Value>> {
        let owned: Vec<String> = self
            .owners
            .lock()
            .expect("lock")
            .iter()
            .filter(|(_, o)| *o == owner)
            .map(|(k, _)| k.clone())
            .collect();
        let mut out = vec![];
        for rid in &owned {
            let rec = self.records.lock().expect("lock").get(rid).cloned();
            let done = match rec {
                Some(r) => r.lock().await.result.is_some(),
                None => true,
            };
            if !done {
                if let Ok(r) = self
                    .stop(
                        rid,
                        owner,
                        Some("task ended before recording was explicitly stopped"),
                        false,
                    )
                    .await
                {
                    out.push(r);
                }
            }
        }
        for rid in owned {
            self.records.lock().expect("lock").remove(&rid);
            self.owners.lock().expect("lock").remove(&rid);
        }
        out
    }

    /// The sources a stopped recording's transcript carries (recording.stop's read sources).
    pub async fn sources_of(&self, rid: &str) -> Vec<(String, String)> {
        let Some(rec) = self.records.lock().expect("lock").get(rid).cloned() else {
            return vec![];
        };
        let r = rec.lock().await;
        if r.listening.is_some() {
            r.sources.clone()
        } else {
            vec![]
        }
    }

    /// tools.py:646 (mac.record).
    pub async fn mac_record(
        self: &Arc<Self>,
        tools: &MacTools,
        args: &Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult> {
        let app = args
            .get("app")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let shot = args
            .get("shot")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let d = tools.driver.clone();
        let paint = tools.paint.clone();
        let (target, capture) =
            crate::input::off(move || d.recording_source(&app, &shot, paint)).await?;
        let with_audio = args
            .get("with_audio")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let audio = if with_audio {
            if !ctx.scopes.contains("audio") || self.hearing.is_none() {
                return Err(HandError::Refused("recording with sound needs the audio scope (hearing); record without with_audio".into()));
            }
            Some((
                ferb_py::value::py_str(&target["app"]),
                target["pid"].as_i64().unwrap_or(0) as i32,
            ))
        } else {
            None
        };
        let path = self.files.resolve(
            args.get("path").and_then(Value::as_str).unwrap_or(""),
            ctx,
            false,
        )?;
        let fps = args.get("fps").and_then(Value::as_f64).unwrap_or(2.0);
        let language = args
            .get("language")
            .and_then(Value::as_str)
            .map(String::from);
        let bundle = ferb_py::value::py_str(&target["app"]);
        let result = self
            .start(
                &ctx.task_id,
                target,
                path,
                capture,
                fps,
                audio,
                language,
                vec![],
            )
            .await?;
        Ok(ToolResult {
            content: result,
            read_sources: crate::tools::sources(&tools.driver, &bundle),
            ..Default::default()
        })
    }
}

/// recording.py:745: the recording a video came from (its receipt's id), else the video.
pub fn recording_id(video: &Path) -> String {
    let receipt = PathBuf::from(format!("{}.json", video.display()));
    let id = std::fs::read_to_string(receipt)
        .ok()
        .and_then(|t| ferb_py::json::loads(&t).ok())
        .and_then(|v| v.get("id").cloned());
    match id {
        Some(v) if ferb_py::value::truthy(&v) => ferb_py::value::py_str(&v),
        _ => video.to_string_lossy().into_owned(),
    }
}

/// The contact sheet of decoded frames with their times (`native::paint`'s drawing; recording.py:677).
pub trait ReviewSheet: Send + Sync {
    fn sheet(&self, frames: &[Vec<u8>], times: &[f64]) -> Result<(Vec<u8>, i64, i64)>;
}

/// recording.py:649: real encoded pixels for a model or a human; never a verdict.
pub async fn inspect_video(
    path: &Path,
    times: Option<&[f64]>,
    sheet: &dyn ReviewSheet,
) -> Result<(Map<String, Value>, Vec<u8>)> {
    if !path.is_file() {
        return Err(HandError::Refused("video file does not exist".into()));
    }
    let probe = find("ffprobe")?;
    let p = path.to_string_lossy().into_owned();
    let raw = command(&[
        probe,
        "-v".into(),
        "error".into(),
        "-protocol_whitelist".into(),
        "file,pipe".into(),
        "-show_entries".into(),
        "format=duration,size".into(),
        "-of".into(),
        "json".into(),
        p.clone(),
    ])
    .await?;
    let meta = ferb_py::json::loads(&String::from_utf8_lossy(&raw)).map_err(HandError::Other)?;
    let format = &meta["format"];
    let duration: f64 = ferb_py::value::py_str(&format["duration"])
        .parse()
        .map_err(|_| HandError::Other("bad duration".into()))?;
    if !duration.is_finite() || duration <= 0.0 {
        return Err(HandError::Refused(
            "video has no finite positive duration".into(),
        ));
    }
    let samples: Vec<f64> = match times {
        Some(t) => t.to_vec(),
        None => vec![0.0, duration / 2.0, (duration - 0.15).max(0.0)],
    };
    if !(1..=6).contains(&samples.len())
        || samples
            .iter()
            .any(|t| !t.is_finite() || *t < 0.0 || *t >= duration)
    {
        return Err(HandError::Refused(
            "request 1–6 frame times inside the video duration".into(),
        ));
    }
    let ffmpeg = find("ffmpeg")?;
    let mut frames = vec![];
    for t in &samples {
        let frame = command(&[
            ffmpeg.clone(),
            "-v".into(),
            "error".into(),
            "-nostdin".into(),
            "-protocol_whitelist".into(),
            "file,pipe".into(),
            "-ss".into(),
            ferb_py::json::python_float(*t),
            "-i".into(),
            p.clone(),
            "-frames:v".into(),
            "1".into(),
            "-vf".into(),
            "scale=640:360:force_original_aspect_ratio=decrease,pad=640:360:(ow-iw)/2:(oh-ih)/2"
                .into(),
            "-f".into(),
            "image2pipe".into(),
            "-c:v".into(),
            "mjpeg".into(),
            "pipe:1".into(),
        ])
        .await?;
        if frame.is_empty() {
            return Err(HandError::Refused(format!(
                "could not decode the frame at {}s",
                ferb_py::json::python_float(*t)
            )));
        }
        frames.push(frame);
    }
    let (image, width, height) = sheet.sheet(&frames, &samples)?;
    let size: i64 = ferb_py::value::py_str(&format["size"]).parse().unwrap_or(0);
    let info = json!({"path": p, "duration_s": duration, "bytes": size, "sample_times_s": samples, "review_width": width, "review_height": height,
                      "next": "Compare these decoded frames with the requested targets/actions; choose other times if coverage is unclear."});
    Ok((info.as_object().cloned().unwrap_or_default(), image))
}
