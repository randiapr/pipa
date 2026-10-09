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
    /// The source tables `pipa-ingestion` captures into Iceberg; changes to any other table are
    /// skipped. Empty until tables are chosen — the default for every source, including one
    /// registered before tables could be chosen.
    #[serde(default)]
    pub ingested_tables: Vec<SourceTableRef>,
}

/// A table of a data source's database, by schema and name (for MySQL, the schema is the
/// database).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SourceTableRef {
    pub schema: String,
    pub name: String,
}

/// A column of a source table, as the source database describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceColumnView {
    pub name: String,
    /// The source database's own spelling of the type (e.g. `character varying(255)`).
    pub data_type: String,
    pub nullable: bool,
    /// Part of the table's primary key.
    pub primary_key: bool,
    /// The column this one references through a foreign key, if it is part of one (the first
    /// by constraint name, if several).
    #[serde(default)]
    pub foreign_key: Option<SourceColumnRef>,
}

/// A column of a source table, by schema, table and name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceColumnRef {
    pub schema: String,
    pub table: String,
    pub column: String,
}

/// A table found in a data source's database by `GET /datasources/{id}/tables`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceTableView {
    pub schema: String,
    pub name: String,
    pub columns: Vec<SourceColumnView>,
    /// Whether the data source currently ingests this table.
    pub ingested: bool,
}

/// `PUT /datasources/{id}/tables` request body: the complete set of tables to ingest, replacing
/// the previous one. An empty list stops ingesting every table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestedTablesUpdate {
    pub tables: Vec<SourceTableRef>,
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

/// Payload of `GET /datasources/{id}/tables`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceTables {
    pub tables: Vec<SourceTableView>,
}

pub type DataSourcesResponse = BaseResponse<DataSources>;
pub type DataSourceResponse = BaseResponse<DataSourceData>;
pub type ConnectionTestResponse = BaseResponse<ConnectionTest>;
pub type SourceTablesResponse = BaseResponse<SourceTables>;

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
    fn view_without_ingested_tables_means_no_table() {
        let view: DataSourceView = serde_json::from_value(serde_json::json!({
            "id": "01a11ed5",
            "name": "orders",
            "engine": "postgres",
            "connection": {
                "host": "db", "port": 5432, "username": "u", "password": "p", "database": "d"
            },
            "project_id": null,
            "registered_at_unix": 0,
        }))
        .unwrap();
        assert!(view.ingested_tables.is_empty());
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
