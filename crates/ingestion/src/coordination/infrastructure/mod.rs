//! Infrastructure layer: concrete [`super::domain::Coordinator`] adapters, and the environment
//! config that picks between them.

mod redis_coordinator;
mod static_shards;

use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use super::domain::{Coordinator, InstanceId};
pub use redis_coordinator::RedisCoordinator;
pub use static_shards::StaticCoordinator;

/// A coordinator and how often to ask it.
pub struct Coordination {
    pub coordinator: Arc<dyn Coordinator>,
    pub interval: Duration,
}

/// Picks the coordinator from the environment: with `REDIS_URL` set, [`RedisCoordinator`]
/// (instances discover each other and sources move when one leaves or dies); otherwise
/// [`StaticCoordinator`], the `INGESTION_SHARD_INDEX`/`INGESTION_SHARD_COUNT` partitioning that
/// needs no external service.
///
/// - `INGESTION_COORDINATION_INTERVAL_SECS` (default 5) — how often a Redis instance renews its
///   claims. They lapse after three missed intervals, which is how long a dead instance's
///   sources wait for a new owner.
/// - `INGESTION_INSTANCE_ID` — this instance's name among its peers; **must be unique** per
///   running instance. Unset, a fresh one is made per process. Setting it to something stable
///   (a StatefulSet pod name, say) lets a restarted instance pick its claims up where it left
///   off instead of waiting for the old ones to lapse.
pub fn from_env() -> anyhow::Result<Coordination> {
    let interval = Duration::from_secs(
        std::env::var("INGESTION_COORDINATION_INTERVAL_SECS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(5)
            .max(1),
    );

    let redis_url = std::env::var("REDIS_URL")
        .ok()
        .filter(|url| !url.is_empty());
    let Some(redis_url) = redis_url else {
        let coordinator = StaticCoordinator::from_env()?;
        return Ok(Coordination {
            coordinator: Arc::new(coordinator),
            interval,
        });
    };

    if std::env::var_os("INGESTION_SHARD_INDEX").is_some()
        || std::env::var_os("INGESTION_SHARD_COUNT").is_some()
    {
        tracing::warn!(
            "REDIS_URL is set, so INGESTION_SHARD_INDEX/INGESTION_SHARD_COUNT are ignored"
        );
    }
    let instance = std::env::var("INGESTION_INSTANCE_ID")
        .ok()
        .filter(|id| !id.is_empty())
        .unwrap_or_else(generated_instance_id);
    tracing::info!(%instance, "coordinating through Redis");
    let coordinator = RedisCoordinator::new(&redis_url, instance, interval)?;
    Ok(Coordination {
        coordinator: Arc::new(coordinator),
        interval,
    })
}

fn generated_instance_id() -> InstanceId {
    let host = std::env::var("HOSTNAME")
        .ok()
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| "pipa-ingestion".to_owned());
    format!("{host}-{}", Uuid::now_v7().simple())
}
