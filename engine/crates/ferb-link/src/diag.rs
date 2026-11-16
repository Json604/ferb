//! What the link is doing, for whoever has to find out why it misbehaves on a real phone: short lines with
//! kinds, counts and times. Never an address, an endpoint id, a relay's name or anything the owner typed.
//!
//! On Android the lines go to logcat under the tag `ferb-link`; on the Mac to stderr when `FERB_LINK_DIAG` is
//! set. iroh's own log (which does name addresses) is off unless a lab build asks for it: `FERB_LINK_TRACE`
//! (`debug` or `trace`) at build time for an app, or in the environment for the harness.

use iroh::Endpoint;

pub fn log(line: &str) {
    #[cfg(target_os = "android")]
    android(4, line);
    #[cfg(not(target_os = "android"))]
    if on() {
        eprintln!("[diag {}] {line}", clock());
    }
}

/// Whether these lines go anywhere: always on a phone (its log is a ring that forgets), on the Mac when asked.
pub fn enabled() -> bool {
    #[cfg(target_os = "android")]
    return true;
    #[cfg(not(target_os = "android"))]
    on()
}

#[cfg(not(target_os = "android"))]
fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var_os("FERB_LINK_DIAG").is_some()
            || std::env::var_os("FERB_LINK_TRACE").is_some()
    })
}
#[cfg(not(target_os = "android"))]
fn clock() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        t.as_secs() % 86_400 / 3600,
        t.as_secs() % 3600 / 60,
        t.as_secs() % 60,
        t.subsec_millis()
    )
}

#[cfg(target_os = "android")]
fn android(priority: i32, line: &str) {
    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(
            prio: std::ffi::c_int,
            tag: *const std::ffi::c_char,
            text: *const std::ffi::c_char,
        ) -> std::ffi::c_int;
    }
    if let Ok(text) = std::ffi::CString::new(line.replace('\0', " ")) {
        unsafe { __android_log_write(priority, c"ferb-link".as_ptr(), text.as_ptr()) };
    }
}

/// The endpoint's own counters since it was built: bytes by the way they travelled, and how often it did the
/// things that cost traffic.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    pub tx_relay: u64,
    pub tx_v4: u64,
    pub tx_v6: u64,
    pub rx_relay: u64,
    pub rx_v4: u64,
    pub rx_v6: u64,
    /// Network reports (address discovery rounds), and how many of them were full ones.
    pub reports: u64,
    pub reports_full: u64,
    /// Rounds of trying a direct path.
    pub punches: u64,
    pub relay_connects: u64,
    pub relay_failures: u64,
    pub relay_closes: u64,
    pub link_changes: u64,
    pub conns_opened: u64,
    pub conns_closed: u64,
}
pub fn counters(ep: &Endpoint) -> Counters {
    let m = ep.metrics();
    let (s, n) = (&m.socket, &m.net_report);
    Counters {
        tx_relay: s.send_relay.get(),
        tx_v4: s.send_ipv4.get(),
        tx_v6: s.send_ipv6.get(),
        rx_relay: s.recv_data_relay.get(),
        rx_v4: s.recv_data_ipv4.get(),
        rx_v6: s.recv_data_ipv6.get(),
        reports: n.reports.get(),
        reports_full: n.reports_full.get(),
        punches: s.holepunch_attempts.get(),
        relay_connects: s.relay_conns_success.get(),
        relay_failures: s.relay_conns_failed.get(),
        relay_closes: s.relay_conns_closed.get(),
        link_changes: s.actor_link_change.get(),
        conns_opened: s.num_conns_opened.get(),
        conns_closed: s.num_conns_closed.get(),
    }
}
impl Counters {
    /// What happened since `before`. An endpoint that was replaced meanwhile starts its counters again: then
    /// the new endpoint's own numbers are the answer.
    pub fn since(&self, before: &Counters) -> Counters {
        let d = |now: u64, was: u64| if now >= was { now - was } else { now };
        Counters {
            tx_relay: d(self.tx_relay, before.tx_relay),
            tx_v4: d(self.tx_v4, before.tx_v4),
            tx_v6: d(self.tx_v6, before.tx_v6),
            rx_relay: d(self.rx_relay, before.rx_relay),
            rx_v4: d(self.rx_v4, before.rx_v4),
            rx_v6: d(self.rx_v6, before.rx_v6),
            reports: d(self.reports, before.reports),
            reports_full: d(self.reports_full, before.reports_full),
            punches: d(self.punches, before.punches),
            relay_connects: d(self.relay_connects, before.relay_connects),
            relay_failures: d(self.relay_failures, before.relay_failures),
            relay_closes: d(self.relay_closes, before.relay_closes),
            link_changes: d(self.link_changes, before.link_changes),
            conns_opened: d(self.conns_opened, before.conns_opened),
            conns_closed: d(self.conns_closed, before.conns_closed),
        }
    }
    /// One line. The bytes are QUIC's own (what iroh handed to a socket or a relay), before the relay's TLS.
    pub fn words(&self) -> String {
        format!(
            "sent relay {} v4 {} v6 {} · received relay {} v4 {} v6 {} · reports {} (full {}) · direct tries {} · relay connects {} failed {} closed {} · link changes {} · connections opened {} closed {}",
            self.tx_relay,
            self.tx_v4,
            self.tx_v6,
            self.rx_relay,
            self.rx_v4,
            self.rx_v6,
            self.reports,
            self.reports_full,
            self.punches,
            self.relay_connects,
            self.relay_failures,
            self.relay_closes,
            self.link_changes,
            self.conns_opened,
            self.conns_closed
        )
    }
}

/// The paths of a connection as kinds: `relay*` (the one in use is starred), `v4-public`, `v6-global`, …
pub fn paths(conn: &iroh::endpoint::Connection) -> String {
    let mut all: Vec<String> = conn
        .paths()
        .iter()
        .map(|p| {
            let kind = match p.remote_addr() {
                iroh::TransportAddr::Ip(a) => addr_kind(&a.ip()).to_string(),
                iroh::TransportAddr::Relay(_) => "relay".to_string(),
                _ => "other".to_string(),
            };
            format!("{kind}{}", if p.is_selected() { "*" } else { "" })
        })
        .collect();
    all.sort();
    if all.is_empty() {
        "none".into()
    } else {
        all.join(" ")
    }
}

pub fn addr_kind(ip: &std::net::IpAddr) -> &'static str {
    match ip.to_canonical() {
        std::net::IpAddr::V4(v) if v.is_loopback() => "v4-this-device",
        std::net::IpAddr::V4(v) if v.is_private() || v.is_link_local() => "v4-lan",
        std::net::IpAddr::V4(v) if v.octets()[0] == 100 && (v.octets()[1] & 0xc0) == 64 => {
            "v4-carrier"
        }
        std::net::IpAddr::V4(v)
            if v.octets()[0] == 192 && v.octets()[1] == 0 && v.octets()[2] == 0 =>
        {
            "v4-translated"
        }
        std::net::IpAddr::V4(_) => "v4-public",
        std::net::IpAddr::V6(v) if v.is_loopback() => "v6-this-device",
        std::net::IpAddr::V6(v) if (v.segments()[0] & 0xe000) == 0x2000 => "v6-global",
        std::net::IpAddr::V6(_) => "v6-local",
    }
}

// ---- iroh's own log, for a lab build only -----------------------------------------------------------------------

/// Turns iroh's own log on when this build or this run asks for it. Those lines name addresses: they are for
/// a lab phone on a cable, never for a build anyone else gets.
pub fn trace_if_asked() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let asked = std::env::var("FERB_LINK_TRACE")
            .ok()
            .or_else(|| option_env!("FERB_LINK_TRACE").map(str::to_string));
        let max = match asked.as_deref() {
            Some("trace") => tracing::Level::TRACE,
            Some("debug") => tracing::Level::DEBUG,
            _ => return,
        };
        let _ = tracing::subscriber::set_global_default(Lines { max });
        log("iroh's own log is on in this build (it names addresses; a lab build)");
    });
}

struct Lines {
    max: tracing::Level,
}
impl tracing::Subscriber for Lines {
    fn enabled(&self, m: &tracing::Metadata<'_>) -> bool {
        // `trace` is only for what decides a direct path (the QUIC connection, the tries for a direct path);
        // everything else stays at `debug`
        let deep = [
            "noq_proto::connection",
            "noq_proto::n0_nat_traversal",
            "iroh::socket::remote_map",
        ]
        .iter()
        .any(|p| m.target().starts_with(p));
        let max = if deep {
            self.max
        } else {
            self.max.min(tracing::Level::DEBUG)
        };
        *m.level() <= max
            && ["iroh", "noq", "netwatch", "portmapper"]
                .iter()
                .any(|p| m.target().starts_with(p))
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
    fn event(&self, e: &tracing::Event<'_>) {
        struct Text(String);
        impl tracing::field::Visit for Text {
            fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
                use std::fmt::Write;
                let _ = if f.name() == "message" {
                    write!(self.0, "{v:?} ")
                } else {
                    write!(self.0, "{}={v:?} ", f.name())
                };
            }
        }
        let mut t = Text(String::new());
        e.record(&mut t);
        let m = e.metadata();
        let mut line = format!("{} {}: {}", m.level(), m.target(), t.0);
        if line.len() > 900 {
            let mut cut = 900;
            while !line.is_char_boundary(cut) {
                cut -= 1;
            }
            line.truncate(cut);
        }
        log(&line);
    }
}
