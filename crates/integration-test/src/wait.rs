// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! Polling, for things that happen on their own schedule (ingestion capturing a change, a lease
//! expiring).

use std::future::Future;
use std::time::{Duration, Instant};

/// How long [`eventually`] waits by default: generous, since a pass returns as soon as it holds.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(90);

/// Calls `check` every 250 ms until it returns `Ok`, and returns that value. Panics with the last
/// `Err` once `timeout` has passed.
pub async fn eventually_within<T, Fut>(
    what: &str,
    timeout: Duration,
    mut check: impl FnMut() -> Fut,
) -> T
where
    Fut: Future<Output = Result<T, String>>,
{
    let deadline = Instant::now() + timeout;
    loop {
        match check().await {
            Ok(value) => return value,
            Err(last) if Instant::now() >= deadline => {
                panic!("timed out after {timeout:?} waiting for {what}; last state: {last}")
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(250)).await,
        }
    }
}

/// [`eventually_within`] with [`DEFAULT_TIMEOUT`].
pub async fn eventually<T, Fut>(what: &str, check: impl FnMut() -> Fut) -> T
where
    Fut: Future<Output = Result<T, String>>,
{
    eventually_within(what, DEFAULT_TIMEOUT, check).await
}

/// Asserts that `check` keeps returning `Ok` for the whole of `duration`: for "nothing happens",
/// which can only be told from "hasn't happened yet" by waiting.
pub async fn stays<Fut>(what: &str, duration: Duration, mut check: impl FnMut() -> Fut)
where
    Fut: Future<Output = Result<(), String>>,
{
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        if let Err(violation) = check().await {
            panic!("{what} did not hold: {violation}");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
