# Changelog

All notable changes to `pipa-catalog-proxy` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(pre-1.0: MINOR bumps may include breaking changes).

## [0.3.3] - 2026-10-21

### Changed

- Added the MIT license header (`// Copyright (c) 2026 Randi Apriansyah` and `// SPDX-License-Identifier: MIT`) to every `.rs` file, checked in CI by `.licenserc.yaml`. No code change.

## [0.3.2] - 2026-10-20

### Changed

- `tokio` 1.53.3 → 1.54.0 (`Cargo.toml`, `Cargo.lock`). No code change.

## [0.3.1] - 2026-10-16

### Changed

- `publish = false`: the crate is released as a binary or container image, never to crates.io.
- `rustls` 0.23.45 → 0.23.46 (`Cargo.lock`), fixing RUSTSEC-2026-0348 and RUSTSEC-2026-0349.

## [0.3.0] - 2026-10-14

### Added

- The standalone binary honors `LOG_FORMAT=json` (one JSON object per line); otherwise its logs
  stay human-readable, now coloured only when stdout is a terminal.

## [0.2.0] - 2026-10-09

### Added

- The crate is now also a library (`pipa_catalog_proxy`): `CatalogProxy` (signing, forwarding,
  `enable_tables`/`enable_tables_with_retry`, `router`) and the `sigv4` module are public.
- `CatalogProxy::serve_embedded`: enables S3 Tables on a bucket, then serves the proxy on an
  ephemeral loopback port inside the calling process and returns the catalog URI to give
  `iceberg-catalog-rest`. `pipa-backend` and `pipa-ingestion` use it instead of the separate
  service, so catalog calls no longer cross the network.

### Changed

- The binary is now a thin wrapper over the library; its behavior and configuration are
  unchanged.

## [0.1.0] - 2026-10-07

### Added

- Initial release: a reverse proxy that SigV4-signs (signing name `s3`) every request to RustFS's
  embedded Iceberg REST catalog ("S3 Tables"), which rejects unsigned requests, so
  `pipa-backend` and `pipa-ingestion` can use `iceberg-catalog-rest`, which cannot sign.
  The signing is implemented in `src/sigv4.rs` on `hmac`/`sha2` (checked against AWS's published
  `get-vanilla` test vector); the crate depends on no AWS SDK, crate or image.
- On startup it enables S3 Tables on `RUSTFS_BUCKET` (default `pipa`) with an idempotent signed
  `PUT /iceberg/v1/buckets/{bucket}`, retrying for about a minute, and exits non-zero instead of
  serving if that never succeeds.
- Configured with the `RUSTFS_*` variables the other services use, plus `CATALOG_PROXY_SERVICE`
  and `CATALOG_PROXY_LISTEN`. Request bodies are buffered up to 16 MiB (413 beyond that); an
  unreachable upstream is a 502.
