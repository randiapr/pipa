//! Change capture: streams row-level changes out of a registered OLTP data source.
//!
//! Same clean-architecture split `pipa-storage`'s bounded contexts use:
//! - [`domain`] — the engine-agnostic `ChangeEvent`/`Operation` shape and the `CdcSource`
//!   port infrastructure adapters implement.
//! - [`infrastructure`] — one adapter per OLTP engine (currently `PostgresWalSource`; a
//!   MySQL binlog adapter follows the same shape).
//!
//! There's no `application` layer yet: `main.rs` drives `CdcSource` directly. Once captured
//! changes are actually written into Iceberg, that orchestration (picking the adapter for a
//! source's `DbEngine`, checkpointing position, retrying) belongs in one.

pub mod domain;
pub mod infrastructure;

pub use domain::CdcSource;
pub use infrastructure::PostgresWalSource;
