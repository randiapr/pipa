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

    let migration_store = store.clone();
    let jwt_secret = std::env::var("JWT_SECRET")
        .context("JWT_SECRET must be set (at least 32 bytes) to sign login tokens")?;
    let user_service = Arc::new(UserService::new(
        Arc::new(ObjectStoreUserRepository::new(store)),
        Arc::new(Argon2PasswordHasher),
        Arc::new(JwtTokenService::new(&jwt_secret).context("invalid JWT_SECRET")?),
    ));
    migrate_legacy_user_roles(&migration_store, &user_service).await?;
    bootstrap_admin(&user_service).await?;

    let query_api = Arc::new(http::QueryApi {
        queries: QueryService::new(
            IcebergCatalogConfig::from_env(&store_config).await?,
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

/// Every route. Everything except `/healthz` and `/auth/login` requires a signed-in user; what a
/// signed-in user may do then depends on their role (see `http::auth`).
fn router(
    datasources: Arc<DataSourceService>,
    projects: Arc<ProjectService>,
    users: Arc<UserService>,
    query_api: Arc<http::QueryApi>,
) -> Router {
    let protected = Router::new()
        .merge(http::datasource_routes().with_state(datasources))
        .merge(http::project_routes().with_state(projects))
        .merge(http::query_routes().with_state(query_api.clone()))
        .merge(http::table_routes().with_state(query_api))
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

/// Object that records the `user` → `developer` migration as done.
const LEGACY_ROLE_MARKER: &str = "migrations/user-role-to-developer.done";

/// Before the `developer` role existed, `user` meant "everything inside my projects", which is
/// what `developer` means now (`user` became view-only). Once per store, promotes every stored
/// `user` account to `developer` so nobody loses access. The marker keeps view-only users
/// created afterwards from being promoted on the next start.
async fn migrate_legacy_user_roles(
    store: &Arc<dyn object_store::ObjectStore>,
    users: &UserService,
) -> anyhow::Result<()> {
    use object_store::{ObjectStoreExt, PutPayload, path::Path};

    let marker = Path::from(LEGACY_ROLE_MARKER);
    match store.head(&marker).await {
        Ok(_) => return Ok(()),
        Err(object_store::Error::NotFound { .. }) => {}
        Err(err) => return Err(err).context("checking the user role migration marker"),
    }

    let promoted = users
        .reassign_role(user::Role::User, user::Role::Developer)
        .await?;
    if promoted > 0 {
        tracing::info!("promoted {promoted} existing \"user\" account(s) to developer");
    }
    store
        .put(&marker, PutPayload::from_static(b"done"))
        .await
        .context("recording the user role migration")?;
    Ok(())
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
                IcebergCatalogConfig {
                    name: "pipa".to_string(),
                    uri: "http://localhost:1/iceberg".to_string(),
                    warehouse: "pipa".to_string(),
                },
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
    async fn legacy_user_accounts_become_developers_exactly_once() {
        let store: Arc<dyn object_store::ObjectStore> = Arc::new(InMemory::new());
        let users = UserService::new(
            Arc::new(ObjectStoreUserRepository::new(store.clone())),
            Arc::new(Argon2PasswordHasher),
            Arc::new(JwtTokenService::new("0123456789abcdef0123456789abcdef").unwrap()),
        );
        let new = |name: &str, role| user::NewUser {
            username: name.to_string(),
            password: "long-enough-password".to_string(),
            role,
            project_ids: Vec::new(),
        };
        users.create(new("root", user::Role::Admin)).await.unwrap();
        users.create(new("old", user::Role::User)).await.unwrap();

        migrate_legacy_user_roles(&store, &users).await.unwrap();
        let roles = |users: Vec<user::User>| {
            let mut roles: Vec<_> = users.into_iter().map(|u| (u.username, u.role)).collect();
            roles.sort_by(|a, b| a.0.cmp(&b.0));
            roles
        };
        assert_eq!(
            roles(users.list().await.unwrap()),
            [
                ("old".to_string(), user::Role::Developer),
                ("root".to_string(), user::Role::Admin)
            ]
        );

        // A view-only account created afterwards survives the next start.
        users.create(new("viewer", user::Role::User)).await.unwrap();
        migrate_legacy_user_roles(&store, &users).await.unwrap();
        let viewer = users
            .list()
            .await
            .unwrap()
            .into_iter()
            .find(|u| u.username == "viewer")
            .unwrap();
        assert_eq!(viewer.role, user::Role::User);
    }

    #[tokio::test]
    async fn protected_routes_need_a_valid_token() {
        let app = app().await;
        for uri in [
            "/datasources",
            "/projects",
            "/users",
            "/auth/me",
            "/tables?project_id=x",
        ] {
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
    async fn a_developer_only_sees_and_touches_their_assigned_project() {
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
                "role": "developer",
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

    /// Registers a source in `project` as `admin`, returning its id.
    async fn register_source(app: &Router, admin: &str, project: &str) -> String {
        let (status, body) = call(
            app,
            "POST",
            "/datasources",
            Some(admin),
            Some(json!({
                "name": "orders",
                "engine": "postgres",
                "connection": {
                    "host": "db", "port": 5432, "username": "u", "password": "p", "database": "d"
                },
                "project_id": project,
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body["datasource"]["id"].as_str().unwrap().to_string()
    }

    async fn create_account(app: &Router, admin: &str, name: &str, role: &str, project: &str) {
        let (status, body) = call(
            app,
            "POST",
            "/users",
            Some(admin),
            Some(json!({
                "username": name,
                "password": format!("{name}-password"),
                "role": role,
                "project_ids": [project],
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }

    async fn create_project(app: &Router, admin: &str, name: &str) -> String {
        let (_, body) = call(
            app,
            "POST",
            "/projects",
            Some(admin),
            Some(json!({ "name": name })),
        )
        .await;
        body["project"]["id"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn a_plain_user_can_only_browse_tables_of_their_project() {
        let app = app().await;
        let admin = login(&app, "root", "root-password").await;
        let alpha = create_project(&app, &admin, "alpha").await;
        let beta = create_project(&app, &admin, "beta").await;
        let alpha_source = register_source(&app, &admin, &alpha).await;
        let beta_source = register_source(&app, &admin, &beta).await;
        create_account(&app, &admin, "viewer", "user", &alpha).await;
        let viewer = login(&app, "viewer", "viewer-password").await;

        // Everything that builds, or reveals connection details, is closed to a viewer.
        let forbidden: [(&str, String, Option<Value>); 7] = [
            ("GET", "/datasources".into(), None),
            ("GET", format!("/datasources?project_id={alpha}"), None),
            ("GET", format!("/datasources/{alpha_source}"), None),
            ("POST", format!("/datasources/{alpha_source}/test"), None),
            ("DELETE", format!("/datasources/{alpha_source}"), None),
            (
                "POST",
                "/query".into(),
                Some(json!({ "sql": "SELECT 1", "project_id": alpha })),
            ),
            ("GET", "/users".into(), None),
        ];
        for (method, uri, body) in forbidden {
            let (status, _) = call(&app, method, &uri, Some(&viewer), body).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
        }

        // Tables of another project are off limits, in the list and by name.
        let (status, _) = call(
            &app,
            "GET",
            &format!("/tables?project_id={beta}"),
            Some(&viewer),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let read = |project: &str, source: &str| json!({ "project_id": project, "source_id": source, "table": "public__orders" });
        let (status, _) = call(
            &app,
            "POST",
            "/tables/rows",
            Some(&viewer),
            Some(read(&beta, &beta_source)),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // In their own project a source of another project is refused before the catalog is
        // touched, and malformed ids are a bad request.
        let (status, _) = call(
            &app,
            "POST",
            "/tables/rows",
            Some(&viewer),
            Some(read(&alpha, &beta_source)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = call(
            &app,
            "POST",
            "/tables/rows",
            Some(&viewer),
            Some(read(&alpha, "nope")),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Iceberg metadata tables aren't browsable, for any role.
        for (who, token) in [("viewer", &viewer), ("admin", &admin)] {
            for table in ["public__orders$snapshots", "public__orders$manifests"] {
                let (status, _) = call(
                    &app,
                    "POST",
                    "/tables/rows",
                    Some(token),
                    Some(json!({ "project_id": alpha, "source_id": alpha_source, "table": table })),
                )
                .await;
                assert_eq!(status, StatusCode::BAD_REQUEST, "{who} {table}");
            }
        }

        // Their own project's tables get as far as the (unreachable) catalog.
        let (status, body) = call(
            &app,
            "GET",
            &format!("/tables?project_id={alpha}"),
            Some(&viewer),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
        let (status, body) = call(
            &app,
            "POST",
            "/tables/rows",
            Some(&viewer),
            Some(read(&alpha, &alpha_source)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    }

    #[tokio::test]
    async fn a_developer_builds_inside_their_project_but_manages_nothing_else() {
        let app = app().await;
        let admin = login(&app, "root", "root-password").await;
        let alpha = create_project(&app, &admin, "alpha").await;
        let beta = create_project(&app, &admin, "beta").await;
        create_account(&app, &admin, "dev", "developer", &alpha).await;
        let dev = login(&app, "dev", "dev-password").await;

        // Developers have no part in user management or project administration.
        let (status, _) = call(&app, "GET", "/users", Some(&dev), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = call(
            &app,
            "POST",
            "/projects",
            Some(&dev),
            Some(json!({ "name": "gamma" })),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // But they build in their own project: sources and free SQL are open to them there.
        let (status, _) = call(&app, "GET", "/datasources", Some(&dev), None).await;
        assert_eq!(status, StatusCode::OK);
        let source = register_source(&app, &dev, &alpha).await;
        let (status, _) = call(
            &app,
            "POST",
            "/query",
            Some(&dev),
            Some(json!({ "sql": "SELECT 1", "project_id": alpha })),
        )
        .await;
        assert_ne!(status, StatusCode::FORBIDDEN);
        let (status, _) = call(
            &app,
            "DELETE",
            &format!("/datasources/{source}"),
            Some(&dev),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // ...and not in anyone else's.
        let (status, _) = call(
            &app,
            "POST",
            "/query",
            Some(&dev),
            Some(json!({ "sql": "SELECT 1", "project_id": beta })),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = call(
            &app,
            "GET",
            &format!("/tables?project_id={beta}"),
            Some(&dev),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
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
