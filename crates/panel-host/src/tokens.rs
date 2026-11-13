//! Random values from the operating system's source: bearer tokens and their stored digests,
//! codes over a caller's alphabet, secrets, salts and nonces. Every random value the panel
//! issues is drawn here; the caller states the shape (alphabet, length, encoding), which
//! stays the shape of the values already issued.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::{Rng, RngCore, rngs::OsRng};
use sha2::{Digest, Sha256};

/// A bearer token: 32 random bytes in unpadded URL-safe base64.
pub fn random_token() -> String {
    URL_SAFE_NO_PAD.encode(bytes::<32>())
}

pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `N` random bytes: a secret, salt or nonce of a fixed size.
pub fn bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0_u8; N];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

/// `length` random bytes, for sizes chosen at run time.
pub fn byte_vec(length: usize) -> Vec<u8> {
    let mut bytes = vec![0_u8; length];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

/// `length` random bytes as lowercase hexadecimal (two characters per byte).
pub fn hex(length: usize) -> String {
    byte_vec(length)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `length` characters drawn uniformly and independently from `alphabet`.
pub fn code(alphabet: &[u8], length: usize) -> String {
    assert!(!alphabet.is_empty(), "a code needs an alphabet");
    (0..length)
        .map(|_| char::from(alphabet[OsRng.gen_range(0..alphabet.len())]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{byte_vec, bytes, code, hex, random_token};

    #[test]
    fn values_keep_their_shapes() {
        assert_eq!(random_token().len(), 43);
        assert_ne!(bytes::<16>(), bytes::<16>());
        assert_eq!(byte_vec(24).len(), 24);
        let hex = hex(4);
        assert_eq!(hex.len(), 8);
        assert!(
            hex.bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        );
        let digits = code(b"0123456789", 6);
        assert_eq!(digits.len(), 6);
        assert!(digits.bytes().all(|byte| byte.is_ascii_digit()));
        // Every symbol of the alphabet is drawn, the first and the last included.
        let drawn = code(b"ab", 256);
        assert!(drawn.contains('a') && drawn.contains('b'), "{drawn}");
    }
}
