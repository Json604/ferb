//! This Mac's TCP and UDP sockets as kernel facts (ferb/act/shell/sockets.py, HAND-226): endpoints, TCP state, the
//! owning process. Nothing here judges what a socket or its program is; `local` and `listeners` decide from these
//! facts and fail closed on a socket whose process can't be named.
//!
//! Two readings: `inspectable()` reads every process of the owner's user through libproc in FERB's own process (no
//! subprocess), and `complete()` adds every other socket `nettop` lists, unnamed (pid None), counting only when nettop
//! lists every listener libproc found. The struct layouts are the SDK's `sys/proc_info.h`, field for field, checked
//! against Python's ctypes definitions (`tests/sockets.rs`); an answer of any size but 792 bytes is no reading.

use std::mem::{offset_of, size_of};

use crate::pynet::{ip_address, Ip};
use crate::pyos::{int_str, is_space};

pub const AF_INET: i32 = 2;
/// macOS's AF_INET6.
pub const AF_INET6: i32 = 30;
pub const IPPROTO_UDP: i32 = 17;
const SOCKINFO_IN: i32 = 1;
const SOCKINFO_TCP: i32 = 2;
const INI_IPV4: u8 = 0x1;
const INI_IPV6: u8 = 0x2;
/// TSI_S_*, by value.
pub const TCP_STATES: [&str; 11] = [
    "CLOSED",
    "LISTEN",
    "SYN_SENT",
    "SYN_RECEIVED",
    "ESTABLISHED",
    "CLOSE_WAIT",
    "FIN_WAIT_1",
    "CLOSING",
    "LAST_ACK",
    "FIN_WAIT_2",
    "TIME_WAIT",
];

// ---------- sys/proc_info.h, field for field (natural alignment) ----------

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VinfoStat {
    pub vst_dev: u32,
    pub vst_mode: u16,
    pub vst_nlink: u16,
    pub vst_ino: u64,
    pub vst_uid: u32,
    pub vst_gid: u32,
    pub vst_times: [i64; 8],
    pub vst_size: i64,
    pub vst_blocks: i64,
    pub vst_blksize: i32,
    pub vst_flags: u32,
    pub vst_gen: u32,
    pub vst_rdev: u32,
    pub vst_qspare: [i64; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SockbufInfo {
    pub sbi_cc: u32,
    pub sbi_hiwat: u32,
    pub sbi_mbcnt: u32,
    pub sbi_mbmax: u32,
    pub sbi_lowat: u32,
    pub sbi_flags: i16,
    pub sbi_timeo: i16,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct InV6 {
    pub in6_hlim: u8,
    pub in6_cksum: i32,
    pub in6_ifindex: u16,
    pub in6_hops: i16,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct InSockinfo {
    pub insi_fport: i32,
    pub insi_lport: i32,
    pub insi_gencnt: u64,
    pub insi_flags: u32,
    pub insi_flow: u32,
    pub insi_vflag: u8,
    pub insi_ip_ttl: u8,
    pub rfu_1: u32,
    /// in4in6_addr | in6_addr
    pub insi_faddr: [u32; 4],
    pub insi_laddr: [u32; 4],
    pub insi_v4: u8,
    pub insi_v6: InV6,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TcpSockinfo {
    pub tcpsi_ini: InSockinfo,
    pub tcpsi_state: i32,
    pub tcpsi_timer: [i32; 4],
    pub tcpsi_mss: i32,
    pub tcpsi_flags: u32,
    pub rfu_1: u32,
    pub tcpsi_tp: u64,
}

/// The union's largest member.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct UnSockinfo {
    pub unsi_conn_so: u64,
    pub unsi_conn_pcb: u64,
    pub unsi_addr: [u8; 255],
    pub unsi_caddr: [u8; 255],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union Proto {
    pub pri_in: InSockinfo,
    pub pri_tcp: TcpSockinfo,
    pub pri_un: UnSockinfo,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SocketInfo {
    pub soi_stat: VinfoStat,
    pub soi_so: u64,
    pub soi_pcb: u64,
    pub soi_type: i32,
    pub soi_protocol: i32,
    pub soi_family: i32,
    pub soi_options: i16,
    pub soi_linger: i16,
    pub soi_state: i16,
    pub soi_qlen: i16,
    pub soi_incqlen: i16,
    pub soi_qlimit: i16,
    pub soi_timeo: i16,
    pub soi_error: u16,
    pub soi_oobmark: u32,
    pub soi_rcv: SockbufInfo,
    pub soi_snd: SockbufInfo,
    pub soi_kind: i32,
    pub rfu_1: u32,
    pub soi_proto: Proto,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ProcFileinfo {
    pub fi_openflags: u32,
    pub fi_status: u32,
    pub fi_offset: i64,
    pub fi_type: i32,
    pub fi_guardflags: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SocketFdinfo {
    pub pfi: ProcFileinfo,
    pub psi: SocketInfo,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FdInfo {
    pub proc_fd: i32,
    pub proc_fdtype: u32,
}

/// 792 on macOS 26 and 27 (arm64, x86_64).
pub const SOCKET_FDINFO_SIZE: usize = size_of::<SocketFdinfo>();

/// One socket as a kernel fact.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Socket {
    /// "tcp" or "udp".
    pub proto: String,
    /// "*" for any address.
    pub local: (String, u32),
    /// None: not connected.
    pub remote: Option<(String, u32)>,
    /// The TCP state as netstat names it; "" for UDP.
    pub state: String,
    /// None: a process FERB can't inspect.
    pub pid: Option<i32>,
    /// A listener's connections no process has accepted yet.
    pub queued: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("socket_fdinfo has an unexpected size")]
pub struct WrongSize;

fn read<const N: usize>(raw: &[u8], at: usize) -> [u8; N] {
    raw[at..at + N]
        .try_into()
        .expect("in bounds: the size was checked")
}

fn i32_at(raw: &[u8], at: usize) -> i32 {
    i32::from_ne_bytes(read(raw, at))
}

fn i16_at(raw: &[u8], at: usize) -> i16 {
    i16::from_ne_bytes(read(raw, at))
}

/// sockets.py `_address`: the 16 bytes of an in4in6_addr / in6_addr as text, "*" for any address.
fn address(raw: [u8; 16], v4: bool) -> String {
    let ip = if v4 {
        Ip::V4([raw[12], raw[13], raw[14], raw[15]])
    } else {
        let mut raw = raw;
        if raw[0] == 0xfe && raw[1] & 0xc0 == 0x80 {
            raw[2] = 0; // link-local: drop a scope the kernel embeds
            raw[3] = 0;
        }
        let mut h = [0u16; 8];
        for (i, x) in h.iter_mut().enumerate() {
            *x = u16::from_be_bytes([raw[2 * i], raw[2 * i + 1]]);
        }
        Ip::V6(h).unmapped()
    };
    if ip.is_unspecified() {
        "*".to_string()
    } else {
        ip.text()
    }
}

/// One PROC_PIDFDSOCKETINFO answer as a Socket; None for anything but an IPv4/IPv6 TCP or UDP socket.
pub fn decode(raw: &[u8], pid: i32) -> Result<Option<Socket>, WrongSize> {
    if raw.len() != SOCKET_FDINFO_SIZE {
        return Err(WrongSize);
    }
    let psi = offset_of!(SocketFdinfo, psi);
    let family = i32_at(raw, psi + offset_of!(SocketInfo, soi_family));
    if family != AF_INET && family != AF_INET6 {
        return Ok(None);
    }
    let kind = i32_at(raw, psi + offset_of!(SocketInfo, soi_kind));
    let protocol = i32_at(raw, psi + offset_of!(SocketInfo, soi_protocol));
    let proto_at = psi + offset_of!(SocketInfo, soi_proto);
    let (proto, ini, state) = if kind == SOCKINFO_TCP {
        let tcp = proto_at + offset_of!(Proto, pri_tcp);
        let v = i32_at(raw, tcp + offset_of!(TcpSockinfo, tcpsi_state));
        let state = if (0..TCP_STATES.len() as i32).contains(&v) {
            TCP_STATES[v as usize].to_string()
        } else {
            format!("STATE_{v}")
        };
        ("tcp", tcp + offset_of!(TcpSockinfo, tcpsi_ini), state)
    } else if kind == SOCKINFO_IN && protocol == IPPROTO_UDP {
        ("udp", proto_at + offset_of!(Proto, pri_in), String::new())
    } else {
        return Ok(None);
    };
    let vflag = raw[ini + offset_of!(InSockinfo, insi_vflag)];
    let v4 = vflag & INI_IPV4 != 0 || (vflag & INI_IPV6 == 0 && family == AF_INET);
    let port = |field: usize| u16::from_be((i32_at(raw, ini + field) & 0xFFFF) as u16) as u32;
    let lport = port(offset_of!(InSockinfo, insi_lport));
    let fport = port(offset_of!(InSockinfo, insi_fport));
    let laddr = address(read(raw, ini + offset_of!(InSockinfo, insi_laddr)), v4);
    let remote = if fport != 0 {
        Some((
            address(read(raw, ini + offset_of!(InSockinfo, insi_faddr)), v4),
            fport,
        ))
    } else {
        None
    };
    let qlen = i16_at(raw, psi + offset_of!(SocketInfo, soi_qlen));
    Ok(Some(Socket {
        proto: proto.to_string(),
        local: (laddr, lport),
        remote,
        state,
        pid: Some(pid),
        queued: (qlen as i64).max(0),
    }))
}

/// `str.splitlines()`.
pub fn splitlines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let brk = matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if brk {
            out.push(&text[start..i]);
            let mut end = i + c.len_utf8();
            if c == '\r' {
                if let Some(&(j, '\n')) = chars.peek() {
                    chars.next();
                    end = j + 1;
                }
            }
            start = end;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// sockets.py `_nettop_endpoint`; Err where Python raises (a non-decimal digit such as "²" in a port).
fn nettop_endpoint(text: &str, v4: bool) -> Result<Option<(String, Option<u32>)>, ()> {
    let sep = if v4 { ':' } else { '.' };
    let Some(i) = text.rfind(sep) else {
        return Ok(None);
    };
    let (address, port) = (&text[..i], &text[i + 1..]);
    let digits = !port.is_empty() && port.chars().all(ferb_sanitize::parity::is_digit);
    if !(digits || port == "*") {
        return Ok(None);
    }
    let mut address = address.to_string();
    if address != "*" {
        let Some(ip) = ip_address(address.split('%').next().unwrap_or("")) else {
            return Ok(None);
        };
        let ip = ip.unmapped();
        address = if ip.is_unspecified() {
            "*".to_string()
        } else {
            ip.text()
        };
    }
    if !digits {
        return Ok(Some((address, None)));
    }
    // int(port): Unicode decimal digits read, other digits ("²") raise in Python
    let n = int_str(port).ok_or(())?;
    Ok(Some((address, Some(u32::try_from(n).map_err(|_| ())?))))
}

fn nettop_states(state: &str) -> String {
    match state {
        "Listen" => "LISTEN",
        "Established" => "ESTABLISHED",
        "SynSent" => "SYN_SENT",
        "SynReceived" => "SYN_RECEIVED",
        "CloseWait" => "CLOSE_WAIT",
        "FinWait1" => "FIN_WAIT_1",
        "Closing" => "CLOSING",
        "LastAck" => "LAST_ACK",
        "FinWait2" => "FIN_WAIT_2",
        "TimeWait" => "TIME_WAIT",
        "Closed" => "CLOSED",
        other => return other.to_ascii_uppercase(),
    }
    .to_string()
}

/// `(.*)\.(\d+),,` fullmatched: the process line's pid.
fn process_line(line: &str) -> Option<i128> {
    let body = line.strip_suffix(",,")?;
    if body.contains('\n') {
        return None;
    }
    let i = body.rfind('.')?;
    let digits = &body[i + 1..];
    if digits.is_empty() || !digits.chars().all(ferb_py::unicode::is_decimal) {
        return None;
    }
    int_str(digits)
}

/// `([a-z]+)([46]) (\S+)<->(\S+),([A-Za-z0-9]*),` fullmatched: (proto, family, local, remote, state).
fn socket_line(line: &str) -> Option<(&str, char, &str, &str, &str)> {
    let letters = line.bytes().take_while(|b| b.is_ascii_lowercase()).count();
    if letters == 0 {
        return None;
    }
    let proto = &line[..letters];
    let rest = &line[letters..];
    let family = rest.chars().next().filter(|c| *c == '4' || *c == '6')?;
    let rest = rest[1..].strip_prefix(' ')?;
    let body = rest.strip_suffix(',')?;
    let comma = body.rfind(',')?;
    let (middle, state) = (&body[..comma], &body[comma + 1..]);
    if !state.bytes().all(|b| b.is_ascii_alphanumeric()) || middle.chars().any(is_space) {
        return None;
    }
    // the last "<->" that leaves both sides non-empty (the first group is greedy)
    let mut found = None;
    for (i, _) in middle
        .match_indices("<->")
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        if i > 0 && i + 3 < middle.len() {
            found = Some(i);
            break;
        }
    }
    let i = found?;
    Some((proto, family, &middle[..i], &middle[i + 3..], state))
}

/// (pid, socket) for each TCP and UDP socket line of `nettop -L 1 -n -x -J state`. Any line in another shape makes the
/// whole listing unreadable: None. A digit Python's `int()` can't read ("²") is no listing either (Python raised).
pub fn parse_nettop(text: &str) -> Option<Vec<(i64, Socket)>> {
    let lines = splitlines(text);
    if lines.first() != Some(&",state,") {
        return None;
    }
    let mut out = Vec::new();
    let mut pid: Option<i64> = None;
    for line in &lines[1..] {
        let Some((proto, family, local_text, remote_text, state)) = socket_line(line) else {
            pid = Some(i64::try_from(process_line(line)?).ok()?);
            continue;
        };
        let pid = pid?;
        if proto != "tcp" && proto != "udp" {
            continue; // a flow over a socket (quic4)
        }
        let v4 = family == '4';
        let local = nettop_endpoint(local_text, v4).ok()??;
        let remote = nettop_endpoint(remote_text, v4).ok()??;
        let Some(lport) = local.1 else {
            continue; // not bound to a port
        };
        let far = remote.1.map(|p| (remote.0.clone(), p));
        out.push((
            pid,
            Socket {
                proto: proto.to_string(),
                local: (local.0, lport),
                remote: far,
                state: if proto == "tcp" {
                    nettop_states(state)
                } else {
                    String::new()
                },
                pid: i32::try_from(pid).ok(),
                queued: 0,
            },
        ));
    }
    Some(out)
}

/// A socket anyone can reach, as both readings name it: a TCP listener or an unconnected bound UDP socket.
pub fn endpoint_key(s: &Socket) -> Option<(String, String, u32)> {
    if (s.proto == "tcp" && s.state == "LISTEN")
        || (s.proto == "udp" && s.remote.is_none() && s.local.1 != 0)
    {
        Some((s.proto.clone(), s.local.0.clone(), s.local.1))
    } else {
        None
    }
}

/// sockets.py `complete`'s merge of one inspectable reading and one nettop listing: None when nettop doesn't list
/// every listener the inspectable reading found (or found none).
pub fn merge(
    inspectable: &(Vec<Socket>, std::collections::BTreeSet<i32>),
    listed: &[(i64, Socket)],
) -> Option<Vec<Socket>> {
    let (sockets, inspected) = inspectable;
    let known: std::collections::HashSet<_> = sockets.iter().filter_map(endpoint_key).collect();
    let seen: std::collections::HashSet<_> =
        listed.iter().filter_map(|(_, s)| endpoint_key(s)).collect();
    if known.is_empty() || !known.is_subset(&seen) {
        return None;
    }
    let mut out = sockets.clone();
    for (pid, s) in listed {
        if !i32::try_from(*pid).is_ok_and(|p| inspected.contains(&p)) {
            out.push(Socket {
                pid: None,
                ..s.clone()
            });
        }
    }
    Some(out)
}

#[cfg(target_os = "macos")]
pub use mac::{complete, executable_of, group_members, inspectable};

#[cfg(target_os = "macos")]
mod mac {
    use std::collections::BTreeSet;
    use std::ffi::c_void;
    use std::path::PathBuf;

    use super::*;

    const PROC_ALL_PIDS: u32 = 1;
    const PROC_PGRP_ONLY: u32 = 2;
    const PROC_PIDLISTFDS: i32 = 1;
    const PROC_PIDFDSOCKETINFO: i32 = 3;
    const PROX_FDTYPE_SOCKET: u32 = 2;
    const NETTOP: &str = "/usr/bin/nettop";

    fn pids() -> Option<Vec<i32>> {
        for _ in 0..3 {
            // SAFETY: a null buffer of size 0 asks only for the size needed.
            let need = unsafe { libc::proc_listpids(PROC_ALL_PIDS, 0, std::ptr::null_mut(), 0) };
            if need <= 0 {
                return None;
            }
            let mut buf = vec![0i32; need as usize / 4 + 256];
            let size = (buf.len() * 4) as i32;
            // SAFETY: `buf` holds `size` bytes, which is what the call is told.
            let got = unsafe {
                libc::proc_listpids(PROC_ALL_PIDS, 0, buf.as_mut_ptr() as *mut c_void, size)
            };
            if got <= 0 {
                return None;
            }
            if got < size {
                return Some(
                    buf[..got as usize / 4]
                        .iter()
                        .copied()
                        .filter(|&p| p > 0)
                        .collect(),
                );
            }
        }
        None
    }

    /// The TCP and UDP sockets of every process FERB may inspect (the owner's user), and those processes' ids.
    pub fn inspectable() -> Option<(Vec<Socket>, BTreeSet<i32>)> {
        let pids = pids()?;
        let mut fds = vec![FdInfo::default(); 4096];
        let mut one = vec![0u8; SOCKET_FDINFO_SIZE];
        let mut sockets = Vec::new();
        let mut inspected = BTreeSet::new();
        for pid in pids {
            let fd_size = size_of::<FdInfo>();
            // SAFETY: `fds` holds fds.len() * 8 bytes, the size passed.
            let mut got = unsafe {
                libc::proc_pidinfo(
                    pid,
                    PROC_PIDLISTFDS,
                    0,
                    fds.as_mut_ptr() as *mut c_void,
                    (fds.len() * fd_size) as i32,
                )
            };
            while got as usize >= fds.len() * fd_size {
                // SAFETY: a null buffer of size 0 asks only for the size needed.
                let need =
                    unsafe { libc::proc_pidinfo(pid, PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
                fds = vec![FdInfo::default(); need.max(got) as usize / fd_size + 1024];
                // SAFETY: as above, with the new buffer's size.
                got = unsafe {
                    libc::proc_pidinfo(
                        pid,
                        PROC_PIDLISTFDS,
                        0,
                        fds.as_mut_ptr() as *mut c_void,
                        (fds.len() * fd_size) as i32,
                    )
                };
            }
            if got <= 0 {
                continue; // another user's (EPERM) or gone (ESRCH)
            }
            inspected.insert(pid);
            for entry in &fds[..got as usize / fd_size] {
                if entry.proc_fdtype != PROX_FDTYPE_SOCKET {
                    continue;
                }
                // SAFETY: `one` holds SOCKET_FDINFO_SIZE bytes, the size passed.
                let n = unsafe {
                    libc::proc_pidfdinfo(
                        pid,
                        entry.proc_fd,
                        PROC_PIDFDSOCKETINFO,
                        one.as_mut_ptr() as *mut c_void,
                        SOCKET_FDINFO_SIZE as i32,
                    )
                };
                if n <= 0 {
                    continue; // closed since the list
                }
                if n as usize != SOCKET_FDINFO_SIZE {
                    return None;
                }
                if let Ok(Some(found)) = decode(&one, pid) {
                    sockets.push(found);
                }
            }
        }
        Some((sockets, inspected))
    }

    fn nettop() -> Option<Vec<(i64, Socket)>> {
        let out = crate::run_capture(
            &[NETTOP, "-L", "1", "-n", "-x", "-J", "state"],
            std::time::Duration::from_secs(5),
        )?;
        if out.0 != 0 {
            return None;
        }
        parse_nettop(&String::from_utf8_lossy(&out.1))
    }

    /// Every TCP and UDP socket on this Mac: the inspectable ones with their process, the rest (another user's,
    /// root's) with pid None. None when either reading fails or nettop misses a listener libproc found.
    pub fn complete() -> Option<Vec<Socket>> {
        for _ in 0..2 {
            let got = inspectable()?;
            let listed = nettop()?;
            if let Some(every) = merge(&got, &listed) {
                return Some(every);
            }
        }
        None
    }

    /// The kernel-reported executable path (proc_pidpath), resolved; None when it can't be read.
    pub fn executable_of(pid: i32) -> Option<PathBuf> {
        let mut buf = vec![0u8; 4096];
        // SAFETY: `buf` holds 4096 bytes, the size passed.
        let n =
            unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
        if n <= 0 {
            return None;
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(n as usize);
        use std::os::unix::ffi::OsStrExt;
        Some(crate::pyos::realpath(std::path::Path::new(
            std::ffi::OsStr::from_bytes(&buf[..end]),
        )))
    }

    /// The processes now in process group `pgid` (proc_listpids PROC_PGRP_ONLY); empty when none or unreadable.
    pub fn group_members(pgid: i32) -> Vec<i32> {
        if pgid <= 0 {
            return Vec::new();
        }
        let mut size = 256usize;
        loop {
            let mut buf = vec![0i32; size];
            // SAFETY: `buf` holds size * 4 bytes, the size passed.
            let n = unsafe {
                libc::proc_listpids(
                    PROC_PGRP_ONLY,
                    pgid as u32,
                    buf.as_mut_ptr() as *mut c_void,
                    (size * 4) as i32,
                )
            };
            if n <= 0 {
                return Vec::new();
            }
            let count = n as usize / 4;
            if count < size {
                return buf[..count].iter().copied().filter(|&p| p > 0).collect();
            }
            size *= 4;
        }
    }
}

/// Off macOS there is no libproc: no reading, so every check fails closed.
#[cfg(not(target_os = "macos"))]
pub fn inspectable() -> Option<(Vec<Socket>, std::collections::BTreeSet<i32>)> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn complete() -> Option<Vec<Socket>> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn executable_of(_pid: i32) -> Option<std::path::PathBuf> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn group_members(_pgid: i32) -> Vec<i32> {
    Vec::new()
}
