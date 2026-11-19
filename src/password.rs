//! Password hashes as Mattermost stores them: PBKDF2 in PHC form (`$pbkdf2$f=SHA256,w=600000,l=32$salt$hash`),
//! and bcrypt from older servers. And MFA's TOTP codes.

// Based on Mattermost server/channels/app/password/hashers and einterfaces/mfa, Copyright (c) 2015-present
// Mattermost, Inc. Licensed under AGPL-3.0.

use base64::Engine;
use base64::engine::general_purpose::STANDARD_NO_PAD as B64;
use hmac::{Hmac, KeyInit, Mac};

const WORK: u32 = 600_000;
const KEY_LEN: usize = 32;

/// `hashers.Hash`: PBKDF2-SHA256.
pub fn hash(password: &str) -> String {
    let salt: [u8; 16] = rand::random();
    let mut key = [0u8; KEY_LEN];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(password.as_bytes(), &salt, WORK, &mut key);
    format!("$pbkdf2$f=SHA256,w={WORK},l={KEY_LEN}${}${}", B64.encode(salt), B64.encode(key))
}

pub enum Check {
    Match,
    /// Matches, but with an older scheme: store [`hash`] instead.
    MatchOld,
    Mismatch,
}

pub fn check(stored: &str, password: &str) -> Check {
    if stored.is_empty() || password.is_empty() || password.len() > 72 && !stored.starts_with("$pbkdf2$") {
        return Check::Mismatch;
    }
    if let Some(rest) = stored.strip_prefix("$pbkdf2$") {
        // f=SHA256,w=600000,l=32$<salt>$<hash>
        let mut parts = rest.split('$');
        let (Some(params), Some(salt), Some(want)) = (parts.next(), parts.next(), parts.next()) else {
            return Check::Mismatch;
        };
        let param = |k: &str| params.split(',').find_map(|p| p.strip_prefix(k)).and_then(|v| v.parse::<u32>().ok());
        let (Some(w), Some(l)) = (param("w="), param("l=")) else { return Check::Mismatch };
        let (Ok(salt), Ok(want)) = (B64.decode(salt), B64.decode(want)) else { return Check::Mismatch };
        let mut key = vec![0u8; l as usize];
        pbkdf2::pbkdf2_hmac::<sha2::Sha256>(password.as_bytes(), &salt, w, &mut key);
        return if constant_eq(&key, &want) {
            if w == WORK && l as usize == KEY_LEN { Check::Match } else { Check::MatchOld }
        } else {
            Check::Mismatch
        };
    }
    match bcrypt::verify(password, stored) {
        Ok(true) => Check::MatchOld,
        _ => Check::Mismatch,
    }
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A 6-digit TOTP code (30 s steps, SHA1) for a base32 secret, checked one step either side as dgryski/dgoogauth.
pub fn totp_ok(secret_b32: &str, code: &str, now_secs: i64) -> bool {
    let Some(secret) = base32_decode(secret_b32) else { return false };
    let Ok(code) = code.trim().parse::<u32>() else { return false };
    let step = now_secs / 30;
    (-1..=1).any(|d| totp(&secret, (step + d) as u64) == code)
}

fn totp(secret: &[u8], counter: u64) -> u32 {
    let mut mac = Hmac::<sha1::Sha1>::new_from_slice(secret).unwrap();
    mac.update(&counter.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let o = (h[19] & 0xf) as usize;
    let bin = u32::from_be_bytes([h[o] & 0x7f, h[o + 1], h[o + 2], h[o + 3]]);
    bin % 1_000_000
}

fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = vec![];
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.trim_end_matches('=').bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a',
            b'2'..=b'7' => c - b'2' + 26,
            _ => return None,
        } as u32;
        acc = (acc << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pbkdf2_round_trip() {
        let h = hash("Secret-123");
        assert!(matches!(check(&h, "Secret-123"), Check::Match));
        assert!(matches!(check(&h, "secret-123"), Check::Mismatch));
    }
    #[test]
    fn rfc6238_sha1() {
        // RFC 6238 test secret "12345678901234567890", T = 59 s → 94287082 (8 digits) → 287082.
        let secret = b"12345678901234567890";
        assert_eq!(totp(secret, 59 / 30), 287082);
    }
}
