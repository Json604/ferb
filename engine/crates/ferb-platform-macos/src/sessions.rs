//! The Mac's side of the owner's sessions (kb HAND-691..695, HAND-652..666): the process table with each process's terminal and
//! its foreground group (libproc), a process's working folder, an app bundle's id and name, and the host adapters
//! that type without the foreground: Terminal's scripting (`do script`) and tmux's own input. Every program FERB runs
//! for this gets an environment built from scratch (HAND-659: an agent's variables leaking into a terminal it opens made
//! Claude Code stop saving its transcript, in the lab).

use std::collections::HashMap;
use std::ffi::{c_void, CStr};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// One process (the fields `ferb_sessions::host::Proc` takes).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Proc {
    pub pid: i32,
    pub ppid: i32,
    pub pgid: i32,
    pub tpgid: i32,
    pub tty: String,
    pub exe: String,
}

/// The owner's processes, and the other users' between them and their app as links only (HAND-704).
///
/// A terminal app starts each shell through `login`, which runs as root: Terminal, iTerm2 and Ghostty → `login` →
/// `-zsh`. With the owner's processes alone, a shell's parent was missing, the walk up to its app stopped there, and
/// no terminal in those apps (nor Claude Code or Codex running in one) had an app, so none was listed or typed into
/// (the owner's live report, 2026-11-23). Such a process is kept with its parent and program, so the walk passes
/// through it, but without its terminal, so a terminal is still found by the owner's processes on it.
pub fn processes() -> HashMap<i32, Proc> {
    let uid = unsafe { libc::getuid() };
    let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if n <= 0 {
        return HashMap::new();
    }
    let mut pids = vec![0i32; n as usize + 64];
    let bytes = (pids.len() * std::mem::size_of::<i32>()) as i32;
    let got = unsafe { libc::proc_listallpids(pids.as_mut_ptr() as *mut c_void, bytes) };
    if got <= 0 {
        return HashMap::new();
    }
    pids.truncate(got as usize);
    let mut all: HashMap<i32, (Proc, bool)> = HashMap::new();
    for pid in pids.into_iter().filter(|p| *p > 0) {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        let r = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                &mut info as *mut _ as *mut c_void,
                size,
            )
        };
        if r != size {
            // decision: HAND-715 — another user's process (a terminal's `login`, which runs as root) refuses the full
            // record to the owner (EPERM, read on his Mac): its short record still gives its parent and group, so it
            // is kept as a link with no terminal and the walk up to the terminal's app passes through it
            if let Some(p) = short_info(pid) {
                all.insert(pid, (p, false));
            }
            continue;
        }
        let tty = tty_name(info.e_tdev);
        all.insert(
            pid,
            (
                Proc {
                    pid,
                    ppid: info.pbi_ppid as i32,
                    pgid: info.pbi_pgid as i32,
                    tpgid: if tty.is_empty() {
                        0
                    } else {
                        info.e_tpgid as i32
                    },
                    tty,
                    exe: String::new(),
                },
                info.pbi_uid == uid,
            ),
        );
    }
    let mut out = with_links(all);
    for p in out.values_mut() {
        p.exe = exe_of(p.pid);
    }
    out
}

/// A process's short record (parent and group, no terminal): what macOS gives the owner of another user's process.
pub fn short_info(pid: i32) -> Option<Proc> {
    let mut info: libc::proc_bsdshortinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdshortinfo>() as i32;
    // SAFETY: `info` is a zeroed proc_bsdshortinfo of exactly `size` bytes that the call fills in.
    let r = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDT_SHORTBSDINFO,
            0,
            &mut info as *mut _ as *mut c_void,
            size,
        )
    };
    (r == size).then(|| Proc {
        pid,
        ppid: info.pbsi_ppid as i32,
        pgid: info.pbsi_pgid as i32,
        ..Proc::default()
    })
}

/// HAND-704: the owner's processes (`true`), with every other process on the way up from one of them to launchd kept
/// as a link: its parent and program, no terminal.
pub fn with_links(all: HashMap<i32, (Proc, bool)>) -> HashMap<i32, Proc> {
    let mut out: HashMap<i32, Proc> = all
        .iter()
        .filter(|(_, (_, mine))| *mine)
        .map(|(pid, (p, _))| (*pid, p.clone()))
        .collect();
    let mine: Vec<i32> = out.keys().copied().collect();
    for pid in mine {
        let mut cur = out.get(&pid).map_or(0, |p| p.ppid);
        let mut steps = 0;
        while cur > 1 && !out.contains_key(&cur) && steps < 64 {
            let Some((p, _)) = all.get(&cur) else { break };
            out.insert(
                cur,
                Proc {
                    tty: String::new(),
                    tpgid: 0,
                    ..p.clone()
                },
            );
            cur = p.ppid;
            steps += 1;
        }
    }
    out
}

fn tty_name(dev: u32) -> String {
    // NODEV (no controlling terminal)
    if dev == u32::MAX || dev == 0 {
        return String::new();
    }
    let p = unsafe { libc::devname(dev as libc::dev_t, libc::S_IFCHR) };
    if p.is_null() {
        return String::new();
    }
    let name = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    if name.is_empty() || name == "??" {
        String::new()
    } else {
        format!("/dev/{name}")
    }
}

/// A process's parent (0 when unreadable).
pub fn parent_of(pid: i32) -> i32 {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let r = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut c_void,
            size,
        )
    };
    if r != size {
        return 0;
    }
    info.pbi_ppid as i32
}

/// This process's ancestors, its parent first (at most 32, stopping at launchd).
pub fn my_ancestors() -> Vec<i32> {
    let mut out = Vec::new();
    let mut p = unsafe { libc::getppid() };
    while p > 1 && out.len() < 32 && !out.contains(&p) {
        out.push(p);
        p = parent_of(p);
    }
    out
}

/// A process's program (its full path), "" when unreadable.
pub fn exe_of(pid: i32) -> String {
    let mut buf = vec![0u8; 4096];
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    if n <= 0 {
        return String::new();
    }
    buf.truncate(n as usize);
    String::from_utf8_lossy(&buf).into_owned()
}

/// A process's working folder, "" when unreadable.
pub fn cwd_of(pid: i32) -> String {
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as i32;
    let r = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            &mut info as *mut _ as *mut c_void,
            size,
        )
    };
    if r != size {
        return String::new();
    }
    let raw = &info.pvi_cdir.vip_path;
    let bytes: Vec<u8> = raw
        .iter()
        .flat_map(|row| row.iter())
        .map(|c| *c as u8)
        .take_while(|b| *b != 0)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// An app bundle's id and the name the owner sees (its Info.plist: display name, else name, else the folder).
pub fn app_of(bundle: &str) -> (String, String) {
    let plist = Path::new(bundle).join("Contents").join("Info.plist");
    let stem = Path::new(bundle)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Ok(v) = plist::Value::from_file(&plist) else {
        return (String::new(), stem);
    };
    let d = v.as_dictionary();
    let get = |k: &str| {
        d.and_then(|d| d.get(k))
            .and_then(|x| x.as_string())
            .map(str::to_owned)
            .filter(|s| !s.is_empty())
    };
    let id = get("CFBundleIdentifier").unwrap_or_default();
    let name = get("CFBundleDisplayName")
        .or_else(|| get("CFBundleName"))
        .unwrap_or(stem);
    (id, name)
}

/// The environment every program started here gets (HAND-659).
pub fn clean_command(program: &str, home: &Path) -> Command {
    let mut c = Command::new(program);
    c.env_clear();
    c.env("HOME", home);
    c.env(
        "PATH",
        format!(
            "{}/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            home.display()
        ),
    );
    for k in ["USER", "LOGNAME", "LANG", "SHELL", "TMPDIR"] {
        if let Ok(v) = std::env::var(k) {
            c.env(k, v);
        }
    }
    c.stdin(Stdio::null());
    c
}

/// Runs a program to its end within `limit`: its output, or why not.
pub fn run(mut c: Command, limit: Duration) -> Result<String, String> {
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().map_err(|e| e.to_string())?;
    let start = Instant::now();
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => {
                let out = child.wait_with_output().map_err(|e| e.to_string())?;
                if status.success() {
                    return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
                }
                let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
                return Err(if err.is_empty() {
                    format!("exited with {status}")
                } else {
                    err
                });
            }
            None if start.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("took too long".into());
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

/// Claude Code's program: a running one's, else the usual install places.
pub fn claude_program(home: &Path, running: Option<&str>) -> Option<PathBuf> {
    if let Some(r) = running.filter(|r| Path::new(r).is_file()) {
        return Some(PathBuf::from(r));
    }
    [
        home.join(".local/bin/claude"),
        home.join(".claude/local/claude"),
        PathBuf::from("/opt/homebrew/bin/claude"),
        PathBuf::from("/usr/local/bin/claude"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// `claude agents --json` (documented for scripts; no model call).
pub fn claude_agents(home: &Path, running: Option<&str>) -> Option<String> {
    let program = claude_program(home, running)?;
    let mut c = clean_command(&program.to_string_lossy(), home);
    c.args(["agents", "--json"]).current_dir(home);
    run(c, Duration::from_secs(8)).ok()
}

/// An AppleScript string literal.
pub fn applescript_string(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\" & linefeed & \""),
            '\r' => out.push_str("\" & return & \""),
            '\t' => out.push_str("\" & tab & \""),
            '\u{1b}' => out.push_str("\" & (ASCII character 27) & \""),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The text Terminal's `do script` types: one line as it is; several lines inside bracketed-paste marks so the agent
/// takes them as one message (the lab's S0-4). `do script` adds the Return.
pub fn terminal_payload(text: &str) -> String {
    if text.contains('\n') {
        format!("\u{1b}[200~{text}\u{1b}[201~")
    } else {
        text.to_owned()
    }
}

/// The AppleScript that types `text` into Terminal's tab on `tty` (in the background: no `activate`).
pub fn terminal_type_script(tty: &str, text: &str) -> String {
    format!(
        "tell application id \"com.apple.Terminal\"\n\
           repeat with w in windows\n\
             repeat with t in tabs of w\n\
               if tty of t is {tty} then\n\
                 do script {payload} in t\n\
                 return \"ok\"\n\
               end if\n\
             end repeat\n\
           end repeat\n\
           return \"no tab\"\n\
         end tell",
        tty = applescript_string(tty),
        payload = applescript_string(&terminal_payload(text)),
    )
}

/// Runs an AppleScript with a clean environment.
pub fn osascript(script: &str, home: &Path) -> Result<String, String> {
    let mut c = clean_command("/usr/bin/osascript", home);
    c.args(["-e", script]);
    run(c, Duration::from_secs(20)).map(|s| s.trim().to_owned())
}

/// Terminal: types into the session's tab.
pub fn terminal_type(home: &Path, tty: &str, text: &str) -> Result<(), String> {
    match osascript(&terminal_type_script(tty, text), home)?.as_str() {
        "ok" => Ok(()),
        _ => Err("Terminal has no tab on this session's terminal any more.".into()),
    }
}

/// Terminal: a new window running `command` (in the background).
pub fn terminal_open(home: &Path, command: &str) -> Result<(), String> {
    let script = format!(
        "tell application id \"com.apple.Terminal\" to do script {}",
        applescript_string(command)
    );
    osascript(&script, home).map(|_| ())
}

/// The tmux server sockets of this user (`$TMUX_TMPDIR` or /private/tmp, `tmux-<uid>/`).
fn tmux_sockets() -> Vec<PathBuf> {
    let uid = unsafe { libc::getuid() };
    let mut dirs = vec![PathBuf::from(format!("/private/tmp/tmux-{uid}"))];
    if let Ok(t) = std::env::var("TMUX_TMPDIR") {
        dirs.insert(0, PathBuf::from(t).join(format!("tmux-{uid}")));
    }
    dirs.into_iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|r| r.flatten().map(|e| e.path()))
        .collect()
}

fn tmux(program: &str, home: &Path, socket: &Path, args: &[&str]) -> Result<String, String> {
    let mut c = clean_command(program, home);
    c.arg("-S").arg(socket).args(args);
    run(c, Duration::from_secs(5))
}

/// The socket of the tmux server `server` (its pid).
fn tmux_socket(program: &str, home: &Path, server: i32) -> Option<PathBuf> {
    tmux_sockets().into_iter().find(|s| {
        tmux(program, home, s, &["display-message", "-p", "#{pid}"])
            .is_ok_and(|o| o.trim() == server.to_string())
    })
}

/// A tmux server's clients: (pid, tty).
pub fn tmux_clients(program: &str, home: &Path, server: i32) -> Vec<(i32, String)> {
    let Some(sock) = tmux_socket(program, home, server) else {
        return Vec::new();
    };
    tmux(
        program,
        home,
        &sock,
        &["list-clients", "-F", "#{client_pid} #{client_tty}"],
    )
    .map(|o| {
        o.lines()
            .filter_map(|l| {
                let (p, t) = l.split_once(' ')?;
                Some((p.trim().parse().ok()?, t.trim().to_owned()))
            })
            .collect()
    })
    .unwrap_or_default()
}

/// tmux: text (literal; several lines as one bracketed paste) or a key into the pane on `tty`, then Return when
/// `enter`.
pub fn tmux_send(
    program: &str,
    home: &Path,
    server: i32,
    tty: &str,
    text: &str,
    key: &str,
    enter: bool,
) -> Result<(), String> {
    let sock = tmux_socket(program, home, server).ok_or("tmux's server isn't there any more")?;
    let panes = tmux(
        program,
        home,
        &sock,
        &["list-panes", "-a", "-F", "#{pane_tty} #{pane_id}"],
    )?;
    let pane = panes
        .lines()
        .find_map(|l| {
            let (t, id) = l.split_once(' ')?;
            (t == tty).then(|| id.trim().to_owned())
        })
        .ok_or("tmux has no pane on this session's terminal any more")?;
    if !key.is_empty() {
        tmux(program, home, &sock, &["send-keys", "-t", &pane, key])?;
    } else if text.contains('\n') {
        tmux(
            program,
            home,
            &sock,
            &["set-buffer", "-b", "ferb-session", "--", text],
        )?;
        tmux(
            program,
            home,
            &sock,
            &[
                "paste-buffer",
                "-p",
                "-d",
                "-b",
                "ferb-session",
                "-t",
                &pane,
            ],
        )?;
    } else {
        tmux(
            program,
            home,
            &sock,
            &["send-keys", "-t", &pane, "-l", "--", text],
        )?;
    }
    if enter {
        tmux(program, home, &sock, &["send-keys", "-t", &pane, "Enter"])?;
    }
    Ok(())
}

// ---------- every terminal as a session (HAND-691..695) ----------

/// A process's argv[0] as it was started ("-zsh", "agy", "claude"), "" when unreadable (another user's, or gone).
pub fn argv0_of(pid: i32) -> String {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size: libc::size_t = 0;
    // SAFETY: a size query (null buffer) on a three-int name that lives through the call.
    let r = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if r != 0 || size < 8 {
        return String::new();
    }
    let mut buf = vec![0u8; size];
    // SAFETY: `buf` holds `size` bytes, as the kernel was told; it writes at most that and updates `size`.
    let r = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr() as *mut c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if r != 0 {
        return String::new();
    }
    buf.truncate(size);
    argv0_from_procargs(&buf)
}

/// argv[0] from KERN_PROCARGS2's layout: argc (4 bytes), the executable's path, its NUL padding, then argv[0].
pub fn argv0_from_procargs(buf: &[u8]) -> String {
    if buf.len() < 4 {
        return String::new();
    }
    let rest = &buf[4..];
    let Some(end_path) = rest.iter().position(|b| *b == 0) else {
        return String::new();
    };
    let after = &rest[end_path..];
    let Some(start) = after.iter().position(|b| *b != 0) else {
        return String::new();
    };
    let arg = &after[start..];
    let end = arg.iter().position(|b| *b == 0).unwrap_or(arg.len());
    String::from_utf8_lossy(&arg[..end]).into_owned()
}

/// When a terminal device last had input or output: the later of its access and change times, in seconds since
/// 1970. None when it can't be read.
pub fn tty_activity(tty: &str) -> Option<f64> {
    use std::os::unix::fs::MetadataExt;
    if !tty.starts_with("/dev/") {
        return None;
    }
    let m = std::fs::metadata(tty).ok()?;
    let at = |s: i64, ns: i64| s as f64 + ns as f64 / 1e9;
    Some(at(m.atime(), m.atime_nsec()).max(at(m.mtime(), m.mtime_nsec())))
}

/// The arguments that open a new window of the app `bundle_id` in `folder` without bringing it forward (`open -g`).
pub fn open_folder_args(bundle_id: &str, folder: &str) -> Vec<String> {
    vec!["-g".into(), "-b".into(), bundle_id.into(), folder.into()]
}

/// A new window of the terminal app `bundle_id` in `folder` (Terminal opens a folder as a new window there), in the
/// background; nothing is typed.
pub fn open_folder_in(home: &Path, bundle_id: &str, folder: &str) -> Result<(), String> {
    let mut c = clean_command("/usr/bin/open", home);
    c.args(open_folder_args(bundle_id, folder));
    run(c, Duration::from_secs(10)).map(|_| ())
}
