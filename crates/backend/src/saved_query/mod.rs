// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! Bounded context: SQL queries a project keeps by name (the analytics queries its developers
//! rerun), so they don't have to be retyped.
//!
//! Organized with the same clean-architecture split as [`crate::project`]:
//! - [`domain`] — the `SavedQuery` aggregate and the `SavedQueryRepository` port.
//! - [`application`] — `SavedQueryService`, the use cases orchestrating that port.
//! - [`infrastructure`] — a concrete object-store-backed repository adapter.
//!
//! Running a saved query is not a use case of this context: the UI sends its SQL to
//! `POST /query`, so project scoping, read-only enforcement and the result cache apply as for any
//! other query.

pub mod application;
pub mod domain;
pub mod infrastructure;

pub use application::SavedQueryService;
pub use domain::{NewSavedQuery, SavedQuery, SavedQueryError, SavedQueryId, SavedQueryUpdate};
