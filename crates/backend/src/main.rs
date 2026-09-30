//! Axum REST API exposing DataFusion/Iceberg query access, plus OLTP data source and project
//! management consumed by the `pipa-ui` dashboard. `pipa-ingestion` reads the same registered
//! data sources back out of the shared object store (via its own standalone duplicate of this
//! read path, not a dependency on this crate) to drive CDC capture.

mod datasource;
mod http;
mod iceberg;
mod project;
mod storage;
mod user;

use std::sync::Arc;

use anyhow::Context;
use axum::{
    Json, Router,
    http::{HeaderValue, Method, header},
    middleware,
    routing::get,
};
use datasource::{
    DataSourceService,
    infrastructure::{ObjectStoreDataSourceRepository, SqlxConnectionTester},
};
use iceberg::{IcebergCatalogConfig, QueryService};
use project::{ProjectService, infrastructure::ObjectStoreProjectRepository};
use storage::ObjectStoreConfig;
use tower_http::cors::CorsLayer;
use user::{
    UserService,
    infrastructure::{Argon2PasswordHasher, JwtTokenService, ObjectStoreUserRepository},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let store_config = ObjectStoreConfig::from_env();
    let store = store_config.build_store()?;

    let datasource_repository = Arc::new(ObjectStoreDataSourceRepository::new(store.clone()));
    let tester = Arc::new(SqlxConnectionTester);
    let datasource_service = Arc::new(DataSourceService::new(datasource_repository, tester));

    let project_repository = Arc::new(ObjectStoreProjectRepository::new(store.clone()));
    let project_service = Arc::new(ProjectService::new(project_repository));

    let jwt_secret = std::env::var("JWT_SECRET")
        .context("JWT_SECRET must be set (at least 32 bytes) to sign login tokens")?;
    let user_service = Arc::new(UserService::new(
        Arc::new(ObjectStoreUserRepository::new(store)),
        Arc::new(Argon2PasswordHasher),
        Arc::new(JwtTokenService::new(&jwt_secret).context("invalid JWT_SECRET")?),
    ));
    bootstrap_admin(&user_service).await?;

    let query_api = Arc::new(http::QueryApi {
        queries: QueryService::new(
            IcebergCatalogConfig::from_env(&store_config.endpoint),
            store_config,
        ),
        datasources: datasource_service.clone(),
    });

    let app =
        router(datasource_service, project_service, user_service, query_api).layer(cors_layer()?);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    tracing::info!("pipa-backend listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;

    Ok(())
}

/// Every route. Everything except `/healthz` and `/auth/login` requires a signed-in user.
fn router(
    datasources: Arc<DataSourceService>,
    projects: Arc<ProjectService>,
    users: Arc<UserService>,
    query_api: Arc<http::QueryApi>,
) -> Router {
    let protected = Router::new()
        .merge(http::datasource_routes().with_state(datasources))
        .merge(http::project_routes().with_state(projects))
        .merge(http::query_routes().with_state(query_api))
        .merge(http::user_routes().with_state(users.clone()))
        .merge(http::me_routes().with_state(users.clone()))
        .route_layer(middleware::from_fn_with_state(
            users.clone(),
            http::require_auth,
        ));

    Router::new()
        .route("/healthz", get(health))
        .merge(http::login_routes().with_state(users))
        .merge(protected)
}

/// Creates the first admin from `PIPA_ADMIN_USERNAME`/`PIPA_ADMIN_PASSWORD` when no user exists
/// yet. Once any user exists these are ignored. Fails if there is no way to sign in at all.
async fn bootstrap_admin(users: &UserService) -> anyhow::Result<()> {
    if let (Ok(username), Ok(password)) = (
        std::env::var("PIPA_ADMIN_USERNAME"),
        std::env::var("PIPA_ADMIN_PASSWORD"),
    ) && users.bootstrap_admin(&username, &password).await?
    {
        tracing::info!("created the initial admin user \"{username}\"");
    }

    if !users.has_users().await? {
        anyhow::bail!(
            "no users exist yet: set PIPA_ADMIN_USERNAME and PIPA_ADMIN_PASSWORD to create the first admin"
        );
    }
    Ok(())
}

/// CORS for the dashboard: only the origins in `CORS_ALLOWED_ORIGINS` (comma-separated, default
/// the local dev server), and only the methods and headers the API uses.
fn cors_layer() -> anyhow::Result<CorsLayer> {
    let origins = std::env::var("CORS_ALLOWED_ORIGINS")
        .unwrap_or_else(|_| "http://localhost:3000".to_string())
        .split(',')
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .map(|origin| {
            HeaderValue::from_str(origin)
                .with_context(|| format!("invalid origin in CORS_ALLOWED_ORIGINS: {origin}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]))
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "response_code": 1000,
        "response_message": "OK",
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use object_store::memory::InMemory;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;

    /// The full router over an in-memory object store, seeded with the admin `root`.
    async fn app() -> Router {
        let store: Arc<dyn object_store::ObjectStore> = Arc::new(InMemory::new());
        let datasources = Arc::new(DataSourceService::new(
            Arc::new(ObjectStoreDataSourceRepository::new(store.clone())),
            Arc::new(SqlxConnectionTester),
        ));
        let projects = Arc::new(ProjectService::new(Arc::new(
            ObjectStoreProjectRepository::new(store.clone()),
        )));
        let users = Arc::new(UserService::new(
            Arc::new(ObjectStoreUserRepository::new(store)),
            Arc::new(Argon2PasswordHasher),
            Arc::new(JwtTokenService::new("0123456789abcdef0123456789abcdef").unwrap()),
        ));
        users
            .bootstrap_admin("root", "root-password")
            .await
            .unwrap();
        let query_api = Arc::new(http::QueryApi {
            queries: QueryService::new(
                IcebergCatalogConfig::from_env("http://localhost:1"),
                ObjectStoreConfig::from_env(),
            ),
            datasources: datasources.clone(),
        });
        router(datasources, projects, users, query_api)
    }

    async fn call(
        app: &Router,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let request = match body {
            Some(body) => request
                .header("content-type", "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
        .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn login(app: &Router, username: &str, password: &str) -> String {
        let (status, body) = call(
            app,
            "POST",
            "/auth/login",
            None,
            Some(json!({ "username": username, "password": password })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["token"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn protected_routes_need_a_valid_token() {
        let app = app().await;
        for uri in ["/datasources", "/projects", "/users", "/auth/me"] {
            let (status, body) = call(&app, "GET", uri, None, None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
            assert_eq!(body["response_code"], 2005);
        }
        let (status, _) = call(&app, "GET", "/projects", Some("garbage"), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, _) = call(&app, "GET", "/healthz", None, None).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn login_rejects_bad_credentials() {
        let app = app().await;
        let (status, _) = call(
            &app,
            "POST",
            "/auth/login",
            None,
            Some(json!({ "username": "root", "password": "wrong" })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_user_only_sees_and_touches_their_assigned_project() {
        let app = app().await;
        let admin = login(&app, "root", "root-password").await;

        let mut project_ids = Vec::new();
        for name in ["alpha", "beta"] {
            let (status, body) = call(
                &app,
                "POST",
                "/projects",
                Some(&admin),
                Some(json!({ "name": name })),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED);
            project_ids.push(body["project"]["id"].as_str().unwrap().to_string());
        }
        let (alpha, beta) = (&project_ids[0], &project_ids[1]);

        let (status, _) = call(
            &app,
            "POST",
            "/users",
            Some(&admin),
            Some(json!({
                "username": "alice",
                "password": "alice-password",
                "role": "user",
                "project_ids": [alpha],
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let alice = login(&app, "alice", "alice-password").await;

        // Admin-only areas are closed to her.
        for (method, uri) in [("GET", "/users"), ("POST", "/projects")] {
            let body = (method == "POST").then(|| json!({ "name": "gamma" }));
            let (status, body) = call(&app, method, uri, Some(&alice), body).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
            assert_eq!(body["response_code"], 2006);
        }

        // She sees only her project.
        let (_, body) = call(&app, "GET", "/projects", Some(&alice), None).await;
        let visible: Vec<_> = body["projects"].as_array().unwrap().iter().collect();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0]["id"], alpha.as_str());
        let (status, _) = call(
            &app,
            "GET",
            &format!("/projects/{beta}"),
            Some(&alice),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // She can register a source in her project, not in another or in none.
        let source = |project_id: Option<&str>| {
            json!({
                "name": "orders",
                "engine": "postgres",
                "connection": {
                    "host": "db", "port": 5432, "username": "u", "password": "p", "database": "d"
                },
                "project_id": project_id,
            })
        };
        let (status, created) = call(
            &app,
            "POST",
            "/datasources",
            Some(&alice),
            Some(source(Some(alpha))),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        for other in [Some(beta.as_str()), None] {
            let (status, _) = call(
                &app,
                "POST",
                "/datasources",
                Some(&alice),
                Some(source(other)),
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN);
        }

        // An admin registers one in beta; she must not see it in lists or by id.
        let (_, hidden) = call(
            &app,
            "POST",
            "/datasources",
            Some(&admin),
            Some(source(Some(beta))),
        )
        .await;
        let hidden_id = hidden["datasource"]["id"].as_str().unwrap();
        let (_, body) = call(&app, "GET", "/datasources", Some(&alice), None).await;
        assert_eq!(body["datasources"].as_array().unwrap().len(), 1);
        let (status, _) = call(
            &app,
            "GET",
            &format!("/datasources/{hidden_id}"),
            Some(&alice),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = call(
            &app,
            "GET",
            &format!("/datasources?project_id={beta}"),
            Some(&alice),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // Queries need one of her own projects.
        for project_id in [None, Some(beta.as_str())] {
            let (status, _) = call(
                &app,
                "POST",
                "/query",
                Some(&alice),
                Some(json!({ "sql": "SELECT 1", "project_id": project_id })),
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{project_id:?}");
        }
    }

    #[tokio::test]
    async fn role_changes_apply_to_tokens_already_issued() {
        let app = app().await;
        let admin = login(&app, "root", "root-password").await;
        let (_, created) = call(
            &app,
            "POST",
            "/users",
            Some(&admin),
            Some(json!({ "username": "bob", "password": "bob-password", "role": "admin" })),
        )
        .await;
        let bob_id = created["user"]["id"].as_str().unwrap();
        let bob = login(&app, "bob", "bob-password").await;
        let (status, _) = call(&app, "GET", "/users", Some(&bob), None).await;
        assert_eq!(status, StatusCode::OK);

        let (status, _) = call(
            &app,
            "PUT",
            &format!("/users/{bob_id}"),
            Some(&admin),
            Some(json!({ "role": "user" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = call(&app, "GET", "/users", Some(&bob), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn the_last_admin_cannot_be_deleted() {
        let app = app().await;
        let admin = login(&app, "root", "root-password").await;
        let (_, me) = call(&app, "GET", "/auth/me", Some(&admin), None).await;
        let id = me["user"]["id"].as_str().unwrap();
        let (status, _) = call(&app, "DELETE", &format!("/users/{id}"), Some(&admin), None).await;
        assert_eq!(status, StatusCode::CONFLICT);
    }
}
