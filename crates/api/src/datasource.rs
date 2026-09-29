//! `/datasources` request and response bodies.

use serde::{Deserialize, Serialize};

use crate::envelope::BaseResponse;

/// The OLTP database engine a source connects to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DbEngine {
    #[serde(rename = "postgres")]
    Postgres,
    #[serde(rename = "mysql")]
    MySql,
}

impl DbEngine {
    /// The value this engine is serialized as on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            DbEngine::Postgres => "postgres",
            DbEngine::MySql => "mysql",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            DbEngine::Postgres => 5432,
            DbEngine::MySql => 3306,
        }
    }
}

/// Connection parameters for an OLTP data source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub database: String,
}

/// `POST /datasources` request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewDataSource {
    pub name: String,
    pub engine: DbEngine,
    pub connection: ConnectionConfig,
    /// UUID of the project this source belongs to, if any.
    #[serde(default)]
    pub project_id: Option<String>,
}

/// A registered data source as returned by the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataSourceView {
    pub id: String,
    pub name: String,
    pub engine: DbEngine,
    pub connection: ConnectionConfig,
    pub project_id: Option<String>,
    pub registered_at_unix: u64,
}

/// Outcome of attempting to open a connection to a data source's OLTP database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum ConnectionTestOutcome {
    Reachable,
    Unreachable { reason: String },
}

/// Payload of `GET /datasources`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataSources {
    pub datasources: Vec<DataSourceView>,
}

/// Payload of `POST /datasources` and `GET /datasources/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataSourceData {
    pub datasource: DataSourceView,
}

/// Payload of `POST /datasources/{id}/test`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionTest {
    pub connection_test: ConnectionTestOutcome,
}

pub type DataSourcesResponse = BaseResponse<DataSources>;
pub type DataSourceResponse = BaseResponse<DataSourceData>;
pub type ConnectionTestResponse = BaseResponse<ConnectionTest>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_serializes_as_its_wire_value() {
        for engine in [DbEngine::Postgres, DbEngine::MySql] {
            assert_eq!(
                serde_json::to_value(engine).unwrap(),
                serde_json::json!(engine.as_str())
            );
        }
    }

    #[test]
    fn new_data_source_defaults_missing_project_id() {
        let source: NewDataSource = serde_json::from_value(serde_json::json!({
            "name": "orders",
            "engine": "postgres",
            "connection": {
                "host": "db", "port": 5432, "username": "u", "password": "p", "database": "d"
            },
        }))
        .unwrap();
        assert_eq!(source.project_id, None);
    }

    #[test]
    fn connection_test_outcome_is_internally_tagged() {
        let json = serde_json::to_value(ConnectionTestOutcome::Unreachable {
            reason: "refused".to_string(),
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "status": "unreachable", "reason": "refused" })
        );
    }
}
