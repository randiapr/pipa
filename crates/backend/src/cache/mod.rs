//! Result caching for the read paths that are expensive to recompute: `POST /query` and the
//! table list of `GET /tables`. Every call builds a fresh DataFusion session over the Iceberg
//! catalog, so an identical dashboard refresh is otherwise paid in full each time.
//!
//! Same clean-architecture split as the other bounded contexts:
//! - [`domain`] — the [`Cache`] port.
//! - [`infrastructure`] — [`InMemoryCache`] (process-local, the default) and [`RedisCache`]
//!   (shared by every backend instance, selected by `REDIS_URL`).
//! - [`application`] — [`CachedQueries`], which puts the cache in front of `QueryService`.
//!
//! The cache is an optimization and never a source of truth or of access control: handlers
//! authorize the caller *before* asking for a result, and the key contains the whole scope
//! (the project's namespaces and the SQL), so a hit is exactly what the same query would
//! return now, give or take the TTL. A cache that is slow or down is skipped, never an error.

pub mod application;
pub mod domain;
pub mod infrastructure;

pub use application::{CacheConfig, CachedQueries};
pub use infrastructure::from_env;
