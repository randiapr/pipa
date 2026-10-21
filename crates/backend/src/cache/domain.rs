// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! The [`Cache`] port.

use std::time::Duration;

use async_trait::async_trait;

/// A key-value store for results that may be recomputed at any time.
///
/// Infallible on purpose: an adapter that can't reach its backing store answers "not cached" and
/// drops writes, so a cache outage costs speed and never availability.
#[async_trait]
pub trait Cache: Send + Sync {
    /// The value stored under `key`, if it is there and hasn't expired.
    async fn get(&self, key: &str) -> Option<Vec<u8>>;

    /// Stores `value` under `key` for `ttl`, replacing what was there.
    async fn set(&self, key: &str, value: Vec<u8>, ttl: Duration);
}
