//! Reads registered OLTP data sources back out of the shared RustFS/S3 object store.
//!
//! Deliberately duplicated (not imported) from `pipa-storage::datasource`: `pipa-ingestion` only
//! ever reads what `pipa-backend` writes there, via the same JSON-under-`datasources/` layout, so
//! it needs just enough of the shape to deserialize it — not the write path, validation, or
//! the `DataSourceRepository` port abstraction pipa-storage's side maintains for that.

use futures::StreamExt;
use object_store::{ObjectStore, ObjectStoreExt, path::Path as ObjectPath};
use serde::Deserialize;
use uuid::Uuid;

const PREFIX: &str = "datasources";

/// Identity of a registered OLTP data source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
pub struct DataSourceId(pub Uuid);

impl std::fmt::Display for DataSourceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The OLTP database engine a source connects to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum DbEngine {
    #[serde(rename = "postgres")]
    Postgres,
    #[serde(rename = "mysql")]
    MySql,
}

/// Connection parameters for an OLTP data source.
#[derive(Debug, Clone, Deserialize)]
pub struct ConnectionConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub database: String,
}

/// A registered OLTP data source, as `pipa-backend` persists it. Fields pipa-backend's own
/// `DataSource` carries but ingestion never uses (e.g. `project_id`) are simply left out —
/// serde ignores JSON fields a struct doesn't declare.
#[derive(Debug, Clone, Deserialize)]
pub struct DataSource {
    pub id: DataSourceId,
    pub name: String,
    pub engine: DbEngine,
    pub connection: ConnectionConfig,
}

/// Lists every data source currently registered in the shared object store.
pub async fn list_registered(store: &dyn ObjectStore) -> anyhow::Result<Vec<DataSource>> {
    let mut listing = store.list(Some(&ObjectPath::from(PREFIX)));
    let mut sources = Vec::new();

    while let Some(meta) = listing.next().await {
        let meta = meta?;
        let bytes = store.get(&meta.location).await?.bytes().await?;
        sources.push(serde_json::from_slice(&bytes)?);
    }

    Ok(sources)
}
