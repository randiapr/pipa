# Changelog

All notable changes to `pipa-backend` (renamed from `pipa-rest`) are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(pre-1.0: MINOR bumps may include breaking changes).

## [0.10.0] - 2026-10-09

### Added

- Data source explorer: `GET /datasources/{id}/tables` lists the tables of the source's
  database with their columns (type, nullability, primary key, and the column a foreign key
  references — the first by constraint name if the column is in several), read live with the source's
  credentials (502 when the database can't be read), each flagged with whether it is ingested.
  `PUT /datasources/{id}/tables` replaces the tables the source ingests. Both need a developer
  or admin with access to the source's project, like the other `/datasources` routes.
- `DataSource::ingested_tables`, persisted in the `datasources/` JSON that `pipa-ingestion`
  reads. A source ingests no table until some are chosen.
- At startup, in the background, every source stored before `ingested_tables` existed — which
  was captured in full — gets the tables it already has in pipa as its choice, so the explorer
  shows what is captured. Its database's tables are matched forward to their pipa names
  (`iceberg::table_for_source_table`, the same sanitized `{schema}__{table}` as
  `pipa-ingestion`), so the match is exact. A source that can't be reached is retried at the
  next start; a choice saved meanwhile is never overwritten.

## [0.9.0] - 2026-10-09

### Changed

- With `ICEBERG_CATALOG_URI` unset (now the default, locally and in compose), the backend embeds
  the `pipa-catalog-proxy` signer in its own process on a loopback port and points its catalog
  client there, instead of going through a separate `catalog` service. It also enables S3 Tables
  on `RUSTFS_BUCKET` at startup (retrying for about a minute, and failing startup if that never
  succeeds). Setting `ICEBERG_CATALOG_URI` still selects any other catalog as before. The old
  unset default, `<RUSTFS_ENDPOINT>/iceberg` unsigned, never worked against RustFS.
- The Iceberg metadata tables (`…$snapshots`, `…$manifests`) are no longer browsable, for any
  role: `GET /tables` leaves them out, and `POST /tables/rows` answers 400 when one is named.
  `POST /query` can still select them.
- `POST /tables/rows` returns a table's current rows, for every role: the latest change per
  row key, without deleted rows or the changelog columns (`_op`, `_source_id`, `_position`,
  `_commit_timestamp_us`), sorted by key — so an updated row appears once, with its current
  values. The key is read from the Iceberg table property `pipa.cdc.key_columns` that
  `pipa-ingestion` 0.5.0 records; a table without it is returned as stored. The changelog itself
  stays available through `POST /query`.

## [0.8.0] - 2026-10-08

### Added

- `developer` role: everything inside its assigned projects (data sources, `POST /query`, tables)
  except user and project management.
- Read-only table browsing for every role with access to the project, including `user`:
  `GET /tables?project_id=` lists the Iceberg tables of the project's data sources and
  `POST /tables/rows` reads a page of one (100 rows by default, at most 1000). Neither accepts
  SQL; a read names a data source of the project and a table, and runs in a session that only
  sees that source's namespace.

### Changed

- **Breaking:** the `user` role is now view-only. It gets `403` on `/datasources` (whose views
  carry connection passwords) and `POST /query`; it can only use `/tables`. Anything that needs
  the old `user` permissions requires `developer`.
- Stored accounts with role `user` are promoted to `developer` once at startup, so nobody loses
  access. A marker object, `migrations/user-role-to-developer.done`, keeps view-only users
  created afterwards from being promoted on later starts.

## [0.7.0] - 2026-10-07

### Fixed

- `POST /query` failed with `StorageFactory must be provided for RestCatalog` against iceberg 0.10,
  which ships no S3 file IO of its own. The REST catalog is now built with an explicit
  `OpenDalStorageFactory::S3` (new `iceberg-storage-opendal` dependency).

### Changed

- The Iceberg catalog is now reached through `pipa-catalog-proxy` in the compose stack
  (`ICEBERG_CATALOG_URI=http://catalog:8080/iceberg`): RustFS's embedded catalog rejects unsigned
  requests and `iceberg-catalog-rest` cannot sign. Pointing `ICEBERG_CATALOG_URI` at RustFS's
  `/iceberg` directly (the unset default) no longer works; set it to the proxy.

## [0.6.1] - 2026-10-07

### Changed

- New data source and project ids are now UUIDv7 (`Uuid::now_v7`) instead of v4, so they sort
  by creation time. Existing ids are unaffected. Bumped `tokio` to 1.53.2.

## [0.6.0] - 2026-09-30

### Added

- **Authentication and roles.** New `user` bounded context (`src/user/`): accounts persisted
  under `users/` in the object store, Argon2id password hashes, HS256 JWTs (8h) as bearer
  tokens. `POST /auth/login`, `GET /auth/me`, and admin-only `/users` CRUD. The token carries
  only the user id and the account is re-read on every request, so role and project changes
  (and deletions) apply immediately. The last admin cannot be deleted or demoted.
- Every route except `/healthz` and `/auth/login` now requires `Authorization: Bearer <token>`
  (401 otherwise, 403 when the role or project assignment doesn't allow it).
- **Project-scoped access.** `admin` sees every project; `user` only the projects assigned to
  it. Creating, editing and deleting projects is admin-only. Data sources are limited to
  accessible projects (project-less ones are admin-only), and `GET /datasources` accepts
  `?project_id=`.
- **Project-scoped queries.** `POST /query` takes a `project_id`; the SQL only sees the
  Iceberg namespaces of that project's data sources. A non-admin must name one of their
  projects; an admin may omit it. Queries are now read-only: DDL, DML and session statements
  are rejected.
- Config: `JWT_SECRET` (required, at least 32 bytes), `PIPA_ADMIN_USERNAME`/
  `PIPA_ADMIN_PASSWORD` (create the first admin while no user exists; startup fails with no
  users), `CORS_ALLOWED_ORIGINS`.

### Changed

- CORS is no longer permissive: only `CORS_ALLOWED_ORIGINS` (default `http://localhost:3000`),
  the `GET/POST/PUT/DELETE` methods, and the `Authorization`/`Content-Type` headers.
- **Breaking:** the backend refuses to start without `JWT_SECRET` and a first admin.
- A `JWT_SECRET` shorter than 32 bytes now fails startup with `invalid JWT_SECRET` (it used to
  surface as an unnamed "token secret" error), so a misconfigured container's log points at the
  variable.

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
