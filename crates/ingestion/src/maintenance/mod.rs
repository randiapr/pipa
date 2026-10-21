// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! Table maintenance: keeps the target tables the write path appends to fast to read and
//! bounded in size — compaction of small data files, snapshot expiry, and deleting the files
//! nothing references any more.
//!
//! Same clean-architecture split as [`crate::capture`]/[`crate::write`]/[`crate::coordination`]:
//! - [`domain`] — [`domain::MaintenanceConfig`], the compaction planner, and the
//!   [`domain::TableMaintainer`] port.
//! - [`infrastructure`] — [`infrastructure::IcebergTableMaintainer`], on the Iceberg REST catalog
//!   and the object store.
//! - [`application`] — [`application::MaintenanceScheduler`], which runs it for the sources this
//!   instance owns.

pub mod application;
pub mod domain;
pub mod infrastructure;

pub use application::MaintenanceScheduler;
pub use domain::MaintenanceConfig;
pub use infrastructure::{IcebergTableMaintainer, OrphanSweeper, RestSnapshotCommitter};
