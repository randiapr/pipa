//! Apache Iceberg catalog access.

use std::collections::HashMap;
use std::sync::Arc;

use iceberg::io::{
    S3_ACCESS_KEY_ID, S3_ENDPOINT, S3_PATH_STYLE_ACCESS, S3_REGION, S3_SECRET_ACCESS_KEY,
};
use iceberg::{Catalog, CatalogBuilder};
use iceberg_catalog_rest::{
    REST_CATALOG_PROP_URI, REST_CATALOG_PROP_WAREHOUSE, RestCatalogBuilder,
};
use pipa_storage::ObjectStoreConfig;
use serde::{Deserialize, Serialize};

/// Connection settings for the Iceberg REST catalog backing CDC target tables.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergCatalogConfig {
    pub name: String,
    pub uri: String,
    pub warehouse: String,
}

impl IcebergCatalogConfig {
    /// Reads connection settings from `ICEBERG_CATALOG_*` environment variables, falling back
    /// to defaults suited to a local REST catalog dev instance.
    pub fn from_env() -> Self {
        Self {
            name: std::env::var("ICEBERG_CATALOG_NAME").unwrap_or_else(|_| "pipa".to_string()),
            uri: std::env::var("ICEBERG_CATALOG_URI")
                .unwrap_or_else(|_| "http://localhost:8181".to_string()),
            warehouse: std::env::var("ICEBERG_CATALOG_WAREHOUSE")
                .unwrap_or_else(|_| "pipa".to_string()),
        }
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

        let catalog = RestCatalogBuilder::default()
            .load(self.name.clone(), props)
            .await?;

        Ok(Arc::new(catalog))
    }
}
