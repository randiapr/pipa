//! `pipa-catalog-proxy` as a standalone service: the [`CatalogProxy`] that `pipa-backend` and
//! `pipa-ingestion` embed in-process, served on its own port for catalog clients outside this
//! workspace. On startup it enables S3 Tables on the bucket, retrying until RustFS is reachable,
//! and exits non-zero instead of serving if that never succeeds.
//!
//! Config (the `RUSTFS_*` variables are the same ones the other services read):
//! - `RUSTFS_ENDPOINT` — upstream RustFS base URL (default `http://localhost:9000`)
//! - `RUSTFS_ACCESS_KEY_ID` / `RUSTFS_SECRET_ACCESS_KEY` — signing credentials (default `rustfsadmin`)
//! - `RUSTFS_BUCKET` — bucket to enable S3 Tables on (default `pipa`)
//! - `RUSTFS_REGION` — signing region (default `ap-southeast-3`)
//! - `CATALOG_PROXY_SERVICE` — SigV4 signing name (default `s3`, what RustFS expects)
//! - `CATALOG_PROXY_LISTEN` — listen address (default `0.0.0.0:8080`)

use std::sync::Arc;

use anyhow::Context;
use pipa_catalog_proxy::{CatalogProxy, DEFAULT_SERVICE, sigv4::Signer};

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let proxy = Arc::new(CatalogProxy::new(
        &env_or("RUSTFS_ENDPOINT", "http://localhost:9000"),
        Signer::new(
            env_or("RUSTFS_ACCESS_KEY_ID", "rustfsadmin"),
            env_or("RUSTFS_SECRET_ACCESS_KEY", "rustfsadmin"),
            env_or("RUSTFS_REGION", "ap-southeast-3"),
            env_or("CATALOG_PROXY_SERVICE", DEFAULT_SERVICE),
        ),
    )?);
    proxy
        .enable_tables_with_retry(&env_or("RUSTFS_BUCKET", "pipa"))
        .await?;
    let listen = env_or("CATALOG_PROXY_LISTEN", "0.0.0.0:8080");
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .with_context(|| format!("failed to bind {listen}"))?;
    tracing::info!(%listen, upstream = %proxy.upstream(), "pipa-catalog-proxy listening");
    axum::serve(listener, proxy.router()).await?;
    Ok(())
}
