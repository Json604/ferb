//! Frames on a QUIC stream: `[u32 big-endian length][u8 kind][body]`, length = body + 1. The length is checked
//! before anything is allocated; reads have a progress deadline, never a whole-frame one (CONTRACT.md §3).

use crate::errors::{self, LinkError, LinkResult};
use iroh::endpoint::{RecvStream, SendStream};
use serde::{Serialize, de::DeserializeOwned};
use std::time::Duration;
use tokio::time::timeout;

pub const K_HELLO: u8 = 1; // phone -> Mac, first frame of the first stream (JSON)
pub const K_WELCOME: u8 = 2; // Mac -> phone (JSON)
pub const K_REQ: u8 = 3; // phone -> Mac, first frame of a call stream (JSON)
pub const K_EVENT: u8 = 4; // Mac -> phone (JSON)
pub const K_RESULT: u8 = 5; // Mac -> phone, ends a call (JSON)
pub const K_ERROR: u8 = 6; // Mac -> phone, ends a call or the hello (JSON LinkError)
pub const K_CHUNK: u8 = 7; // bulk bytes: [u32 seq][bytes]
pub const K_ACK: u8 = 8; // bulk: [u32 seq] received

fn io(e: impl std::fmt::Display) -> LinkError {
    LinkError::new(errors::LINK_CLOSED, format!("The connection closed ({e})."))
}

pub async fn write_frame(tx: &mut SendStream, kind: u8, body: &[u8]) -> LinkResult<()> {
    let mut head = [0u8; 5];
    head[..4].copy_from_slice(&((body.len() + 1) as u32).to_be_bytes());
    head[4] = kind;
    tx.write_all(&head).await.map_err(io)?;
    tx.write_all(body).await.map_err(io)
}

pub async fn write_json<T: Serialize>(tx: &mut SendStream, kind: u8, v: &T) -> LinkResult<()> {
    let body =
        serde_json::to_vec(v).map_err(|e| LinkError::new(errors::BAD_REQUEST, e.to_string()))?;
    write_frame(tx, kind, &body).await
}

/// Reads one frame of at most `max` body bytes. `stall` bounds every read step, so a slow relay is fine and a
/// silent peer is not. `idle` true lets the stream sit silent before a frame starts (an event stream).
pub async fn read_frame(
    rx: &mut RecvStream,
    max: u64,
    stall: Duration,
    idle: bool,
) -> LinkResult<Option<(u8, Vec<u8>)>> {
    let stalled = || {
        LinkError::new(
            errors::STALLED,
            "The other side stopped sending in the middle of a message.",
        )
    };
    let mut head = [0u8; 5];
    let mut got = 0;
    while got < 5 {
        let step = rx.read(&mut head[got..]);
        let n = if idle && got == 0 {
            step.await
        } else {
            timeout(stall, step).await.map_err(|_| stalled())?
        };
        match n.map_err(io)? {
            Some(n) => got += n,
            None if got == 0 => return Ok(None),
            None => return Err(io("ended inside a frame header")),
        }
    }
    let len = u32::from_be_bytes(head[..4].try_into().unwrap()) as u64;
    if len < 1 || len - 1 > max {
        return Err(LinkError::new(
            errors::FRAME_TOO_LARGE,
            format!(
                "A message of {} was refused; the limit here is {}.",
                errors::human(len.saturating_sub(1)),
                errors::human(max)
            ),
        ));
    }
    let mut body = vec![0u8; (len - 1) as usize];
    let mut got = 0;
    while got < body.len() {
        match timeout(stall, rx.read(&mut body[got..]))
            .await
            .map_err(|_| stalled())?
            .map_err(io)?
        {
            Some(n) => got += n,
            None => return Err(io("ended inside a frame")),
        }
    }
    Ok(Some((head[4], body)))
}

pub fn parse<T: DeserializeOwned>(body: &[u8]) -> LinkResult<T> {
    serde_json::from_slice(body).map_err(|e| {
        LinkError::new(
            errors::BAD_REQUEST,
            format!("The message could not be read ({e})."),
        )
    })
}
