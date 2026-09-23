//! Domain layer: the `DataSource` aggregate, its value objects, and the ports
//! (repository + connection tester) that the application layer depends on.

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
        Self(Uuid::new_v4())
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

impl DbEngine {
    pub fn default_port(self) -> u16 {
        match self {
            DbEngine::Postgres => 5432,
            DbEngine::MySql => 3306,
        }
    }
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
        })
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
