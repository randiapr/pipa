// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! End-to-end tests of the real `pipa-backend` and `pipa-ingestion` binaries, against RustFS,
//! Postgres and Redis in containers. See the docs of `pipa_integration_test` for how they run,
//! and `just test-integration`.
//!
//! One module per area; `cargo test -p pipa-integration-test --test e2e -- <filter>` runs the
//! tests whose path contains `<filter>` (e.g. `auth::` or `cdc::`). A test is named after what
//! happens, e.g. `login_with_a_wrong_password_is_unauthorized`, and covers either a positive or a
//! negative case. Docker must be running.

mod auth;
mod cache;
mod cdc;
mod common;
mod coordination;
mod datasources;
mod maintenance;
mod projects;
mod query;
mod tables;
mod users;
