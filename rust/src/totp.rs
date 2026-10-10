//! TOTP (RFC 6238), Google Authenticator flavor: HMAC-SHA1, 30 s, 6 digits.

use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use std::time::{SystemTime, UNIX_EPOCH};

pub const PERIOD: u64 = 30;
/// Minimum seconds left to use the token of the current window.
pub const MIN_TOKEN_LIFETIME: u64 = 5;

/// Removes spaces, uppercases, strips '=' and validates the base32.
pub fn normalize_seed(seed: &str) -> Option<String> {
    let s: String = seed.split_whitespace().collect();
    let s = s.to_uppercase();
    let s = s.trim_end_matches('=').to_string();
    if s.is_empty() {
        return None;
    }
    base32::decode(base32::Alphabet::Rfc4648 { padding: false }, &s)?;
    Some(s)
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before 1970")
        .as_secs()
}

/// Seconds left until the next 30 s window.
pub fn seconds_remaining() -> u64 {
    PERIOD - (now_unix() % PERIOD)
}

pub fn totp_at(seed_b32: &str, unix: u64) -> Option<String> {
    let key = base32::decode(base32::Alphabet::Rfc4648 { padding: false }, seed_b32)?;
    let counter = unix / PERIOD;
    let mut mac = Hmac::<Sha1>::new_from_slice(&key).ok()?;
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let off = (digest[19] & 0x0f) as usize;
    let code = (u32::from(digest[off] & 0x7f) << 24
        | u32::from(digest[off + 1]) << 16
        | u32::from(digest[off + 2]) << 8
        | u32::from(digest[off + 3]))
        % 1_000_000;
    Some(format!("{code:06}"))
}

pub fn totp_now(seed_b32: &str) -> Option<String> {
    totp_at(seed_b32, now_unix())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Official RFC 6238 test vectors (SHA1): ASCII seed "12345678901234567890".
    const SEED: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

    #[test]
    fn vetores_rfc6238() {
        assert_eq!(totp_at(SEED, 59).unwrap(), "287082");
        assert_eq!(totp_at(SEED, 1_111_111_109).unwrap(), "081804");
        assert_eq!(totp_at(SEED, 1_234_567_890).unwrap(), "005924");
        assert_eq!(totp_at(SEED, 2_000_000_000).unwrap(), "279037");
    }

    #[test]
    fn normalizacao() {
        assert_eq!(
            normalize_seed("gezd gnbv gy3t qojq gezd gnbv gy3t qojq").unwrap(),
            SEED
        );
        assert!(normalize_seed("189!").is_none());
        assert!(normalize_seed("").is_none());
    }
}
