//! A throwaway iroh relay on 127.0.0.1 for local testing (plain http, like `iroh-relay --dev`), with the two
//! things a real own relay has that matter to the link: an access token and an optional rate limit.
//!   dev-relay [--port 3340] [--token TOKEN] [--limit BYTES_PER_S]
use iroh_relay::server::{
    Access, AccessControl, ClientRateLimit, ClientRequest, Limits, RelayConfig, Server,
    ServerConfig,
};
use std::{net::Ipv4Addr, num::NonZeroU32, sync::Arc};

#[derive(Debug)]
struct Token(String);
impl AccessControl for Token {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        match request.auth_token() {
            Some(t) if t == self.0 => Access::Allow,
            _ => Access::Deny { reason: None },
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let port: u16 = arg("--port").and_then(|p| p.parse().ok()).unwrap_or(3340);
    let mut relay = RelayConfig::new((Ipv4Addr::LOCALHOST, port));
    if let Some(t) = arg("--token") {
        relay.access = Arc::new(Token(t));
    }
    if let Some(rate) = arg("--limit")
        .and_then(|r| r.parse::<u32>().ok())
        .and_then(NonZeroU32::new)
    {
        let mut l = Limits::default();
        let mut rx = ClientRateLimit::new(rate);
        rx.max_burst_bytes = NonZeroU32::new(rate.get() * 2);
        l.client_rx = Some(rx);
        relay.limits = l;
    }
    let mut config = ServerConfig::default();
    config.relay = Some(relay);
    let server = Server::spawn(config).await?;
    println!("dev relay: http://localhost:{port}  (Ctrl-C stops it)");
    tokio::signal::ctrl_c().await?;
    server.shutdown().await?;
    Ok(())
}
