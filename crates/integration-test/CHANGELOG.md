# Changelog

All notable changes to `pipa-integration-test` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(pre-1.0: MINOR bumps may include breaking changes).

## [0.3.3] - 2026-10-21

### Changed

- Added the MIT license header (`// Copyright (c) 2026 Randi Apriansyah` and `// SPDX-License-Identifier: MIT`) to every `.rs` file, checked in CI by `.licenserc.yaml`. No code change.

## [0.3.2] - 2026-10-20

### Changed

- `tokio` 1.53.3 → 1.54.0 (`Cargo.toml`, `Cargo.lock`). No code change.

## [0.3.1] - 2026-10-19

### Changed

- Updated `rstest` to 0.28.0. `auth::the_backend_refuses_to_start_with_an_unsafe_configuration`
  now marks its optional settings `#[case(opt)]`, so a case names only the values it sets (`_`
  skips one) instead of spelling out `Some(..)`/`None`.

## [0.3.0] - 2026-10-18

### Added

- `cdc`: existing rows are backfilled page by page as `read` rows that later changes win over.
  A table without a primary key is not backfilled.
- `pipeline::wait_for_backfill` and `pipeline::target_table`.

### Changed

- `Pipeline` setup also waits for the chosen tables' backfill, so the changes a test makes are
  captured as changes rather than read in as existing rows.
- `cdc::a_table_chosen_later_is_backfilled_and_captured_from_then_on` (was
  `…_is_ingested_from_then_on`) expects the rows from before the choice. `cache`'s table-list
  test expects the table its backfill creates.

## [0.2.1] - 2026-10-18

### Changed

- The RustFS container is `rustfs/rustfs:1.0.3` (was 1.0.1), matching `docker-compose.yml`. The
  suite passes against 1.0.2, and 1.0.3 keeps its server code.

## [0.2.0] - 2026-10-16

### Added

- `maintenance`: small data files are compacted and old snapshots expired while every row,
  and ingestion itself, carries on unchanged.
- `IngestionConfig::extra_env`, `TestEnv::ingestion_with` and `Pipeline::start_configured`,
  for tests that tune their ingestion instance.

## [0.1.0] - 2026-10-15

### Added

- End-to-end tests (`cargo test -p pipa-integration-test --test e2e`, or `just test-integration`;
  Docker required) that run the real `pipa-backend` and `pipa-ingestion` binaries against RustFS,
  Postgres and Redis started with testcontainers, driven over HTTP with the `pipa-api` types. 269
  tests, positive and negative, with authorization checked for each role:
  - sign-in, tokens, CORS and the backend's startup refusals (`auth`);
  - accounts, including the last-admin protection (`users`), and projects (`projects`);
  - data sources: registering, connection test, the table explorer, choosing tables
    (`datasources`);
  - change capture from Postgres into Iceberg: inserts, updates, deletes, transactions, column
    types, ingestion stopped, crashed and restarted without loss or duplicates (`cdc`);
  - table browsing (`tables`) and project-scoped SQL (`query`);
  - the query cache in memory and in Redis (`cache`);
  - how ingestion instances share sources through Redis or static shards, and hand them over
    (`coordination`).
- Most tests share one backend (and one ingestion instance), each making its own projects,
  accounts, databases and sources, so the suite runs in about a minute; tests that need a pristine
  store, their own settings, or to stop ingestion start their own.
- Regression tests for the list-during-delete 500 and the project delete/update 404s fixed in
  `pipa-backend` 0.12.0.
