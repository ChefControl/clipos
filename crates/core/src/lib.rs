//! Shared building blocks for the clipos `api` and `worker` binaries.

pub mod analysis;
pub mod auth;
pub mod azure;
pub mod clips;
pub mod db;
pub mod dedup;
pub mod invites;
pub mod jobs;
pub mod shares;
pub mod shows;
pub mod social;
pub mod storage;
pub mod telemetry;
#[cfg(any(test, feature = "test-util"))]
pub mod testing;
pub mod users;

/// Build identifier (the git SHA in deployed images, `dev` otherwise). The deploy
/// workflow reads it from the `x-clipos-version` header on `/healthz` to know a rollout
/// finished.
pub fn version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| std::env::var("CLIPOS_VERSION").unwrap_or_else(|_| "dev".into()))
}
