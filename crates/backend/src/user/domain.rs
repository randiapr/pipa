//! Domain layer: the `User` aggregate, its `Role`, and the ports the application layer depends
//! on (`UserRepository`, `PasswordHasher`, `TokenService`). A user is an account that signs in to
//! the dashboard; non-admin accounts only see the projects an admin assigned to them.

use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::project::ProjectId;

/// Minimum length of a password, enforced when one is set.
pub const MIN_PASSWORD_LEN: usize = 8;

/// Identity of a user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserId(pub Uuid);

impl UserId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for UserId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What an account may do: `Admin` manages users and projects and sees every project.
/// `Developer` can do everything inside its assigned projects except manage users or projects.
/// `User` can only browse the tables of its assigned projects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Developer,
    User,
}

/// A user account (aggregate root).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub username: String,
    /// PHC-format password hash; the plaintext is never stored.
    pub password_hash: String,
    pub role: Role,
    /// Projects a non-admin account may access. Ignored for admins, who see everything.
    pub project_ids: Vec<ProjectId>,
    pub created_at_unix: u64,
}

/// Fields needed to create a new user, before an identity is assigned.
#[derive(Debug, Clone)]
pub struct NewUser {
    pub username: String,
    pub password: String,
    pub role: Role,
    pub project_ids: Vec<ProjectId>,
}

/// Fields that can be changed on an existing user. `None` leaves the field unchanged.
#[derive(Debug, Clone, Default)]
pub struct UserUpdate {
    pub role: Option<Role>,
    pub password: Option<String>,
    pub project_ids: Option<Vec<ProjectId>>,
}

/// Rejects a password shorter than [`MIN_PASSWORD_LEN`].
pub fn validate_password(password: &str) -> Result<(), UserError> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(UserError::InvalidField(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    Ok(())
}

impl NewUser {
    /// Checks the username and password. The service calls this before hashing the password.
    pub fn validate(&self) -> Result<(), UserError> {
        if self.username.trim().is_empty() {
            return Err(UserError::InvalidField(
                "username must not be empty".to_string(),
            ));
        }
        validate_password(&self.password)
    }
}

impl User {
    /// Constructs a new `User` aggregate from validated input and an already-computed password
    /// hash, assigning it a fresh identity.
    pub fn register(new: NewUser, password_hash: String) -> Self {
        Self {
            id: UserId::new(),
            username: new.username.trim().to_string(),
            password_hash,
            role: new.role,
            project_ids: new.project_ids,
            created_at_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or_default(),
        }
    }
}

/// Errors surfaced by the user domain and its use cases.
#[derive(Debug, thiserror::Error)]
pub enum UserError {
    #[error("invalid user: {0}")]
    InvalidField(String),
    #[error("a user named \"{0}\" already exists")]
    DuplicateUsername(String),
    #[error("user {0} was not found")]
    NotFound(UserId),
    #[error("invalid username or password")]
    InvalidCredentials,
    #[error("missing, invalid or expired token")]
    InvalidToken,
    #[error("at least one admin must remain")]
    LastAdmin,
    #[error("user storage error: {0}")]
    Storage(String),
}

/// Port: persistence for `User` aggregates, implemented by an infrastructure adapter.
#[async_trait]
pub trait UserRepository: Send + Sync {
    async fn save(&self, user: &User) -> Result<(), UserError>;
    async fn find_by_id(&self, id: UserId) -> Result<Option<User>, UserError>;
    async fn list(&self) -> Result<Vec<User>, UserError>;
    async fn delete(&self, id: UserId) -> Result<(), UserError>;
}

/// Port: one-way password hashing.
#[async_trait]
pub trait PasswordHasher: Send + Sync {
    /// Returns a self-describing (PHC-format) hash of `password`, with a fresh random salt.
    async fn hash(&self, password: &str) -> Result<String, UserError>;
    /// Whether `password` matches `hash`. A malformed hash never matches.
    async fn verify(&self, password: &str, hash: &str) -> bool;
}

/// Port: issuing and verifying the bearer tokens handed out at login.
pub trait TokenService: Send + Sync {
    fn issue(&self, user: &User) -> Result<String, UserError>;
    /// The user a still-valid token was issued to.
    fn verify(&self, token: &str) -> Result<UserId, UserError>;
}
