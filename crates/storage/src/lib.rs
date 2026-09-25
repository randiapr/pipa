//! RustFS/S3 object-store access and OLTP data source/project management, used by `pipa-backend`
//! (including its own `iceberg` module, for `ObjectStoreConfig`). Not used by `pipa-ingestion` —
//! that crate deliberately duplicates the bits it needs instead, to stay standalone (see its own
//! docs). Apache Iceberg catalog/query integration lives in `pipa-backend::iceberg` instead of
//! here — it depends on this crate for `ObjectStoreConfig`, not the other way around.

pub mod datasource;
pub mod project;
pub mod storage;

pub use storage::ObjectStoreConfig;
