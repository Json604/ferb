//! ORCH-720 (ORCH-719 step 1): the cached prefix, block by block. A provider reuses a request's start only while it
//! is the same bytes as before; one changed block anywhere in it makes everything after it a cache write. A short
//! hash per block says which block changed, so a guard (or a trace) names it instead of reporting "the cache broke".

use sha2::{Digest, Sha256};

/// The first 12 hex digits of each block's SHA-256, in order.
pub fn block_hashes(blocks: &[String]) -> Vec<String> {
    blocks
        .iter()
        .map(|b| hex::encode(Sha256::digest(b.as_bytes()))[..12].to_string())
        .collect()
}

/// Where a later request stopped repeating an earlier one's cached prefix: the index of the first of the earlier
/// request's `cached` leading blocks that the later request does not carry with the same bytes at the same place, or
/// None when the whole cached prefix is repeated.
pub fn broken_at(before: &[String], cached: usize, after: &[String]) -> Option<usize> {
    let (a, b) = (block_hashes(before), block_hashes(after));
    (0..cached.min(a.len())).find(|&i| b.get(i) != Some(&a[i]))
}

/// The block's first line, cut to 80 characters: how a guard names it.
pub fn block_name(block: &str) -> String {
    let line = block.lines().next().unwrap_or_default();
    line.chars().take(80).collect()
}
