// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! Change capture: streams row-level changes out of a registered OLTP data source.
//!
//! Same clean-architecture split `pipa-backend`'s bounded contexts use:
//! - [`domain`] — the engine-agnostic `ChangeEvent`/`Operation` shape and the `CdcSource`
//!   port infrastructure adapters implement.
//! - [`infrastructure`] — one adapter per OLTP engine (currently `PostgresWalSource`; a
//!   MySQL binlog adapter follows the same shape).
//! - [`application`] — `CaptureOrchestrator`, which drives a `CdcSource` and
//!   [`crate::write::IcebergWriter`] together: batching, per-table dedup, and confirming
//!   consumed positions back to the adapter only once a batch durably commits to Iceberg. See
//!   its module doc for the exactly-once design.
//! - [`backfill`] — reading the rows chosen tables already held through a `SnapshotSource`,
//!   committed by the orchestrator next to the captured changes.

pub mod application;
pub mod backfill;
pub mod domain;
pub mod infrastructure;

pub use application::{BatchConfig, CaptureOrchestrator};
pub use backfill::BackfillConfig;
pub use domain::{CdcSource, SnapshotSource};
pub use infrastructure::{PostgresSnapshotSource, PostgresWalSource};
