//! Authentication and authorization: the `POST /auth/login` and `GET /auth/me` routes, the
//! `require_auth` middleware every protected route sits behind, and the `AuthUser`/`AdminOnly`
//! extractors handlers use to see who is calling.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{FromRequestParts, Request, State},
    http::{StatusCode, header::AUTHORIZATION, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use pipa_api::{LoginData, LoginRequest, LoginResponse, MeData, MeResponse, path};

use crate::project::ProjectId;
use crate::user::{Role, User, UserError, UserService};

use super::error::{BaseResponse, ResponseCode, error_response};

type SharedUserService = Arc<UserService>;

/// Routes reachable without a token.
pub fn login_routes() -> Router<SharedUserService> {
    Router::new().route(path::LOGIN, post(login))
}

/// Routes about the caller themself. Must be mounted behind [`require_auth`].
pub fn me_routes() -> Router<SharedUserService> {
    Router::new().route(path::ME, get(me))
}

/// The authenticated caller, as resolved by [`require_auth`] on this request.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub role: Role,
    pub project_ids: Vec<ProjectId>,
    user: User,
}

impl From<User> for AuthUser {
    fn from(user: User) -> Self {
        Self {
            role: user.role,
            project_ids: user.project_ids.clone(),
            user,
        }
    }
}

impl AuthUser {
    pub fn is_admin(&self) -> bool {
        self.role == Role::Admin
    }

    /// Whether the caller may see `project`. Admins may see every project.
    pub fn can_access(&self, project: ProjectId) -> bool {
        self.is_admin() || self.project_ids.contains(&project)
    }

    /// Like [`can_access`](Self::can_access) for something that may belong to no project at all.
    /// Only admins may touch project-less things.
    pub fn require_project(&self, project: Option<ProjectId>) -> Result<(), AuthError> {
        match project {
            Some(project) if self.can_access(project) => Ok(()),
            None if self.is_admin() => Ok(()),
            _ => Err(AuthError::Forbidden),
        }
    }

    pub fn require_admin(&self) -> Result<(), AuthError> {
        if self.is_admin() {
            Ok(())
        } else {
            Err(AuthError::Forbidden)
        }
    }
}

/// Why a request was turned away before reaching its handler's own logic.
#[derive(Debug)]
pub enum AuthError {
    /// No valid credentials were presented.
    Unauthorized,
    /// The caller is signed in but not allowed to do this.
    Forbidden,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        match self {
            Self::Unauthorized => error_response(
                StatusCode::UNAUTHORIZED,
                ResponseCode::Unauthorized,
                "missing, invalid or expired token",
            ),
            Self::Forbidden => error_response(
                StatusCode::FORBIDDEN,
                ResponseCode::Forbidden,
                "you are not allowed to do this",
            ),
        }
    }
}

impl<S: Send + Sync> FromRequestParts<S> for AuthUser {
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<AuthUser>()
            .cloned()
            .ok_or(AuthError::Unauthorized)
    }
}

/// Extractor that only lets admins through.
pub struct AdminOnly;

impl<S: Send + Sync> FromRequestParts<S> for AdminOnly {
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state).await?;
        user.require_admin()?;
        Ok(Self)
    }
}

/// Middleware: requires a valid `Authorization: Bearer <token>` header and stores the resolved
/// [`AuthUser`] in the request extensions. Anything else is a 401.
pub async fn require_auth(
    State(users): State<SharedUserService>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(token) = bearer_token(&request) else {
        return AuthError::Unauthorized.into_response();
    };
    match users.verify_token(token).await {
        Ok(user) => {
            request.extensions_mut().insert(AuthUser::from(user));
            next.run(request).await
        }
        Err(UserError::InvalidToken) => AuthError::Unauthorized.into_response(),
        Err(err) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            ResponseCode::InternalError,
            err,
        ),
    }
}

fn bearer_token(request: &Request) -> Option<&str> {
    let value = request.headers().get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then_some(token.trim())
        .filter(|token| !token.is_empty())
}

async fn login(
    State(service): State<SharedUserService>,
    Json(request): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, LoginError> {
    let (user, token) = service
        .authenticate(&request.username, &request.password)
        .await?;
    Ok(Json(BaseResponse::new(
        ResponseCode::Ok,
        LoginData {
            token,
            user: user.into(),
        },
    )))
}

async fn me(auth: AuthUser) -> Json<MeResponse> {
    Json(BaseResponse::new(
        ResponseCode::Ok,
        MeData {
            user: auth.user.into(),
        },
    ))
}

/// A failed login: wrong credentials are a 401, anything else (e.g. storage) a 500.
struct LoginError(UserError);

impl From<UserError> for LoginError {
    fn from(err: UserError) -> Self {
        Self(err)
    }
}

impl IntoResponse for LoginError {
    fn into_response(self) -> Response {
        match self.0 {
            UserError::InvalidCredentials => {
                error_response(StatusCode::UNAUTHORIZED, ResponseCode::Unauthorized, self.0)
            }
            _ => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                ResponseCode::InternalError,
                self.0,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::Request as HttpRequest;

    use super::*;
    use crate::user::UserId;

    fn request_with(header: Option<&str>) -> Request {
        let mut builder = HttpRequest::builder();
        if let Some(header) = header {
            builder = builder.header(AUTHORIZATION, header);
        }
        builder.body(axum::body::Body::empty()).unwrap()
    }

    fn user(role: Role, projects: &[ProjectId]) -> AuthUser {
        User {
            id: UserId::new(),
            username: "u".to_string(),
            password_hash: String::new(),
            role,
            project_ids: projects.to_vec(),
            created_at_unix: 0,
        }
        .into()
    }

    #[test]
    fn parses_bearer_tokens() {
        assert_eq!(bearer_token(&request_with(Some("Bearer abc"))), Some("abc"));
        assert_eq!(bearer_token(&request_with(Some("bearer abc"))), Some("abc"));
        assert_eq!(bearer_token(&request_with(Some("Basic abc"))), None);
        assert_eq!(bearer_token(&request_with(Some("Bearer "))), None);
        assert_eq!(bearer_token(&request_with(None)), None);
    }

    #[test]
    fn admins_reach_every_project_and_project_less_things() {
        let admin = user(Role::Admin, &[]);
        assert!(admin.can_access(ProjectId::new()));
        assert!(admin.require_project(None).is_ok());
    }

    #[test]
    fn users_only_reach_their_assigned_projects() {
        let mine = ProjectId::new();
        let user = user(Role::User, &[mine]);
        assert!(user.can_access(mine));
        assert!(!user.can_access(ProjectId::new()));
        assert!(user.require_project(Some(mine)).is_ok());
        assert!(user.require_project(Some(ProjectId::new())).is_err());
        assert!(user.require_project(None).is_err());
        assert!(user.require_admin().is_err());
    }
}
