//! "Test" for an own relay: each step that can fail has its own stable code and says what to fix.
//! Steps: the address, DNS, the TCP port, the TLS certificate, then a real relay handshake with this device's
//! key and the access token (which tells "not an iroh relay", "wrong version" and "access denied" apart).

use crate::errors::{self, LinkError, LinkResult};
use crate::net::RelaySpec;
use iroh::SecretKey;
use iroh_relay::client::{ClientBuilder, ConnectError};
use serde::Serialize;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::time::timeout;

#[derive(Debug, Serialize)]
pub struct RelayOk {
    pub url: String,
    pub ms: u64,
    pub steps: Vec<String>,
    /// The relay's own version, from its /healthz, when it says.
    pub relay_version: Option<String>,
    /// Things that work but could be better, each with what to do.
    pub warnings: Vec<String>,
}

/// The iroh release this link is built on; a relay of another major version is refused by the Test.
pub const IROH_VERSION: &str = "1.4.1";

pub async fn test(spec: &RelaySpec, key: SecretKey) -> LinkResult<RelayOk> {
    let t = Instant::now();
    let tls = spec.client_tls()?;
    let pinned = spec.ca.is_some();
    let relay_url = spec.parsed()?;
    let url = url::Url::parse(relay_url.as_str())
        .map_err(|e| LinkError::new(errors::OWN_RELAY_URL, e.to_string()))?;
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_string();
    let https = url.scheme() == "https";
    let port = url.port().unwrap_or(if https { 443 } else { 80 });
    let mut steps = vec![];

    let addrs: Vec<_> = match timeout(
        Duration::from_secs(5),
        tokio::net::lookup_host((host.as_str(), port)),
    )
    .await
    {
        Ok(Ok(a)) => a.collect(),
        Ok(Err(e)) => return Err(dns(&host, &e.to_string())),
        Err(_) => return Err(dns(&host, "the lookup took more than 5 s")),
    };
    if addrs.is_empty() {
        return Err(dns(&host, "it has no address"));
    }
    steps.push(format!("name found: {host}"));

    let mut tcp = None;
    let mut why = String::new();
    for a in &addrs {
        match timeout(Duration::from_secs(5), tokio::net::TcpStream::connect(a)).await {
            Ok(Ok(s)) => {
                tcp = Some(s);
                break;
            }
            Ok(Err(e)) => why = e.to_string(),
            Err(_) => why = "no answer in 5 s".into(),
        }
    }
    let Some(tcp) = tcp else {
        return Err(LinkError::new(
            errors::OWN_RELAY_UNREACHABLE,
            format!("Nothing answers at {host} port {port} ({why})."),
        )
        .allow("Check that the relay is running and that the server's firewall allows this port"));
    };
    steps.push(format!("port {port} answers"));

    let health_req = format!("GET /healthz HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    let health = if https {
        let name = rustls::pki_types::ServerName::try_from(host.clone()).map_err(|_| {
            LinkError::new(
                errors::OWN_RELAY_URL,
                format!("\"{host}\" is not a valid host name."),
            )
        })?;
        let conn = rustls::ClientConnection::new(Arc::new(tls.clone()), name)
            .map_err(|e| LinkError::new(errors::OWN_RELAY_TLS, e.to_string()))?;
        match timeout(
            Duration::from_secs(8),
            tls_get(tcp, conn, health_req.as_bytes()),
        )
        .await
        {
            Ok(Ok(body)) => {
                steps.push("certificate is valid".into());
                body
            }
            Ok(Err(e)) => {
                return Err(LinkError::new(errors::OWN_RELAY_TLS, format!("The certificate of {host} was not accepted ({e})."))
                    .allow(if pinned { "Paste the whole relay link again: the certificate in it must be the one the relay's setup printed" } else { "The relay needs a valid certificate for exactly this name (the setup script gets one from Let's Encrypt; ports 80 and 443 must be open). A relay set up with --shared needs its whole relay link pasted, not just the address" }));
            }
            Err(_) => {
                return Err(LinkError::new(
                    errors::OWN_RELAY_TLS,
                    format!("{host} did not finish the TLS handshake in 8 s."),
                )
                .allow("Check that an iroh relay, not another service, listens on this port"));
            }
        }
    } else {
        timeout(
            Duration::from_secs(4),
            plain_get(tcp, health_req.as_bytes()),
        )
        .await
        .ok()
        .unwrap_or_default()
    };
    // the relay's own version, when its /healthz says: another major version cannot serve this FERB
    let relay_version = health
        .find('{')
        .and_then(|i| serde_json::from_str::<serde_json::Value>(&health[i..]).ok())
        .and_then(|v| v["version"].as_str().map(str::to_string));
    if let Some(v) = &relay_version {
        if v.split('.').next() != IROH_VERSION.split('.').next() {
            return Err(LinkError::new(errors::OWN_RELAY_VERSION, format!("The relay at {host} runs iroh-relay {v}; FERB is built on iroh {IROH_VERSION}."))
                .allow("Update the relay (sh setup.sh --update) or FERB, so both are on the same major version"));
        }
        steps.push(format!("relay version {v}"));
    }

    let mut b = ClientBuilder::new(relay_url.clone(), key, iroh::dns::DnsResolver::new())
        .tls_client_config(tls);
    if let Some(tok) = &spec.token {
        b = b.auth_token(tok.clone());
    }
    match timeout(Duration::from_secs(10), b.connect()).await {
        Ok(Ok(_client)) => steps.push("relay accepted this device".into()),
        Ok(Err(e)) => return Err(classify(&host, e)),
        Err(_) => {
            return Err(LinkError::new(
                errors::OWN_RELAY_NOT_RELAY,
                format!("{host} answered but did not complete the relay handshake in 10 s."),
            )
            .allow("Check that it runs iroh-relay"));
        }
    }
    // Does the relay also answer address discovery (UDP)? Without it a device that uses only this relay never
    // learns its public address, and a direct connection happens only on the same network.
    let mut warnings = vec![];
    // (no port mapping: a mapping at this Mac's gateway says nothing about the relay)
    let probe = crate::net::NetConfig {
        portmapper: false,
        ..crate::net::NetConfig::new(
            SecretKey::from_bytes(&[7u8; 32]),
            Some(spec.clone()),
            crate::net::Shared::Custom(vec![]),
        )
    };
    if let Ok(ep) = crate::net::bind(&probe).await {
        let seen = timeout(Duration::from_secs(5), async {
            loop {
                let f = crate::net::facts(&ep);
                if f.udp_v4 || f.udp_v6 {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .is_ok();
        ep.close().await;
        if seen {
            steps.push("address discovery answers (direct connections can be found)".into());
        } else {
            warnings.push(format!("The relay works, but its address discovery (UDP port 7842 on {host}) does not answer, so through this relay FERB can only connect directly on the same Wi-Fi. Open UDP 7842 to the relay (its setup turns discovery on; a firewall in front of the server may still block it)."));
        }
    }
    Ok(RelayOk {
        url: spec.url.clone(),
        ms: t.elapsed().as_millis() as u64,
        steps,
        relay_version,
        warnings,
    })
}

fn dns(host: &str, why: &str) -> LinkError {
    LinkError::new(
        errors::OWN_RELAY_DNS,
        format!("The name {host} could not be found ({why})."),
    )
    .allow("Check the spelling, and that the domain's DNS record points at the relay server")
}

fn classify(host: &str, e: ConnectError) -> LinkError {
    let text = format!("{e:#}");
    match &e {
        ConnectError::Handshake { .. } if text.contains("denied") => LinkError::new(
            errors::OWN_RELAY_ACCESS_DENIED,
            format!("The relay at {host} refused this device ({text})."),
        )
        .allow("Enter the relay's access token (the setup script printed it) in FERB's settings"),
        ConnectError::BadVersionHeader { .. } => LinkError::new(
            errors::OWN_RELAY_VERSION,
            format!("The relay at {host} speaks another relay version ({text})."),
        )
        .allow(
            "Update the relay (rerun the setup script) or FERB, so both use the same iroh release",
        ),
        ConnectError::Tls { .. } => LinkError::new(
            errors::OWN_RELAY_TLS,
            format!("The certificate of {host} was not accepted ({text})."),
        )
        .allow("The relay needs a valid certificate for exactly this name"),
        ConnectError::Dial { .. } => LinkError::new(
            errors::OWN_RELAY_UNREACHABLE,
            format!("{host} could not be reached ({text})."),
        )
        .allow("Check that the relay is running"),
        _ => LinkError::new(
            errors::OWN_RELAY_NOT_RELAY,
            format!("{host} is not an iroh relay, or not a compatible one ({text})."),
        )
        .allow("Check the address; it must be the relay itself, not a website on the same server"),
    }
}

async fn plain_get(mut tcp: tokio::net::TcpStream, request: &[u8]) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut out = Vec::new();
    if tcp.write_all(request).await.is_ok() {
        let _ = (&mut tcp).take(64 * 1024).read_to_end(&mut out).await;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// A TLS handshake over an open TCP stream, then one small request. A certificate problem is the error, in
/// rustls's own words; a failed request after a good handshake is just an empty answer.
async fn tls_get(
    mut tcp: tokio::net::TcpStream,
    mut conn: rustls::ClientConnection,
    request: &[u8],
) -> Result<String, String> {
    use std::io::{Read, Write};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = vec![0u8; 16 * 1024];
    let mut sent = false;
    let mut body = Vec::new();
    loop {
        while conn.wants_write() {
            let mut out = Vec::new();
            conn.write_tls(&mut out).map_err(|e| e.to_string())?;
            if tcp.write_all(&out).await.is_err() {
                return if conn.is_handshaking() {
                    Err("the server closed the connection during the handshake".into())
                } else {
                    Ok(String::from_utf8_lossy(&body).to_string())
                };
            }
        }
        if !conn.is_handshaking() && !sent {
            sent = true;
            let _ = conn.writer().write_all(request);
            continue;
        }
        let n = match tokio::time::timeout(Duration::from_secs(3), tcp.read(&mut buf)).await {
            Ok(Ok(n)) => n,
            Ok(Err(e)) if conn.is_handshaking() => return Err(e.to_string()),
            _ if conn.is_handshaking() => {
                return Err("the server went silent during the handshake".into());
            }
            _ => 0,
        };
        if n == 0 {
            return if conn.is_handshaking() {
                Err("the server closed the connection during the handshake".into())
            } else {
                Ok(String::from_utf8_lossy(&body).to_string())
            };
        }
        let mut slice = &buf[..n];
        while !slice.is_empty() {
            conn.read_tls(&mut slice).map_err(|e| e.to_string())?;
            if let Err(e) = conn.process_new_packets() {
                return if conn.is_handshaking() {
                    Err(e.to_string())
                } else {
                    Ok(String::from_utf8_lossy(&body).to_string())
                };
            }
        }
        let mut chunk = [0u8; 4096];
        while let Ok(n) = conn.reader().read(&mut chunk) {
            if n == 0 {
                return Ok(String::from_utf8_lossy(&body).to_string());
            }
            body.extend_from_slice(&chunk[..n]);
            if body.len() > 64 * 1024 {
                return Ok(String::from_utf8_lossy(&body).to_string());
            }
        }
    }
}
