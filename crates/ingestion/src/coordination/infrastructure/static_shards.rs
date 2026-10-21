// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! [`StaticCoordinator`]: sources are partitioned across a fixed number of instances by id.

use std::collections::HashSet;

use async_trait::async_trait;

use crate::coordination::domain::{CoordinationError, Coordinator};
use crate::datasource::DataSourceId;

/// `INGESTION_SHARD_INDEX`/`INGESTION_SHARD_COUNT` (default `0`/`1`, one instance handling
/// everything) split the registered sources deterministically by id, so N instances each own a
/// disjoint subset without talking to each other.
///
/// This is static sharding, not consistent hashing: changing the shard count reassigns most
/// sources to a different shard, briefly interrupting their capture as the old shard's slot is
/// released and the new shard's instance reattaches. That is safe for delivery guarantees
/// because checkpoint state lives durably in Iceberg, not in-process — a source moving to a
/// different instance behaves exactly like a cold restart of the old one. It also never moves a
/// source off an instance that has died: that needs [`super::RedisCoordinator`].
pub struct StaticCoordinator {
    index: u32,
    count: u32,
}

impl StaticCoordinator {
    pub fn new(index: u32, count: u32) -> anyhow::Result<Self> {
        let count = count.max(1);
        if index >= count {
            anyhow::bail!(
                "INGESTION_SHARD_INDEX ({index}) must be less than INGESTION_SHARD_COUNT ({count})"
            );
        }
        Ok(Self { index, count })
    }

    pub fn from_env() -> anyhow::Result<Self> {
        let index: u32 = std::env::var("INGESTION_SHARD_INDEX")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let count: u32 = std::env::var("INGESTION_SHARD_COUNT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(1);
        tracing::info!(
            shard_index = index,
            shard_count = count.max(1),
            "static shard coordination"
        );
        Self::new(index, count)
    }

    /// Deterministically assigns `source` to one of the shards from its id — stable across
    /// restarts and independent of registration order.
    fn shard_of(&self, source: DataSourceId) -> u32 {
        (source.0.as_u128() % u128::from(self.count)) as u32
    }
}

#[async_trait]
impl Coordinator for StaticCoordinator {
    async fn assign(
        &self,
        registered: &[DataSourceId],
    ) -> Result<HashSet<DataSourceId>, CoordinationError> {
        Ok(registered
            .iter()
            .copied()
            .filter(|source| self.shard_of(*source) == self.index)
            .collect())
    }

    async fn release(&self, _sources: &[DataSourceId]) {}

    async fn leave(&self, _held: &[DataSourceId]) {}
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    fn id(n: u128) -> DataSourceId {
        DataSourceId(Uuid::from_u128(n))
    }

    #[tokio::test]
    async fn shards_partition_the_sources() {
        let registered: Vec<_> = (0..10).map(id).collect();
        let mut seen = HashSet::new();
        for index in 0..3 {
            let owned = StaticCoordinator::new(index, 3)
                .unwrap()
                .assign(&registered)
                .await
                .unwrap();
            assert!(seen.is_disjoint(&owned), "a source belongs to two shards");
            seen.extend(owned);
        }
        assert_eq!(seen.len(), registered.len());
    }

    #[test]
    fn rejects_an_index_outside_the_count() {
        assert!(StaticCoordinator::new(2, 2).is_err());
        assert!(StaticCoordinator::new(0, 0).is_ok());
    }
}
