//! Domain layer: the `DataSource` aggregate, its value objects, and the ports
//! (repository, connection tester, schema explorer) that the application layer depends on.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::project::domain::ProjectId;

/// Identity of a registered OLTP data source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DataSourceId(pub Uuid);

impl DataSourceId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for DataSourceId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for DataSourceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The OLTP database engine a source connects to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DbEngine {
    #[serde(rename = "postgres")]
    Postgres,
    #[serde(rename = "mysql")]
    MySql,
}

/// Connection parameters for an OLTP data source (value object).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub database: String,
}

/// A registered OLTP data source (aggregate root).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataSource {
    pub id: DataSourceId,
    pub name: String,
    pub engine: DbEngine,
    pub connection: ConnectionConfig,
    pub project_id: Option<ProjectId>,
    pub registered_at_unix: u64,
    /// The source tables `pipa-ingestion` captures; changes to any other table are skipped.
    /// Empty until tables are chosen, which is also what a source stored before this field
    /// existed reads as. `pipa-ingestion` reads this field from the same JSON
    /// (`crates/ingestion/src/datasource.rs`), so keep the two in step.
    #[serde(default)]
    pub ingested_tables: BTreeSet<TableRef>,
}

/// A table of a data source's database, by schema and name (value object). For MySQL the
/// schema is the database.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TableRef {
    pub schema: String,
    pub name: String,
}

/// A column of a source table, as the source database describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceColumn {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key: bool,
    /// The column this one references through a foreign key, if it is part of one. A column in
    /// several foreign keys reports the first by constraint name.
    pub foreign_key: Option<ColumnRef>,
}

/// A column of a source table, by table and name (value object).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnRef {
    pub table: TableRef,
    pub column: String,
}

/// A table found in a data source's database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTable {
    pub table: TableRef,
    pub columns: Vec<SourceColumn>,
}

/// Fields needed to register a new data source, before an identity is assigned.
#[derive(Debug, Clone, Deserialize)]
pub struct NewDataSource {
    pub name: String,
    pub engine: DbEngine,
    pub connection: ConnectionConfig,
    #[serde(default)]
    pub project_id: Option<ProjectId>,
}

impl DataSource {
    /// Validates and constructs a new `DataSource` aggregate, assigning it a fresh identity.
    pub fn register(new: NewDataSource) -> Result<Self, DataSourceError> {
        if new.name.trim().is_empty() {
            return Err(DataSourceError::InvalidField(
                "name must not be empty".to_string(),
            ));
        }
        if new.connection.host.trim().is_empty() {
            return Err(DataSourceError::InvalidField(
                "host must not be empty".to_string(),
            ));
        }
        if new.connection.username.trim().is_empty() {
            return Err(DataSourceError::InvalidField(
                "username must not be empty".to_string(),
            ));
        }
        if new.connection.database.trim().is_empty() {
            return Err(DataSourceError::InvalidField(
                "database must not be empty".to_string(),
            ));
        }
        if new.connection.port == 0 {
            return Err(DataSourceError::InvalidField(
                "port must not be zero".to_string(),
            ));
        }

        Ok(Self {
            id: DataSourceId::new(),
            name: new.name,
            engine: new.engine,
            connection: new.connection,
            project_id: new.project_id,
            registered_at_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or_default(),
            // Nothing is ingested until tables are chosen.
            ingested_tables: BTreeSet::new(),
        })
    }

    /// Whether `pipa-ingestion` captures `table` of this source.
    pub fn ingests(&self, table: &TableRef) -> bool {
        self.ingested_tables.contains(table)
    }

    /// Replaces the set of tables to ingest. Tables aren't checked against the source database
    /// (it may be unreachable just now); one that doesn't exist simply never has changes.
    pub fn choose_ingested_tables(
        &mut self,
        tables: impl IntoIterator<Item = TableRef>,
    ) -> Result<(), DataSourceError> {
        let tables: BTreeSet<TableRef> = tables.into_iter().collect();
        if tables
            .iter()
            .any(|table| table.schema.trim().is_empty() || table.name.trim().is_empty())
        {
            return Err(DataSourceError::InvalidField(
                "table schema and name must not be empty".to_string(),
            ));
        }
        self.ingested_tables = tables;
        Ok(())
    }
}

/// Outcome of attempting to open a connection to a data source's OLTP database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum ConnectionTestOutcome {
    Reachable,
    Unreachable { reason: String },
}

/// Errors surfaced by the data source domain and its use cases.
#[derive(Debug, thiserror::Error)]
pub enum DataSourceError {
    #[error("invalid data source: {0}")]
    InvalidField(String),
    #[error("data source {0} was not found")]
    NotFound(DataSourceId),
    #[error("data source storage error: {0}")]
    Storage(String),
    #[error("could not read the data source's database: {0}")]
    SourceUnavailable(String),
}

/// Port: persistence for `DataSource` aggregates, implemented by an infrastructure adapter.
#[async_trait]
pub trait DataSourceRepository: Send + Sync {
    async fn save(&self, source: &DataSource) -> Result<(), DataSourceError>;
    async fn find_by_id(&self, id: DataSourceId) -> Result<Option<DataSource>, DataSourceError>;
    async fn list(&self) -> Result<Vec<DataSource>, DataSourceError>;
    async fn delete(&self, id: DataSourceId) -> Result<(), DataSourceError>;
}

/// Port: verifying that a data source's OLTP database is reachable with its stored credentials.
#[async_trait]
pub trait ConnectionTester: Send + Sync {
    async fn test(&self, source: &DataSource) -> ConnectionTestOutcome;
}

/// Port: listing the tables (and their columns) of a data source's OLTP database, read with its
/// stored credentials. Fails with [`DataSourceError::SourceUnavailable`].
#[async_trait]
pub trait SchemaExplorer: Send + Sync {
    async fn list_tables(&self, source: &DataSource) -> Result<Vec<SourceTable>, DataSourceError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> DataSource {
        DataSource::register(NewDataSource {
            name: "orders".to_string(),
            engine: DbEngine::Postgres,
            connection: ConnectionConfig {
                host: "db".to_string(),
                port: 5432,
                username: "u".to_string(),
                password: "p".to_string(),
                database: "d".to_string(),
            },
            project_id: None,
        })
        .unwrap()
    }

    fn table(schema: &str, name: &str) -> TableRef {
        TableRef {
            schema: schema.to_string(),
            name: name.to_string(),
        }
    }

    #[test]
    fn a_new_source_ingests_nothing() {
        assert!(!source().ingests(&table("public", "orders")));
    }

    #[test]
    fn a_source_stored_without_a_choice_ingests_nothing() {
        let mut json = serde_json::to_value(source()).unwrap();
        json.as_object_mut().unwrap().remove("ingested_tables");
        let legacy: DataSource = serde_json::from_value(json).unwrap();
        assert!(!legacy.ingests(&table("public", "orders")));
    }

    #[test]
    fn only_chosen_tables_are_ingested() {
        let mut source = source();
        source
            .choose_ingested_tables([table("public", "orders")])
            .unwrap();
        assert!(source.ingests(&table("public", "orders")));
        assert!(!source.ingests(&table("public", "customers")));
        assert!(!source.ingests(&table("audit", "orders")));
    }

    #[test]
    fn rejects_a_blank_table() {
        let err = source()
            .choose_ingested_tables([table("public", " ")])
            .unwrap_err();
        assert!(matches!(err, DataSourceError::InvalidField(_)));
    }
}
