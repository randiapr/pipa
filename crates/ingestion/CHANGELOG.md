# Changelog

All notable changes to `pipa-ingestion` (renamed from `pipa-core`) are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(pre-1.0: MINOR bumps may include breaking changes).

## [0.6.0] - 2026-10-09

### Added

- Only the tables chosen for a data source (`ingested_tables` in its `datasources/` JSON) are
  captured; changes to any other table are skipped without holding back the slot's confirm. A
  table chosen later is captured from then on, with no backfill of its earlier changes.
- The registered data sources are read again every `INGESTION_SOURCE_REFRESH_INTERVAL_SECS`
  (default 30): a changed table choice reaches the running capture without a restart, a newly
  registered source starts capturing and a removed one stops. A source keeps capturing with no
  table chosen, so its replication slot keeps advancing rather than holding WAL.

### Changed

- A source without `ingested_tables` (registered before it existed), which used to capture
  every table, now captures exactly the tables it already has in pipa — worked out by
  ingestion itself, so it doesn't depend on `pipa-backend` being deployed first. A source
  whose existing tables can't be listed isn't started until they can (its slot keeps the
  changes meanwhile).

### Fixed

- Backfilling `pipa.cdc.key_columns` matched source tables to target tables without the
  target name's sanitizing, so a table whose name has uppercase letters or characters
  outside `[a-z0-9_]` never got its key recorded.

## [0.5.0] - 2026-10-09

### Added

- Each target table records its source table's row key (replica identity columns, the primary
  key by default) as the Iceberg table property `pipa.cdc.key_columns` (a JSON array; `[]` for
  a table without one): on creation, and backfilled for existing tables at the start of every
  capture session. `pipa-backend` uses it to show current rows from Iceberg alone.

### Changed

- With `ICEBERG_CATALOG_URI` unset (now the default, locally and in compose), ingestion embeds
  the `pipa-catalog-proxy` signer in its own process on a loopback port and points its catalog
  client there, instead of going through a separate `catalog` service. It also enables S3 Tables
  on `RUSTFS_BUCKET` at startup (retrying for about a minute, and failing startup if that never
  succeeds). Setting `ICEBERG_CATALOG_URI` still selects any other catalog as before. The old
  unset default, `<RUSTFS_ENDPOINT>/iceberg` unsigned, never worked against RustFS.
- `pipa-catalog-proxy` is now its one workspace dependency; it still doesn't depend on
  `pipa-backend`.

### Fixed

- Capture for a source stopped for good when its session ended — e.g. the source Postgres
  restarted (`Connection reset by peer`) or the source or catalog was unreachable at startup —
  until the process was restarted. It now reconnects with exponential backoff (1s doubling to
  60s, reset after a session that stayed up a minute), resuming from the replication slot as on
  a cold start.
- SIGTERM (what `docker stop`/`docker compose down` send) was ignored, so containers were only
  killed after the stop timeout. It now shuts down like Ctrl-C.
- `_commit_timestamp_us` was 30 years early: Postgres sends commit times as microseconds since
  2000-01-01, and they were stored unconverted instead of as microseconds since the Unix epoch.
  Rows written before this fix keep the old values.
- Overlapping transactions lost data: dedup compared each change's own position with the
  table's checkpoint, but a transaction that starts first and commits last carries lower change
  positions than one already landed, so its rows were skipped as "already committed".
  Checkpoints, dedup and confirms now use the transaction's commit position, and a batch is only
  committed between transactions so none is ever split. New checkpoints are stored as
  `pipa.cdc.commit_position`; a table whose last snapshot still carries the old
  `pipa.cdc.position` is deduplicated the old way until its next commit, so upgrading doesn't
  duplicate rows. Rows already lost this way are not recovered.
- A table that stopped changing held the replication slot at its last commit, so the source
  kept all WAL from there on (and every restart replayed it). The slot is now confirmed up to
  just below the oldest buffered transaction, or to the latest delivered position when nothing
  is buffered.
- The slot never advanced while the source's own tables were idle, even as the server wrote
  WAL elsewhere. Commits and between-transaction keepalives now count as progress.
- Startup could skip a transaction that was still buffered for a brand-new table when the
  previous session stopped: it resumed from the lowest checkpoint in Iceberg, past the slot.
  Capture now always resumes from the slot's own position.

## [0.4.0] - 2026-10-07

### Fixed

- Iceberg commits failed with `StorageFactory must be provided for RestCatalog` against iceberg
  0.10 (no built-in S3 file IO). The catalog is now built with an explicit
  `OpenDalStorageFactory::S3` (new `iceberg-storage-opendal` dependency).
- Every batch wrote its data file as `pipa-cdc-00000.parquet`, so the second commit to a table
  failed with "Cannot add files that are already referenced by table" and stayed buffered
  forever. Data file names now carry a per-batch UUIDv7 prefix.

### Changed

- The Iceberg catalog is now reached through `pipa-catalog-proxy` in the compose stack
  (`ICEBERG_CATALOG_URI=http://catalog:8080/iceberg`), since RustFS's embedded catalog requires
  SigV4-signed requests that `iceberg-catalog-rest` cannot produce. Pointing
  `ICEBERG_CATALOG_URI` at RustFS's `/iceberg` directly (the unset default) no longer works.

## [0.3.1] - 2026-10-07

### Changed

- Test fixtures build source ids with UUIDv7 (`Uuid::now_v7`), matching `pipa-backend`'s id
  generation. Bumped `tokio` to 1.53.2.

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
