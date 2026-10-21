// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! `/projects`: admins manage them, everyone sees the ones they are assigned.

use pipa_api::{NewProject, ProjectData, ProjectUpdate, Projects, Role, path};
use pipa_integration_test::env::{TestEnv, unique};
use rstest::rstest;
use serde_json::json;

use crate::common::{env, own_env};

const UNKNOWN_ID: &str = "0190a1b2-0000-7000-8000-000000000000";

fn new_project(name: &str) -> NewProject {
    NewProject {
        name: name.to_string(),
        description: None,
    }
}

// ---- positive ----

#[rstest]
#[tokio::test]
async fn an_admin_creates_a_project_with_or_without_a_description(env: &'static TestEnv) {
    let bare: ProjectData = env
        .backend
        .post(path::PROJECTS)
        .bearer(&env.admin)
        .json(&new_project(&unique("bare")))
        .send()
        .await
        .created()
        .parse();
    assert_eq!(bare.project.description, None);
    assert!(bare.project.created_at_unix > 0);

    let described: ProjectData = env
        .backend
        .post(path::PROJECTS)
        .bearer(&env.admin)
        .json(&NewProject {
            name: unique("described"),
            description: Some("Orders and payments".to_string()),
        })
        .send()
        .await
        .created()
        .parse();
    assert_eq!(
        described.project.description.as_deref(),
        Some("Orders and payments")
    );
    assert_ne!(bare.project.id, described.project.id);
}

#[rstest]
#[tokio::test]
async fn a_project_is_fetched_by_id_and_listed(env: &'static TestEnv) {
    let project = env.unique_project().await;

    let fetched: ProjectData = env
        .backend
        .get(&path::project(&project.id))
        .bearer(&env.admin)
        .send()
        .await
        .ok()
        .parse();
    assert_eq!(fetched.project.name, project.name);

    let listed: Projects = env
        .backend
        .get(path::PROJECTS)
        .bearer(&env.admin)
        .send()
        .await
        .ok()
        .parse();
    assert!(
        listed
            .projects
            .iter()
            .any(|candidate| candidate.id == project.id)
    );
}

#[rstest]
#[tokio::test]
async fn an_update_replaces_the_name_and_description(env: &'static TestEnv) {
    let project = env.unique_project().await;
    let renamed = unique("renamed");

    let updated: ProjectData = env
        .backend
        .put(&path::project(&project.id))
        .bearer(&env.admin)
        .json(&ProjectUpdate {
            name: renamed.clone(),
            description: Some("now described".to_string()),
        })
        .send()
        .await
        .ok()
        .parse();
    assert_eq!(updated.project.id, project.id);
    assert_eq!(updated.project.name, renamed);
    assert_eq!(updated.project.created_at_unix, project.created_at_unix);

    // And the description can be cleared again.
    let cleared: ProjectData = env
        .backend
        .put(&path::project(&project.id))
        .bearer(&env.admin)
        .json(&ProjectUpdate {
            name: renamed,
            description: None,
        })
        .send()
        .await
        .ok()
        .parse();
    assert_eq!(cleared.project.description, None);
}

#[rstest]
#[tokio::test]
async fn a_project_can_keep_its_own_name_on_update(env: &'static TestEnv) {
    let project = env.unique_project().await;
    env.backend
        .put(&path::project(&project.id))
        .bearer(&env.admin)
        .json(&ProjectUpdate {
            name: project.name.clone(),
            description: Some("only the description changes".to_string()),
        })
        .send()
        .await
        .ok();
}

#[rstest]
#[tokio::test]
async fn a_deleted_project_is_gone(#[future] own_env: TestEnv) {
    let env = own_env.await;
    let project = env.unique_project().await;
    env.backend
        .delete(&path::project(&project.id))
        .bearer(&env.admin)
        .send()
        .await
        .deleted();

    env.backend
        .get(&path::project(&project.id))
        .bearer(&env.admin)
        .send()
        .await
        .not_found();
    let listed: Projects = env
        .backend
        .get(path::PROJECTS)
        .bearer(&env.admin)
        .send()
        .await
        .ok()
        .parse();
    assert!(
        !listed
            .projects
            .iter()
            .any(|candidate| candidate.id == project.id)
    );

    // Its name is free again.
    env.project(&project.name).await;
}

// ---- negative: validation ----

#[rstest]
#[tokio::test]
async fn a_duplicate_project_name_is_a_conflict_whatever_its_case(env: &'static TestEnv) {
    let project = env.unique_project().await;
    for name in [
        project.name.clone(),
        project.name.to_uppercase(),
        format!("  {}  ", project.name),
    ] {
        env.backend
            .post(path::PROJECTS)
            .bearer(&env.admin)
            .json(&new_project(&name))
            .send()
            .await
            .conflict();
    }
}

#[rstest]
#[tokio::test]
async fn renaming_to_another_projects_name_is_a_conflict(env: &'static TestEnv) {
    let first = env.unique_project().await;
    let second = env.unique_project().await;
    env.backend
        .put(&path::project(&second.id))
        .bearer(&env.admin)
        .json(&ProjectUpdate {
            name: first.name.to_uppercase(),
            description: None,
        })
        .send()
        .await
        .conflict();

    // The second project is untouched.
    let still: ProjectData = env
        .backend
        .get(&path::project(&second.id))
        .bearer(&env.admin)
        .send()
        .await
        .ok()
        .parse();
    assert_eq!(still.project.name, second.name);
}

#[rstest]
#[case::empty("")]
#[case::blank("   ")]
#[tokio::test]
async fn a_project_needs_a_name(env: &'static TestEnv, #[case] name: &str) {
    env.backend
        .post(path::PROJECTS)
        .bearer(&env.admin)
        .json(&new_project(name))
        .send()
        .await
        .bad_request();

    let project = env.unique_project().await;
    env.backend
        .put(&path::project(&project.id))
        .bearer(&env.admin)
        .json(&ProjectUpdate {
            name: name.to_string(),
            description: None,
        })
        .send()
        .await
        .bad_request();
}

#[rstest]
#[case::no_name(json!({"description": "x"}))]
#[case::name_of_the_wrong_type(json!({"name": 7}))]
#[case::description_of_the_wrong_type(json!({"name": "typed", "description": ["x"]}))]
#[tokio::test]
async fn a_malformed_project_is_rejected(env: &'static TestEnv, #[case] body: serde_json::Value) {
    let reply = env
        .backend
        .post(path::PROJECTS)
        .bearer(&env.admin)
        .json(&body)
        .send()
        .await;
    assert_eq!(reply.status, 422, "{}", reply.body);
}

// ---- negative: unknown and malformed ids ----

#[rstest]
#[case::get("GET")]
#[case::update("PUT")]
#[case::delete("DELETE")]
#[tokio::test]
async fn an_unknown_project_is_not_found(env: &'static TestEnv, #[case] method: &str) {
    env.backend
        .request(method.parse().unwrap(), &path::project(UNKNOWN_ID))
        .bearer(&env.admin)
        .json(&json!({"name": unique("ghost")}))
        .send()
        .await
        .not_found();
}

#[rstest]
#[case::get("GET")]
#[case::update("PUT")]
#[case::delete("DELETE")]
#[tokio::test]
async fn a_project_id_that_is_not_a_uuid_is_rejected(env: &'static TestEnv, #[case] method: &str) {
    let reply = env
        .backend
        .request(method.parse().unwrap(), &path::project("not-a-uuid"))
        .bearer(&env.admin)
        .raw_json("{}")
        .send()
        .await;
    assert_eq!(reply.status, 400, "{}", reply.body);
}

// ---- who sees what ----

#[rstest]
#[case::developer(Role::Developer)]
#[case::user(Role::User)]
#[tokio::test]
async fn a_non_admin_sees_only_its_assigned_projects(env: &'static TestEnv, #[case] role: Role) {
    let mine = env.unique_project().await;
    let other = env.unique_project().await;
    let account = env.account(&unique("member"), role, &[&mine]).await;

    let listed: Projects = env
        .backend
        .get(path::PROJECTS)
        .bearer(&account.token)
        .send()
        .await
        .ok()
        .parse();
    let ids: Vec<_> = listed
        .projects
        .iter()
        .map(|project| project.id.as_str())
        .collect();
    assert_eq!(ids, vec![mine.id.as_str()]);

    env.backend
        .get(&path::project(&mine.id))
        .bearer(&account.token)
        .send()
        .await
        .ok();
    env.backend
        .get(&path::project(&other.id))
        .bearer(&account.token)
        .send()
        .await
        .forbidden();
}

#[rstest]
#[tokio::test]
async fn an_account_without_projects_sees_none(env: &'static TestEnv) {
    env.unique_project().await;
    let account = env.developer(&[]).await;
    let listed: Projects = env
        .backend
        .get(path::PROJECTS)
        .bearer(&account.token)
        .send()
        .await
        .ok()
        .parse();
    assert!(listed.projects.is_empty());
}

#[rstest]
#[tokio::test]
async fn an_unknown_project_is_forbidden_not_not_found_to_a_non_admin(env: &'static TestEnv) {
    let account = env.developer(&[]).await;
    // A non-admin cannot tell which ids exist.
    env.backend
        .get(&path::project(UNKNOWN_ID))
        .bearer(&account.token)
        .send()
        .await
        .forbidden();
}

#[rstest]
#[case::developer(Role::Developer)]
#[case::user(Role::User)]
#[tokio::test]
async fn only_an_admin_creates_changes_or_deletes_projects(
    env: &'static TestEnv,
    #[case] role: Role,
) {
    let project = env.unique_project().await;
    let account = env.account(&unique("member"), role, &[&project]).await;
    let token = &account.token;

    env.backend
        .post(path::PROJECTS)
        .bearer(token)
        .json(&new_project(&unique("by-member")))
        .send()
        .await
        .forbidden();
    env.backend
        .put(&path::project(&project.id))
        .bearer(token)
        .json(&ProjectUpdate {
            name: unique("renamed-by-member"),
            description: None,
        })
        .send()
        .await
        .forbidden();
    env.backend
        .delete(&path::project(&project.id))
        .bearer(token)
        .send()
        .await
        .forbidden();

    let intact: ProjectData = env
        .backend
        .get(&path::project(&project.id))
        .bearer(&env.admin)
        .send()
        .await
        .ok()
        .parse();
    assert_eq!(intact.project.name, project.name);
}

#[rstest]
#[tokio::test]
async fn assigning_a_project_gives_access_at_once_and_unassigning_takes_it_away(
    env: &'static TestEnv,
) {
    let project = env.unique_project().await;
    let account = env.developer(&[]).await;
    env.backend
        .get(&path::project(&project.id))
        .bearer(&account.token)
        .send()
        .await
        .forbidden();

    let assign = |project_ids: Vec<String>| {
        env.backend
            .put(&path::user(&account.user.id))
            .bearer(&env.admin)
            .json(&pipa_api::UserUpdate {
                project_ids: Some(project_ids),
                ..Default::default()
            })
            .send()
    };
    assign(vec![project.id.clone()]).await.ok();
    env.backend
        .get(&path::project(&project.id))
        .bearer(&account.token)
        .send()
        .await
        .ok();

    assign(vec![]).await.ok();
    env.backend
        .get(&path::project(&project.id))
        .bearer(&account.token)
        .send()
        .await
        .forbidden();
}

#[rstest]
#[tokio::test]
async fn a_deleted_project_leaves_its_members_with_nothing_to_see(#[future] own_env: TestEnv) {
    let env = own_env.await;
    let project = env.unique_project().await;
    let account = env.developer(&[&project]).await;
    env.backend
        .delete(&path::project(&project.id))
        .bearer(&env.admin)
        .send()
        .await
        .deleted();

    let listed: Projects = env
        .backend
        .get(path::PROJECTS)
        .bearer(&account.token)
        .send()
        .await
        .ok()
        .parse();
    assert!(listed.projects.is_empty());
    env.backend
        .get(&path::project(&project.id))
        .bearer(&account.token)
        .send()
        .await
        .not_found();
    // The account itself is fine; the stale id is harmless.
    env.backend
        .get(path::ME)
        .bearer(&account.token)
        .send()
        .await
        .ok();
}

#[rstest]
#[tokio::test]
async fn deleting_a_project_twice_is_not_found_the_second_time(env: &'static TestEnv) {
    let project = env.unique_project().await;
    let delete = || {
        env.backend
            .delete(&path::project(&project.id))
            .bearer(&env.admin)
            .send()
    };
    delete().await.deleted();
    delete().await.not_found();
}

#[rstest]
#[tokio::test]
async fn updating_an_unknown_project_is_not_found_even_with_a_taken_name(env: &'static TestEnv) {
    let taken = env.unique_project().await;
    env.backend
        .put(&path::project(UNKNOWN_ID))
        .bearer(&env.admin)
        .json(&ProjectUpdate {
            name: taken.name,
            description: None,
        })
        .send()
        .await
        .not_found();
}

/// Listing reads every project one by one; a project deleted meanwhile must not fail the list.
#[rstest]
#[tokio::test]
async fn listing_projects_while_others_are_deleted_never_fails(env: &'static TestEnv) {
    let mut projects = Vec::new();
    for _ in 0..30 {
        projects.push(env.unique_project().await);
    }

    let deleters: Vec<_> = projects
        .into_iter()
        .map(|project| {
            tokio::spawn(async move {
                env.backend
                    .delete(&path::project(&project.id))
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
                .get(path::PROJECTS)
                .bearer(&env.admin)
                .send()
                .await
                .ok();
        }
    });
    for deleter in deleters {
        deleter.await.unwrap();
    }
    lister.await.unwrap();
}
