# Changelog

All notable changes to `pipa-api` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(pre-1.0: MINOR bumps may include breaking changes).

## [0.3.0] - 2026-10-08

### Added

- `Role::Developer` (`"developer"` on the wire), alongside `admin` and `user`.
- `table` module for read-only table browsing: `TableView`, `Tables`/`TablesResponse` and
  `ReadTableRequest`, with the `TABLES` (`/tables`) and `TABLE_ROWS` (`/tables/rows`) paths.

### Changed

- **Breaking:** adding a `Role` variant makes existing exhaustive `match`es on it
  non-exhaustive. `user` now means view-only; the old "everything inside my projects" meaning
  is `developer`.

## [0.2.0] - 2026-09-30

### Added

- `user` module: `Role` (`admin`/`user`), `LoginRequest`, `LoginData`/`LoginResponse`,
  `MeResponse`, `NewUser`, `UserUpdate`, `UserView` and the `/users` list/single wrappers.
- Route paths `LOGIN`, `ME`, `USERS`, `USER` and the `user(id)` helper.
- `ResponseCode::Unauthorized` (2005) and `ResponseCode::Forbidden` (2006).
- `QueryRequest::project_id`, which scopes a query to one project's tables.

### Changed

- `QueryRequest` gained a field, so code building it with a struct literal must set
  `project_id` (`None` for an unscoped admin query).

## [0.1.0] - 2026-09-29

### Added

- **Initial release: the HTTP API contract between `pipa-ui` and `pipa-backend`.** Previously
  `pipa-ui` hand-mirrored the backend's wire types and response envelope, so the two could drift
  silently; both now compile against this crate instead.
- Request/response bodies for `/datasources` (`NewDataSource`, `DataSourceView`,
  `ConnectionConfig`, `DbEngine`, `ConnectionTestOutcome`, and the `DataSources`/`DataSourceData`/
  `ConnectionTest` payloads), `/projects` (`NewProject`, `ProjectUpdate`, `ProjectView`, and the
  `Projects`/`ProjectData` payloads) and `/query` (`QueryRequest`, `Rows`).
- The shared response envelope — `BaseResponse<T>`, the `ResponseCode` registry, `ErrorBody`,
  `Empty` — plus `*Response` type aliases for each route. The envelope both serializes and
  deserializes, so clients parse errors with the same type the backend emits.
- `path` module: route pattern constants the backend registers (axum `{id}` syntax) and helpers
  that build the concrete client paths.

### Notes

- Wire types only: no validation, persistence, or HTTP-framework code. Depends on `serde` and
  `serde_json` alone so it builds for both native targets and `wasm32-unknown-unknown`.
- Identifiers are UUID `String`s rather than `uuid::Uuid`, since `uuid`'s `v4` feature doesn't
  build on wasm32 without extra setup. `pipa-backend` validates them at its HTTP boundary.
- Used by `pipa-backend` and `pipa-ui` only; `pipa-ingestion` does not depend on it.
