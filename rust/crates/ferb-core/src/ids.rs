//! Production and deterministic identifiers (ferb/core/ids.py).

use std::sync::atomic::{AtomicU64, Ordering};

use crate::interfaces::IdSource;

/// ids.py:8: `uuid.uuid4().hex`, 32 lowercase hex digits with the version and variant bits of a random UUID.
#[derive(Debug, Default, Clone, Copy)]
pub struct RandomIds;

impl IdSource for RandomIds {
    fn new_id(&self) -> String {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).expect("the system's random source");
        bytes[6] = bytes[6] & 0x0f | 0x40;
        bytes[8] = bytes[8] & 0x3f | 0x80;
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// ids.py:13: deterministic ids for tests and replays: `<prefix>-000001`, `<prefix>-000002`, ...
#[derive(Debug)]
pub struct SequentialIds {
    prefix: String,
    n: AtomicU64,
}

impl SequentialIds {
    pub fn new(prefix: &str) -> Self {
        SequentialIds {
            prefix: prefix.to_string(),
            n: AtomicU64::new(0),
        }
    }
}

impl Default for SequentialIds {
    fn default() -> Self {
        SequentialIds::new("id")
    }
}

impl IdSource for SequentialIds {
    fn new_id(&self) -> String {
        format!(
            "{}-{:06}",
            self.prefix,
            self.n.fetch_add(1, Ordering::SeqCst) + 1
        )
    }
}
