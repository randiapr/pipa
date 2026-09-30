//! HTTP client for the `pipa-backend` API.
//!
//! This is the Model's I/O: it moves [`pipa_api`] contract types to and from `pipa-backend` over
//! HTTP and nothing else. It is called only from `crate::viewmodel` — views never reach
//! into this module directly.
//!
//! Every request carries the login token (kept in `localStorage`, see [`crate::storage`]). A 401
//! response means that token is missing or no longer valid; the handler registered with
//! [`on_unauthorized`] is called so the session can be dropped.

use std::cell::RefCell;
use std::rc::Rc;

use gloo_net::Error as NetError;
use gloo_net::http::{Request, RequestBuilder, Response};
use pipa_api::{
    ConnectionTestOutcome, ConnectionTestResponse, DataSourceResponse, DataSourceView,
    DataSourcesResponse, ErrorResponse, LoginData, LoginRequest, LoginResponse, MeResponse,
    NewDataSource, NewProject, NewUser, ProjectResponse, ProjectUpdate, ProjectView,
    ProjectsResponse, QueryRequest, RowsResponse, UserResponse, UserUpdate, UserView,
    UsersResponse, path,
};
use serde::de::DeserializeOwned;

use crate::storage;

/// Base URL of the `pipa-backend` API. Defaults to the local dev server.
const API_BASE: &str = "http://localhost:8080";

thread_local! {
    static ON_UNAUTHORIZED: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
}

/// Registers what to do when the backend answers 401 (the session is gone or expired).
pub fn on_unauthorized(handler: impl Fn() + 'static) {
    ON_UNAUTHORIZED.with(|slot| *slot.borrow_mut() = Some(Rc::new(handler)));
}

fn url(path: &str) -> String {
    format!("{API_BASE}{path}")
}

/// Adds the `Authorization: Bearer` header when there is a token.
fn authed(builder: RequestBuilder) -> RequestBuilder {
    match storage::get(storage::TOKEN_KEY) {
        Some(token) => builder.header("Authorization", &format!("Bearer {token}")),
        None => builder,
    }
}

async fn error_message(response: Response) -> String {
    match response.json::<ErrorResponse>().await {
        Ok(body) => body.body.error,
        Err(_) => format!("request failed with status {}", response.status()),
    }
}

/// Sends `request`, turning a non-2xx status into the backend's `error` message.
async fn dispatch(request: Result<Request, NetError>) -> Result<Response, String> {
    let response = request
        .map_err(|err| err.to_string())?
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if response.ok() {
        return Ok(response);
    }
    if response.status() == 401 {
        // Clone the handler out so it can freely call back into this module.
        let handler = ON_UNAUTHORIZED.with(|slot| slot.borrow().clone());
        if let Some(handler) = handler {
            handler();
        }
    }
    Err(error_message(response).await)
}

/// Sends the request `build`/`json` produced and decodes the body as `T`.
async fn send<T: DeserializeOwned>(request: Result<Request, NetError>) -> Result<T, String> {
    let response = dispatch(request).await?;
    response.json::<T>().await.map_err(|err| err.to_string())
}

/// Like [`send`], for responses whose body carries nothing beyond the envelope.
async fn send_discard(request: Result<Request, NetError>) -> Result<(), String> {
    dispatch(request).await.map(|_| ())
}

pub async fn login(credentials: &LoginRequest) -> Result<LoginData, String> {
    let request = Request::post(&url(path::LOGIN)).json(credentials);
    let response: LoginResponse = send(request).await?;
    Ok(response.body)
}

pub async fn me() -> Result<UserView, String> {
    let response: MeResponse = send(authed(Request::get(&url(path::ME))).build()).await?;
    Ok(response.body.user)
}

pub async fn list_users() -> Result<Vec<UserView>, String> {
    let response: UsersResponse = send(authed(Request::get(&url(path::USERS))).build()).await?;
    Ok(response.body.users)
}

pub async fn create_user(new_user: &NewUser) -> Result<UserView, String> {
    let request = authed(Request::post(&url(path::USERS))).json(new_user);
    let response: UserResponse = send(request).await?;
    Ok(response.body.user)
}

pub async fn update_user(id: &str, update: &UserUpdate) -> Result<UserView, String> {
    let request = authed(Request::put(&url(&path::user(id)))).json(update);
    let response: UserResponse = send(request).await?;
    Ok(response.body.user)
}

pub async fn delete_user(id: &str) -> Result<(), String> {
    send_discard(authed(Request::delete(&url(&path::user(id)))).build()).await
}

/// Lists data sources, only those of `project_id` when given.
pub async fn list_sources(project_id: Option<&str>) -> Result<Vec<DataSourceView>, String> {
    let mut target = url(path::DATASOURCES);
    if let Some(project_id) = project_id {
        target.push_str("?project_id=");
        target.push_str(project_id);
    }
    let response: DataSourcesResponse = send(authed(Request::get(&target)).build()).await?;
    Ok(response.body.datasources)
}

pub async fn register_source(new_source: &NewDataSource) -> Result<DataSourceView, String> {
    let request = authed(Request::post(&url(path::DATASOURCES))).json(new_source);
    let response: DataSourceResponse = send(request).await?;
    Ok(response.body.datasource)
}

pub async fn delete_source(id: &str) -> Result<(), String> {
    send_discard(authed(Request::delete(&url(&path::datasource(id)))).build()).await
}

pub async fn test_source(id: &str) -> Result<ConnectionTestOutcome, String> {
    let response: ConnectionTestResponse =
        send(authed(Request::post(&url(&path::datasource_test(id)))).build()).await?;
    Ok(response.body.connection_test)
}

pub async fn list_projects() -> Result<Vec<ProjectView>, String> {
    let response: ProjectsResponse =
        send(authed(Request::get(&url(path::PROJECTS))).build()).await?;
    Ok(response.body.projects)
}

pub async fn register_project(new_project: &NewProject) -> Result<ProjectView, String> {
    let request = authed(Request::post(&url(path::PROJECTS))).json(new_project);
    let response: ProjectResponse = send(request).await?;
    Ok(response.body.project)
}

pub async fn update_project(id: &str, update: &ProjectUpdate) -> Result<ProjectView, String> {
    let request = authed(Request::put(&url(&path::project(id)))).json(update);
    let response: ProjectResponse = send(request).await?;
    Ok(response.body.project)
}

pub async fn delete_project(id: &str) -> Result<(), String> {
    send_discard(authed(Request::delete(&url(&path::project(id)))).build()).await
}

/// Runs read-only SQL over the Iceberg tables of `project_id` (every table, for an admin who
/// passes `None`), returning the rows as a JSON array of objects.
pub async fn query(sql: &str, project_id: Option<&str>) -> Result<serde_json::Value, String> {
    let body = QueryRequest {
        sql: sql.to_string(),
        project_id: project_id.map(str::to_string),
    };
    let request = authed(Request::post(&url(path::QUERY))).json(&body);
    let response: RowsResponse = send(request).await?;
    Ok(response.body.rows)
}
