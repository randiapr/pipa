//! [`InMemoryCache`]: a bounded, process-local cache.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::cache::domain::Cache;

/// Keeps at most `max_entries` results. When full, expired entries go first, then whichever
/// would expire soonest. Not shared between backend instances: each warms its own.
pub struct InMemoryCache {
    max_entries: usize,
    entries: Mutex<HashMap<String, (Instant, Vec<u8>)>>,
}

impl InMemoryCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            max_entries,
            entries: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl Cache for InMemoryCache {
    async fn get(&self, key: &str) -> Option<Vec<u8>> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match entries.get(key) {
            Some((expires, value)) if *expires > Instant::now() => Some(value.clone()),
            Some(_) => {
                entries.remove(key);
                None
            }
            None => None,
        }
    }

    async fn set(&self, key: &str, value: Vec<u8>, ttl: Duration) {
        if self.max_entries == 0 {
            return;
        }
        let now = Instant::now();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if entries.len() >= self.max_entries && !entries.contains_key(key) {
            entries.retain(|_, (expires, _)| *expires > now);
        }
        if entries.len() >= self.max_entries && !entries.contains_key(key) {
            let soonest = entries
                .iter()
                .min_by_key(|(_, (expires, _))| *expires)
                .map(|(key, _)| key.clone());
            if let Some(soonest) = soonest {
                entries.remove(&soonest);
            }
        }
        entries.insert(key.to_owned(), (now + ttl, value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LONG: Duration = Duration::from_secs(60);

    #[tokio::test]
    async fn returns_what_was_stored() {
        let cache = InMemoryCache::new(4);
        assert_eq!(cache.get("a").await, None);
        cache.set("a", b"1".to_vec(), LONG).await;
        assert_eq!(cache.get("a").await, Some(b"1".to_vec()));
        cache.set("a", b"2".to_vec(), LONG).await;
        assert_eq!(cache.get("a").await, Some(b"2".to_vec()));
    }

    #[tokio::test]
    async fn entries_expire() {
        let cache = InMemoryCache::new(4);
        cache
            .set("a", b"1".to_vec(), Duration::from_millis(30))
            .await;
        assert!(cache.get("a").await.is_some());
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(cache.get("a").await, None);
    }

    #[tokio::test]
    async fn stays_within_its_bound_dropping_the_soonest_to_expire() {
        let cache = InMemoryCache::new(2);
        cache
            .set("soon", b"1".to_vec(), Duration::from_secs(10))
            .await;
        cache.set("later", b"2".to_vec(), LONG).await;
        cache.set("new", b"3".to_vec(), LONG).await;
        assert_eq!(cache.get("soon").await, None);
        assert!(cache.get("later").await.is_some());
        assert!(cache.get("new").await.is_some());
    }

    #[tokio::test]
    async fn a_zero_bound_stores_nothing() {
        let cache = InMemoryCache::new(0);
        cache.set("a", b"1".to_vec(), LONG).await;
        assert_eq!(cache.get("a").await, None);
    }
}
