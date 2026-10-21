// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! [`CachedQueries`]: `QueryService` with a result cache in front of its two expensive reads.
//!
//! What is cached, and what a hit can and can't be:
//! - **Query results** (`POST /query`) and **the table list** (`GET /tables`) are cached; a read
//!   of a table's rows is not (browsing pages through it, so hits would be rare).
//! - The key is a hash of the *scope* — the namespaces the session may see, or "everything" — and
//!   the SQL. The same query on the same scope returns the same rows whoever asks, so entries
//!   are shared between users and roles; callers have already been authorized, and a source
//!   added to or removed from a project changes the scope and with it the key.
//! - Ingestion lands new data at any time and nothing here invalidates on it, so a hit can be up
//!   to a TTL old (`CACHE_QUERY_TTL_SECS`/`CACHE_TABLES_TTL_SECS`, 30s each; 0 turns that
//!   cache off). Likewise `now()` and the like are frozen for a TTL.
//! - Only successes are stored, and only results up to `CACHE_MAX_ENTRY_BYTES` (1 MiB).

use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use serde::de::IgnoredAny;
use sha2::{Digest, Sha256};

use crate::iceberg::{QueryError, QueryService};

use super::domain::Cache;

/// Bump when what an entry means changes, so entries from an older build are never read.
const KEY_VERSION: &str = "v1";

#[derive(Debug, Clone)]
pub struct CacheConfig {
    pub query_ttl: Duration,
    pub tables_ttl: Duration,
    pub max_entry_bytes: usize,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            query_ttl: Duration::from_secs(30),
            tables_ttl: Duration::from_secs(30),
            max_entry_bytes: 1024 * 1024,
        }
    }
}

impl CacheConfig {
    /// `CACHE_QUERY_TTL_SECS`, `CACHE_TABLES_TTL_SECS` and `CACHE_MAX_ENTRY_BYTES`, falling back
    /// to the defaults.
    pub fn from_env() -> Self {
        fn var<T: std::str::FromStr>(name: &str) -> Option<T> {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse().ok())
        }
        let defaults = Self::default();
        Self {
            query_ttl: var("CACHE_QUERY_TTL_SECS")
                .map(Duration::from_secs)
                .unwrap_or(defaults.query_ttl),
            tables_ttl: var("CACHE_TABLES_TTL_SECS")
                .map(Duration::from_secs)
                .unwrap_or(defaults.tables_ttl),
            max_entry_bytes: var("CACHE_MAX_ENTRY_BYTES").unwrap_or(defaults.max_entry_bytes),
        }
    }
}

/// `QueryService` whose `query` and `list_tables` go through the cache.
pub struct CachedQueries {
    queries: QueryService,
    results: ResultCache,
}

impl CachedQueries {
    pub fn new(queries: QueryService, cache: Arc<dyn Cache>, config: CacheConfig) -> Self {
        Self {
            queries,
            results: ResultCache { cache, config },
        }
    }

    /// The underlying service, for callers that must see the catalog as it is now.
    pub fn uncached(&self) -> &QueryService {
        &self.queries
    }

    /// [`QueryService::query`], remembered for `CACHE_QUERY_TTL_SECS`.
    pub async fn query(
        &self,
        sql: &str,
        allowed_namespaces: Option<HashSet<String>>,
    ) -> Result<Vec<u8>, QueryError> {
        let key = self
            .results
            .key("query", &scope(allowed_namespaces.as_ref()), sql);
        let ttl = self.results.config.query_ttl;
        self.results
            .remember(&key, ttl, self.queries.query(sql, allowed_namespaces))
            .await
    }

    /// [`QueryService::list_tables`], remembered for `CACHE_TABLES_TTL_SECS`.
    pub async fn list_tables(
        &self,
        namespaces: HashSet<String>,
    ) -> Result<Vec<(String, String)>, QueryError> {
        let key = self.results.key("tables", &scope(Some(&namespaces)), "");
        let ttl = self.results.config.tables_ttl;
        let compute = async {
            let tables = self.queries.list_tables(namespaces.clone()).await?;
            Ok::<_, QueryError>(
                serde_json::to_vec(&tables).expect("a list of string pairs always encodes"),
            )
        };
        let encoded = self.results.remember(&key, ttl, compute).await?;
        match serde_json::from_slice(&encoded) {
            Ok(tables) => Ok(tables),
            // Not what this build stores (an entry from something else): ask for real.
            Err(_) => self.queries.list_tables(namespaces).await,
        }
    }

    /// [`QueryService::read_table`], never cached.
    pub async fn read_table(
        &self,
        namespace: &str,
        table: &str,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<u8>, QueryError> {
        self.queries
            .read_table(namespace, table, limit, offset)
            .await
    }
}

/// The caching policy, apart from what is being cached.
struct ResultCache {
    cache: Arc<dyn Cache>,
    config: CacheConfig,
}

impl ResultCache {
    /// The cache key for `kind` over `scope` and `text`: a hash, so it is a fixed size however
    /// long the SQL is, and nothing of the query or the project shows in Redis's key space.
    fn key(&self, kind: &str, scope: &str, text: &str) -> String {
        let mut hash = Sha256::new();
        hash.update(scope.as_bytes());
        hash.update([0]);
        hash.update(text.as_bytes());
        let digest: String = hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        format!("pipa:cache:{KEY_VERSION}:{kind}:{digest}")
    }

    /// The JSON stored under `key`, or — on a miss — what `compute` produces, which is stored for
    /// `ttl` unless it failed or is over the size limit. A `ttl` of zero skips the cache
    /// entirely. A stored value that isn't valid JSON counts as a miss.
    async fn remember<E>(
        &self,
        key: &str,
        ttl: Duration,
        compute: impl Future<Output = Result<Vec<u8>, E>>,
    ) -> Result<Vec<u8>, E> {
        if ttl.is_zero() {
            return compute.await;
        }
        if let Some(hit) = self.cache.get(key).await
            && serde_json::from_slice::<IgnoredAny>(&hit).is_ok()
        {
            tracing::debug!(key, "cache hit");
            return Ok(hit);
        }
        let computed = compute.await?;
        if computed.len() <= self.config.max_entry_bytes {
            self.cache.set(key, computed.clone(), ttl).await;
        }
        Ok(computed)
    }
}

/// What a session may see, as text for a key: every namespace, or this sorted set.
fn scope(namespaces: Option<&HashSet<String>>) -> String {
    match namespaces {
        None => "all".to_owned(),
        Some(namespaces) => {
            let mut sorted: Vec<&str> = namespaces.iter().map(String::as_str).collect();
            sorted.sort_unstable();
            format!("only:{}", sorted.join(","))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::cache::infrastructure::InMemoryCache;

    fn results(config: CacheConfig) -> ResultCache {
        ResultCache {
            cache: Arc::new(InMemoryCache::new(16)),
            config,
        }
    }

    /// A `compute` that counts how often it runs.
    fn counted(
        runs: &AtomicUsize,
        value: &'static [u8],
    ) -> impl Future<Output = Result<Vec<u8>, String>> {
        runs.fetch_add(1, Ordering::SeqCst);
        async move { Ok(value.to_vec()) }
    }

    const TTL: Duration = Duration::from_secs(60);

    #[tokio::test]
    async fn a_hit_skips_the_computation() {
        let results = results(CacheConfig::default());
        let runs = AtomicUsize::new(0);
        let first = results.remember("k", TTL, counted(&runs, b"[1]")).await;
        let second = results
            .remember("k", TTL, async {
                Err::<Vec<u8>, _>("must not run".to_owned())
            })
            .await;
        assert_eq!(first.unwrap(), b"[1]");
        assert_eq!(second.unwrap(), b"[1]");
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failures_are_not_cached() {
        let results = results(CacheConfig::default());
        let failed = results
            .remember("k", TTL, async { Err::<Vec<u8>, _>("boom".to_owned()) })
            .await;
        assert_eq!(failed.unwrap_err(), "boom");
        let runs = AtomicUsize::new(0);
        let retried = results.remember("k", TTL, counted(&runs, b"[2]")).await;
        assert_eq!(retried.unwrap(), b"[2]");
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_zero_ttl_bypasses_the_cache() {
        let results = results(CacheConfig::default());
        let runs = AtomicUsize::new(0);
        for _ in 0..2 {
            results
                .remember("k", Duration::ZERO, counted(&runs, b"[1]"))
                .await
                .unwrap();
        }
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn results_over_the_size_limit_are_not_stored() {
        let results = results(CacheConfig {
            max_entry_bytes: 2,
            ..CacheConfig::default()
        });
        let runs = AtomicUsize::new(0);
        for _ in 0..2 {
            results
                .remember("k", TTL, counted(&runs, b"[1,2,3]"))
                .await
                .unwrap();
        }
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_stored_value_that_is_not_json_is_a_miss() {
        let results = results(CacheConfig::default());
        results.cache.set("k", b"not json".to_vec(), TTL).await;
        let runs = AtomicUsize::new(0);
        let value = results.remember("k", TTL, counted(&runs, b"[1]")).await;
        assert_eq!(value.unwrap(), b"[1]");
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn keys_separate_scope_sql_and_kind() {
        let results = results(CacheConfig::default());
        let a = HashSet::from(["cdc_a".to_owned()]);
        let ab = HashSet::from(["cdc_a".to_owned(), "cdc_b".to_owned()]);
        let key = |kind, namespaces: Option<&HashSet<String>>, sql| {
            results.key(kind, &scope(namespaces), sql)
        };

        let base = key("query", Some(&a), "SELECT 1");
        assert_eq!(base, key("query", Some(&a), "SELECT 1"));
        assert_ne!(base, key("query", Some(&a), "SELECT 2"));
        assert_ne!(base, key("query", Some(&ab), "SELECT 1"));
        assert_ne!(
            base,
            key("query", None, "SELECT 1"),
            "a project's scope must not equal everything"
        );
        assert_ne!(base, key("tables", Some(&a), "SELECT 1"));
        // Set order doesn't matter.
        let ba = HashSet::from(["cdc_b".to_owned(), "cdc_a".to_owned()]);
        assert_eq!(key("query", Some(&ab), "x"), key("query", Some(&ba), "x"));
    }

    #[test]
    fn config_defaults() {
        let config = CacheConfig::default();
        assert_eq!(config.query_ttl, Duration::from_secs(30));
        assert_eq!(config.max_entry_bytes, 1024 * 1024);
    }
}
