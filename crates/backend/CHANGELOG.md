# Changelog

All notable changes to `pipa-backend` (renamed from `pipa-rest`) are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(pre-1.0: MINOR bumps may include breaking changes).

## [0.5.4] - 2026-09-29

### Changed

- **Request/response types and route paths now come from the new `pipa-api` contract crate**
  (shared with `pipa-ui`) instead of route-local structs. `src/http/convert.rs` maps between
  those wire types and the domain types; the domain layer is unchanged. The response envelope
  (`BaseResponse`, `ResponseCode`) moved into `pipa-api`.
- `POST /datasources` with a malformed `project_id` now returns `400` with the standard error
  envelope instead of axum's `422` JSON-rejection response.

## [0.5.3] - 2026-09-29

### Changed

- **Merged the `pipa-storage` crate into this one.** Its `datasource`, `project` and
  `storage` (`ObjectStoreConfig`) modules now live directly under `crates/backend/src/`
  (as `crate::datasource`, `crate::project`, `crate::storage`), keeping the same
  clean-architecture split; `pipa-storage` (last version 0.4.1) no longer exists as a
  workspace member. No HTTP-facing or env-var behavior changes. `pipa-ingestion` was
  already independent of it and is unaffected.
- Dropped the now-unused `DbEngine::default_port` (only `pipa-ui`'s own copy uses it) and
  trimmed re-exports that only made sense for a library crate.

## [0.5.2] - 2026-09-28

### Changed

- **Every response body now follows a shared `{response_code, response_message, ...}`
  envelope**: list endpoints nest their array under a plural resource key (e.g.
  `GET /projects` → `{..., "projects": [...]}`), single-resource endpoints nest under the
  singular key (e.g. `{..., "project": {...}}`), `/datasources/{id}/test` nests under
  `connection_test`, `POST /query` nests result rows under `rows`, and `/healthz` gains the
  same `response_code`/`response_message` pair flattened alongside its existing fields.
  `response_code` is an application-level code from a new global registry, independent of
  the real HTTP status still sent on the wire (e.g. a `404` can carry `response_code: 2001`).
- Error bodies are now `{response_code, response_message, error}` instead of `{"error": ...}`.
- **`DELETE /projects/{id}` and `DELETE /datasources/{id}` now return `200` with
  `{response_code, response_message}`** instead of `204 No Content` — a body is now always
  present, so the delete confirmation is no longer distinguished by an empty response.

## [0.5.1] - 2026-09-26

### Changed

- `.env.example`'s `RUSTFS_REGION` default updated to `ap-southeast-3`, matching
  `pipa-storage`/`pipa-ingestion`.

## [0.5.0] - 2026-09-25

### Changed

- **Architecture doc correction**: stopped describing this crate as a "facade in front of
  `pipa-storage`" in the root `README.md` and `CLAUDE.md` — it owns real behavior of its own
  (the Iceberg catalog/query integration), not just a pass-through. Now described as the
  workspace's only HTTP-facing service, combining OLTP data source/project management
  (backed by `pipa-storage`) with its own Iceberg integration.

## [0.4.0] - 2026-09-25

### Changed

- **Renamed from `pipa-rest` to `pipa-backend`**, and the crate directory from
  `crates/rest` to `crates/backend` — no functional change, just the name.
- **Absorbed the `pipa-iceberg` crate** as an `iceberg` module (`src/iceberg/`) — its
  `catalog.rs`/`query.rs` and `IcebergCatalogConfig`/`QueryError`/`QueryService` move here
  unchanged in behavior. `pipa-iceberg` no longer exists as a separate crate; every
  `iceberg`/`iceberg-datafusion`/`datafusion` dependency now lives directly in this crate's
  own `Cargo.toml` instead.

## [0.3.0] - 2026-09-25

### Added

- **`POST /query`**: runs ad-hoc SQL, via DataFusion, against the Iceberg tables a REST
  catalog exposes (`SELECT * FROM <catalog>.<namespace>.<table>`), backed by the new
  `pipa-iceberg` crate's `QueryService`. Returns the result rows as a raw JSON array.
  Errors map to `502` (catalog unreachable), `400` (bad SQL), or `500` (encoding failure).

### Changed

- Depends on the new `pipa-iceberg` crate (instead of `pipa-data`) for
  `IcebergCatalogConfig`/`QueryService` — no behavior change, just where the Iceberg
  dependency comes from.

## [0.2.1] - 2026-09-24

### Changed

- The projects API now maps `ProjectError::DuplicateName` to `409 Conflict` (previously
  unreachable, since `pipa-data` didn't reject duplicate names).

## [0.2.0] - 2026-09-23

### Added

- **Facade API for projects**: `GET/POST /projects`, `GET/PUT/DELETE /projects/{id}`, wired
  to `pipa-data`'s new `ProjectService`. `POST /datasources` and its `DataSource` responses
  now carry an optional `project_id`, linking a data source to a project.

## [0.1.0] - 2026-09-22

### Added

- Axum HTTP server (`0.0.0.0:8080`) with a `GET /healthz` liveness check.
- **Facade API for OLTP data sources**: `GET/POST /datasources`, `GET/DELETE
/datasources/{id}`, `POST /datasources/{id}/test`, wired to `pipa-data`'s
  `DataSourceService`. `pipa-rest` is the only HTTP-facing surface in the workspace —
  external callers (the `pipa-ui` dashboard, any future client) talk to it, never directly
  to `pipa-data` or `pipa-core`.
- Permissive CORS so the `pipa-ui` dev server (a different origin) can call the API.
