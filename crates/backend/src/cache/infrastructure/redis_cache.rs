//! [`RedisCache`]: results shared through Redis.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use redis::Client;
use redis::aio::ConnectionManager;
use tokio::sync::Mutex as AsyncMutex;

use crate::cache::domain::Cache;

/// Longest a single Redis call may take before it counts as a failure. A cache hit should beat
/// recomputing by a wide margin, so a slow Redis is worse than none.
const CALL_TIMEOUT: Duration = Duration::from_millis(250);
/// How long Redis is left alone after a failure, so a Redis that is down costs each request
/// nothing instead of a timeout.
const BACKOFF: Duration = Duration::from_secs(5);

pub struct RedisCache {
    client: Client,
    /// Connected on first use, so a Redis that is down at startup is just another outage.
    connection: AsyncMutex<Option<ConnectionManager>>,
    /// While set and in the future, Redis is skipped.
    down_until: Mutex<Option<Instant>>,
}

impl RedisCache {
    pub fn new(url: &str) -> anyhow::Result<Self> {
        Ok(Self {
            client: Client::open(url)?,
            connection: AsyncMutex::new(None),
            down_until: Mutex::new(None),
        })
    }

    fn is_down(&self) -> bool {
        let down = self
            .down_until
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        down.is_some_and(|until| until > Instant::now())
    }

    fn mark(&self, outcome: &Result<(), String>) {
        let mut down = self
            .down_until
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match outcome {
            Ok(()) => *down = None,
            Err(reason) => {
                if down.is_none_or(|until| until <= Instant::now()) {
                    tracing::warn!(error = %reason, "cache unavailable, skipping it for a few seconds");
                }
                *down = Some(Instant::now() + BACKOFF);
            }
        }
    }

    async fn connection(&self) -> Result<ConnectionManager, String> {
        let mut slot = self.connection.lock().await;
        if let Some(connection) = slot.as_ref() {
            return Ok(connection.clone());
        }
        let connection = ConnectionManager::new(self.client.clone())
            .await
            .map_err(|err| err.to_string())?;
        *slot = Some(connection.clone());
        Ok(connection)
    }

    async fn call<T>(
        &self,
        run: impl AsyncFnOnce(&mut ConnectionManager) -> Result<T, redis::RedisError>,
    ) -> Option<T> {
        if self.is_down() {
            return None;
        }
        let attempt = async {
            let mut connection = self.connection().await?;
            run(&mut connection).await.map_err(|err| err.to_string())
        };
        let result = match tokio::time::timeout(CALL_TIMEOUT, attempt).await {
            Ok(result) => result,
            Err(_) => Err("timed out".to_owned()),
        };
        self.mark(&result.as_ref().map(|_| ()).map_err(Clone::clone));
        result.ok()
    }
}

#[async_trait]
impl Cache for RedisCache {
    async fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.call(async |connection| {
            redis::cmd("GET")
                .arg(key)
                .query_async::<Option<Vec<u8>>>(connection)
                .await
        })
        .await
        .flatten()
    }

    async fn set(&self, key: &str, value: Vec<u8>, ttl: Duration) {
        let _ = self
            .call(async |connection| {
                redis::cmd("SET")
                    .arg(key)
                    .arg(value)
                    .arg("PX")
                    .arg(ttl.as_millis().max(1) as u64)
                    .query_async::<()>(connection)
                    .await
            })
            .await;
    }
}

/// Live tests against a real Redis: `REDIS_URL=redis://127.0.0.1:6379 cargo test -p pipa-backend
/// -- --ignored redis_cache`.
#[cfg(test)]
mod live_tests {
    use uuid::Uuid;

    use super::*;

    #[tokio::test]
    #[ignore = "needs a Redis at REDIS_URL"]
    async fn redis_cache_stores_and_expires() {
        let url = std::env::var("REDIS_URL").expect("REDIS_URL must point at a Redis");
        let cache = RedisCache::new(&url).unwrap();
        let key = format!("pipa:cache:test:{}", Uuid::now_v7());

        assert_eq!(cache.get(&key).await, None);
        cache
            .set(&key, b"[1,2]".to_vec(), Duration::from_millis(300))
            .await;
        assert_eq!(cache.get(&key).await, Some(b"[1,2]".to_vec()));
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(cache.get(&key).await, None);
    }

    #[tokio::test]
    #[ignore = "needs a Redis at REDIS_URL"]
    async fn redis_cache_is_skipped_while_redis_is_down() {
        // Nothing listens on this port.
        let cache = RedisCache::new("redis://127.0.0.1:1").unwrap();
        assert_eq!(cache.get("k").await, None);
        assert!(cache.is_down());
        cache.set("k", b"v".to_vec(), Duration::from_secs(1)).await;
        assert_eq!(cache.get("k").await, None);
    }
}
