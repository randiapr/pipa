//! Apache Iceberg catalog and query integration, backing `pipa-backend`'s `POST /query`.
//! Depends on `crate::storage` for RustFS/S3 object-store config, but keeps every
//! Iceberg/DataFusion dependency confined to this module.

pub mod catalog;
pub mod query;

pub use catalog::IcebergCatalogConfig;
pub use query::{QueryError, QueryService, namespace_for_source};
