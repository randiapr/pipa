// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! `/saved-queries` routes: save/list/update/delete the named SQL queries of a project. Every
//! route needs a developer or admin (the same as `/query`, which is how a saved query is run) and
//! is limited to the projects the caller may access.

use std::sync::Arc;

use crate::project::ProjectId;
use crate::saved_query::{SavedQueryError, SavedQueryId, SavedQueryService};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use pipa_api::{
    NewSavedQuery, SavedQueries, SavedQueriesResponse, SavedQueryData, SavedQueryResponse,
    SavedQueryUpdate, path,
};
use serde::Deserialize;
use uuid::Uuid;

use super::auth::{AuthError, AuthUser};
use super::error::{BaseResponse, Empty, MessageResponse, ResponseCode, error_response};

type SharedSavedQueryService = Arc<SavedQueryService>;

pub fn routes() -> Router<SharedSavedQueryService> {
    Router::new()
        .route(
            path::SAVED_QUERIES,
            get(list_saved_queries).post(save_query),
        )
        .route(
            path::SAVED_QUERY,
            axum::routing::put(update_saved_query).delete(delete_saved_query),
        )
}

/// Query string of `GET /saved-queries`.
#[derive(Debug, Deserialize)]
struct ListParams {
    project_id: Uuid,
}

async fn list_saved_queries(
    auth: AuthUser,
    State(service): State<SharedSavedQueryService>,
    Query(params): Query<ListParams>,
) -> Result<Json<SavedQueriesResponse>, ApiError> {
    auth.require_developer()?;
    let project = ProjectId(params.project_id);
    auth.require_project(Some(project))?;
    let saved_queries = service
        .list(project)
        .await?
        .into_iter()
        .map(Into::into)
        .collect();
    Ok(Json(BaseResponse::new(
        ResponseCode::Ok,
        SavedQueries { saved_queries },
    )))
}

async fn save_query(
    auth: AuthUser,
    State(service): State<SharedSavedQueryService>,
    Json(new_query): Json<NewSavedQuery>,
) -> Result<(StatusCode, Json<SavedQueryResponse>), ApiError> {
    auth.require_developer()?;
    let new_query: crate::saved_query::NewSavedQuery = new_query.try_into()?;
    auth.require_project(Some(new_query.project_id))?;
    let saved_query = service.register(new_query).await?.into();
    Ok((
        StatusCode::CREATED,
        Json(BaseResponse::new(
            ResponseCode::Created,
            SavedQueryData { saved_query },
        )),
    ))
}

async fn update_saved_query(
    auth: AuthUser,
    State(service): State<SharedSavedQueryService>,
    Path(id): Path<Uuid>,
    Json(update): Json<SavedQueryUpdate>,
) -> Result<Json<SavedQueryResponse>, ApiError> {
    auth.require_developer()?;
    let id = SavedQueryId(id);
    auth.require_project(Some(service.get(id).await?.project_id))?;
    let saved_query = service.update(id, update.into()).await?.into();
    Ok(Json(BaseResponse::new(
        ResponseCode::Ok,
        SavedQueryData { saved_query },
    )))
}

async fn delete_saved_query(
    auth: AuthUser,
    State(service): State<SharedSavedQueryService>,
    Path(id): Path<Uuid>,
) -> Result<Json<MessageResponse>, ApiError> {
    auth.require_developer()?;
    let id = SavedQueryId(id);
    auth.require_project(Some(service.get(id).await?.project_id))?;
    service.remove(id).await?;
    Ok(Json(BaseResponse::new(ResponseCode::Deleted, Empty {})))
}

enum ApiError {
    SavedQuery(SavedQueryError),
    Auth(AuthError),
}

impl From<SavedQueryError> for ApiError {
    fn from(err: SavedQueryError) -> Self {
        Self::SavedQuery(err)
    }
}

impl From<AuthError> for ApiError {
    fn from(err: AuthError) -> Self {
        Self::Auth(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let err = match self {
            Self::SavedQuery(err) => err,
            Self::Auth(err) => return err.into_response(),
        };
        let (status, code) = match &err {
            SavedQueryError::NotFound(_) => (StatusCode::NOT_FOUND, ResponseCode::NotFound),
            SavedQueryError::InvalidField(_) => (StatusCode::BAD_REQUEST, ResponseCode::BadRequest),
            SavedQueryError::DuplicateName(_) => (StatusCode::CONFLICT, ResponseCode::Conflict),
            SavedQueryError::Storage(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ResponseCode::InternalError,
            ),
        };
        error_response(status, code, err)
    }
}
