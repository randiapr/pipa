# Changelog

All notable changes to `pipa-ingestion` (renamed from `pipa-core`) are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(pre-1.0: MINOR bumps may include breaking changes).

## [0.3.0] - 2026-09-29

### Added

- **Iceberg write path** (`src/write/`): the `IcebergWriter` port and `IcebergChangelogWriter`
  adapter append captured changes to Iceberg as changelog rows (`_op`, `_source_id`,
  `_position`, `_commit_timestamp_us`, plus the source table's own columns) via `fast_append`.
  Target tables (`cdc_{source_id}.{schema}__{table}`) are auto-provisioned on first sight by
  introspecting the source Postgres table's columns via `information_schema`.
- **`CaptureOrchestrator`** (`capture/application.rs`): drives a `CdcSource` and an
  `IcebergWriter` together with per-table batching (`INGESTION_BATCH_MAX_EVENTS`,
  `INGESTION_BATCH_FLUSH_INTERVAL_MS`; defaults 1000 events / 5s), per-table position dedup,
  and a safe confirm floor that is withheld while any table has uncommitted pending events.
- **Effectively-once delivery**: each commit tags its Iceberg snapshot with the batch's
  high-watermark position (`pipa.cdc.position`), so the checkpoint lands atomically with the
  data it describes. On restart, capture resumes from what Iceberg reports as last committed
  rather than from any separate checkpoint store.
- `ICEBERG_CATALOG_NAME`/`ICEBERG_CATALOG_URI`/`ICEBERG_CATALOG_WAREHOUSE` env vars — an
  independent copy of `pipa-backend`'s catalog config, with `ICEBERG_CATALOG_URI` defaulting to
  RustFS's embedded S3 Tables catalog at `<RUSTFS_ENDPOINT>/iceberg`. `.env.example` and
  `docker-compose.yml` updated to match.

### Changed

- **`CdcSource::stream_changes` signature changed** (internal port; MINOR-level break): it now
  takes `resume_from: Option<&str>` and returns `ChangeStream { events, confirm }` instead of
  a bare receiver. The Postgres adapter advances the replication slot's `confirmed_flush_lsn`
  only in response to a `confirm` message, never on decode.
- `CaptureError` gains a `Checkpoint` variant for failures reading back a source's Iceberg
  checkpoint.
- `main.rs` now runs a shared `CaptureOrchestrator` per Postgres source instead of the previous
  log-only placeholder. MySQL sources are still skipped with a warning.
- New dependencies: `iceberg`, `iceberg-catalog-rest`, `arrow-array`, `arrow-schema`,
  `parquet`, `chrono`.
- Iceberg writes require S3 Tables to be enabled on the `pipa` bucket (see
  `docker-compose.yml`); without it, target-table provisioning and commits fail.

## [0.2.1] - 2026-09-26

### Changed

- `ObjectStoreConfig::from_env()`'s default `RUSTFS_REGION` fallback (used when the env var
  isn't set) changed from `us-east-1` to `ap-southeast-3`, matching `pipa-storage`'s
  duplicated copy; `.env.example` updated to match.

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
