// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! Infrastructure layer: concrete [`super::domain::CdcSource`] adapters, one per OLTP engine.

mod postgres;

pub use postgres::{PostgresSnapshotSource, PostgresWalSource};
