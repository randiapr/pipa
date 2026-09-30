//! `/users` routes: create/list/get/update/delete accounts. Admin only.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use pipa_api::{NewUser, UserData, UserResponse, UserUpdate, Users, UsersResponse, path};
use uuid::Uuid;

use crate::user::{UserError, UserId, UserService};

use super::auth::AdminOnly;
use super::error::{BaseResponse, Empty, MessageResponse, ResponseCode, error_response};

type SharedUserService = Arc<UserService>;

pub fn routes() -> Router<SharedUserService> {
    Router::new()
        .route(path::USERS, get(list_users).post(create_user))
        .route(
            path::USER,
            get(get_user).put(update_user).delete(delete_user),
        )
}

async fn list_users(
    _admin: AdminOnly,
    State(service): State<SharedUserService>,
) -> Result<Json<UsersResponse>, ApiError> {
    let users = service.list().await?.into_iter().map(Into::into).collect();
    Ok(Json(BaseResponse::new(ResponseCode::Ok, Users { users })))
}

async fn create_user(
    _admin: AdminOnly,
    State(service): State<SharedUserService>,
    Json(new_user): Json<NewUser>,
) -> Result<(StatusCode, Json<UserResponse>), ApiError> {
    let user = service.create(new_user.try_into()?).await?.into();
    Ok((
        StatusCode::CREATED,
        Json(BaseResponse::new(ResponseCode::Created, UserData { user })),
    ))
}

async fn get_user(
    _admin: AdminOnly,
    State(service): State<SharedUserService>,
    Path(id): Path<Uuid>,
) -> Result<Json<UserResponse>, ApiError> {
    let user = service.get(UserId(id)).await?.into();
    Ok(Json(BaseResponse::new(ResponseCode::Ok, UserData { user })))
}

async fn update_user(
    _admin: AdminOnly,
    State(service): State<SharedUserService>,
    Path(id): Path<Uuid>,
    Json(update): Json<UserUpdate>,
) -> Result<Json<UserResponse>, ApiError> {
    let user = service.update(UserId(id), update.try_into()?).await?.into();
    Ok(Json(BaseResponse::new(ResponseCode::Ok, UserData { user })))
}

async fn delete_user(
    _admin: AdminOnly,
    State(service): State<SharedUserService>,
    Path(id): Path<Uuid>,
) -> Result<Json<MessageResponse>, ApiError> {
    service.remove(UserId(id)).await?;
    Ok(Json(BaseResponse::new(ResponseCode::Deleted, Empty {})))
}

struct ApiError(UserError);

impl From<UserError> for ApiError {
    fn from(err: UserError) -> Self {
        Self(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match &self.0 {
            UserError::NotFound(_) => (StatusCode::NOT_FOUND, ResponseCode::NotFound),
            UserError::InvalidField(_) => (StatusCode::BAD_REQUEST, ResponseCode::BadRequest),
            UserError::DuplicateUsername(_) | UserError::LastAdmin => {
                (StatusCode::CONFLICT, ResponseCode::Conflict)
            }
            UserError::InvalidCredentials | UserError::InvalidToken => {
                (StatusCode::UNAUTHORIZED, ResponseCode::Unauthorized)
            }
            UserError::Storage(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ResponseCode::InternalError,
            ),
        };
        error_response(status, code, self.0)
    }
}
