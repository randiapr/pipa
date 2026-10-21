// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! Infrastructure layer: concrete [`super::domain::Cache`] adapters, and the environment config
//! that picks between them.

mod memory;
mod redis_cache;

use std::sync::Arc;

use super::domain::Cache;
pub use memory::InMemoryCache;
pub use redis_cache::RedisCache;

/// Picks the cache from the environment: [`RedisCache`] with `REDIS_URL` set — one cache for
/// every backend instance, and it survives a restart — otherwise a process-local
/// [`InMemoryCache`] holding up to `CACHE_MAX_ENTRIES` (default 256) results.
///
/// A cache entry is a query result, so Redis must be reachable only from the backend (and
/// `pipa-ingestion`, which keeps its own keys there). There is no TLS (`rediss://`) yet.
pub fn from_env() -> anyhow::Result<Arc<dyn Cache>> {
    match std::env::var("REDIS_URL")
        .ok()
        .filter(|url| !url.is_empty())
    {
        Some(url) => {
            tracing::info!("caching query results in Redis");
            Ok(Arc::new(RedisCache::new(&url)?))
        }
        None => {
            let max_entries = std::env::var("CACHE_MAX_ENTRIES")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(256);
            tracing::info!(max_entries, "caching query results in memory");
            Ok(Arc::new(InMemoryCache::new(max_entries)))
        }
    }
}
