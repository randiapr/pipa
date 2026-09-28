//! HTTP client for the `pipa-backend` data source and project APIs.
//!
//! This is the Model's I/O: it moves [`crate::model`] types to and from `pipa-backend` over
//! HTTP and nothing else. It is called only from `crate::viewmodel` — views never reach
//! into this module directly.

use gloo_net::http::{Request, Response};

use crate::model::{
    ConnectionTestOutcome, DataSourceView, NewDataSource, NewProject, ProjectUpdate, ProjectView,
};
use serde::Deserialize;

/// Base URL of the `pipa-backend` API. Defaults to the local dev server.
const API_BASE: &str = "http://localhost:8080";

#[derive(Deserialize)]
struct ErrorBody {
    #[allow(dead_code)]
    response_code: u32,
    #[allow(dead_code)]
    response_message: String,
    error: String,
}

#[derive(Deserialize)]
struct DataSourcesEnvelope {
    datasources: Vec<DataSourceView>,
}

#[derive(Deserialize)]
struct DataSourceEnvelope {
    datasource: DataSourceView,
}

#[derive(Deserialize)]
struct ConnectionTestEnvelope {
    connection_test: ConnectionTestOutcome,
}

#[derive(Deserialize)]
struct ProjectsEnvelope {
    projects: Vec<ProjectView>,
}

#[derive(Deserialize)]
struct ProjectEnvelope {
    project: ProjectView,
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
        .json::<DataSourcesEnvelope>()
        .await
        .map(|envelope| envelope.datasources)
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
        .json::<DataSourceEnvelope>()
        .await
        .map(|envelope| envelope.datasource)
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
        .json::<ConnectionTestEnvelope>()
        .await
        .map(|envelope| envelope.connection_test)
        .map_err(|err| err.to_string())
}

pub async fn list_projects() -> Result<Vec<ProjectView>, String> {
    let response = Request::get(&format!("{API_BASE}/projects"))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.ok() {
        return Err(error_message(response).await);
    }
    response
        .json::<ProjectsEnvelope>()
        .await
        .map(|envelope| envelope.projects)
        .map_err(|err| err.to_string())
}

pub async fn register_project(new_project: &NewProject) -> Result<ProjectView, String> {
    let response = Request::post(&format!("{API_BASE}/projects"))
        .json(new_project)
        .map_err(|err| err.to_string())?
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.ok() {
        return Err(error_message(response).await);
    }
    response
        .json::<ProjectEnvelope>()
        .await
        .map(|envelope| envelope.project)
        .map_err(|err| err.to_string())
}

pub async fn update_project(id: &str, update: &ProjectUpdate) -> Result<ProjectView, String> {
    let response = Request::put(&format!("{API_BASE}/projects/{id}"))
        .json(update)
        .map_err(|err| err.to_string())?
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.ok() {
        return Err(error_message(response).await);
    }
    response
        .json::<ProjectEnvelope>()
        .await
        .map(|envelope| envelope.project)
        .map_err(|err| err.to_string())
}

pub async fn delete_project(id: &str) -> Result<(), String> {
    let response = Request::delete(&format!("{API_BASE}/projects/{id}"))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.ok() {
        return Err(error_message(response).await);
    }
    Ok(())
}
