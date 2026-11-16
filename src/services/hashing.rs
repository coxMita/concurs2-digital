//! Argon2id hashing on the blocking pool, bounded by a semaphore sized to the CPU count
//! so a burst of logins cannot starve the async runtime or exhaust memory.

use std::sync::Arc;

use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use tokio::sync::Semaphore;

use crate::error::AppError;

#[derive(Clone)]
pub struct Hasher {
    permits: Arc<Semaphore>,
    params: Params,
    /// Hash of a random password, verified against when the username does not exist,
    /// so response timing does not reveal which usernames exist.
    dummy_hash: Arc<String>,
}

impl Hasher {
    pub fn new(memory_kib: u32, iterations: u32) -> Self {
        let params = Params::new(memory_kib, iterations, 1, None).expect("valid argon2 params");
        let cpus = std::thread::available_parallelism().map_or(2, |n| n.get());
        let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params.clone());
        let dummy_hash = argon
            .hash_password(crate::services::credentials::random_token(16).as_bytes())
            .expect("hash")
            .to_string();
        Self {
            permits: Arc::new(Semaphore::new(cpus)),
            params,
            dummy_hash: Arc::new(dummy_hash),
        }
    }

    fn argon(params: Params) -> Argon2<'static> {
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
    }

    pub async fn hash(&self, password: String) -> Result<String, AppError> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let params = self.params.clone();
        tokio::task::spawn_blocking(move || {
            Self::argon(params)
                .hash_password(password.as_bytes())
                .map(|h| h.to_string())
                .map_err(|e| AppError::Internal(e.to_string()))
        })
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
    }

    /// Verifies `password` against `hash`; with `None` verifies against a dummy hash
    /// (always false) to keep timing uniform.
    pub async fn verify(&self, password: String, hash: Option<String>) -> Result<bool, AppError> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let real = hash.is_some();
        let hash = hash.unwrap_or_else(|| self.dummy_hash.as_str().to_string());
        let params = self.params.clone();
        let ok = tokio::task::spawn_blocking(move || {
            let Ok(parsed) = PasswordHash::new(&hash) else {
                return false;
            };
            Self::argon(params)
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(ok && real)
    }
}
