//! Iceberg write path: lands captured changes as append-only changelog rows.
//!
//! Same clean-architecture split [`crate::capture`] uses:
//! - [`domain`] — the [`domain::IcebergWriter`] port, [`domain::TargetTable`], and the
//!   `ChangeEvent` → Arrow `RecordBatch` mapping.
//! - [`infrastructure`] — the concrete adapter, [`infrastructure::IcebergChangelogWriter`], on
//!   `iceberg`/`iceberg-catalog-rest` — a second, independent copy of that dependency from
//!   `pipa-backend`'s own `src/iceberg/` (kept separate deliberately; see the root
//!   `CLAUDE.md`'s notes on why `pipa-ingestion` never depends on
//!   `pipa-backend`).
//!
//! Driven by [`crate::capture::application::CaptureOrchestrator`], which owns batching,
//! per-table dedup, and confirming consumed WAL positions back to the capture adapter only
//! after a commit through this module durably succeeds.

pub mod domain;
pub mod infrastructure;

pub use domain::IcebergWriter;
pub use infrastructure::{IcebergCatalogConfig, IcebergChangelogWriter};
