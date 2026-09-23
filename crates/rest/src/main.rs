//! Axum REST API exposing DataFusion/Iceberg query access, plus OLTP data source and project
//! management consumed by the `pipa-ui` dashboard. `pipa-core` reads the same registered data
//! sources back out of `pipa-data`'s object-store-backed repository to drive CDC capture.

mod http;

use std::sync::Arc;

use axum::{Json, Router, routing::get};
use pipa_data::{
    ObjectStoreConfig,
    datasource::{
        DataSourceService,
        infrastructure::{ObjectStoreDataSourceRepository, SqlxConnectionTester},
    },
    project::{ProjectService, infrastructure::ObjectStoreProjectRepository},
};
use tower_http::cors::CorsLayer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let store = ObjectStoreConfig::from_env().build_store()?;

    let datasource_repository = Arc::new(ObjectStoreDataSourceRepository::new(store.clone()));
    let tester = Arc::new(SqlxConnectionTester);
    let datasource_service = Arc::new(DataSourceService::new(datasource_repository, tester));

    let project_repository = Arc::new(ObjectStoreProjectRepository::new(store));
    let project_service = Arc::new(ProjectService::new(project_repository));

    let app = Router::new()
        .route("/healthz", get(health))
        .merge(http::datasource_routes().with_state(datasource_service))
        .merge(http::project_routes().with_state(project_service))
        .layer(CorsLayer::permissive());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    tracing::info!("pipa-rest listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;

    Ok(())
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}
