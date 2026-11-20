//! Password hashes as Mattermost stores them: PBKDF2 in PHC form (`$pbkdf2$f=SHA256,w=600000,l=32$salt$hash`),
//! and bcrypt from older servers. (MFA codes are checked in `user_admin.rs`.)

// Based on Mattermost server/channels/app/password/hashers and einterfaces/mfa, Copyright (c) 2015-present
// Mattermost, Inc. Licensed under AGPL-3.0.

use base64::Engine;
use base64::engine::general_purpose::STANDARD_NO_PAD as B64;

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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pbkdf2_round_trip() {
        let h = hash("Secret-123");
        assert!(matches!(check(&h, "Secret-123"), Check::Match));
        assert!(matches!(check(&h, "secret-123"), Check::Mismatch));
    }
}
