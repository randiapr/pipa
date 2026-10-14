//! Coordination: which running `pipa-ingestion` instance captures which registered data source.
//!
//! A Postgres replication slot tolerates one consumer, so instances must never race to attach
//! to the same source. Same clean-architecture split as [`crate::capture`]:
//! - [`domain`] — the [`Coordinator`] port and the rendezvous-hashing assignment rule.
//! - [`infrastructure`] — [`StaticCoordinator`] (`INGESTION_SHARD_INDEX`/`INGESTION_SHARD_COUNT`,
//!   no external service) and [`RedisCoordinator`] (membership heartbeats and per-source leases
//!   in Redis, so instances can come and go).
//!
//! Coordination only decides *liveness* — who should be capturing what, and how fast a dead
//! instance's sources are taken over. It is not what keeps delivery correct: the slot admits one
//! consumer and the write is idempotent (see [`crate::capture::application`]), so even two
//! instances that briefly both believe they own a source cannot lose or duplicate data.

pub mod domain;
pub mod infrastructure;

pub use domain::Coordinator;
pub use infrastructure::from_env;
