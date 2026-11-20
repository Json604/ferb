//! `ferb-power`: FERB's root helper program (kb APP-986). launchd starts it as root from FERB.app
//! (`Contents/Library/LaunchDaemons/com.ferb.power.plist`, `SMAppService.daemon`) at boot and keeps it running. It clears
//! a lease left behind, listens on its socket, answers only ferbd, and ends any lease when it stops.

#[cfg(target_os = "macos")]
fn main() {
    mac_main::run();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("ferb-power runs only on macOS");
    std::process::exit(2);
}

#[cfg(target_os = "macos")]
mod mac_main {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use ferb_power_helper::helper::SOCKET;
    use ferb_power_helper::mac::{Callers, FileMarker, Iokit};
    use ferb_power_helper::Helper;

    /// The longest request line, and the most requests one connection makes.
    const LINE_MAX: u64 = 1024;
    const REQUESTS_MAX: usize = 64;

    static STOP: AtomicBool = AtomicBool::new(false);

    extern "C" fn on_term(_sig: libc::c_int) {
        STOP.store(true, Ordering::SeqCst);
    }

    fn wall() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    pub fn run() {
        // SAFETY: a plain query.
        if unsafe { libc::geteuid() } != 0 {
            eprintln!("ferb-power: runs only as root, from FERB.app's background item");
            std::process::exit(1);
        }
        let start = Instant::now();
        let mono = move || start.elapsed().as_secs_f64();
        let helper = Arc::new(Helper::new(Iokit, FileMarker::system()));
        if helper.boot() {
            eprintln!("ferb-power: cleared a sleep lease left behind");
        }
        // SAFETY: installs a handler that only stores to an atomic.
        unsafe {
            libc::signal(libc::SIGTERM, on_term as *const () as libc::sighandler_t);
            libc::signal(libc::SIGINT, on_term as *const () as libc::sighandler_t);
        }
        let _ = std::fs::remove_file(SOCKET);
        let listener = match UnixListener::bind(SOCKET) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("ferb-power: couldn't listen on {SOCKET}: {e}");
                std::process::exit(1);
            }
        };
        // any process may connect; only ferbd's signature is answered
        let _ = std::fs::set_permissions(SOCKET, std::fs::Permissions::from_mode(0o666));
        let callers = Arc::new(Callers::new());
        // the watchdog: a lease not renewed ends; a stop ends the lease, then the program
        let h = helper.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            h.tick(mono());
            if STOP.load(Ordering::SeqCst) {
                h.stop();
                let _ = std::fs::remove_file(SOCKET);
                std::process::exit(0);
            }
        });
        for conn in listener.incoming().flatten() {
            let (h, c) = (helper.clone(), callers.clone());
            std::thread::spawn(move || serve(conn, &h, &c, mono));
        }
    }

    fn serve(
        conn: UnixStream,
        helper: &Helper<Iokit, FileMarker>,
        callers: &Callers,
        mono: impl Fn() -> f64,
    ) {
        let _ = conn.set_read_timeout(Some(Duration::from_secs(10)));
        let token = ferb_platform_macos::peer::peer_token(&conn);
        let mut out = match conn.try_clone() {
            Ok(o) => o,
            Err(_) => return,
        };
        if !callers.allowed(token.as_ref()) {
            let _ = out.write_all(b"{\"ok\": false, \"error\": \"only FERB's daemon may ask\"}\n");
            return;
        }
        let mut reader = BufReader::new(conn);
        for _ in 0..REQUESTS_MAX {
            let mut line = String::new();
            match (&mut reader).take(LINE_MAX).read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            if !line.ends_with('\n') {
                return; // too long, or cut off
            }
            let answer = helper.line(&line, mono(), wall());
            if out.write_all(answer.as_bytes()).is_err() {
                return;
            }
        }
    }
}
