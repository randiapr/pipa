// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! `/users`: admin-only account management.

use pipa_api::{NewUser, Role, UserData, UserResponse, UserUpdate, Users, path};
use pipa_integration_test::backend::{ADMIN_PASSWORD, ADMIN_USERNAME};
use pipa_integration_test::env::{ACCOUNT_PASSWORD, TestEnv, unique};
use rstest::rstest;
use serde_json::json;

use crate::common::{env, own_env};

fn new_user(username: &str, role: Role) -> NewUser {
    NewUser {
        username: username.to_string(),
        password: ACCOUNT_PASSWORD.to_string(),
        role,
        project_ids: vec![],
    }
}

// ---- positive ----

#[rstest]
#[case::admin(Role::Admin)]
#[case::developer(Role::Developer)]
#[case::user(Role::User)]
#[tokio::test]
async fn an_admin_creates_an_account_of_any_role(env: &'static TestEnv, #[case] role: Role) {
    let project = env.unique_project().await;
    let username = unique("new");
    let created: UserData = env
        .backend
        .post(path::USERS)
        .bearer(&env.admin)
        .json(&NewUser {
            project_ids: vec![project.id.clone()],
            ..new_user(&username, role)
        })
        .send()
        .await
        .created()
        .parse();
    assert_eq!(created.user.username, username);
    assert_eq!(created.user.role, role);
    assert_eq!(created.user.project_ids, vec![project.id]);

    // And the account can sign in.
    env.backend.login(&username, ACCOUNT_PASSWORD).await.ok();
}

#[rstest]
#[tokio::test]
async fn a_username_is_trimmed(env: &'static TestEnv) {
    let username = unique("padded");
    let created: UserData = env
        .backend
        .post(path::USERS)
        .bearer(&env.admin)
        .json(&new_user(&format!("  {username}  "), Role::User))
        .send()
        .await
        .created()
        .parse();
    assert_eq!(created.user.username, username);
}

#[rstest]
#[tokio::test]
async fn accounts_are_listed_and_fetched_by_id(env: &'static TestEnv) {
    let account = env.viewer(&[]).await;

    let listed: Users = env
        .backend
        .get(path::USERS)
        .bearer(&env.admin)
        .send()
        .await
        .ok()
        .parse();
    let usernames: Vec<_> = listed
        .users
        .iter()
        .map(|user| user.username.as_str())
        .collect();
    assert!(usernames.contains(&ADMIN_USERNAME), "{usernames:?}");
    assert!(
        usernames.contains(&account.user.username.as_str()),
        "{usernames:?}"
    );

    let fetched: UserData = env
        .backend
        .get(&path::user(&account.user.id))
        .bearer(&env.admin)
        .send()
        .await
        .ok()
        .parse();
    assert_eq!(fetched.user.id, account.user.id);
}

#[rstest]
#[tokio::test]
async fn an_update_changes_only_the_fields_it_names(env: &'static TestEnv) {
    let first = env.unique_project().await;
    let second = env.unique_project().await;
    let account = env.developer(&[&first]).await;

    // Role only: projects stay.
    let updated: UserData = env
        .backend
        .put(&path::user(&account.user.id))
        .bearer(&env.admin)
        .json(&UserUpdate {
            role: Some(Role::User),
            ..Default::default()
        })
        .send()
        .await
        .ok()
        .parse();
    assert_eq!(updated.user.role, Role::User);
    assert_eq!(updated.user.project_ids, vec![first.id.clone()]);

    // Projects only: role stays. They are replaced, not added to.
    let updated: UserData = env
        .backend
        .put(&path::user(&account.user.id))
        .bearer(&env.admin)
        .json(&UserUpdate {
            project_ids: Some(vec![second.id.clone()]),
            ..Default::default()
        })
        .send()
        .await
        .ok()
        .parse();
    assert_eq!(updated.user.role, Role::User);
    assert_eq!(updated.user.project_ids, vec![second.id.clone()]);

    // An empty update changes nothing.
    let unchanged: UserData = env
        .backend
        .put(&path::user(&account.user.id))
        .bearer(&env.admin)
        .json(&json!({}))
        .send()
        .await
        .ok()
        .parse();
    assert_eq!(unchanged.user.role, Role::User);
    assert_eq!(unchanged.user.project_ids, vec![second.id]);
}

#[rstest]
#[tokio::test]
async fn a_deleted_account_is_gone_and_cannot_sign_in(#[future] own_env: TestEnv) {
    let env = own_env.await;
    let account = env.viewer(&[]).await;
    env.backend
        .delete(&path::user(&account.user.id))
        .bearer(&env.admin)
        .send()
        .await
        .deleted();

    env.backend
        .get(&path::user(&account.user.id))
        .bearer(&env.admin)
        .send()
        .await
        .not_found();
    env.backend
        .login(&account.user.username, ACCOUNT_PASSWORD)
        .await
        .unauthorized();
}

#[rstest]
#[tokio::test]
async fn an_admin_can_be_demoted_or_deleted_while_another_admin_remains(
    #[future] own_env: TestEnv,
) {
    let env = own_env.await;
    let second = env.account(&unique("admin2"), Role::Admin, &[]).await;

    env.backend
        .put(&path::user(&second.user.id))
        .bearer(&env.admin)
        .json(&UserUpdate {
            role: Some(Role::Developer),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    env.backend
        .put(&path::user(&second.user.id))
        .bearer(&env.admin)
        .json(&UserUpdate {
            role: Some(Role::Admin),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    env.backend
        .delete(&path::user(&second.user.id))
        .bearer(&env.admin)
        .send()
        .await
        .deleted();
}

// ---- negative: validation and conflicts ----

#[rstest]
#[tokio::test]
async fn a_duplicate_username_is_a_conflict_whatever_its_case(env: &'static TestEnv) {
    let account = env.viewer(&[]).await;
    for username in [
        account.user.username.clone(),
        account.user.username.to_uppercase(),
        format!(" {} ", account.user.username),
    ] {
        env.backend
            .post(path::USERS)
            .bearer(&env.admin)
            .json(&new_user(&username, Role::User))
            .send()
            .await
            .conflict();
    }
}

#[rstest]
#[case::empty("")]
#[case::blank("   ")]
#[tokio::test]
async fn an_account_needs_a_username(env: &'static TestEnv, #[case] username: &str) {
    env.backend
        .post(path::USERS)
        .bearer(&env.admin)
        .json(&new_user(username, Role::User))
        .send()
        .await
        .bad_request();
}

#[rstest]
#[case::empty("")]
#[case::seven_characters("1234567")]
#[tokio::test]
async fn a_short_password_is_rejected(env: &'static TestEnv, #[case] password: &str) {
    let username = unique("weak");
    let reply = env
        .backend
        .post(path::USERS)
        .bearer(&env.admin)
        .json(&NewUser {
            password: password.to_string(),
            ..new_user(&username, Role::User)
        })
        .send()
        .await
        .bad_request();
    assert!(reply.error().contains("password"), "{}", reply.error());
    env.backend.login(&username, password).await.unauthorized();
}

#[rstest]
#[tokio::test]
async fn an_eight_character_password_is_enough(env: &'static TestEnv) {
    let username = unique("eight");
    env.backend
        .post(path::USERS)
        .bearer(&env.admin)
        .json(&NewUser {
            password: "12345678".to_string(),
            ..new_user(&username, Role::User)
        })
        .send()
        .await
        .created();
}

#[rstest]
#[tokio::test]
async fn updating_to_a_short_password_is_rejected_and_keeps_the_old_one(env: &'static TestEnv) {
    let account = env.viewer(&[]).await;
    env.backend
        .put(&path::user(&account.user.id))
        .bearer(&env.admin)
        .json(&UserUpdate {
            password: Some("short".to_string()),
            ..Default::default()
        })
        .send()
        .await
        .bad_request();
    env.backend
        .login(&account.user.username, ACCOUNT_PASSWORD)
        .await
        .ok();
}

#[rstest]
#[case::unknown_role(json!({"username": "x1", "password": "long-enough-1", "role": "superuser"}))]
#[case::no_role(json!({"username": "x2", "password": "long-enough-1"}))]
#[case::no_password(json!({"username": "x3", "role": "user"}))]
#[case::no_username(json!({"password": "long-enough-1", "role": "user"}))]
#[case::project_ids_of_the_wrong_type(json!({"username": "x4", "password": "long-enough-1", "role": "user", "project_ids": "p"}))]
#[tokio::test]
async fn a_malformed_account_is_rejected(env: &'static TestEnv, #[case] body: serde_json::Value) {
    let reply = env
        .backend
        .post(path::USERS)
        .bearer(&env.admin)
        .json(&body)
        .send()
        .await;
    assert_eq!(reply.status, 422, "{}", reply.body);
}

#[rstest]
#[tokio::test]
async fn a_project_id_that_is_not_a_uuid_is_rejected(env: &'static TestEnv) {
    let reply = env
        .backend
        .post(path::USERS)
        .bearer(&env.admin)
        .json(&NewUser {
            project_ids: vec!["not-a-uuid".to_string()],
            ..new_user(&unique("bad"), Role::User)
        })
        .send()
        .await;
    assert!(matches!(reply.status, 400 | 422), "{}", reply.body);
}

#[rstest]
#[tokio::test]
async fn renaming_is_not_possible_through_an_update(env: &'static TestEnv) {
    let account = env.viewer(&[]).await;
    // `username` isn't an updatable field: it is ignored, not applied.
    let updated: UserData = env
        .backend
        .put(&path::user(&account.user.id))
        .bearer(&env.admin)
        .json(&json!({"username": "someone-else"}))
        .send()
        .await
        .ok()
        .parse();
    assert_eq!(updated.user.username, account.user.username);
}

// ---- negative: unknown and malformed ids ----

#[rstest]
#[case::get("GET")]
#[case::update("PUT")]
#[case::delete("DELETE")]
#[tokio::test]
async fn an_unknown_account_is_not_found(env: &'static TestEnv, #[case] method: &str) {
    let unknown = path::user("0190a1b2-0000-7000-8000-000000000000");
    let request = env
        .backend
        .request(method.parse().unwrap(), &unknown)
        .bearer(&env.admin);
    let request = if method == "PUT" {
        request.json(&json!({"role": "user"}))
    } else {
        request
    };
    request.send().await.not_found();
}

#[rstest]
#[case::get("GET")]
#[case::update("PUT")]
#[case::delete("DELETE")]
#[tokio::test]
async fn an_account_id_that_is_not_a_uuid_is_rejected(env: &'static TestEnv, #[case] method: &str) {
    let request = env
        .backend
        .request(method.parse().unwrap(), &path::user("not-a-uuid"))
        .bearer(&env.admin)
        .raw_json("{}");
    let reply = request.send().await;
    assert_eq!(reply.status, 400, "{}", reply.body);
}

// ---- negative: the last admin ----

#[rstest]
#[tokio::test]
async fn the_last_admin_cannot_be_deleted(#[future] own_env: TestEnv) {
    let env = own_env.await;
    let me: pipa_api::MeData = env
        .backend
        .get(path::ME)
        .bearer(&env.admin)
        .send()
        .await
        .ok()
        .parse();
    let reply = env
        .backend
        .delete(&path::user(&me.user.id))
        .bearer(&env.admin)
        .send()
        .await
        .conflict();
    assert!(
        reply.error().to_lowercase().contains("admin"),
        "{}",
        reply.error()
    );
    // Still there, still working.
    env.backend.login(ADMIN_USERNAME, ADMIN_PASSWORD).await.ok();
}

#[rstest]
#[tokio::test]
async fn the_last_admin_cannot_be_demoted(#[future] own_env: TestEnv) {
    let env = own_env.await;
    let me: pipa_api::MeData = env
        .backend
        .get(path::ME)
        .bearer(&env.admin)
        .send()
        .await
        .ok()
        .parse();
    for role in [Role::Developer, Role::User] {
        env.backend
            .put(&path::user(&me.user.id))
            .bearer(&env.admin)
            .json(&UserUpdate {
                role: Some(role),
                ..Default::default()
            })
            .send()
            .await
            .conflict();
    }
    env.backend
        .get(path::USERS)
        .bearer(&env.admin)
        .send()
        .await
        .ok();
}

#[rstest]
#[tokio::test]
async fn the_last_admin_can_be_updated_in_other_ways(#[future] own_env: TestEnv) {
    let env = own_env.await;
    let me: pipa_api::MeData = env
        .backend
        .get(path::ME)
        .bearer(&env.admin)
        .send()
        .await
        .ok()
        .parse();
    // Staying an admin is not a demotion.
    let reply = env
        .backend
        .put(&path::user(&me.user.id))
        .bearer(&env.admin)
        .json(&UserUpdate {
            role: Some(Role::Admin),
            password: Some("admin-new-password".to_string()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let _: UserResponse = reply.parse();
    env.backend
        .login(ADMIN_USERNAME, "admin-new-password")
        .await
        .ok();
}

// ---- negative: authorization ----

#[rstest]
#[case::developer(Role::Developer)]
#[case::user(Role::User)]
#[tokio::test]
async fn only_an_admin_manages_accounts(env: &'static TestEnv, #[case] role: Role) {
    let caller = env.account(&unique("caller"), role, &[]).await;
    let target = env.viewer(&[]).await;
    let token = &caller.token;

    env.backend
        .get(path::USERS)
        .bearer(token)
        .send()
        .await
        .forbidden();
    env.backend
        .post(path::USERS)
        .bearer(token)
        .json(&new_user(&unique("by-caller"), Role::Admin))
        .send()
        .await
        .forbidden();
    env.backend
        .get(&path::user(&target.user.id))
        .bearer(token)
        .send()
        .await
        .forbidden();
    env.backend
        .put(&path::user(&target.user.id))
        .bearer(token)
        .json(&UserUpdate {
            role: Some(Role::Admin),
            ..Default::default()
        })
        .send()
        .await
        .forbidden();
    env.backend
        .delete(&path::user(&target.user.id))
        .bearer(token)
        .send()
        .await
        .forbidden();

    // Nothing of that happened.
    let still: UserData = env
        .backend
        .get(&path::user(&target.user.id))
        .bearer(&env.admin)
        .send()
        .await
        .ok()
        .parse();
    assert_eq!(still.user.role, Role::User);
}

#[rstest]
#[case::developer(Role::Developer)]
#[case::user(Role::User)]
#[tokio::test]
async fn an_account_cannot_promote_itself(env: &'static TestEnv, #[case] role: Role) {
    let caller = env.account(&unique("climber"), role, &[]).await;
    env.backend
        .put(&path::user(&caller.user.id))
        .bearer(&caller.token)
        .json(&UserUpdate {
            role: Some(Role::Admin),
            ..Default::default()
        })
        .send()
        .await
        .forbidden();
    env.backend
        .get(path::USERS)
        .bearer(&caller.token)
        .send()
        .await
        .forbidden();
}

/// Signing in and creating accounts read every account; one deleted meanwhile must not fail them.
#[rstest]
#[tokio::test]
async fn listing_accounts_while_others_are_deleted_never_fails(env: &'static TestEnv) {
    let mut accounts = Vec::new();
    for _ in 0..12 {
        accounts.push(env.viewer(&[]).await);
    }

    let deleters: Vec<_> = accounts
        .into_iter()
        .map(|account| {
            tokio::spawn(async move {
                env.backend
                    .delete(&path::user(&account.user.id))
                    .bearer(&env.admin)
                    .send()
                    .await
                    .deleted();
            })
        })
        .collect();
    let lister = tokio::spawn(async move {
        for _ in 0..40 {
            env.backend
                .get(path::USERS)
                .bearer(&env.admin)
                .send()
                .await
                .ok();
            env.backend.login(ADMIN_USERNAME, ADMIN_PASSWORD).await.ok();
        }
    });
    for deleter in deleters {
        deleter.await.unwrap();
    }
    lister.await.unwrap();
}
