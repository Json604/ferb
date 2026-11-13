//! The egress proxy against Python's (rust/fixtures/shell/proxy.json): address and host decisions, URL splits,
//! ClientHello server names, credentials, facts aggregation, the proxy environment, and whole exchanges: the same raw
//! invented requests sent to the Rust proxy must give the client the same bytes (the Date header line dropped; Python's
//! Server line is the one listed difference), the origin the same bytes, and the same facts and final record.
// decision: HAND-214, HAND-228, HAND-245, GATE-573

mod common;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{fixture, Tally};
use ferb_shell::proxy::*;
use serde_json::{json, Map, Value};

#[test]
fn hosts_urls_hellos_and_credentials_read_as_python_reads_them() {
    let fx = fixture("proxy.json");
    let mut tally = Tally::default();
    for c in fx["hosts"].as_array().unwrap() {
        let h = c["host"].as_str().unwrap();
        tally.check(
            &format!("literal {h:?}"),
            literal(h),
            c["literal"].as_bool().unwrap(),
        );
        tally.check(
            &format!("this_mac {h:?}"),
            this_mac(h),
            c["this_mac"].as_bool().unwrap(),
        );
        tally.check(
            &format!("local_host {h:?}"),
            local_host(h),
            c["local_host"].as_bool().unwrap(),
        );
        let got: Vec<Value> = local_addresses(h, 8080)
            .into_iter()
            .map(|(v6, host, port)| {
                // Python's localhost route names (host, port, flowinfo, scope) for ::1; a literal is (host, port)
                if v6 && h.trim_end_matches('.').eq_ignore_ascii_case("localhost") {
                    json!([true, [host, port, 0, 0]])
                } else {
                    json!([v6, [host, port]])
                }
            })
            .collect();
        tally.check(
            &format!("local_addresses {h:?}"),
            Value::from(got),
            c["local_addresses"].clone(),
        );
    }
    for pair in fx["connect"].as_array().unwrap() {
        let t = pair[0].as_str().unwrap();
        let got = match ferb_shell::pynet::urlsplit(t) {
            None => json!({"error": "ValueError"}),
            Some(s) => match s.port() {
                Ok(p) => json!({"host": s.hostname, "port": p}),
                Err(_) => json!({"error": "ValueError"}),
            },
        };
        tally.check(&format!("connect {t:?}"), got, pair[1].clone());
    }
    for pair in fx["forward"].as_array().unwrap() {
        let u = pair[0].as_str().unwrap();
        let got = match ferb_shell::pynet::urlsplit(u) {
            None => json!({"error": "ValueError"}),
            Some(s) => match s.port() {
                Ok(p) => {
                    json!({"scheme": s.scheme, "netloc": s.netloc, "path": s.path, "query": s.query,
                                "host": s.hostname, "port": p})
                }
                Err(_) => json!({"error": "ValueError"}),
            },
        };
        tally.check(&format!("forward {u:?}"), got, pair[1].clone());
    }
    for h in fx["hellos"].as_array().unwrap() {
        let data = hex::decode(h["bytes"].as_str().unwrap()).unwrap();
        for feed in h["feeds"].as_array().unwrap() {
            let step = feed[0].as_u64().unwrap() as usize;
            let mut reader = HelloReader::new();
            let mut got = None;
            for chunk in data.chunks(step.max(1)) {
                got = reader.feed(chunk);
                if got.is_some() {
                    break;
                }
            }
            tally.check(
                &format!("hello {} step {step}", h["name"]),
                got.map(Value::from).unwrap_or(Value::Null),
                feed[1].clone(),
            );
        }
    }
    for pair in fx["sni_bodies"].as_array().unwrap() {
        let body = hex::decode(pair[0].as_str().unwrap()).unwrap();
        tally.check(
            "sni body",
            sni_of(&body),
            pair[1].as_str().unwrap().to_string(),
        );
    }
    let tokens: HashMap<String, String> = fx["who"]["tokens"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
        .collect();
    for pair in fx["who"]["cases"].as_array().unwrap() {
        let v = pair[0].as_str().unwrap();
        tally.check(
            &format!("who {v:?}"),
            who(v, &tokens),
            pair[1].as_str().map(str::to_string),
        );
    }
    let c = &fx["constants"];
    tally.check("ENDED", ENDED, c["ENDED"].as_str().unwrap());
    tally.check("OWN", OWN, c["OWN"].as_str().unwrap());
    tally.check(
        "DEST_REFUSED_NO",
        DEST_REFUSED_NO,
        c["DEST_REFUSED_NO"].as_str().unwrap(),
    );
    tally.check(
        "DEST_SNI_NONE",
        DEST_SNI_NONE,
        c["DEST_SNI_NONE"].as_str().unwrap(),
    );
    tally.check("SNI_MAX", SNI_MAX as u64, c["SNI_MAX"].as_u64().unwrap());
    tally.check(
        "UNAUTH_TIMEOUT",
        UNAUTH_TIMEOUT,
        c["UNAUTH_TIMEOUT"].as_f64().unwrap(),
    );
    tally.check(
        "MAX_UNAUTH",
        MAX_UNAUTH as u64,
        c["MAX_UNAUTH"].as_u64().unwrap(),
    );
    tally.check("MAX_HEAD", MAX_HEAD as u64, c["MAX_HEAD"].as_u64().unwrap());
    let mut hop: Vec<String> = HOP.iter().map(|s| s.to_string()).collect();
    hop.sort();
    tally.check("HOP", hop, common::strs(&c["HOP"]));
    tally.done("pure");
}

#[test]
fn facts_aggregate_and_the_environment_is_pythons() {
    let fx = fixture("proxy.json");
    let mut tally = Tally::default();
    let mut cfg = ProxyConfig::new(Arc::new(|_, _, _| (true, String::new())));
    cfg.excluded = Arc::new(|h: &str| h.ends_with(".hidden"));
    let p = EgressProxy::new(cfg).unwrap();
    p.credentials("p-a");
    p.credentials("p-b");
    let peer = |a: &str, e: &str| ferb_shell::local::LocalPeer {
        address: a.into(),
        executable: e.into(),
    };
    p.opened("p-a", "b.test", 443, None);
    p.opened("p-a", "a.test", 80, None);
    p.opened("p-a", "a.test", 80, None);
    p.opened("p-a", "x.hidden", 443, None);
    p.opened("p-a", "y.hidden", 443, None);
    p.opened(
        "p-a",
        "localhost",
        5432,
        Some(&peer("127.0.0.1", "/bin/pg")),
    );
    p.opened("p-a", "localhost", 5432, Some(&peer("::1", "/bin/pg")));
    p.opened("p-a", "z.hidden", 1, Some(&peer("127.0.0.1", "/bin/z")));
    p.count("p-a", "a.test", 80, true, 100);
    p.count("p-a", "a.test", 80, false, 7);
    p.count("p-a", "x.hidden", 443, true, 5);
    p.count("p-a", "y.hidden", 443, true, 6);
    p.count("p-a", "nope.test", 1, true, 6);
    p.note("p-a", "c.test", 443, "why one");
    p.note("p-a", "c.test", 443, "why one");
    p.note("p-a", "q.hidden", 443, "why two");
    p.note("p-a", "r.hidden", 443, "why two");
    p.record_sni("p-a", "sni.test", 443);
    p.record_sni("p-a", "b.test", 443);
    tally.check(
        "facts p-a",
        Value::Object(p.facts("p-a")),
        fx["facts"]["p-a"].clone(),
    );
    tally.check(
        "facts none",
        Value::Object(p.facts("p-none")),
        fx["facts"]["none"].clone(),
    );
    let token = p.credentials("p-a");
    let env: Map<String, Value> = p
        .env("p-a")
        .into_iter()
        .map(|(k, v)| {
            (
                k,
                json!(v
                    .replace(&token, "@TOKEN@")
                    .replace(&p.port().to_string(), "@PORT@")),
            )
        })
        .collect();
    tally.check("env", Value::Object(env), fx["env"].clone());
    tally.check("token length", token.len(), 32);
    p.close();
    tally.done("facts/env");
}

// ---------- whole exchanges ----------

/// One raw origin connection: records what it got, then answers with canned bytes (make.py's Origin).
struct Origin {
    port: u16,
    got: Arc<Mutex<Option<Vec<u8>>>>,
    stop: Arc<Mutex<bool>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn complete(buf: &[u8]) -> bool {
    let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        return false;
    };
    let head = String::from_utf8_lossy(&buf[..i]).to_ascii_lowercase();
    let body = &buf[i + 4..];
    for line in head.split("\r\n") {
        if let Some(v) = line.strip_prefix("content-length:") {
            if let Ok(n) = v.trim().parse::<usize>() {
                return body.len() >= n;
            }
        }
    }
    if head
        .split("\r\n")
        .any(|l| l.starts_with("transfer-encoding:") && l.contains("chunked"))
    {
        return body.ends_with(b"0\r\n\r\n");
    }
    true
}

impl Origin {
    fn new(mode: &str, reply: Vec<u8>) -> Origin {
        let srv = TcpListener::bind("127.0.0.1:0").unwrap();
        srv.set_nonblocking(true).unwrap();
        let port = srv.local_addr().unwrap().port();
        let got = Arc::new(Mutex::new(None));
        let stop = Arc::new(Mutex::new(false));
        let (g, s, mode) = (got.clone(), stop.clone(), mode.to_string());
        let thread = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(4);
            let mut c = loop {
                match srv.accept() {
                    Ok((c, _)) => break c,
                    Err(_) => {
                        if *s.lock().unwrap() || std::time::Instant::now() > deadline {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
            };
            c.set_nonblocking(false).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
            let mut reply = reply;
            let mut buf = Vec::new();
            if mode == "greet" {
                let _ = c.write_all(&reply);
                reply.clear();
            }
            let mut chunk = vec![0u8; 65536];
            loop {
                if mode == "http" && complete(&buf) {
                    break;
                }
                match c.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            }
            *g.lock().unwrap() = Some(buf);
            if !reply.is_empty() {
                let _ = c.write_all(&reply);
            }
            let _ = c.shutdown(Shutdown::Write);
            std::thread::sleep(Duration::from_millis(50));
        });
        Origin {
            port,
            got,
            stop,
            thread: Some(thread),
        }
    }

    fn close(mut self) -> Option<Vec<u8>> {
        *self.stop.lock().unwrap() = true;
        let _ = self.thread.take().unwrap().join();
        self.got.lock().unwrap().clone()
    }
}

fn allow(_proc: &str, host: &str, _port: u16) -> (bool, String) {
    if host.ends_with(".refused.test") {
        return (false, "this destination needs the owner's approval".into());
    }
    if host == "no.test" {
        return (false, DEST_REFUSED_NO.into());
    }
    if host == "blank.test" {
        return (false, String::new());
    }
    (true, String::new())
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Object(o) if o.contains_key("head") => format!(
            "{}{}{}",
            o["head"].as_str().unwrap(),
            o["repeat"]
                .as_str()
                .unwrap()
                .repeat(o["count"].as_u64().unwrap() as usize),
            o["tail"].as_str().unwrap()
        ),
        Value::Object(o) => o["repeat"]
            .as_str()
            .unwrap()
            .repeat(o["count"].as_u64().unwrap() as usize),
        _ => panic!("text {v}"),
    }
}

fn latin1_bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u32 as u8).collect()
}

fn latin1_text(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

/// The proxy's own Date lines dropped (Python's Server lines were dropped by make.py).
fn normalize(raw: &[u8]) -> String {
    let t = latin1_text(raw);
    let mut out = String::new();
    let mut rest = t.as_str();
    while let Some(i) = rest.find("Date: ") {
        let line_end = rest[i..].find("\r\n").map(|e| i + e + 2);
        let at_line_start = i == 0 || rest[..i].ends_with('\n');
        match line_end {
            Some(end) if at_line_start => {
                out.push_str(&rest[..i]);
                rest = &rest[end..];
            }
            _ => {
                out.push_str(&rest[..i + 6]);
                rest = &rest[i + 6..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[test]
fn every_exchange_gives_the_client_and_the_origin_pythons_bytes() {
    let fx = fixture("proxy.json");
    let mut tally = Tally::default();
    let closed_port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    for sc in fx["wire"].as_array().unwrap() {
        let name = sc["name"].as_str().unwrap();
        let reply = latin1_bytes(&text(&sc["reply"]));
        let origin = Origin::new(sc["mode"].as_str().unwrap(), reply);
        let origin_port = origin.port;
        let reports: Arc<Mutex<Vec<Map<String, Value>>>> = Arc::default();
        let mut cfg = ProxyConfig::new(Arc::new(allow));
        cfg.resolve = Arc::new(
            move |host: &str, _port: u16| -> Result<Vec<SocketAddr>, Unreachable> {
                if host == "unresolvable.test" {
                    return Err(Unreachable("couldn't resolve the name (gaierror)".into()));
                }
                let p = if host == "closed.test" {
                    closed_port
                } else {
                    origin_port
                };
                Ok(vec![SocketAddr::from(([127, 0, 0, 1], p))])
            },
        );
        cfg.excluded = Arc::new(|h: &str| h.ends_with(".hidden"));
        if sc["sni"].as_bool().unwrap() {
            cfg.inspect_sni = Arc::new(|_, _, _| true);
        }
        let r = reports.clone();
        cfg.on_report = Some(Arc::new(move |_proc: &str, rec: Map<String, Value>| {
            r.lock().unwrap().push(rec)
        }));
        let proxy = EgressProxy::new(cfg).unwrap();
        let token = proxy.credentials("p-1");
        let auth = format!("Basic {}", b64encode_std(format!("p-1:{token}").as_bytes()));
        let mut raw = latin1_bytes(&text(&sc["request"]).replace("@AUTH@", &auth));
        raw.extend(hex::decode(sc["after"].as_str().unwrap()).unwrap());
        let mut c = TcpStream::connect(("127.0.0.1", proxy.port())).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(&raw).unwrap();
        if sc["shutdown"].as_bool().unwrap() {
            std::thread::sleep(Duration::from_millis(200));
            c.shutdown(Shutdown::Write).unwrap();
        }
        let mut got = Vec::new();
        let _ = c.read_to_end(&mut got);
        drop(c);
        std::thread::sleep(Duration::from_millis(50));
        let origin_got = origin.close();
        let facts = proxy.facts("p-1");
        reports.lock().unwrap().clear();
        proxy.forget("p-1");
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while reports.lock().unwrap().is_empty()
            && proxy.pending("p-1")
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        let report = reports.lock().unwrap().first().cloned();
        proxy.close();
        tally.check(
            &format!("{name}: client"),
            normalize(&got),
            sc["client_got"].as_str().unwrap().to_string(),
        );
        tally.check(
            &format!("{name}: origin"),
            origin_got.map(|o| latin1_text(&o)),
            sc["origin_got"].as_str().map(str::to_string),
        );
        tally.check(
            &format!("{name}: facts"),
            Value::Object(facts),
            sc["facts"].clone(),
        );
        tally.check(
            &format!("{name}: report"),
            report.map(Value::Object).unwrap_or(Value::Null),
            sc["report"].clone(),
        );
    }
    tally.done("wire");
}

// ---------- behaviour Python's tests pin, on short timeouts ----------

fn auth_header(p: &EgressProxy, proc: &str) -> String {
    format!(
        "Proxy-Authorization: Basic {}\r\n",
        b64encode_std(format!("{proc}:{}", p.credentials(proc)).as_bytes())
    )
}

#[test]
fn an_unauthenticated_connection_is_closed_at_its_deadline_and_a_flood_never_locks_out_a_client() {
    let mut cfg = ProxyConfig::new(Arc::new(|_, _, _| (true, String::new())));
    cfg.unauth_timeout = Duration::from_millis(300);
    cfg.max_unauth = 4;
    let p = EgressProxy::new(cfg).unwrap();
    // a client dripping bytes is still closed at the absolute deadline
    let mut slow = TcpStream::connect(("127.0.0.1", p.port())).unwrap();
    slow.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    slow.write_all(b"GET http://x.test/ HTTP/1.1\r\n").unwrap();
    let start = std::time::Instant::now();
    let mut buf = [0u8; 16];
    assert_eq!(slow.read(&mut buf).unwrap_or(0), 0);
    assert!(start.elapsed() < Duration::from_secs(2));
    // a flood past the cap closes the oldest; a new client still gets its answer
    let flood: Vec<TcpStream> = (0..8)
        .map(|_| TcpStream::connect(("127.0.0.1", p.port())).unwrap())
        .collect();
    std::thread::sleep(Duration::from_millis(50));
    let mut c = TcpStream::connect(("127.0.0.1", p.port())).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    c.write_all(b"GET http://x.test/ HTTP/1.1\r\n\r\n").unwrap();
    let mut got = Vec::new();
    let _ = c.read_to_end(&mut got);
    assert!(
        got.starts_with(b"HTTP/1.1 407 "),
        "{}",
        String::from_utf8_lossy(&got)
    );
    drop(flood);
    p.close();
}

#[test]
fn a_forgotten_credential_opens_nothing_and_tokens_leave_the_redactor() {
    let redactor = Arc::new(ferb_sanitize::Redactor::new(&[], &[]));
    let mut cfg = ProxyConfig::new(Arc::new(|_, _, _| (true, String::new())));
    cfg.redactor = Some(redactor.clone());
    let p = EgressProxy::new(cfg).unwrap();
    let header = auth_header(&p, "p-3");
    let token = p.credentials("p-3");
    assert_eq!(
        redactor.text(&format!("x {token} y")),
        format!("x {} y", ferb_sanitize::REDACTED)
    );
    p.forget("p-3");
    assert!(redactor.text(&format!("x {token} y")).contains(&token));
    let mut c = TcpStream::connect(("127.0.0.1", p.port())).unwrap();
    c.write_all(format!("GET http://x.test/ HTTP/1.1\r\n{header}\r\n").as_bytes())
        .unwrap();
    let mut got = Vec::new();
    let _ = c.read_to_end(&mut got);
    assert!(got.starts_with(b"HTTP/1.1 407 "));
    assert!(
        format!("{p:?}").find(&token).is_none(),
        "a token never shows in Debug"
    );
    p.close();
}

#[test]
fn bytes_sent_after_forget_are_counted_and_reported_once_the_tunnel_closes() {
    let origin = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = origin.local_addr().unwrap().port();
    let reports: Arc<Mutex<Vec<Map<String, Value>>>> = Arc::default();
    let r = reports.clone();
    let mut cfg = ProxyConfig::new(Arc::new(|_, _, _| (true, String::new())));
    cfg.resolve = Arc::new(move |_, _| Ok(vec![SocketAddr::from(([127, 0, 0, 1], port))]));
    cfg.on_report = Some(Arc::new(move |_, rec| r.lock().unwrap().push(rec)));
    let p = EgressProxy::new(cfg).unwrap();
    let mut c = TcpStream::connect(("127.0.0.1", p.port())).unwrap();
    c.write_all(
        format!(
            "CONNECT svc.test:9 HTTP/1.1\r\n{}\r\n",
            auth_header(&p, "p-t")
        )
        .as_bytes(),
    )
    .unwrap();
    let (mut server, _) = origin.accept().unwrap();
    let mut head = [0u8; 39];
    c.read_exact(&mut head).unwrap();
    p.forget("p-t"); // the command ended; a setsid child keeps the tunnel
    server.write_all(b"late bytes").unwrap();
    let mut late = [0u8; 10];
    c.read_exact(&mut late).unwrap();
    assert!(reports.lock().unwrap().is_empty() && p.pending("p-t"));
    drop(c);
    drop(server);
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while reports.lock().unwrap().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "the record was never reported"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let rec = reports.lock().unwrap()[0].clone();
    assert_eq!(rec["destinations"][0]["bytes_in"], json!(10));
    p.close();
}

#[test]
fn a_record_still_open_at_shutdown_is_reported_and_close_is_final() {
    let reports: Arc<Mutex<Vec<Map<String, Value>>>> = Arc::default();
    let r = reports.clone();
    let mut cfg = ProxyConfig::new(Arc::new(|_, _, _| (true, String::new())));
    cfg.on_report = Some(Arc::new(move |_, rec| r.lock().unwrap().push(rec)));
    let p = EgressProxy::new(cfg).unwrap();
    p.credentials("p-x");
    p.opened("p-x", "a.test", 443, None);
    let probe = p.probe();
    assert_eq!(probe["alive"], json!(true));
    assert_eq!(probe["restarted"], json!(false));
    p.close();
    let rec = reports.lock().unwrap()[0].clone();
    assert_eq!(rec["still_open"], json!(true));
    assert_eq!(p.probe()["alive"], json!(false));
    assert!(!p.ensure_serving());
}

#[test]
fn http_date_is_email_formatdate() {
    assert_eq!(http_date(0), "Thu, 01 Jan 1970 00:00:00 GMT");
    assert_eq!(http_date(1_794_528_000), "Fri, 13 Nov 2026 00:00:00 GMT");
}
