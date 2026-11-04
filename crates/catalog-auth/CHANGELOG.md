# Changelog

All notable changes to `pipa-catalog-auth` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(pre-1.0: MINOR bumps may include breaking changes).

## [0.1.0] - 2026-11-04

### Added

- Replaces `pipa-catalog-proxy` (last released as 0.3.4). It does the same SigV4 signing for RustFS's embedded Iceberg REST catalog, now inside the catalog client rather than through a loopback listener:
  - `SigV4AuthManager` implements iceberg-rust 0.11's `iceberg_catalog_rest::AuthManager`, for `RestCatalogBuilder::with_auth_manager`. Its sessions sign every catalog request, the `/v1/config` handshake included.
  - `sigv4::Signer::sign` signs a request's headers in place, and `Signer::sign_request` signs a built `reqwest` request (`sigv4.rs` is otherwise carried over unchanged, along with its AWS `get-vanilla` test).
  - `enable_s3_tables` / `enable_s3_tables_with_retry` send the idempotent signed `PUT /iceberg/v1/buckets/{bucket}` directly, with the same retry policy as before (30 attempts, 2 s apart).
  - `rustfs_catalog_uri` returns the catalog URI RustFS serves at an S3 endpoint (`{endpoint}/iceberg`).
  - `Signer`'s `Debug` output leaves out the secret key.

### Removed

- The embedded loopback proxy (`CatalogProxy`, `serve_embedded`). The `axum` dependency goes with it.
- The standalone `pipa-catalog-proxy` binary, its Docker target and its `CATALOG_PROXY_SERVICE` / `CATALOG_PROXY_LISTEN` settings. Catalog clients outside the workspace need SigV4 support of their own.
