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
use pipa_catalog_proxy::{CatalogProxy, DEFAULT_SERVICE, sigv4::Signer};
use serde::{Deserialize, Serialize};

/// Connection settings for the Iceberg REST catalog backing CDC target tables. Defaults to
/// RustFS's own embedded "S3 Tables" Iceberg REST Catalog rather than a separately-run service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergCatalogConfig {
    pub name: String,
    pub uri: String,
    pub warehouse: String,
}

impl IcebergCatalogConfig {
    /// Reads connection settings from `ICEBERG_CATALOG_*` environment variables. `uri` defaults to
    /// RustFS's "S3 Tables" catalog, which RustFS embeds directly into the object store itself
    /// (`store`'s endpoint, `/iceberg` path), so no separate catalog service needs to be run. That
    /// catalog only accepts SigV4-signed requests, which `iceberg-catalog-rest` can't make, so the
    /// default starts a `pipa-catalog-proxy` signer inside this process (which also enables S3
    /// Tables on `store`'s bucket) and points `uri` at it. Set `ICEBERG_CATALOG_URI` to use some
    /// other catalog, or a standalone `pipa-catalog-proxy`, instead.
    pub async fn from_env(store: &ObjectStoreConfig) -> anyhow::Result<Self> {
        let uri = match std::env::var("ICEBERG_CATALOG_URI") {
            Ok(uri) if !uri.is_empty() => uri,
            _ => embedded_catalog_uri(store).await?,
        };
        Ok(Self {
            name: std::env::var("ICEBERG_CATALOG_NAME").unwrap_or_else(|_| "pipa".to_string()),
            uri,
            warehouse: std::env::var("ICEBERG_CATALOG_WAREHOUSE")
                .unwrap_or_else(|_| "pipa".to_string()),
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

        // iceberg 0.10 ships no S3 FileIO of its own: the REST catalog needs an explicit storage
        // factory to read/write table data and metadata files.
        let catalog = RestCatalogBuilder::default()
            .with_storage_factory(Arc::new(OpenDalStorageFactory::S3 {
                customized_credential_load: None,
            }))
            .load(self.name.clone(), props)
            .await?;

        Ok(Arc::new(catalog))
    }
}

/// Starts the in-process signer in front of RustFS's catalog at `store`'s endpoint, with `store`'s
/// credentials, and returns its catalog URI.
async fn embedded_catalog_uri(store: &ObjectStoreConfig) -> anyhow::Result<String> {
    let signer = Signer::new(
        store.access_key_id.clone(),
        store.secret_access_key.clone(),
        store.region.clone(),
        DEFAULT_SERVICE.to_string(),
    );
    CatalogProxy::new(&store.endpoint, signer)?
        .serve_embedded(&store.bucket)
        .await
}
