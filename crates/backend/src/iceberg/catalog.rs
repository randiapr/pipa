// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! Apache Iceberg catalog access.

use std::collections::HashMap;
use std::sync::Arc;

use crate::storage::ObjectStoreConfig;
use iceberg::io::{
    S3_ACCESS_KEY_ID, S3_ENDPOINT, S3_PATH_STYLE_ACCESS, S3_REGION, S3_SECRET_ACCESS_KEY,
};
use iceberg::{Catalog, CatalogBuilder};
use iceberg_catalog_rest::{
    REST_CATALOG_PROP_URI, REST_CATALOG_PROP_WAREHOUSE, RestCatalogBuilder,
};
use iceberg_storage_opendal::OpenDalStorageFactory;
use pipa_catalog_auth::{
    DEFAULT_SERVICE, SigV4AuthManager, enable_s3_tables_with_retry, rustfs_catalog_uri,
    sigv4::Signer,
};
use serde::{Deserialize, Serialize};

/// Connection settings for the Iceberg REST catalog backing CDC target tables. Defaults to
/// RustFS's own embedded "S3 Tables" Iceberg REST Catalog rather than a separately-run service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergCatalogConfig {
    pub name: String,
    pub uri: String,
    pub warehouse: String,
    /// Signs catalog requests (RustFS's catalog only accepts SigV4-signed ones); `None` sends
    /// them unsigned.
    #[serde(skip)]
    pub sigv4: Option<Signer>,
}

impl IcebergCatalogConfig {
    /// Reads connection settings from `ICEBERG_CATALOG_*` environment variables. `uri` defaults to
    /// RustFS's "S3 Tables" catalog, which RustFS embeds directly into the object store itself
    /// (`store`'s endpoint, `/iceberg` path), so no separate catalog service needs to be run. That
    /// catalog only accepts SigV4-signed requests, so the default signs them with `store`'s
    /// credentials (and first enables S3 Tables on `store`'s bucket). Set `ICEBERG_CATALOG_URI`
    /// to use some other catalog instead; its requests are sent unsigned.
    pub async fn from_env(store: &ObjectStoreConfig) -> anyhow::Result<Self> {
        let (uri, sigv4) = match std::env::var("ICEBERG_CATALOG_URI") {
            Ok(uri) if !uri.is_empty() => (uri, None),
            _ => {
                let signer = Signer::new(
                    store.access_key_id.clone(),
                    store.secret_access_key.clone(),
                    store.region.clone(),
                    DEFAULT_SERVICE.to_string(),
                );
                enable_s3_tables_with_retry(&store.endpoint, &store.bucket, &signer).await?;
                (rustfs_catalog_uri(&store.endpoint), Some(signer))
            }
        };
        Ok(Self {
            name: std::env::var("ICEBERG_CATALOG_NAME").unwrap_or_else(|_| "pipa".to_string()),
            uri,
            warehouse: std::env::var("ICEBERG_CATALOG_WAREHOUSE")
                .unwrap_or_else(|_| "pipa".to_string()),
            sigv4,
        })
    }

    /// Builds a REST [`Catalog`] client for this catalog. `store` supplies the RustFS/S3
    /// credentials the client uses for direct FileIO access to table data/metadata files — the
    /// REST server itself only serves metadata, it doesn't proxy file contents.
    pub async fn build_catalog(
        &self,
        store: &ObjectStoreConfig,
    ) -> anyhow::Result<Arc<dyn Catalog>> {
        let props = HashMap::from([
            (REST_CATALOG_PROP_URI.to_string(), self.uri.clone()),
            (
                REST_CATALOG_PROP_WAREHOUSE.to_string(),
                self.warehouse.clone(),
            ),
            (S3_ENDPOINT.to_string(), store.endpoint.clone()),
            (S3_REGION.to_string(), store.region.clone()),
            (S3_ACCESS_KEY_ID.to_string(), store.access_key_id.clone()),
            (
                S3_SECRET_ACCESS_KEY.to_string(),
                store.secret_access_key.clone(),
            ),
            (S3_PATH_STYLE_ACCESS.to_string(), "true".to_string()),
        ]);

        // iceberg ships no S3 FileIO of its own: the REST catalog needs an explicit storage
        // factory to read/write table data and metadata files.
        let mut builder = RestCatalogBuilder::default().with_storage_factory(Arc::new(
            OpenDalStorageFactory::S3 {
                customized_credential_load: None,
            },
        ));
        // A fresh manager per catalog: `iceberg-catalog-rest` doesn't allow sharing one.
        if let Some(signer) = &self.sigv4 {
            builder = builder.with_auth_manager(SigV4AuthManager::new(signer.clone()));
        }
        let catalog = builder.load(self.name.clone(), props).await?;

        Ok(Arc::new(catalog))
    }
}
