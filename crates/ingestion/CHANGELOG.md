# Changelog

All notable changes to `pipa-ingestion` (renamed from `pipa-core`) are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(pre-1.0: MINOR bumps may include breaking changes).

## [0.2.0] - 2026-09-25

### Changed

- **Renamed from `pipa-core` to `pipa-ingestion`.**
- **No longer depends on `pipa-data`.** `storage.rs`/`datasource.rs` duplicate just enough of
  `ObjectStoreConfig` and the `DataSource` shape to read what `pipa-rest` writes to the shared
  RustFS object store's `datasources/` JSON layout — read-only, no repository port, no write
  path. This crate now takes zero dependencies on any other crate in this workspace, so its
  release/deploy cycle is fully decoupled from `pipa-data`/`pipa-rest`'s.

### Added

- **Static sharding** for running multiple instances: `INGESTION_SHARD_INDEX`/
  `INGESTION_SHARD_COUNT` env vars (default `0`/`1`) deterministically split registered
  sources by id across instances, so each instance owns a disjoint subset — necessary since a
  Postgres replication slot can only be consumed by one client at a time.

## [0.1.0] - 2026-09-22

### Added

- CDC engine startup: reads registered OLTP data sources from the shared `pipa-data`
  object store (no direct dependency on `pipa-rest` — both read/write the same store).
- **Postgres change capture** (`capture/`): a `CdcSource` port and engine-agnostic
  `ChangeEvent`/`Operation`/`ColumnValue` domain shape, plus a `PostgresWalSource` adapter
  using `pgwire-replication` for logical replication (WAL). Includes a hand-written
  `pgoutput` message decoder (Relation/Insert/Update/Delete), unit-tested against
  synthetic byte sequences and a live-Postgres integration test (`--ignored`, requires
  `wal_level=logical`). Automatically provisions the `FOR ALL TABLES` publication and
  replication slot a data source needs on first use.
- `main.rs` streams and logs captured changes for every registered Postgres source at
  startup. MySQL capture is not implemented yet — MySQL sources are logged and skipped.
