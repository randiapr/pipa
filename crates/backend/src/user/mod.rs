//! Bounded context: accounts, roles and authentication.
//!
//! Organized with the same clean-architecture split as [`crate::datasource`]:
//! - [`domain`] — the `User` aggregate, `Role`, and the `UserRepository` / `PasswordHasher` /
//!   `TokenService` ports.
//! - [`application`] — `UserService`, the use cases (account management, login, token checks).
//! - [`infrastructure`] — concrete adapters: an object-store-backed repository, Argon2id
//!   password hashing and HS256 JWTs.

pub mod application;
pub mod domain;
pub mod infrastructure;

pub use application::UserService;
pub use domain::{NewUser, Role, User, UserError, UserId, UserUpdate};
