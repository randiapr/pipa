// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! Harness for the end-to-end tests in `tests/e2e`.
//!
//! The tests drive the real `pipa-backend` and `pipa-ingestion` binaries (built from this
//! workspace, see [`binaries`]) as child processes, over HTTP and against real services started
//! with [testcontainers](https://github.com/testcontainers/testcontainers-rs): RustFS (object
//! store and Iceberg catalog), Postgres (the CDC source) and Redis (cache and ingestion
//! coordination). Nothing is mocked and no workspace crate's internals are linked in, only the
//! `pipa-api` wire types, so a test sees exactly what a client outside the workspace sees.
//!
//! The containers are shared by all tests ([`infra`]); everything else is per test: each test gets
//! a bucket of its own (so its users, projects, data sources and Iceberg tables are its own),
//! its own backend (and ingestion) process on that bucket, and its own Postgres database.

pub mod backend;
pub mod binaries;
pub mod cleanup;
pub mod env;
pub mod infra;
pub mod ingestion;
pub mod pipeline;
pub mod postgres;
pub mod process;
pub mod redis_db;
pub mod reply;
pub mod shared;
pub mod wait;
