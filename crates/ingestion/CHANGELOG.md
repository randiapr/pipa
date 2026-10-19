# Changelog

All notable changes to `pipa-ingestion` (renamed from `pipa-core`) are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(pre-1.0: MINOR bumps may include breaking changes).

## [0.9.1] - 2026-10-19

### Changed

- Updated `pgwire-replication` to 0.5.0. An acknowledged position now reaches the server within
  the status interval even on an idle stream, so the replication slot advances (and WAL is
  released) sooner when nothing is changing.
- A capture session now waits for the server to accept `START_REPLICATION` before it counts as
  open. A refusal fails the session start (and is retried with backoff) rather than surfacing
  later on the stream, and the snapshot backfill no longer starts ahead of it.
- The replication connection is named `pipa-ingestion` in `pg_stat_activity` and
  `pg_stat_replication`.

### Fixed

- The capture loop's timer no longer ticks the moment a session starts. That tick could win a race
  against the stream and confirm just below a transaction that was already buffered, which made
  some orchestrator unit tests fail now and then. Nothing is lost by dropping it: there is nothing
  to flush, retry or confirm at that point.

## [0.9.0] - 2026-10-18

### Added

- Snapshot backfill. The rows a chosen table already holds are read and written to its target
  table, so its current rows are the whole table rather than only the rows changed since it was
  chosen. Rows are read with plain key-ordered pages (`INGESTION_BACKFILL_CHUNK_ROWS`, 5000), so
  no lock is held on the source. Each row lands as a changelog row with `_op = 'read'` and
  `_position = '0/0'`, below every captured change, so any change captured after the backfill
  began wins over it. Each page commits together with the table property `pipa.cdc.backfill`
  (`running` with the last key, then `done`), so a backfill that stops resumes after the last
  page that landed. The table's checkpoint is carried forward unchanged. Tables without a row
  key are not backfilled (`skipped`). `INGESTION_BACKFILL=off` turns it off.

### Changed

- Upgrading backfills every chosen table once, including tables that were already being
  captured. Expect read load on the sources while that runs, or set `INGESTION_BACKFILL=off`
  first.
- Choosing a table creates its target table straight away, even while the source table is
  empty.
- A table is backfilled once. A table chosen again after being unchosen isn't read again.

## [0.8.1] - 2026-10-16

### Changed

- `publish = false`: the crate is released as a binary or container image, never to crates.io.
- `rustls` 0.23.45 → 0.23.46 (`Cargo.lock`), fixing RUSTSEC-2026-0348 and RUSTSEC-2026-0349.

## [0.8.0] - 2026-10-16

### Added

- Table maintenance. Every `INGESTION_MAINTENANCE_INTERVAL_SECS` (default 300; 0 turns it off),
  each instance maintains the target tables of the sources it captures, one table at a time:
  - **Compaction**: runs of small data files (under `INGESTION_COMPACTION_TARGET_FILE_BYTES` ×
    `INGESTION_COMPACTION_SMALL_FILE_RATIO`, 128 MiB × 0.75) are rewritten into files of up to
    the target size once `INGESTION_COMPACTION_MIN_INPUT_FILES` (8) have built up, at most
    `INGESTION_COMPACTION_MAX_INPUT_BYTES` (1 GiB) per table per run. Files are grouped in commit
    order, so each new file covers one contiguous range of commits. Each rewrite is an Iceberg
    `replace` snapshot carrying the table's checkpoint forward, so dedup is unaffected. A
    rewrite racing an ingestion commit is rebuilt on top of it and retried, and one whose input
    files are gone is dropped. Rows are counted before and after: a mismatch replaces nothing.
  - **Snapshot expiry**: snapshots older than `INGESTION_SNAPSHOT_MAX_AGE_SECS` (3600) are
    expired, keeping at least `INGESTION_SNAPSHOT_MIN_TO_KEEP` (10), and the files no remaining
    snapshot mentions are deleted. No data goes with them: every change is still a changelog
    row. Only time travel to older table states is lost.
  - **Orphan sweep**: Parquet data files and Avro manifests under a table's location that
    nothing references (left by commits that never landed) are deleted once older than
    `INGESTION_ORPHAN_CLEANUP_AGE_SECS` (3 days; 0 turns it off). Each table is swept at most
    hourly.

### Changed

- The checkpoint is read from the nearest snapshot that carries one. Previously it was read
  from the current snapshot only, so a snapshot made by another engine (for example its own
  compaction) no longer resets dedup.

## [0.7.1] - 2026-10-15

### Changed

- The `pipa-ingestion` Docker image is published for `linux/amd64` and `linux/arm64`, so the
  compose stack runs natively on Apple Silicon and other arm64 hosts. No code change.

## [0.7.0] - 2026-10-14

### Added

- Instances can now share sources dynamically. With `REDIS_URL` set they register in Redis,
  split the registered sources by rendezvous hashing and renew a lease on each source they own,
  so an instance can be added or removed with only about 1/N of the sources moving, and a dead
  instance's sources move to the others once its keys lapse (three intervals;
  `INGESTION_COORDINATION_INTERVAL_SECS`, default 5). `INGESTION_INSTANCE_ID` names an instance
  and must be unique; unset, one is generated per process. A source is handed over only after
  its old owner has stopped the capture and released the lease, and delivery stays
  effectively-once because the replication slot and the Iceberg checkpoints hold all the
  state. If Redis can't be reached the current assignment stands: running captures continue and
  nothing is rebalanced until it answers.
- `LOG_FORMAT=json` makes the logs one JSON object per line; otherwise they stay human-readable,
  now coloured only when stdout is a terminal.

### Changed

- Without `REDIS_URL` the static `INGESTION_SHARD_INDEX`/`INGESTION_SHARD_COUNT` split behaves
  as before, now behind the same coordination interface; the assignment is re-evaluated on
  its own interval, and shutdown stops every capture before handing the sources back.

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
