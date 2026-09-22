# Changelog

All notable changes to `pipa-core` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(pre-1.0: MINOR bumps may include breaking changes).

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
