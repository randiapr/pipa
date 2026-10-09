//! Reads registered OLTP data sources back out of the shared RustFS/S3 object store.
//!
//! Deliberately duplicated (not imported) from `pipa-backend::datasource`: `pipa-ingestion` only
//! ever reads what `pipa-backend` writes there, via the same JSON-under-`datasources/` layout, so
//! it needs just enough of the shape to deserialize it — not the write path, validation, or
//! the `DataSourceRepository` port abstraction pipa-backend's side maintains for that.

use std::collections::HashSet;

use futures::StreamExt;

use crate::write::domain::target_table_name;
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

/// A table of a source's database, by schema and name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
pub struct TableRef {
    pub schema: String,
    pub name: String,
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
    /// The tables to capture. Absent on a source stored before tables could be chosen (which
    /// was captured in full): such a source keeps capturing just the tables it already has in
    /// pipa ([`TableSelection::Existing`]) until `pipa-backend` records that as its choice.
    #[serde(default)]
    pub ingested_tables: Option<Vec<TableRef>>,
}

impl DataSource {
    /// The saved choice, or `None` for a source stored before tables could be chosen.
    pub fn chosen_tables(&self) -> Option<TableSelection> {
        self.ingested_tables.as_ref().map(|tables| {
            TableSelection::of(
                tables
                    .iter()
                    .map(|table| (table.schema.as_str(), table.name.as_str())),
            )
        })
    }
}

/// Which of a source's tables are captured; changes to any other table are skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableSelection {
    /// The saved choice, as `(schema, table)` pairs.
    Chosen(HashSet<(String, String)>),
    /// A source without a saved choice: the source tables whose target table already exists,
    /// by target table name ([`target_table_name`]) — matched forward, so exactly.
    Existing(HashSet<String>),
}

impl Default for TableSelection {
    /// Nothing chosen.
    fn default() -> Self {
        Self::Chosen(HashSet::new())
    }
}

impl TableSelection {
    pub fn of<'a>(tables: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        Self::Chosen(
            tables
                .into_iter()
                .map(|(schema, table)| (schema.to_string(), table.to_string()))
                .collect(),
        )
    }

    pub fn includes(&self, schema: &str, table: &str) -> bool {
        match self {
            Self::Chosen(tables) => tables.contains(&(schema.to_string(), table.to_string())),
            Self::Existing(targets) => targets.contains(&target_table_name(schema, table)),
        }
    }

    /// For logging.
    pub fn describe(&self) -> String {
        match self {
            Self::Chosen(tables) if tables.is_empty() => "none".to_string(),
            Self::Chosen(tables) => {
                let mut names: Vec<String> = tables
                    .iter()
                    .map(|(schema, table)| format!("{schema}.{table}"))
                    .collect();
                names.sort();
                names.join(", ")
            }
            Self::Existing(targets) => {
                format!("no saved choice: the {} already in pipa", targets.len())
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn source(ingested_tables: serde_json::Value) -> DataSource {
        let mut json = serde_json::json!({
            "id": "0199c0de-0000-7000-8000-000000000000",
            "name": "orders",
            "engine": "postgres",
            "connection": {
                "host": "db", "port": 5432, "username": "u", "password": "p", "database": "d"
            },
            "project_id": null,
            "registered_at_unix": 0,
        });
        if !ingested_tables.is_null() {
            json["ingested_tables"] = ingested_tables;
        }
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn a_source_stored_without_a_choice_has_no_saved_choice() {
        assert_eq!(source(serde_json::Value::Null).chosen_tables(), None);
    }

    /// Matched forward through the target naming, so case, punctuation and `__` inside names
    /// can't make one source table pass for another.
    #[test]
    fn existing_tables_match_their_source_tables_exactly() {
        let selection = TableSelection::Existing(HashSet::from([
            "fintech__orders".to_string(),
            "sales__order_items".to_string(),
        ]));
        assert!(selection.includes("fintech", "orders"));
        assert!(selection.includes("Sales", "Order-Items"));
        assert!(!selection.includes("fintech", "customers"));
        assert!(!selection.includes("fin", "tech__orders"));
    }

    #[test]
    fn only_chosen_tables_are_captured() {
        let selection = source(serde_json::json!([{ "schema": "public", "name": "orders" }]))
            .chosen_tables()
            .unwrap();
        assert!(selection.includes("public", "orders"));
        assert!(!selection.includes("public", "customers"));
        assert!(!selection.includes("audit", "orders"));
        assert!(
            !source(serde_json::json!([]))
                .chosen_tables()
                .unwrap()
                .includes("public", "orders")
        );
    }
}
