//! The bridge's client for FERB's daemon: newline-delimited JSON-RPC 2.0 on the daemon's Unix socket, first
//! request `auth.hello` (ferb-rpc's server). The daemon answers role `phone` and `link` only for this program, by its
//! code signature (kb PHN-468, ferb-rpc `link_peer`). For a phone the bridge says role `phone`, names the device and
//! the connection (`session`), and passes the phone's unlock proof through untouched (`proof`, beside `params`):
//! the daemon holds the paired phones and the classes, verifies every proof and keeps the unlock window itself, so
//! nothing the bridge says about a phone is trusted (kb PHN-124).
//!
//! Role `link` is the helper's own: the paired phones (`link.devices`), their changes and the daemon's asks
//! (`link.events`), a pairing the owner says Yes to on the Mac (`link.pair_request`, `link.pair_wait`,
//! `link.pair_cancel`) and its answers to the asks (`link.answer`). Never on behalf of a phone.

use crate::errors::{self, LinkError, LinkResult};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{
        UnixStream,
        unix::{OwnedReadHalf, OwnedWriteHalf},
    },
    time::timeout,
};

/// server.py MAX_MESSAGE
pub const MAX_MESSAGE: usize = 1024 * 1024;
const UNAUTHORIZED: i64 = -32001;
const METHOD_NOT_FOUND: i64 = -32601;
/// The daemon's refusals for a phone, as JSON-RPC error codes (ferb-rpc `phone_gate::CODES`; a test compares the two),
/// with what would allow each.
pub const REFUSALS: &[(i64, &str, &str)] = &[
    (
        -32011,
        errors::LOCKED,
        "Unlock with your fingerprint, face or PIN",
    ),
    (
        -32012,
        errors::UNLOCK_REQUIRED,
        "Unlock when the phone asks; a phone paired without a screen lock answers this on the Mac",
    ),
    (-32013, errors::UNLOCK_INVALID, "Try again"),
    (-32014, errors::FRESH_UNLOCK, "Unlock when the phone asks"),
    (
        -32015,
        errors::PROTECTED_FOLDER,
        "Answer the question FERB just sent, then open it again",
    ),
    (-32016, errors::FILE_NOT_ALLOWED, "Open it on the Mac"),
    (-32017, errors::NOT_A_FILE, ""),
    (-32018, errors::NOT_FOUND, ""),
    (-32019, errors::QUESTION_NOT_WAITING, ""),
    (-32020, UNKNOWN_PHONE, ""),
    (
        -32021,
        errors::MAC_DISK_FULL,
        "Free some space on the Mac, or send a smaller file",
    ),
    (-32022, errors::NEEDS_THE_MAC, "Click it at the Mac"),
    (
        -32023,
        errors::DEVICE_LIMIT,
        "Remove one in FERB's phone list on the Mac, then pair again",
    ),
    (-32024, errors::FILE_CHANGED, "Fetch it again"),
    // PHN-828: the daemon's words say what to do (sign in with the same account; scan a new code)
    (-32025, errors::ACCOUNT_MISMATCH, ""),
    (-32026, errors::MAC_SIGNED_OUT, ""),
    (-32027, errors::ACCOUNT_UNCONFIRMED, ""),
    (-32600, errors::BAD_REQUEST, ""),
    // -32040..-32049 were the coding sessions' refusals, retired with the sessions on 2026-11-24 (HAND-717,
    // engine/ARCHIVE-sessions.md): never reuse them, an older phone still reads them as those refusals.
];

/// The daemon does not know this phone (it was restarted, or the phone was removed): never shown to a phone as is.
pub const UNKNOWN_PHONE: &str = "unknown_phone";

#[derive(Clone)]
pub struct Daemon {
    pub socket: PathBuf,
}

pub struct Conn {
    rx: BufReader<OwnedReadHalf>,
    tx: OwnedWriteHalf,
    next: u64,
}

fn down(why: impl std::fmt::Display) -> LinkError {
    LinkError::new(
        errors::FERB_NOT_RUNNING,
        format!("FERB on the Mac is not answering ({why})."),
    )
    .allow("Open FERB on the Mac")
}

/// Overwrites every string of a value that carried a hidden answer. Best effort: copies made by the allocator,
/// the kernel's socket buffers and the daemon's own handling are out of this function's reach.
pub fn wipe(v: &mut Value) {
    match v {
        Value::String(s) => unsafe { s.as_bytes_mut().fill(0) },
        Value::Array(a) => a.iter_mut().for_each(wipe),
        Value::Object(o) => o.values_mut().for_each(wipe),
        _ => {}
    }
}

fn rpc_error(method: &str, e: &Value) -> LinkError {
    let code = e.get("code").and_then(Value::as_i64).unwrap_or(0);
    let message = e
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown error")
        .to_string();
    if let Some((_, link_code, allow)) = REFUSALS.iter().find(|(c, _, _)| *c == code) {
        let message = if *link_code == errors::FILE_NOT_ALLOWED {
            format!("FERB does not show this to a phone: {message}.")
        } else {
            message
        };
        let e = LinkError::new(link_code, message);
        return if allow.is_empty() { e } else { e.allow(*allow) };
    }
    match code {
        UNAUTHORIZED | METHOD_NOT_FOUND => LinkError::new(
            errors::METHOD_NOT_ALLOWED,
            format!("FERB refused \"{method}\" for a phone ({message})."),
        )
        .allow("Do it in FERB on the Mac"),
        _ => LinkError::new(errors::FERB_ERROR, message),
    }
}

impl Daemon {
    async fn open(&self, hello: Value) -> LinkResult<Conn> {
        let stream = timeout(Duration::from_secs(3), UnixStream::connect(&self.socket))
            .await
            .map_err(|_| down("no answer in 3 s"))?
            .map_err(down)?;
        let (rx, tx) = stream.into_split();
        let mut c = Conn {
            rx: BufReader::new(rx),
            tx,
            next: 1,
        };
        c.call("auth.hello", hello, Duration::from_secs(5)).await?;
        Ok(c)
    }
    /// A connection as one phone's one link connection (`session` names it: its challenges and unlock end with it),
    /// on the path that connection takes now (`tier`: the daemon refuses files on the shared relay too, kb PHN-444).
    pub async fn connect(&self, device: &str, session: &str, tier: &str) -> LinkResult<Conn> {
        self.open(json!({"role": "phone", "device": device, "session": session, "tier": tier}))
            .await
    }
    /// One call on a connection of its own, with the phone's proof passed through.
    pub async fn call(
        &self,
        device: &str,
        session: &str,
        method: &str,
        params: Value,
        proof: Option<Value>,
        wait: Duration,
    ) -> LinkResult<Value> {
        self.connect(device, session, "")
            .await?
            .call_with(method, params, proof, false, wait)
            .await
    }
    /// A connection as the helper itself (role `link`).
    pub async fn link(&self) -> LinkResult<Conn> {
        self.open(json!({"role": "link"})).await
    }
    /// One call as the helper itself, on a connection of its own.
    pub async fn link_call(
        &self,
        method: &str,
        params: Value,
        wait: Duration,
    ) -> LinkResult<Value> {
        self.link().await?.call(method, params, wait).await
    }
}

impl Conn {
    async fn send(&mut self, method: &str, params: Value) -> LinkResult<u64> {
        self.send_then(method, params, None, false).await
    }
    async fn send_then(
        &mut self,
        method: &str,
        params: Value,
        proof: Option<Value>,
        sealed: bool,
    ) -> LinkResult<u64> {
        let id = self.next;
        self.next += 1;
        let mut msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if let Some(p) = proof {
            msg["proof"] = p;
        }
        let mut line = serde_json::to_vec(&msg).unwrap();
        if sealed {
            wipe(&mut msg);
        }
        if line.len() >= MAX_MESSAGE {
            return Err(LinkError::new(
                errors::FRAME_TOO_LARGE,
                "The request is larger than FERB's daemon accepts (1 MiB).",
            ));
        }
        line.push(b'\n');
        let sent = self.tx.write_all(&line).await.map_err(down);
        line.fill(0); // a request may carry a hidden answer: its bytes do not linger here
        sent?;
        Ok(id)
    }
    async fn read(&mut self) -> LinkResult<Value> {
        let mut line = Vec::new();
        let n = (&mut self.rx)
            .take(MAX_MESSAGE as u64 + 1)
            .read_until(b'\n', &mut line)
            .await
            .map_err(down)?;
        if n == 0 {
            return Err(down("it closed the connection"));
        }
        serde_json::from_slice(&line).map_err(|e| {
            LinkError::new(
                errors::FERB_ERROR,
                format!("FERB's reply could not be read ({e})"),
            )
        })
    }

    /// A call whose parameters hold a hidden answer: every copy this client made is overwritten once it is sent.
    pub async fn call_sealed(
        &mut self,
        method: &str,
        params: Value,
        proof: Option<Value>,
        wait: Duration,
    ) -> LinkResult<Value> {
        self.call_with(method, params, proof, true, wait).await
    }
    pub async fn call_with(
        &mut self,
        method: &str,
        params: Value,
        proof: Option<Value>,
        sealed: bool,
        wait: Duration,
    ) -> LinkResult<Value> {
        let id = self.send_then(method, params, proof, sealed).await?;
        self.reply(method, id, wait).await
    }

    pub async fn call(&mut self, method: &str, params: Value, wait: Duration) -> LinkResult<Value> {
        let id = self.send(method, params).await?;
        self.reply(method, id, wait).await
    }

    async fn reply(&mut self, method: &str, id: u64, wait: Duration) -> LinkResult<Value> {
        let reply = timeout(wait, async {
            loop {
                let msg = self.read().await?;
                if msg.get("id").and_then(Value::as_u64) == Some(id)
                    || msg.get("id").is_some_and(Value::is_null)
                {
                    return Ok::<_, LinkError>(msg);
                }
            }
        })
        .await
        .map_err(|_| {
            LinkError::new(
                errors::FERB_ERROR,
                format!("FERB did not answer \"{method}\" in {} s.", wait.as_secs()),
            )
        })??;
        match reply.get("error") {
            Some(e) => Err(rpc_error(method, e)),
            None => Ok(reply.get("result").cloned().unwrap_or(Value::Null)),
        }
    }

    /// A streaming method: every `<method>.event` notification goes to `on_event` (which may end the stream by
    /// returning false: the connection is then dropped, which is how the daemon learns its caller left).
    pub async fn stream<F, Fut>(
        &mut self,
        method: &str,
        params: Value,
        proof: Option<Value>,
        mut on_event: F,
    ) -> LinkResult<Value>
    where
        F: FnMut(Value) -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let id = self.send_then(method, params, proof, false).await?;
        loop {
            let msg = self.read().await?;
            if msg.get("id").and_then(Value::as_u64) == Some(id) {
                return match msg.get("error") {
                    Some(e) => Err(rpc_error(method, e)),
                    None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                };
            }
            if msg
                .get("method")
                .and_then(Value::as_str)
                .is_some_and(|m| m.strip_suffix(".event") == Some(method))
                && !on_event(msg.get("params").cloned().unwrap_or(Value::Null)).await
            {
                return Ok(Value::Null);
            }
        }
    }

    /// Starts a streaming method and returns its id: the caller then pulls events one by one with `next_event`
    /// (a file being read: the bridge takes the next piece only when the link has room for it).
    pub async fn start_stream(
        &mut self,
        method: &str,
        params: Value,
        proof: Option<Value>,
    ) -> LinkResult<u64> {
        self.send_then(method, params, proof, false).await
    }
    /// The next event of that stream; `Ok(None)` when it ended. An error ends it.
    pub async fn next_event(
        &mut self,
        method: &str,
        id: u64,
        wait: Duration,
    ) -> LinkResult<Option<Value>> {
        loop {
            let msg = timeout(wait, self.read()).await.map_err(|_| {
                LinkError::new(
                    errors::FERB_ERROR,
                    format!(
                        "FERB did not go on with \"{method}\" in {} s.",
                        wait.as_secs()
                    ),
                )
            })??;
            if msg.get("id").and_then(Value::as_u64) == Some(id) {
                return match msg.get("error") {
                    Some(e) => Err(rpc_error(method, e)),
                    None => Ok(None),
                };
            }
            if msg
                .get("method")
                .and_then(Value::as_str)
                .is_some_and(|m| m.strip_suffix(".event") == Some(method))
            {
                return Ok(Some(msg.get("params").cloned().unwrap_or(Value::Null)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The refusal codes the daemon's phone gate answers with (ferb-rpc `phone_gate::CODES`) are the ones this client
    /// reads, each to the same name: one table, a code added on one side fails here (the lab compared its Python
    /// stand-in's; S4 adds −32023 `device_limit`).
    // decision: PHN-468
    #[test]
    fn the_refusal_codes_are_the_daemon_s() {
        for (name, code) in ferb_rpc::phone_gate::CODES {
            let ours = REFUSALS.iter().find(|(c, _, _)| c == code);
            assert_eq!(ours.map(|(_, n, _)| *n), Some(*name), "{name} {code}");
        }
        assert_eq!(REFUSALS.len(), ferb_rpc::phone_gate::CODES.len());
        let e = rpc_error(
            "link.pair_request",
            &json!({"code": -32023, "message": "full"}),
        );
        assert_eq!(e.code, errors::DEVICE_LIMIT);
        assert!(e.allow.is_some());
    }
}
