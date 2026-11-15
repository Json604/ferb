//! Newline framing on one connection, with the size cap and the "has the caller left?" looks the server needs.
//!
//! Port of how `ferb/daemon/server.py` uses asyncio's `StreamReader(limit=MAX_MESSAGE)`: `readline()`
//! (server.py:229-234), `read(1)` as the watch of a stream's or an MCP shim call's caller (server.py:385, 420),
//! and `at_eof()` for calls that end when their caller leaves (server.py:363).

use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::watch;

use crate::wire::MAX_MESSAGE;

/// Where the server is: `ready` false while the daemon starts (DMN-510), `closing` once `Server::close` ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Phase {
    pub ready: bool,
    pub closing: bool,
}

/// Resolves when the server closes (or is gone).
pub(crate) async fn closing(phase: &mut watch::Receiver<Phase>) {
    let _ = phase.wait_for(|p| p.closing).await;
}

/// Why no line came.
#[derive(Debug)]
pub enum FrameError {
    /// More than `MAX_MESSAGE` bytes without a newline (asyncio's `LimitOverrunError`, raised by `readline` as
    /// `ValueError`).
    TooLarge,
    Io(io::Error),
}

/// What ended the wait for a caller to leave: asyncio's `reader.read(1)` returning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gone {
    /// The caller closed its side (or the server is closing, or the read failed).
    Closed,
    /// The caller sent something instead. As with `read(1)`, its first byte is taken off the input.
    Sent,
}

/// The reading side of a connection.
pub struct FrameReader {
    half: OwnedReadHalf,
    buffer: Vec<u8>,
    scanned: usize, // bytes of `buffer` already known to hold no newline
    eof: bool,
    phase: watch::Receiver<Phase>,
}

impl FrameReader {
    pub(crate) fn new(half: OwnedReadHalf, phase: watch::Receiver<Phase>) -> Self {
        FrameReader {
            half,
            buffer: Vec::new(),
            scanned: 0,
            eof: false,
            phase,
        }
    }

    /// Reads once more from the socket into the buffer; 0 at the end of input. The server closing reads as the
    /// end of input (`Server.close` closes every connection's transport, server.py:215-216). Cancel safe: bytes
    /// are appended in the same step that reads them.
    async fn fill(&mut self) -> io::Result<usize> {
        if self.eof {
            return Ok(0);
        }
        let mut chunk = [0u8; 16 * 1024];
        let read = tokio::select! {
            biased;
            _ = closing(&mut self.phase) => Ok(0),
            read = self.half.read(&mut chunk) => read,
        };
        match read {
            Ok(0) | Err(_) => self.eof = true,
            Ok(n) => self.buffer.extend_from_slice(&chunk[..n]),
        }
        read
    }

    /// The next line with its newline; at the end of input the unfinished last line, then `None`
    /// (`StreamReader.readline`). A line whose content is longer than `MAX_MESSAGE`, or that many bytes with no
    /// newline yet, is `TooLarge` (`readuntil`'s two `LimitOverrunError`s).
    pub async fn read_line(&mut self) -> Result<Option<Vec<u8>>, FrameError> {
        loop {
            if let Some(at) = self.buffer[self.scanned..].iter().position(|b| *b == b'\n') {
                let at = self.scanned + at;
                if at > MAX_MESSAGE {
                    return Err(FrameError::TooLarge);
                }
                let rest = self.buffer.split_off(at + 1);
                let line = std::mem::replace(&mut self.buffer, rest);
                self.scanned = 0;
                return Ok(Some(line));
            }
            self.scanned = self.buffer.len();
            if self.buffer.len() > MAX_MESSAGE {
                return Err(FrameError::TooLarge);
            }
            if self.eof {
                self.scanned = 0;
                let last = std::mem::take(&mut self.buffer);
                return Ok(if last.is_empty() { None } else { Some(last) });
            }
            self.fill().await.map_err(FrameError::Io)?;
        }
    }

    /// Waits like `await reader.read(1)`: until the caller closes or sends. Something already waiting in the
    /// buffer counts as sent at once.
    pub async fn gone(&mut self) -> Gone {
        if self.buffer.is_empty() {
            let _ = self.fill().await;
        }
        if self.buffer.is_empty() {
            return Gone::Closed;
        }
        self.buffer.remove(0);
        self.scanned = self.scanned.saturating_sub(1);
        Gone::Sent
    }

    /// Resolves once `reader.at_eof()` would be true: the caller closed and nothing it sent is left unread.
    /// What a caller sends meanwhile stays in the buffer for the next `read_line`; a caller that sent something
    /// and then left is never "at the end" (server.py:355-356, 363). asyncio stops reading from the socket at
    /// twice the limit; so does this.
    pub async fn at_eof(&mut self) {
        loop {
            if self.eof && self.buffer.is_empty() {
                return;
            }
            if self.eof || self.buffer.len() > 2 * MAX_MESSAGE {
                return std::future::pending().await;
            }
            let _ = self.fill().await;
        }
    }

    /// The reading half and what was read past the last line, for a channel that takes the connection over.
    pub fn into_parts(self) -> (OwnedReadHalf, Vec<u8>) {
        (self.half, self.buffer)
    }
}

/// The writing side of a connection.
pub struct FrameWriter {
    half: OwnedWriteHalf,
    phase: watch::Receiver<Phase>,
}

impl FrameWriter {
    pub(crate) fn new(half: OwnedWriteHalf, phase: watch::Receiver<Phase>) -> Self {
        FrameWriter { half, phase }
    }

    /// Writes one whole line (`writer.write(line); await writer.drain()`, server.py:442-444). Once the server
    /// is closing nothing more is written: its connections are closed.
    pub async fn send(&mut self, line: &[u8]) -> io::Result<()> {
        if self.phase.borrow().closing {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        tokio::select! {
            biased;
            _ = closing(&mut self.phase) => Err(io::ErrorKind::BrokenPipe.into()),
            written = self.half.write_all(line) => written,
        }
    }

    /// Ends the output: the caller reads the end of input.
    pub async fn shutdown(&mut self) {
        let _ = self.half.shutdown().await;
    }

    pub fn into_half(self) -> OwnedWriteHalf {
        self.half
    }
}
