//! Shared by the tests: the chat-entries fixtures and a seeded generator (no real randomness, TEST-508).
#![allow(dead_code)]

use std::path::PathBuf;

use ferb_mobile::entry::ChatEntry;
use serde_json::Value;

pub fn folder() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/chat-entries")
}

pub fn read(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(folder().join(name)).unwrap()).unwrap()
}

/// The scenarios of the three files, in the golden's order: (file, name, entries).
pub fn scenarios() -> Vec<(String, String, Vec<ChatEntry>)> {
    let mut out = Vec::new();
    for file in ["scenarios.json", "engine_chat.json", "engine_more.json"] {
        for s in read(file)["scenarios"].as_array().unwrap() {
            out.push((
                file.to_string(),
                s["name"].as_str().unwrap().to_string(),
                ChatEntry::list(&s["entries"]),
            ));
        }
    }
    out
}

/// Each conversation's entries of a scenario, by conversation id.
pub fn conversations(entries: &[ChatEntry]) -> Vec<(String, Vec<ChatEntry>)> {
    let mut ids: Vec<String> = entries.iter().map(|e| e.conversation_id.clone()).collect();
    ids.sort();
    ids.dedup();
    ids.into_iter()
        .map(|c| {
            let mine = entries
                .iter()
                .filter(|e| e.conversation_id == c)
                .cloned()
                .collect();
            (c, mine)
        })
        .collect()
}

/// xorshift64*: the same numbers every run.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = self.below(i + 1);
            v.swap(i, j);
        }
    }
}

pub mod mac;
pub mod phone;

/// The single-conversation scenario with the most entries (a long chat to page through).
pub fn longest() -> (String, Vec<ChatEntry>) {
    scenarios()
        .into_iter()
        .filter(|(_, _, e)| conversations(e).len() == 1)
        .max_by_key(|(_, _, e)| e.len())
        .map(|(_, n, e)| (n, e))
        .unwrap()
}
