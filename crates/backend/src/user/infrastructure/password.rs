//! Argon2id password hashing.

use argon2::{Argon2, PasswordHash, PasswordHasher as _, PasswordVerifier as _};
use async_trait::async_trait;

use crate::user::domain::{PasswordHasher, UserError};

/// Hashes passwords with Argon2id (the `argon2` crate's defaults) and a random per-hash salt.
///
/// Hashing is CPU- and memory-heavy by design, so it runs on the blocking pool rather than
/// stalling an async worker thread.
#[derive(Debug, Default, Clone, Copy)]
pub struct Argon2PasswordHasher;

#[async_trait]
impl PasswordHasher for Argon2PasswordHasher {
    async fn hash(&self, password: &str) -> Result<String, UserError> {
        let password = password.to_owned();
        tokio::task::spawn_blocking(move || {
            Argon2::default()
                .hash_password(password.as_bytes())
                .map(|hash| hash.to_string())
                .map_err(|err| UserError::Storage(format!("failed to hash password: {err}")))
        })
        .await
        .map_err(|err| UserError::Storage(format!("password hashing task failed: {err}")))?
    }

    async fn verify(&self, password: &str, hash: &str) -> bool {
        let password = password.to_owned();
        let hash = hash.to_owned();
        tokio::task::spawn_blocking(move || {
            PasswordHash::new(&hash).is_ok_and(|parsed| {
                Argon2::default()
                    .verify_password(password.as_bytes(), &parsed)
                    .is_ok()
            })
        })
        .await
        .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn verifies_only_the_original_password() {
        let hasher = Argon2PasswordHasher;
        let hash = hasher.hash("correct horse").await.unwrap();

        assert!(hash.starts_with("$argon2id$"));
        assert!(hasher.verify("correct horse", &hash).await);
        assert!(!hasher.verify("wrong", &hash).await);
        assert!(!hasher.verify("correct horse", "not a hash").await);
    }

    #[tokio::test]
    async fn salts_each_hash_differently() {
        let hasher = Argon2PasswordHasher;
        let first = hasher.hash("same password").await.unwrap();
        let second = hasher.hash("same password").await.unwrap();
        assert_ne!(first, second);
    }
}
