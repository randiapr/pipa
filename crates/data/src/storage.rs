//! RustFS / S3-compatible object storage access underlying Iceberg tables.

use std::sync::Arc;

use object_store::{aws::AmazonS3Builder, ObjectStore};
use serde::{Deserialize, Serialize};

/// Connection settings for the RustFS (S3-compatible) object store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectStoreConfig {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    /// Whether to allow plain HTTP, typical for a local/dev RustFS endpoint.
    pub allow_http: bool,
}

impl ObjectStoreConfig {
    /// Reads connection settings from `RUSTFS_*` environment variables, falling back to
    /// defaults suited to a local RustFS instance for development.
    pub fn from_env() -> Self {
        Self {
            endpoint: std::env::var("RUSTFS_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:9000".to_string()),
            bucket: std::env::var("RUSTFS_BUCKET").unwrap_or_else(|_| "pipa".to_string()),
            region: std::env::var("RUSTFS_REGION").unwrap_or_else(|_| "us-east-1".to_string()),
            // Matches the `--access-key`/`--secret-key` the `just rustfs` recipe starts the
            // local dev server with, so `just core`/`just rest` connect with no extra setup.
            access_key_id: std::env::var("RUSTFS_ACCESS_KEY_ID")
                .unwrap_or_else(|_| "rustfsadmin".to_string()),
            secret_access_key: std::env::var("RUSTFS_SECRET_ACCESS_KEY")
                .unwrap_or_else(|_| "rustfsadmin".to_string()),
            allow_http: std::env::var("RUSTFS_ALLOW_HTTP")
                .map(|value| value != "false")
                .unwrap_or(true),
        }
    }

    /// Builds an [`ObjectStore`] client for this RustFS/S3-compatible endpoint.
    pub fn build_store(&self) -> anyhow::Result<Arc<dyn ObjectStore>> {
        let store = AmazonS3Builder::new()
            .with_endpoint(&self.endpoint)
            .with_bucket_name(&self.bucket)
            .with_region(&self.region)
            .with_access_key_id(&self.access_key_id)
            .with_secret_access_key(&self.secret_access_key)
            .with_allow_http(self.allow_http)
            .build()?;
        Ok(Arc::new(store))
    }
}
