// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! Signing in, tokens, who is let through, and the backend's startup checks.

use pipa_api::{LoginData, MeData, NewUser, Role, UserUpdate, path};
use pipa_integration_test::backend::{
    ADMIN_PASSWORD, ADMIN_USERNAME, Backend, BackendConfig, JWT_SECRET,
};
use pipa_integration_test::env::{TestEnv, new_bucket};
use reqwest::Method;
use rstest::rstest;

use crate::common::{env, own_env};

// ---- health ----

#[rstest]
#[tokio::test]
async fn healthz_is_public_and_reports_the_version(env: &'static TestEnv) {
    let reply = env.backend.get("/healthz").send().await.ok();
    assert_eq!(reply.body["status"], "ok");
    assert!(!reply.body["version"].as_str().unwrap().is_empty());
}

// ---- login ----

#[rstest]
#[tokio::test]
async fn login_returns_a_token_and_the_account(env: &'static TestEnv) {
    let reply = env.backend.login(ADMIN_USERNAME, ADMIN_PASSWORD).await.ok();
    let data: LoginData = reply.parse();
    assert!(!data.token.is_empty());
    assert_eq!(data.user.username, ADMIN_USERNAME);
    assert_eq!(data.user.role, Role::Admin);
}

#[rstest]
#[tokio::test]
async fn login_never_exposes_the_password_or_its_hash(env: &'static TestEnv) {
    let reply = env.backend.login(ADMIN_USERNAME, ADMIN_PASSWORD).await.ok();
    let text = reply.body.to_string();
    assert!(!text.contains(ADMIN_PASSWORD), "{text}");
    assert!(!text.contains("password"), "{text}");
    assert!(!text.contains("argon2"), "{text}");
}

#[rstest]
#[tokio::test]
async fn me_returns_the_signed_in_account(env: &'static TestEnv) {
    let project = env.unique_project().await;
    let developer = env.developer(&[&project]).await;

    let me: MeData = env
        .backend
        .get(path::ME)
        .bearer(&developer.token)
        .send()
        .await
        .ok()
        .parse();
    assert_eq!(me.user.id, developer.user.id);
    assert_eq!(me.user.role, Role::Developer);
    assert_eq!(me.user.project_ids, vec![project.id]);
}

#[rstest]
#[tokio::test]
async fn login_with_a_wrong_password_is_unauthorized(env: &'static TestEnv) {
    env.backend
        .login(ADMIN_USERNAME, "not-the-password")
        .await
        .unauthorized();
}

#[rstest]
#[tokio::test]
async fn login_with_an_unknown_user_is_unauthorized_and_looks_like_a_wrong_password(
    env: &'static TestEnv,
) {
    let wrong_password = env.backend.login(ADMIN_USERNAME, "not-the-password").await;
    let unknown_user = env
        .backend
        .login("nobody-by-this-name", "whatever-it-is")
        .await;
    // The two must not tell an attacker which usernames exist.
    assert_eq!(wrong_password.error(), unknown_user.error());
    unknown_user.unauthorized();
}

#[rstest]
#[case::empty_username("", ADMIN_PASSWORD)]
#[case::empty_password(ADMIN_USERNAME, "")]
#[case::both_empty("", "")]
#[tokio::test]
async fn login_with_unusable_credentials_is_unauthorized(
    env: &'static TestEnv,
    #[case] username: &str,
    #[case] password: &str,
) {
    env.backend.login(username, password).await.unauthorized();
}

#[rstest]
#[tokio::test]
async fn usernames_are_matched_without_regard_to_case(env: &'static TestEnv) {
    env.backend.login("ADMIN", ADMIN_PASSWORD).await.ok();
    // The password is not.
    env.backend
        .login(ADMIN_USERNAME, &ADMIN_PASSWORD.to_uppercase())
        .await
        .unauthorized();
}

#[rstest]
#[case::not_json("this is not json", 400)]
#[case::truncated_json("{\"username\": \"admin\"", 400)]
#[case::no_fields("{}", 422)]
#[case::missing_password("{\"username\": \"admin\"}", 422)]
#[case::wrong_types("{\"username\": 1, \"password\": 2}", 422)]
#[tokio::test]
async fn login_with_a_malformed_body_is_rejected(
    env: &'static TestEnv,
    #[case] body: &str,
    #[case] status: u16,
) {
    let reply = env.backend.post(path::LOGIN).raw_json(body).send().await;
    assert_eq!(reply.status, status, "{}", reply.body);
}

#[rstest]
#[tokio::test]
async fn login_without_a_json_content_type_is_rejected(env: &'static TestEnv) {
    let reply = env
        .backend
        .post(path::LOGIN)
        .header("content-type", "text/plain")
        .header("content-length", "2")
        .send()
        .await;
    assert_eq!(reply.status, 415, "{}", reply.body);
}

// ---- tokens ----

/// Every route but `/healthz` and `/auth/login` is behind the sign-in check.
#[rstest]
#[case::me("GET", "/auth/me")]
#[case::list_users("GET", "/users")]
#[case::create_user("POST", "/users")]
#[case::get_user("GET", "/users/00000000-0000-0000-0000-000000000000")]
#[case::update_user("PUT", "/users/00000000-0000-0000-0000-000000000000")]
#[case::delete_user("DELETE", "/users/00000000-0000-0000-0000-000000000000")]
#[case::list_projects("GET", "/projects")]
#[case::create_project("POST", "/projects")]
#[case::get_project("GET", "/projects/00000000-0000-0000-0000-000000000000")]
#[case::update_project("PUT", "/projects/00000000-0000-0000-0000-000000000000")]
#[case::delete_project("DELETE", "/projects/00000000-0000-0000-0000-000000000000")]
#[case::list_datasources("GET", "/datasources")]
#[case::register_datasource("POST", "/datasources")]
#[case::get_datasource("GET", "/datasources/00000000-0000-0000-0000-000000000000")]
#[case::delete_datasource("DELETE", "/datasources/00000000-0000-0000-0000-000000000000")]
#[case::test_datasource("POST", "/datasources/00000000-0000-0000-0000-000000000000/test")]
#[case::source_tables("GET", "/datasources/00000000-0000-0000-0000-000000000000/tables")]
#[case::choose_tables("PUT", "/datasources/00000000-0000-0000-0000-000000000000/tables")]
#[case::list_tables("GET", "/tables?project_id=00000000-0000-0000-0000-000000000000")]
#[case::read_table("POST", "/tables/rows")]
#[case::query("POST", "/query")]
#[tokio::test]
async fn every_protected_route_requires_a_token(
    env: &'static TestEnv,
    #[case] method: &str,
    #[case] route: &str,
) {
    let method = Method::from_bytes(method.as_bytes()).unwrap();
    env.backend
        .request(method, route)
        .raw_json("{}")
        .send()
        .await
        .unauthorized();
}

#[rstest]
#[case::garbage("Bearer not-a-jwt")]
#[case::empty_token("Bearer ")]
#[case::wrong_scheme("Basic YWRtaW46YWRtaW4=")]
#[case::no_scheme("not-a-jwt")]
#[case::almost_a_jwt("Bearer aaaa.bbbb.cccc")]
#[tokio::test]
async fn an_invalid_authorization_header_is_unauthorized(
    env: &'static TestEnv,
    #[case] header: &str,
) {
    env.backend
        .get(path::ME)
        .header("authorization", header)
        .send()
        .await
        .unauthorized();
}

#[rstest]
#[tokio::test]
async fn the_bearer_scheme_is_case_insensitive(env: &'static TestEnv) {
    env.backend
        .get(path::ME)
        .header("authorization", &format!("bEaReR {}", env.admin))
        .send()
        .await
        .ok();
}

#[rstest]
#[tokio::test]
async fn a_tampered_token_is_unauthorized(env: &'static TestEnv) {
    // Flip the first character of the signature (the last of the three parts). Not the last
    // character: its low bits are base64 padding that don't change the decoded signature.
    let (head, signature) = env.admin.rsplit_once('.').unwrap();
    let flipped = if signature.starts_with('A') { 'B' } else { 'A' };
    let tampered = format!("{head}.{flipped}{}", &signature[1..]);
    assert_ne!(tampered, env.admin);
    env.backend
        .get(path::ME)
        .bearer(&tampered)
        .send()
        .await
        .unauthorized();
}

#[rstest]
#[tokio::test]
async fn a_token_signed_with_another_secret_is_unauthorized(env: &'static TestEnv) {
    // A second backend on the same store (so the same admin account) that signs with another
    // secret: its token names a real account, but this backend must not accept it.
    let other = Backend::start(BackendConfig {
        jwt_secret: Some("another-secret-0123456789abcdefghijkl".to_string()),
        ..env.backend.config.clone()
    })
    .await
    .unwrap();
    let foreign = other.token(ADMIN_USERNAME, ADMIN_PASSWORD).await;
    other.get(path::ME).bearer(&foreign).send().await.ok();

    env.backend
        .get(path::ME)
        .bearer(&foreign)
        .send()
        .await
        .unauthorized();
}

#[rstest]
#[tokio::test]
async fn the_token_of_a_deleted_account_stops_working_at_once(#[future] own_env: TestEnv) {
    let env = own_env.await;
    let developer = env.developer(&[]).await;
    env.backend
        .get(path::ME)
        .bearer(&developer.token)
        .send()
        .await
        .ok();

    env.backend
        .delete(&path::user(&developer.user.id))
        .bearer(&env.admin)
        .send()
        .await
        .deleted();

    env.backend
        .get(path::ME)
        .bearer(&developer.token)
        .send()
        .await
        .unauthorized();
}

#[rstest]
#[tokio::test]
async fn a_role_change_applies_to_a_token_already_issued(env: &'static TestEnv) {
    let developer = env.developer(&[]).await;
    env.backend
        .get(path::USERS)
        .bearer(&developer.token)
        .send()
        .await
        .forbidden();

    env.backend
        .put(&path::user(&developer.user.id))
        .bearer(&env.admin)
        .json(&UserUpdate {
            role: Some(Role::Admin),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    env.backend
        .get(path::USERS)
        .bearer(&developer.token)
        .send()
        .await
        .ok();

    env.backend
        .put(&path::user(&developer.user.id))
        .bearer(&env.admin)
        .json(&UserUpdate {
            role: Some(Role::User),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    env.backend
        .get(path::USERS)
        .bearer(&developer.token)
        .send()
        .await
        .forbidden();
}

#[rstest]
#[tokio::test]
async fn a_new_password_replaces_the_old_one(env: &'static TestEnv) {
    let account = env.developer(&[]).await;
    let username = account.user.username.clone();

    env.backend
        .put(&path::user(&account.user.id))
        .bearer(&env.admin)
        .json(&UserUpdate {
            password: Some("a-brand-new-password".to_string()),
            ..Default::default()
        })
        .send()
        .await
        .ok();

    env.backend
        .login(&username, pipa_integration_test::env::ACCOUNT_PASSWORD)
        .await
        .unauthorized();
    env.backend
        .login(&username, "a-brand-new-password")
        .await
        .ok();
}

// ---- first admin and restarts ----

#[rstest]
#[tokio::test]
async fn accounts_survive_a_restart_and_the_bootstrap_admin_is_not_recreated(
    #[future] own_env: TestEnv,
) {
    let mut env = own_env.await;
    let account = env.developer(&[]).await;
    // Asking for another first admin on a store that has users changes nothing.
    env.backend.config.admin_password = Some("a-different-admin-password".to_string());
    env.backend.restart().await.unwrap();

    env.backend.login(ADMIN_USERNAME, ADMIN_PASSWORD).await.ok();
    env.backend
        .login(ADMIN_USERNAME, "a-different-admin-password")
        .await
        .unauthorized();
    env.backend
        .login(
            &account.user.username,
            pipa_integration_test::env::ACCOUNT_PASSWORD,
        )
        .await
        .ok();
    // A token from before the restart is still good: it only names the account.
    env.backend
        .get(path::ME)
        .bearer(&account.token)
        .send()
        .await
        .ok();
}

/// The backend refuses to start rather than run with no way in or with weak secrets.
#[rstest]
#[case::no_jwt_secret("JWT_SECRET", _, "admin", "admin-password")]
#[case::short_jwt_secret("JWT_SECRET", "too-short", "admin", "admin-password")]
#[case::no_admin_on_an_empty_store("no users exist", JWT_SECRET)]
#[case::no_admin_password("no users exist", JWT_SECRET, "admin")]
#[case::short_admin_password("password", JWT_SECRET, "admin", "short")]
#[case::empty_admin_username("username", JWT_SECRET, " ", "admin-password")]
#[tokio::test]
async fn the_backend_refuses_to_start_with_an_unsafe_configuration(
    #[case] reason: &str,
    #[case(opt)] jwt_secret: Option<&str>,
    #[case(opt)] admin_username: Option<&str>,
    #[case(opt)] admin_password: Option<&str>,
) {
    // `TestEnv` isn't used: it starts a healthy backend. A bucket of its own, so the store is
    // empty and there is no admin already.
    let bucket = new_bucket().await;
    let log = Backend::start_expecting_failure(BackendConfig {
        jwt_secret: jwt_secret.map(str::to_string),
        admin_username: admin_username.map(str::to_string),
        admin_password: admin_password.map(str::to_string),
        ..BackendConfig::new(&bucket)
    })
    .await
    .unwrap();
    assert!(
        log.to_lowercase().contains(&reason.to_lowercase()),
        "the log should mention {reason:?}:\n{log}"
    );
}

#[rstest]
#[tokio::test]
async fn the_backend_refuses_to_start_when_it_cannot_listen(env: &'static TestEnv) {
    // Its own address is taken by the running backend.
    let taken = env
        .backend
        .base_url
        .trim_start_matches("http://")
        .to_string();
    let log = Backend::start_expecting_failure(BackendConfig {
        listen: Some(taken),
        ..env.backend.config.clone()
    })
    .await
    .unwrap();
    assert!(log.contains("BACKEND_LISTEN"), "{log}");
}

// ---- CORS ----

#[rstest]
#[tokio::test]
async fn cors_allows_the_configured_origin_only(env: &'static TestEnv) {
    let preflight = |origin: &'static str| {
        env.backend
            .request(Method::OPTIONS, path::PROJECTS)
            .header("origin", origin)
            .header("access-control-request-method", "POST")
            .header(
                "access-control-request-headers",
                "authorization,content-type",
            )
    };

    let allowed = preflight("http://localhost:3000").raw().await;
    assert_eq!(
        allowed
            .headers
            .get("access-control-allow-origin")
            .map(String::as_str),
        Some("http://localhost:3000")
    );

    let other = preflight("http://evil.example").raw().await;
    assert_eq!(other.headers.get("access-control-allow-origin"), None);
}

#[tokio::test]
async fn cors_origins_can_be_configured() {
    // No `env` fixture: this test needs a backend of its own configuration.
    let env = TestEnv::with(|config| {
        config.cors_allowed_origins = Some("https://one.example, https://two.example".to_string());
    })
    .await;
    for (origin, allowed) in [
        ("https://one.example", true),
        ("https://two.example", true),
        ("http://localhost:3000", false),
    ] {
        let reply = env
            .backend
            .request(Method::OPTIONS, path::PROJECTS)
            .header("origin", origin)
            .header("access-control-request-method", "GET")
            .raw()
            .await;
        assert_eq!(
            reply.headers.contains_key("access-control-allow-origin"),
            allowed,
            "{origin}"
        );
    }
}

#[rstest]
#[tokio::test]
async fn a_viewer_cannot_create_accounts(env: &'static TestEnv) {
    let viewer = env.viewer(&[]).await;
    env.backend
        .post(path::USERS)
        .bearer(&viewer.token)
        .json(&NewUser {
            username: "sneaky".to_string(),
            password: "sneaky-password".to_string(),
            role: Role::Admin,
            project_ids: vec![],
        })
        .send()
        .await
        .forbidden();
    env.backend
        .login("sneaky", "sneaky-password")
        .await
        .unauthorized();
}
