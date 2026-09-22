# Changelog

All notable changes to `pipa-data` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(pre-1.0: MINOR bumps may include breaking changes).

## [0.1.0] - 2026-09-22

### Added

- Iceberg catalog config/access (`catalog.rs`).
- RustFS/S3-compatible object store config and client builder (`storage.rs`),
  `ObjectStoreConfig::from_env()`/`build_store()`.
- **OLTP data source bounded context** (`datasource/`): a `DataSource` aggregate
  (`DbEngine`, `ConnectionConfig`) with validation, behind `DataSourceRepository` and
  `ConnectionTester` ports; a `DataSourceService` application use case
  (register/list/get/remove/test_connection); and infrastructure adapters —
  `ObjectStoreDataSourceRepository` (persists sources as JSON in the shared object store)
  and `SqlxConnectionTester` (verifies reachability with a short-lived Postgres/MySQL
  connection).
