//! libproc's socket structs laid out as Python's ctypes definitions (sizes and every field offset), socket answers
//! decoded as Python decodes them (the recorded ones from tests/support and random invented ones), nettop listings
//! parsed the same, and the complete-reading merge.
// decision: HAND-226

mod common;

use std::collections::BTreeSet;
use std::mem::{align_of, offset_of, size_of};

use common::{fixture, Tally};
use ferb_shell::sockets::*;
use serde_json::{json, Value};

macro_rules! fields {
    ($t:ty, $($f:ident),*) => {
        vec![$((stringify!($f), offset_of!($t, $f))),*]
    };
}

/// (name, size, alignment, field offsets).
type Layout = (&'static str, usize, usize, Vec<(&'static str, usize)>);

#[test]
fn the_structs_match_pythons_ctypes_layout() {
    let fx = fixture("sockets.json");
    let mut tally = Tally::default();
    let ours: Vec<Layout> = vec![
        (
            "_VinfoStat",
            size_of::<VinfoStat>(),
            align_of::<VinfoStat>(),
            fields!(
                VinfoStat,
                vst_dev,
                vst_mode,
                vst_nlink,
                vst_ino,
                vst_uid,
                vst_gid,
                vst_times,
                vst_size,
                vst_blocks,
                vst_blksize,
                vst_flags,
                vst_gen,
                vst_rdev,
                vst_qspare
            ),
        ),
        (
            "_SockbufInfo",
            size_of::<SockbufInfo>(),
            align_of::<SockbufInfo>(),
            fields!(
                SockbufInfo,
                sbi_cc,
                sbi_hiwat,
                sbi_mbcnt,
                sbi_mbmax,
                sbi_lowat,
                sbi_flags,
                sbi_timeo
            ),
        ),
        (
            "_InV6",
            size_of::<InV6>(),
            align_of::<InV6>(),
            fields!(InV6, in6_hlim, in6_cksum, in6_ifindex, in6_hops),
        ),
        (
            "_InSockinfo",
            size_of::<InSockinfo>(),
            align_of::<InSockinfo>(),
            fields!(
                InSockinfo,
                insi_fport,
                insi_lport,
                insi_gencnt,
                insi_flags,
                insi_flow,
                insi_vflag,
                insi_ip_ttl,
                rfu_1,
                insi_faddr,
                insi_laddr,
                insi_v4,
                insi_v6
            ),
        ),
        (
            "_TcpSockinfo",
            size_of::<TcpSockinfo>(),
            align_of::<TcpSockinfo>(),
            fields!(
                TcpSockinfo,
                tcpsi_ini,
                tcpsi_state,
                tcpsi_timer,
                tcpsi_mss,
                tcpsi_flags,
                rfu_1,
                tcpsi_tp
            ),
        ),
        (
            "_UnSockinfo",
            size_of::<UnSockinfo>(),
            align_of::<UnSockinfo>(),
            fields!(
                UnSockinfo,
                unsi_conn_so,
                unsi_conn_pcb,
                unsi_addr,
                unsi_caddr
            ),
        ),
        (
            "_Proto",
            size_of::<Proto>(),
            align_of::<Proto>(),
            fields!(Proto, pri_in, pri_tcp, pri_un),
        ),
        (
            "_SocketInfo",
            size_of::<SocketInfo>(),
            align_of::<SocketInfo>(),
            fields!(
                SocketInfo,
                soi_stat,
                soi_so,
                soi_pcb,
                soi_type,
                soi_protocol,
                soi_family,
                soi_options,
                soi_linger,
                soi_state,
                soi_qlen,
                soi_incqlen,
                soi_qlimit,
                soi_timeo,
                soi_error,
                soi_oobmark,
                soi_rcv,
                soi_snd,
                soi_kind,
                rfu_1,
                soi_proto
            ),
        ),
        (
            "_ProcFileinfo",
            size_of::<ProcFileinfo>(),
            align_of::<ProcFileinfo>(),
            fields!(
                ProcFileinfo,
                fi_openflags,
                fi_status,
                fi_offset,
                fi_type,
                fi_guardflags
            ),
        ),
        (
            "_SocketFdinfo",
            size_of::<SocketFdinfo>(),
            align_of::<SocketFdinfo>(),
            fields!(SocketFdinfo, pfi, psi),
        ),
        (
            "_FdInfo",
            size_of::<FdInfo>(),
            align_of::<FdInfo>(),
            fields!(FdInfo, proc_fd, proc_fdtype),
        ),
    ];
    for (name, size, align, offsets) in ours {
        let want = &fx["layouts"][name];
        tally.check(
            &format!("{name} size"),
            size as u64,
            want["size"].as_u64().unwrap(),
        );
        tally.check(
            &format!("{name} align"),
            align as u64,
            want["align"].as_u64().unwrap(),
        );
        let want_fields: Vec<(String, u64)> = want["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| (f[0].as_str().unwrap().to_string(), f[1].as_u64().unwrap()))
            .collect();
        let got: Vec<(String, u64)> = offsets
            .iter()
            .map(|(n, o)| (n.to_string(), *o as u64))
            .collect();
        tally.check(&format!("{name} fields"), got, want_fields);
    }
    tally.check(
        "SOCKET_FDINFO_SIZE",
        SOCKET_FDINFO_SIZE as u64,
        fx["size"].as_u64().unwrap(),
    );
    tally.check("792", SOCKET_FDINFO_SIZE, 792);
    tally.done("layout");
}

fn socket_json(s: &Socket) -> Value {
    json!({"proto": s.proto, "local": [s.local.0, s.local.1], "remote": s.remote.as_ref().map(|r| json!([r.0, r.1])),
           "state": s.state, "pid": s.pid, "queued": s.queued})
}

#[test]
fn socket_answers_decode_as_python_decodes_them() {
    let fx = fixture("sockets.json");
    let mut tally = Tally::default();
    for case in fx["decode"].as_array().unwrap() {
        let raw = hex::decode(case["raw"].as_str().unwrap()).unwrap();
        let got = match decode(&raw, case["pid"].as_i64().unwrap() as i32) {
            Err(e) => json!({"error": e.to_string()}),
            Ok(None) => Value::Null,
            Ok(Some(s)) => socket_json(&s),
        };
        tally.check(case["name"].as_str().unwrap(), got, case["decoded"].clone());
    }
    tally.done("decode");
}

#[test]
fn nettop_listings_parse_as_python_parses_them() {
    let fx = fixture("sockets.json");
    let mut tally = Tally::default();
    for case in fx["nettop"].as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        let got = parse_nettop(text).map(|rows| {
            rows.iter()
                .map(|(pid, s)| {
                    json!([
                        pid,
                        s.proto,
                        [s.local.0, s.local.1],
                        s.remote.as_ref().map(|r| json!([r.0, r.1])),
                        s.state,
                        s.pid
                    ])
                })
                .collect::<Vec<_>>()
        });
        let want = &case["parsed"];
        tally.check(
            &format!("{text:?}"),
            got.map(Value::from).unwrap_or(Value::Null),
            want.clone(),
        );
    }
    tally.done("nettop");
}

fn sock(proto: &str, local: (&str, u32), state: &str, pid: Option<i32>) -> Socket {
    Socket {
        proto: proto.into(),
        local: (local.0.into(), local.1),
        remote: None,
        state: state.into(),
        pid,
        queued: 0,
    }
}

#[test]
fn a_nettop_that_misses_a_known_listener_is_no_reading() {
    // tests/test_local_sockets.py::test_a_nettop_that_misses_a_known_listener_is_no_reading, on invented readings
    let mine = sock("tcp", ("127.0.0.1", 4000), "LISTEN", Some(10));
    let inspectable = (vec![mine.clone()], BTreeSet::from([10]));
    assert_eq!(merge(&inspectable, &[]), None);
    let hidden = sock("tcp", ("*", 8021), "LISTEN", Some(1));
    let listing = vec![(10, mine.clone()), (1, hidden.clone())];
    let every = merge(&inspectable, &listing).unwrap();
    assert!(every.contains(&mine));
    assert!(every.contains(&Socket {
        pid: None,
        ..hidden
    })); // root's listener: unnamed
    assert_eq!(merge(&(vec![], BTreeSet::new()), &listing), None); // nothing known: no reading
}

#[cfg(target_os = "macos")]
#[test]
fn live_a_listener_queues_until_accepted_and_the_accepted_end_belongs_to_its_process() {
    use std::net::{TcpListener, TcpStream};
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = server.local_addr().unwrap().port() as u32;
    let client = TcpStream::connect(("127.0.0.1", port as u16)).unwrap();
    let source = client.local_addr().unwrap();
    let me = std::process::id() as i32;
    let mine = || -> Vec<Socket> {
        let (all, pids) = inspectable().expect("libproc reads our own process");
        assert!(pids.contains(&me));
        all.into_iter()
            .filter(|s| {
                s.pid == Some(me)
                    && (s.local.1 == port || s.remote.as_ref().is_some_and(|r| r.1 == port))
            })
            .collect()
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !mine().iter().any(|s| s.state == "LISTEN" && s.queued == 1) {
        assert!(
            std::time::Instant::now() < deadline,
            "the queued connection never showed on its listener"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let remote = Some(("127.0.0.1".to_string(), source.port() as u32));
    assert!(!mine()
        .iter()
        .any(|s| s.local == ("127.0.0.1".into(), port) && s.remote == remote));
    let (accepted, _) = server.accept().unwrap();
    assert!(mine().contains(&Socket {
        proto: "tcp".into(),
        local: ("127.0.0.1".into(), port),
        remote,
        state: "ESTABLISHED".into(),
        pid: Some(me),
        queued: 0
    }));
    drop(accepted);
}

#[cfg(target_os = "macos")]
#[test]
fn live_a_process_group_lists_its_members() {
    let me = std::process::id() as i32;
    // SAFETY: reads our own process group id.
    let pgid = unsafe { libc::getpgrp() };
    assert!(group_members(pgid).contains(&me));
    assert!(executable_of(me).is_some());
}
