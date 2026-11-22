//! The WebSocket frames Codex's local socket speaks (RFC 6455, text frames only): the client's opening request, its
//! masked frames, and a reader for the server's frames. The daemon owns the socket; this is the byte format only.

/// The opening request (the socket is local: the host is a formality).
pub fn handshake(key_b64: &str) -> String {
    format!(
        "GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
Sec-WebSocket-Key: {key_b64}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    )
}

/// Whether the server's answer head switched protocols.
pub fn accepted(head: &str) -> bool {
    head.lines().next().is_some_and(|l| l.contains(" 101 "))
}

/// A random key for the opening request (16 bytes, base64).
pub fn new_key() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::fill(&mut b);
    b64(&b)
}

fn b64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            A[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            A[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Frame opcodes FERB handles.
pub const TEXT: u8 = 1;
pub const CLOSE: u8 = 8;
pub const PING: u8 = 9;
pub const PONG: u8 = 10;

/// One client frame (always masked, FIN set).
pub fn frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut mask = [0u8; 4];
    let _ = getrandom::fill(&mut mask);
    frame_with(opcode, payload, mask)
}

/// [`frame`] with a given mask (tests).
pub fn frame_with(opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut out = vec![0x80 | (opcode & 0x0f)];
    let n = payload.len();
    if n < 126 {
        out.push(0x80 | n as u8);
    } else if n < 65_536 {
        out.push(0x80 | 126);
        out.extend_from_slice(&(n as u16).to_be_bytes());
    } else {
        out.push(0x80 | 127);
        out.extend_from_slice(&(n as u64).to_be_bytes());
    }
    out.extend_from_slice(&mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    out
}

/// One whole message from the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Text(String),
    Ping(Vec<u8>),
    Close,
}

/// Reads the server's frames from bytes as they arrive; fragments are joined. A message larger than `max` bytes ends
/// the reader (the socket is closed by the caller).
#[derive(Default)]
pub struct Reader {
    buf: Vec<u8>,
    partial: Vec<u8>,
    pub max: usize,
}

impl Reader {
    pub fn new(max: usize) -> Reader {
        Reader {
            max,
            ..Reader::default()
        }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next whole message, None until one is complete; Err when the stream is broken or too large.
    pub fn next_message(&mut self) -> Result<Option<Message>, String> {
        loop {
            if self.buf.len() < 2 {
                return Ok(None);
            }
            let (b0, b1) = (self.buf[0], self.buf[1]);
            let masked = b1 & 0x80 != 0;
            let mut n = (b1 & 0x7f) as usize;
            let mut at = 2;
            if n == 126 {
                if self.buf.len() < 4 {
                    return Ok(None);
                }
                n = u16::from_be_bytes([self.buf[2], self.buf[3]]) as usize;
                at = 4;
            } else if n == 127 {
                if self.buf.len() < 10 {
                    return Ok(None);
                }
                let mut b = [0u8; 8];
                b.copy_from_slice(&self.buf[2..10]);
                n = u64::from_be_bytes(b) as usize;
                at = 10;
            }
            if n > self.max {
                return Err("a message from Codex was too large".into());
            }
            let mask_at = at;
            if masked {
                at += 4;
            }
            if self.buf.len() < at + n {
                return Ok(None);
            }
            let mut payload = self.buf[at..at + n].to_vec();
            if masked {
                let m = [
                    self.buf[mask_at],
                    self.buf[mask_at + 1],
                    self.buf[mask_at + 2],
                    self.buf[mask_at + 3],
                ];
                for (i, b) in payload.iter_mut().enumerate() {
                    *b ^= m[i % 4];
                }
            }
            self.buf.drain(..at + n);
            let fin = b0 & 0x80 != 0;
            match b0 & 0x0f {
                PING => return Ok(Some(Message::Ping(payload))),
                CLOSE => return Ok(Some(Message::Close)),
                PONG => continue,
                0 | TEXT | 2 => {
                    self.partial.extend_from_slice(&payload);
                    if self.partial.len() > self.max {
                        return Err("a message from Codex was too large".into());
                    }
                    if fin {
                        let all = std::mem::take(&mut self.partial);
                        return Ok(Some(Message::Text(
                            String::from_utf8_lossy(&all).into_owned(),
                        )));
                    }
                }
                _ => return Err("Codex sent a frame FERB doesn't know".into()),
            }
        }
    }
}
