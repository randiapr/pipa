//! Interface adapter: translates between Axum HTTP requests/responses and the
//! `crate::datasource`/`crate::project`/`crate::iceberg`/`crate::user` application
//! services. Keeps those layers free of any HTTP
//! concerns — `pipa-backend` is the only crate that should grow HTTP-facing surface area, so all of it lives here, one module per resource.

mod auth;
mod convert;
mod datasource;
mod error;
mod project;
mod query;
mod table;
mod user;

pub use auth::{login_routes, me_routes, require_auth};
pub use datasource::routes as datasource_routes;
pub use project::routes as project_routes;
pub use query::{QueryApi, routes as query_routes};
pub use table::routes as table_routes;
pub use user::routes as user_routes;
