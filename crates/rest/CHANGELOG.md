# Changelog

All notable changes to `pipa-rest` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(pre-1.0: MINOR bumps may include breaking changes).

## [0.1.0] - 2026-09-22

### Added

- Axum HTTP server (`0.0.0.0:8080`) with a `GET /healthz` liveness check.
- **Facade API for OLTP data sources**: `GET/POST /datasources`, `GET/DELETE
/datasources/{id}`, `POST /datasources/{id}/test`, wired to `pipa-data`'s
  `DataSourceService`. `pipa-rest` is the only HTTP-facing surface in the workspace —
  external callers (the `pipa-ui` dashboard, any future client) talk to it, never directly
  to `pipa-data` or `pipa-core`.
- Permissive CORS so the `pipa-ui` dev server (a different origin) can call the API.
