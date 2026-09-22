//! Iceberg table and RustFS/object-store access shared by `pipa-core` and `pipa-rest`.

pub mod catalog;
pub mod datasource;
pub mod storage;

pub use catalog::IcebergCatalogConfig;
pub use storage::ObjectStoreConfig;
