//! `/auth` and `/users` request and response bodies.

use serde::{Deserialize, Serialize};

use crate::envelope::BaseResponse;

/// What an account is allowed to do. `Admin` manages users and projects and sees every project;
/// `User` only sees the projects an admin assigned to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    User,
}

/// `POST /auth/login` request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// `POST /users` request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewUser {
    pub username: String,
    pub password: String,
    pub role: Role,
    #[serde(default)]
    pub project_ids: Vec<String>,
}

/// `PUT /users/{id}` request body. Absent fields are left unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserUpdate {
    #[serde(default)]
    pub role: Option<Role>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub project_ids: Option<Vec<String>>,
}

/// A user as returned by the API. Never carries the password or its hash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserView {
    pub id: String,
    pub username: String,
    pub role: Role,
    pub project_ids: Vec<String>,
    pub created_at_unix: u64,
}

/// Payload of `POST /auth/login`: a bearer token plus the account it belongs to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginData {
    pub token: String,
    pub user: UserView,
}

/// Payload of `GET /auth/me`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeData {
    pub user: UserView,
}

/// Payload of `GET /users`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Users {
    pub users: Vec<UserView>,
}

/// Payload of `POST /users`, `GET /users/{id}` and `PUT /users/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserData {
    pub user: UserView,
}

pub type LoginResponse = BaseResponse<LoginData>;
pub type MeResponse = BaseResponse<MeData>;
pub type UsersResponse = BaseResponse<Users>;
pub type UserResponse = BaseResponse<UserData>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Role::Admin).unwrap(), "\"admin\"");
        assert_eq!(
            serde_json::from_str::<Role>("\"user\"").unwrap(),
            Role::User
        );
    }

    #[test]
    fn new_user_defaults_to_no_projects() {
        let user: NewUser =
            serde_json::from_str(r#"{"username":"a","password":"b","role":"user"}"#).unwrap();
        assert!(user.project_ids.is_empty());
    }

    #[test]
    fn user_update_fields_are_optional() {
        let update: UserUpdate = serde_json::from_str("{}").unwrap();
        assert!(update.role.is_none() && update.password.is_none() && update.project_ids.is_none());
    }
}
