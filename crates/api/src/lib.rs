//! The HTTP API contract between `pipa-ui` and `pipa-backend`: request/response bodies, the
//! shared response envelope, and route paths.
//!
//! This crate is deliberately dependency-light (`serde`/`serde_json` only) so it compiles for
//! both native targets and `wasm32-unknown-unknown`. It holds *wire* types only — no
//! validation, persistence, or HTTP-framework code. `pipa-backend` maps its domain types to and
//! from these at its HTTP boundary (`src/http/convert.rs`), and `pipa-ui` uses them directly, so
//! a change to the wire format is a compile error on both sides rather than silent drift.
//! Identifiers are plain UUID strings for that reason: the crate needs no `uuid` dependency
//! (whose `v4` feature doesn't build on wasm32 without extra setup).
//!
//! `pipa-ingestion` does not use this crate — it never speaks HTTP.

pub mod datasource;
pub mod envelope;
pub mod path;
pub mod project;
pub mod query;
pub mod user;

pub use datasource::{
    ConnectionConfig, ConnectionTest, ConnectionTestOutcome, ConnectionTestResponse,
    DataSourceData, DataSourceResponse, DataSourceView, DataSources, DataSourcesResponse, DbEngine,
    NewDataSource,
};
pub use envelope::{BaseResponse, Empty, ErrorBody, ErrorResponse, MessageResponse, ResponseCode};
pub use project::{
    NewProject, ProjectData, ProjectResponse, ProjectUpdate, ProjectView, Projects,
    ProjectsResponse,
};
pub use query::{QueryRequest, Rows, RowsResponse};
pub use user::{
    LoginData, LoginRequest, LoginResponse, MeData, MeResponse, NewUser, Role, UserData,
    UserResponse, UserUpdate, UserView, Users, UsersResponse,
};
