use std::{ops::Deref, sync::Arc};

use sqlx::PgPool;

use crate::{
    auth::ratelimit::LoginLimiter,
    config::Config,
    services::{exam_cache::ExamCache, hashing::Hasher},
};

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

pub struct Inner {
    pub db: PgPool,
    pub config: Config,
    pub hasher: Hasher,
    pub login_limiter: LoginLimiter,
    pub cache: ExamCache,
}

impl AppState {
    pub fn new(db: PgPool, config: Config) -> Self {
        let hasher = Hasher::new(config.argon2_memory_kib, config.argon2_iterations);
        let login_limiter = LoginLimiter::new(config.login_fail_per_user_per_min);
        Self(Arc::new(Inner {
            db,
            config,
            hasher,
            login_limiter,
            cache: ExamCache::new(),
        }))
    }
}

impl Deref for AppState {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}
