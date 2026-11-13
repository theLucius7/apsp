//! Password hashing: Argon2id with the library's default parameters and a fresh random
//! 16-byte salt, stored as a PHC string. Verification reads the parameters from the stored
//! string, so hashes written with other parameters keep verifying. Both operations are
//! deliberately slow and blocking: callers run them on a blocking thread and bound how
//! many run at once.

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};

/// The PHC string of `password` hashed with a fresh random salt.
pub fn hash(password: &[u8]) -> anyhow::Result<String> {
    let salt = SaltString::encode_b64(&crate::tokens::bytes::<16>())
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Argon2::default()
        .hash_password(password, &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| anyhow::anyhow!(error.to_string()))
}

/// Whether `password` matches the stored PHC string; anything else stored matches nothing.
pub fn verify(password: &[u8], stored: &str) -> bool {
    PasswordHash::new(stored)
        .is_ok_and(|hash| Argon2::default().verify_password(password, &hash).is_ok())
}

#[cfg(test)]
mod tests {
    use super::{hash, verify};
    use argon2::{
        Algorithm, Argon2, Params, Version,
        password_hash::{PasswordHasher, SaltString},
    };

    #[test]
    fn hashes_verify_with_their_own_password_only() {
        let first = hash(b"correct horse battery").unwrap();
        let second = hash(b"correct horse battery").unwrap();
        assert!(
            first.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
            "{first}"
        );
        assert_ne!(first, second, "every hash has its own salt");
        assert!(verify(b"correct horse battery", &first));
        assert!(!verify(b"correct horse battery!", &first));
        assert!(!verify(b"anything", "not a PHC string"));
        // A hash written with other parameters verifies with its own.
        let cheap = Argon2::new(
            Algorithm::Argon2id,
            Version::V0x13,
            Params::new(8, 1, 1, None).unwrap(),
        )
        .hash_password(
            b"older",
            &SaltString::encode_b64(b"fixture salt 16b").unwrap(),
        )
        .unwrap()
        .to_string();
        assert!(verify(b"older", &cheap));
        assert!(!verify(b"newer", &cheap));
    }
}
