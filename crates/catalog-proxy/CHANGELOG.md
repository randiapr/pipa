# Changelog

All notable changes to `pipa-catalog-proxy` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(pre-1.0: MINOR bumps may include breaking changes).

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
