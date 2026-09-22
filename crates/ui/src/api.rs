//! Interface adapter: HTTP client for the `pipa-rest` data source API.
//!
//! These types are the dashboard's own presentation-layer contract with `pipa-rest` — they
//! mirror the wire format of `pipa_data::datasource`'s domain types without depending on that
//! crate directly, since `pipa-data` pulls in native-only dependencies that don't target
//! `wasm32-unknown-unknown`.

use gloo_net::http::{Request, Response};
use serde::{Deserialize, Serialize};

/// Base URL of the `pipa-rest` API. Defaults to the local dev server.
const API_BASE: &str = "http://localhost:8080";

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
}

#[derive(Debug, Clone, Deserialize)]
pub struct DataSourceView {
    pub id: String,
    pub name: String,
    pub engine: DbEngine,
    pub connection: ConnectionConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ConnectionTestOutcome {
    Reachable,
    Unreachable { reason: String },
}

#[derive(Deserialize)]
struct ErrorBody {
    error: String,
}

async fn error_message(response: Response) -> String {
    match response.json::<ErrorBody>().await {
        Ok(body) => body.error,
        Err(_) => format!("request failed with status {}", response.status()),
    }
}

pub async fn list_sources() -> Result<Vec<DataSourceView>, String> {
    let response = Request::get(&format!("{API_BASE}/datasources"))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.ok() {
        return Err(error_message(response).await);
    }
    response
        .json::<Vec<DataSourceView>>()
        .await
        .map_err(|err| err.to_string())
}

pub async fn register_source(new_source: &NewDataSource) -> Result<DataSourceView, String> {
    let response = Request::post(&format!("{API_BASE}/datasources"))
        .json(new_source)
        .map_err(|err| err.to_string())?
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.ok() {
        return Err(error_message(response).await);
    }
    response
        .json::<DataSourceView>()
        .await
        .map_err(|err| err.to_string())
}

pub async fn delete_source(id: &str) -> Result<(), String> {
    let response = Request::delete(&format!("{API_BASE}/datasources/{id}"))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.ok() {
        return Err(error_message(response).await);
    }
    Ok(())
}

pub async fn test_source(id: &str) -> Result<ConnectionTestOutcome, String> {
    let response = Request::post(&format!("{API_BASE}/datasources/{id}/test"))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.ok() {
        return Err(error_message(response).await);
    }
    response
        .json::<ConnectionTestOutcome>()
        .await
        .map_err(|err| err.to_string())
}
