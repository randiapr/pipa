//! Axum REST API exposing DataFusion/Iceberg query access, plus OLTP data source management
//! consumed by the `pipa-ui` dashboard. `pipa-core` reads the same registered data sources
//! back out of `pipa-data`'s object-store-backed repository to drive CDC capture.

mod http;

use std::sync::Arc;

use axum::{routing::get, Router};
use pipa_data::{
    datasource::{
        infrastructure::{ObjectStoreDataSourceRepository, SqlxConnectionTester},
        DataSourceService,
    },
    ObjectStoreConfig,
};
use tower_http::cors::CorsLayer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let store = ObjectStoreConfig::from_env().build_store()?;
    let repository = Arc::new(ObjectStoreDataSourceRepository::new(store));
    let tester = Arc::new(SqlxConnectionTester);
    let datasource_service = Arc::new(DataSourceService::new(repository, tester));

    let app = Router::new()
        .route("/healthz", get(health))
        .merge(http::datasource_routes())
        .with_state(datasource_service)
        .layer(CorsLayer::permissive());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    tracing::info!("pipa-rest listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;

    Ok(())
}

async fn health() -> &'static str {
    "ok"
}
