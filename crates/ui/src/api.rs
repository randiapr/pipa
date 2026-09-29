//! HTTP client for the `pipa-backend` data source and project APIs.
//!
//! This is the Model's I/O: it moves [`pipa_api`] contract types to and from `pipa-backend` over
//! HTTP and nothing else. It is called only from `crate::viewmodel` — views never reach
//! into this module directly.

use gloo_net::Error as NetError;
use gloo_net::http::{Request, Response};
use pipa_api::{
    ConnectionTestOutcome, ConnectionTestResponse, DataSourceResponse, DataSourceView,
    DataSourcesResponse, ErrorResponse, NewDataSource, NewProject, ProjectResponse, ProjectUpdate,
    ProjectView, ProjectsResponse, path,
};
use serde::de::DeserializeOwned;

/// Base URL of the `pipa-backend` API. Defaults to the local dev server.
const API_BASE: &str = "http://localhost:8080";

fn url(path: &str) -> String {
    format!("{API_BASE}{path}")
}

async fn error_message(response: Response) -> String {
    match response.json::<ErrorResponse>().await {
        Ok(body) => body.body.error,
        Err(_) => format!("request failed with status {}", response.status()),
    }
}

/// Sends the request `build`/`json` produced, turning a non-2xx status into the backend's
/// `error` message and otherwise decoding the body as `T`.
async fn send<T: DeserializeOwned>(request: Result<Request, NetError>) -> Result<T, String> {
    let response = request
        .map_err(|err| err.to_string())?
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.ok() {
        return Err(error_message(response).await);
    }
    response.json::<T>().await.map_err(|err| err.to_string())
}

/// Like [`send`], for responses whose body carries nothing beyond the envelope.
async fn send_discard(request: Result<Request, NetError>) -> Result<(), String> {
    let response = request
        .map_err(|err| err.to_string())?
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.ok() {
        return Err(error_message(response).await);
    }
    Ok(())
}

pub async fn list_sources() -> Result<Vec<DataSourceView>, String> {
    let response: DataSourcesResponse = send(Request::get(&url(path::DATASOURCES)).build()).await?;
    Ok(response.body.datasources)
}

pub async fn register_source(new_source: &NewDataSource) -> Result<DataSourceView, String> {
    let request = Request::post(&url(path::DATASOURCES)).json(new_source);
    let response: DataSourceResponse = send(request).await?;
    Ok(response.body.datasource)
}

pub async fn delete_source(id: &str) -> Result<(), String> {
    send_discard(Request::delete(&url(&path::datasource(id))).build()).await
}

pub async fn test_source(id: &str) -> Result<ConnectionTestOutcome, String> {
    let response: ConnectionTestResponse =
        send(Request::post(&url(&path::datasource_test(id))).build()).await?;
    Ok(response.body.connection_test)
}

pub async fn list_projects() -> Result<Vec<ProjectView>, String> {
    let response: ProjectsResponse = send(Request::get(&url(path::PROJECTS)).build()).await?;
    Ok(response.body.projects)
}

pub async fn register_project(new_project: &NewProject) -> Result<ProjectView, String> {
    let request = Request::post(&url(path::PROJECTS)).json(new_project);
    let response: ProjectResponse = send(request).await?;
    Ok(response.body.project)
}

pub async fn update_project(id: &str, update: &ProjectUpdate) -> Result<ProjectView, String> {
    let request = Request::put(&url(&path::project(id))).json(update);
    let response: ProjectResponse = send(request).await?;
    Ok(response.body.project)
}

pub async fn delete_project(id: &str) -> Result<(), String> {
    send_discard(Request::delete(&url(&path::project(id))).build()).await
}
