//! Model layer: wire/domain types shared by the API client and the views.
//!
//! These mirror the wire format of `pipa_storage::datasource`/`pipa_storage::project`'s domain
//! types without depending on that crate directly, since `pipa-storage` pulls in native-only
//! dependencies that don't target `wasm32-unknown-unknown`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DbEngine {
    #[serde(rename = "postgres")]
    Postgres,
    #[serde(rename = "mysql")]
    Mysql,
}

impl DbEngine {
    pub fn label(self) -> &'static str {
        match self {
            DbEngine::Postgres => "PostgreSQL",
            DbEngine::Mysql => "MySQL",
        }
    }

    pub fn wire_value(self) -> &'static str {
        match self {
            DbEngine::Postgres => "postgres",
            DbEngine::Mysql => "mysql",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            DbEngine::Postgres => 5432,
            DbEngine::Mysql => 3306,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub database: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NewDataSource {
    pub name: String,
    pub engine: DbEngine,
    pub connection: ConnectionConfig,
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DataSourceView {
    pub id: String,
    pub name: String,
    pub engine: DbEngine,
    pub connection: ConnectionConfig,
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NewProject {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectUpdate {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectView {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ConnectionTestOutcome {
    Reachable,
    Unreachable { reason: String },
}
