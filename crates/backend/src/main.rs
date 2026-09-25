//! Axum REST API exposing DataFusion/Iceberg query access, plus OLTP data source and project
//! management consumed by the `pipa-ui` dashboard. `pipa-ingestion` reads the same registered
//! data sources back out of the shared object store (via its own standalone duplicate of this
//! read path, not a dependency on `pipa-storage`) to drive CDC capture.

mod http;
mod iceberg;

use std::sync::Arc;

use axum::{Json, Router, routing::get};
use iceberg::{IcebergCatalogConfig, QueryService};
use pipa_storage::{
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

    let store_config = ObjectStoreConfig::from_env();
    let store = store_config.build_store()?;

    let datasource_repository = Arc::new(ObjectStoreDataSourceRepository::new(store.clone()));
    let tester = Arc::new(SqlxConnectionTester);
    let datasource_service = Arc::new(DataSourceService::new(datasource_repository, tester));

    let project_repository = Arc::new(ObjectStoreProjectRepository::new(store));
    let project_service = Arc::new(ProjectService::new(project_repository));

    let query_service = Arc::new(QueryService::new(
        IcebergCatalogConfig::from_env(),
        store_config,
    ));

    let app = Router::new()
        .route("/healthz", get(health))
        .merge(http::datasource_routes().with_state(datasource_service))
        .merge(http::project_routes().with_state(project_service))
        .merge(http::query_routes().with_state(query_service))
        .layer(CorsLayer::permissive());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    tracing::info!("pipa-backend listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;

    Ok(())
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}
