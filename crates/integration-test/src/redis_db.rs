// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! A Redis database of a test's own on the shared server, and a way to look into it.
//!
//! Redis has 16 numbered databases; each test that uses Redis takes the next one (flushed first),
//! so the keys it counts and inspects are only its own. At most [`crate::env`]'s few concurrent
//! tests are alive at once, so a database is never handed out again while still in use.

use std::sync::atomic::{AtomicUsize, Ordering};

use redis::AsyncCommands;
use redis::aio::MultiplexedConnection;

use crate::infra;

pub struct RedisDb {
    /// The URL (naming the database) to give `REDIS_URL`.
    pub url: String,
    connection: MultiplexedConnection,
}

impl RedisDb {
    pub async fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let index = 1 + NEXT.fetch_add(1, Ordering::Relaxed) % 15;
        let url = format!("{}/{index}", infra::endpoints().redis_url);
        let client = redis::Client::open(url.as_str()).expect("a valid Redis URL");
        let mut connection = client
            .get_multiplexed_async_connection()
            .await
            .expect("connecting to Redis");
        let _: () = redis::cmd("FLUSHDB")
            .query_async(&mut connection)
            .await
            .expect("flushing the Redis database");
        Self { url, connection }
    }

    /// Keys matching the glob `pattern`, sorted.
    pub async fn keys(&self, pattern: &str) -> Vec<String> {
        let mut keys: Vec<String> = self.connection.clone().keys(pattern).await.expect("KEYS");
        keys.sort();
        keys
    }

    pub async fn get(&self, key: &str) -> Option<String> {
        self.connection.clone().get(key).await.expect("GET")
    }

    /// Remaining life of `key` in milliseconds (negative when it has none or doesn't exist).
    pub async fn ttl_ms(&self, key: &str) -> i64 {
        self.connection.clone().pttl(key).await.expect("PTTL")
    }
}
