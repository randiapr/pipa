//! Apache Iceberg catalog access.

use serde::{Deserialize, Serialize};

/// Connection settings for the Iceberg catalog backing CDC target tables.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergCatalogConfig {
    pub name: String,
    pub uri: String,
    pub warehouse: String,
}
