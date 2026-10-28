//! `pcbir.editor.server`: the loopback-only HTTP transport.

use std::hash::{BuildHasher, RandomState};

/// Unpredictable bytes for capability tokens and temporary names.
///
/// `RandomState` is keyed from the operating system's random source and
/// SipHash-1-3 is a keyed pseudorandom function, so hashing a counter gives
/// bytes an observer cannot predict without the per-process key; the
/// `attempt` number keeps retries distinct within one call.
pub fn random_bytes(count: usize, attempt: u32) -> Vec<u8> {
    let state = RandomState::new();
    let mut bytes = Vec::with_capacity(count + 8);
    let mut counter: u64 = u64::from(attempt) << 32;
    while bytes.len() < count {
        bytes.extend_from_slice(&state.hash_one(counter).to_le_bytes());
        counter += 1;
    }
    bytes.truncate(count);
    bytes
}

/// `secrets.token_hex(count)` style text: lower-case hexadecimal.
pub fn random_token(count: usize, attempt: u32) -> String {
    random_bytes(count, attempt).iter().map(|byte| format!("{byte:02x}")).collect()
}
