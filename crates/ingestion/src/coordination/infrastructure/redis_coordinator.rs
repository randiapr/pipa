//! [`RedisCoordinator`]: live instances and per-source claims kept in Redis.
//!
//! Keys (all with a TTL, so a dead instance just disappears):
//! - `pipa:ingestion:member:{instance}` — present while the instance keeps renewing it.
//! - `pipa:ingestion:lease:{source_id}` — the instance id that currently captures the source.
//!
//! Every round an instance renews its member key, lists the members, and works out which
//! registered sources it wins by rendezvous hashing ([`owner_of`]). It claims the ones it wins
//! (taking a free lease or renewing its own, atomically in a Lua script) and stops renewing the
//! ones it doesn't, which lapse unless it releases them first. A source is returned as owned
//! only while its lease is held, so a new owner waits for the old one to let go — by
//! [`release`](Coordinator::release) after stopping the capture, or by the lease lapsing.
//!
//! Redis holds only this ephemeral liveness state, so it needs no persistence, and nothing
//! needs it for correctness (see the module doc of [`crate::coordination`]): a single plain
//! instance is enough, with no Redlock.

use std::collections::{BTreeSet, HashSet};
use std::time::Duration;

use async_trait::async_trait;
use redis::Client;
use redis::aio::ConnectionManager;
use tokio::sync::Mutex;

use crate::coordination::domain::{CoordinationError, Coordinator, InstanceId, owner_of};
use crate::datasource::DataSourceId;

const MEMBER_PREFIX: &str = "pipa:ingestion:member:";
const LEASE_PREFIX: &str = "pipa:ingestion:lease:";

/// Takes the lease if it is free, or renews it if this instance already holds it. 1 = held.
const CLAIM: &str = r"
local holder = redis.call('GET', KEYS[1])
if holder == false or holder == ARGV[1] then
  redis.call('SET', KEYS[1], ARGV[1], 'PX', ARGV[2])
  return 1
end
return 0
";

/// Deletes the lease only if this instance still holds it.
const RELEASE: &str = r"
if redis.call('GET', KEYS[1]) == ARGV[1] then
  return redis.call('DEL', KEYS[1])
end
return 0
";

pub struct RedisCoordinator {
    client: Client,
    /// Connected on first use, so a Redis that is down at startup is just another outage to
    /// retry, not a reason not to start.
    connection: Mutex<Option<ConnectionManager>>,
    instance: InstanceId,
    /// How long a member key or lease lives without being renewed: three rounds.
    ttl: Duration,
    /// Longest a round may take before it counts as an outage.
    timeout: Duration,
}

impl RedisCoordinator {
    pub fn new(url: &str, instance: InstanceId, interval: Duration) -> anyhow::Result<Self> {
        Ok(Self {
            client: Client::open(url)?,
            connection: Mutex::new(None),
            instance,
            ttl: interval * 3,
            timeout: interval,
        })
    }

    async fn connection(&self) -> Result<ConnectionManager, CoordinationError> {
        let mut slot = self.connection.lock().await;
        if let Some(connection) = slot.as_ref() {
            return Ok(connection.clone());
        }
        let connection = ConnectionManager::new(self.client.clone())
            .await
            .map_err(unavailable)?;
        *slot = Some(connection.clone());
        Ok(connection)
    }

    fn member_key(instance: &str) -> String {
        format!("{MEMBER_PREFIX}{instance}")
    }

    fn lease_key(source: DataSourceId) -> String {
        format!("{LEASE_PREFIX}{}", source.0)
    }

    /// Shows this instance alive and returns every live member, itself included.
    async fn heartbeat(
        &self,
        connection: &mut ConnectionManager,
    ) -> Result<BTreeSet<InstanceId>, redis::RedisError> {
        redis::cmd("SET")
            .arg(Self::member_key(&self.instance))
            .arg(1)
            .arg("PX")
            .arg(self.ttl.as_millis() as u64)
            .query_async::<()>(connection)
            .await?;

        let mut members = BTreeSet::new();
        let mut cursor: u64 = 0;
        loop {
            let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(format!("{MEMBER_PREFIX}*"))
                .arg("COUNT")
                .arg(100)
                .query_async(connection)
                .await?;
            members.extend(
                keys.iter()
                    .filter_map(|key| key.strip_prefix(MEMBER_PREFIX))
                    .map(str::to_owned),
            );
            if next == 0 {
                break;
            }
            cursor = next;
        }
        members.insert(self.instance.clone());
        Ok(members)
    }

    async fn round(
        &self,
        registered: &[DataSourceId],
    ) -> Result<HashSet<DataSourceId>, CoordinationError> {
        let mut connection = self.connection().await?;
        let members = self.heartbeat(&mut connection).await.map_err(unavailable)?;

        let wanted: Vec<DataSourceId> = registered
            .iter()
            .copied()
            .filter(|source| {
                owner_of(&members, *source).is_some_and(|owner| *owner == self.instance)
            })
            .collect();
        if wanted.is_empty() {
            return Ok(HashSet::new());
        }

        let mut pipeline = redis::pipe();
        for source in &wanted {
            pipeline.add_command(eval(
                CLAIM,
                Self::lease_key(*source),
                &self.instance,
                Some(self.ttl),
            ));
        }
        let held: Vec<i64> = pipeline
            .query_async(&mut connection)
            .await
            .map_err(unavailable)?;
        Ok(wanted
            .into_iter()
            .zip(held)
            .filter_map(|(source, held)| (held == 1).then_some(source))
            .collect())
    }

    async fn release_leases(&self, sources: &[DataSourceId]) -> Result<(), CoordinationError> {
        if sources.is_empty() {
            return Ok(());
        }
        let mut connection = self.connection().await?;
        let mut pipeline = redis::pipe();
        for source in sources {
            pipeline.add_command(eval(
                RELEASE,
                Self::lease_key(*source),
                &self.instance,
                None,
            ));
        }
        pipeline
            .query_async::<()>(&mut connection)
            .await
            .map_err(unavailable)
    }
}

#[async_trait]
impl Coordinator for RedisCoordinator {
    async fn assign(
        &self,
        registered: &[DataSourceId],
    ) -> Result<HashSet<DataSourceId>, CoordinationError> {
        tokio::time::timeout(self.timeout, self.round(registered))
            .await
            .map_err(|_| CoordinationError::Unavailable("timed out".to_owned()))?
    }

    async fn release(&self, sources: &[DataSourceId]) {
        match tokio::time::timeout(self.timeout, self.release_leases(sources)).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                tracing::warn!(error = %err, "could not release claims, they lapse on their own");
            }
            Err(_) => tracing::warn!("timed out releasing claims, they lapse on their own"),
        }
    }

    async fn leave(&self, held: &[DataSourceId]) {
        let leave = async {
            self.release_leases(held).await?;
            let mut connection = self.connection().await?;
            redis::cmd("DEL")
                .arg(Self::member_key(&self.instance))
                .query_async::<()>(&mut connection)
                .await
                .map_err(unavailable)
        };
        match tokio::time::timeout(self.timeout, leave).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                tracing::warn!(error = %err, "could not leave the group, it lapses on its own");
            }
            Err(_) => tracing::warn!("timed out leaving the group, it lapses on its own"),
        }
    }
}

/// `EVAL` of `script` on one `key`, with the instance (and the lease length) as arguments.
/// Sent as `EVAL` rather than `EVALSHA`: a pipelined `EVALSHA` doesn't fall back to loading the
/// script when Redis doesn't know it (a fresh or restarted Redis), and the scripts are tiny.
fn eval(script: &str, key: String, instance: &str, ttl: Option<Duration>) -> redis::Cmd {
    let mut cmd = redis::cmd("EVAL");
    cmd.arg(script).arg(1).arg(key).arg(instance);
    if let Some(ttl) = ttl {
        cmd.arg(ttl.as_millis() as u64);
    }
    cmd
}

fn unavailable(err: impl std::fmt::Display) -> CoordinationError {
    CoordinationError::Unavailable(err.to_string())
}

/// Live tests against a real Redis, run with `REDIS_URL=redis://127.0.0.1:6379 cargo test -p
/// pipa-ingestion --bin pipa-ingestion -- --ignored redis_coordinator`. They only touch keys of
/// instances and sources they make up, so any Redis will do — but they wait out real lease
/// lapses, so they take several seconds.
#[cfg(test)]
mod live_tests {
    use std::time::Duration;

    use uuid::Uuid;

    use super::*;

    fn coordinator(name: &str, run: Uuid) -> RedisCoordinator {
        let url = std::env::var("REDIS_URL").expect("REDIS_URL must point at a Redis");
        RedisCoordinator::new(&url, format!("{name}-{run}"), Duration::from_secs(1)).unwrap()
    }

    fn sources(count: usize) -> Vec<DataSourceId> {
        (0..count).map(|_| DataSourceId(Uuid::now_v7())).collect()
    }

    /// Rounds until `check` holds, failing after `within`.
    async fn eventually(within: Duration, mut check: impl AsyncFnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            if check().await {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "did not settle in {within:?}"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    #[tokio::test]
    #[ignore = "needs a Redis at REDIS_URL"]
    async fn redis_coordinator_shares_sources_and_fails_over() {
        let run = Uuid::now_v7();
        let (a, b) = (coordinator("a", run), coordinator("b", run));
        let registered = sources(40);
        let all: HashSet<_> = registered.iter().copied().collect();

        // Both join (b announces itself, then a sees both), and the sources split between them
        // with no overlap and nothing dropped.
        b.assign(&registered).await.unwrap();
        let (mut owned_a, mut owned_b) = (HashSet::new(), HashSet::new());
        eventually(Duration::from_secs(3), async || {
            owned_a = a.assign(&registered).await.unwrap();
            owned_b = b.assign(&registered).await.unwrap();
            owned_a.len() + owned_b.len() == all.len()
        })
        .await;
        assert!(owned_a.is_disjoint(&owned_b));
        assert!(
            !owned_a.is_empty() && !owned_b.is_empty(),
            "one instance took everything"
        );

        // b leaves cleanly: a picks up all its sources straight away.
        b.leave(&owned_b.iter().copied().collect::<Vec<_>>()).await;
        let mut taken = HashSet::new();
        eventually(Duration::from_secs(3), async || {
            taken = a.assign(&registered).await.unwrap();
            taken == all
        })
        .await;

        // A new member c joins, then dies without a word: a gets everything back only after
        // c's member key and leases lapse.
        let c = coordinator("c", run);
        c.assign(&registered).await.unwrap();
        let mut with_c = HashSet::new();
        eventually(Duration::from_secs(8), async || {
            with_c = c.assign(&registered).await.unwrap();
            a.assign(&registered).await.unwrap();
            !with_c.is_empty()
        })
        .await;
        drop(c);
        eventually(Duration::from_secs(8), async || {
            a.assign(&registered).await.unwrap() == all
        })
        .await;

        a.leave(&all.iter().copied().collect::<Vec<_>>()).await;
    }

    #[tokio::test]
    #[ignore = "needs a Redis at REDIS_URL"]
    async fn redis_coordinator_reports_an_outage() {
        // Nothing listens on this port.
        let down = RedisCoordinator::new(
            "redis://127.0.0.1:1",
            "lonely".to_owned(),
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(down.assign(&sources(3)).await.is_err());
    }
}
