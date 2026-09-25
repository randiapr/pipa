//! Interface adapter: translates between Axum HTTP requests/responses and the
//! `pipa-storage`/`crate::iceberg` application services. Keeps those layers free of any HTTP
//! concerns — `pipa-backend` is the only crate that should grow HTTP-facing surface area, so all of it lives here, one module per resource.

mod datasource;
mod error;
mod project;
mod query;

pub use datasource::routes as datasource_routes;
pub use project::routes as project_routes;
pub use query::routes as query_routes;
