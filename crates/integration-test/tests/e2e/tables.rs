//! `/tables`: browsing the ingested tables of a project without SQL, open to every role with
//! access to the project.

use pipa_api::{ReadTableRequest, Role, TableView, Tables, path};
use std::sync::OnceLock;

use pipa_integration_test::env::{TestEnv, unique};
use pipa_integration_test::pipeline::{ORDERS, Pipeline};
use pipa_integration_test::shared;
use rstest::{fixture, rstest};

use crate::common::{env, own_env};

const UNKNOWN_ID: &str = "0190a1b2-0000-7000-8000-000000000000";

/// A source with ten orders ingested, shared by the tests below: they only read it.
#[fixture]
fn pipeline() -> &'static Pipeline {
    static TEN_ORDERS: OnceLock<Result<Pipeline, String>> = OnceLock::new();
    shared::get_or_init(&TEN_ORDERS, async {
        let p = Pipeline::shared().await;
        p.db.execute("INSERT INTO orders SELECT g, 'new', g FROM generate_series(1, 10) AS g")
            .await;
        p.rows_when(ORDERS, "the ten orders", |rows| rows.len() == 10)
            .await;
        p
    })
}

/// A pipeline of its own, for the test that adds a lot of rows.
#[fixture]
async fn own_pipeline(#[future] own_env: TestEnv) -> Pipeline {
    let p = Pipeline::start(own_env.await).await;
    p.db.execute("INSERT INTO orders SELECT g, 'new', g FROM generate_series(1, 10) AS g")
        .await;
    p.rows_when(ORDERS, "the ten orders", |rows| rows.len() == 10)
        .await;
    p
}

async fn list(p: &Pipeline, token: &str, project_id: &str) -> pipa_integration_test::reply::Reply {
    p.env
        .backend
        .get(&format!("{}?project_id={project_id}", path::TABLES))
        .bearer(token)
        .send()
        .await
}

// ---- listing ----

#[rstest]
#[tokio::test]
async fn the_tables_of_a_project_are_listed_with_their_source(pipeline: &'static Pipeline) {
    let p = pipeline;
    let tables: Tables = list(p, &p.env.admin, &p.project.id).await.ok().parse();
    assert_eq!(
        tables.tables,
        vec![TableView {
            source_id: p.source.id.clone(),
            source_name: p.source.name.clone(),
            name: ORDERS.to_string(),
        }]
    );
}

#[rstest]
#[tokio::test]
async fn iceberg_metadata_tables_are_not_listed(pipeline: &'static Pipeline) {
    let p = pipeline;
    let tables: Tables = list(p, &p.env.admin, &p.project.id).await.ok().parse();
    assert!(
        tables.tables.iter().all(|table| !table.name.contains('$')),
        "{:?}",
        tables.tables
    );
}

#[rstest]
#[tokio::test]
async fn a_project_without_sources_has_no_tables(pipeline: &'static Pipeline) {
    let p = pipeline;
    let empty = p.env.unique_project().await;
    let tables: Tables = list(p, &p.env.admin, &empty.id).await.ok().parse();
    assert!(tables.tables.is_empty());
}

#[rstest]
#[tokio::test]
async fn a_project_lists_only_its_own_sources_tables(pipeline: &'static Pipeline) {
    let p = pipeline;
    let other = p.env.unique_project().await;
    let other_db = pipa_integration_test::postgres::Database::create().await;
    other_db
        .execute("CREATE TABLE secrets (id integer PRIMARY KEY)")
        .await;
    let other_source = p
        .env
        .source(Some(&other), &unique("other"), &other_db)
        .await;
    p.env.ingest(&other_source, &[("public", "secrets")]).await;
    pipa_integration_test::pipeline::wait_for_slot(&other_source).await;
    other_db.execute("INSERT INTO secrets VALUES (1)").await;
    p.env
        .wait_for_rows(
            &other,
            &other_source,
            "public__secrets",
            "the secret",
            |rows| rows.len() == 1,
        )
        .await;

    let mine: Tables = list(p, &p.env.admin, &p.project.id).await.ok().parse();
    assert!(
        mine.tables
            .iter()
            .all(|table| table.name != "public__secrets")
    );
    let theirs: Tables = list(p, &p.env.admin, &other.id).await.ok().parse();
    assert_eq!(theirs.tables.len(), 1);
    assert_eq!(theirs.tables[0].name, "public__secrets");
}

// ---- reading ----

#[rstest]
#[tokio::test]
async fn a_page_of_rows_is_read_in_key_order(pipeline: &'static Pipeline) {
    let p = pipeline;
    let page = |limit, offset| {
        p.env
            .table_rows(&p.env.admin, &p.project, &p.source, ORDERS, limit, offset)
    };
    let ids = |reply: pipa_integration_test::reply::Reply| -> Vec<i64> {
        reply
            .ok()
            .rows()
            .iter()
            .map(|row| row["id"].as_i64().unwrap())
            .collect()
    };

    assert_eq!(ids(page(Some(3), None).await), vec![1, 2, 3]);
    assert_eq!(ids(page(Some(3), Some(3)).await), vec![4, 5, 6]);
    assert_eq!(ids(page(Some(5), Some(8)).await), vec![9, 10]);
    assert!(ids(page(Some(5), Some(10)).await).is_empty());
    assert!(ids(page(Some(5), Some(1000)).await).is_empty());
    assert_eq!(ids(page(None, None).await).len(), 10);
}

#[rstest]
#[tokio::test]
async fn reads_are_capped_at_a_thousand_rows_and_default_to_a_hundred(
    #[future] own_pipeline: Pipeline,
) {
    let p = own_pipeline.await;
    p.db.execute("INSERT INTO orders SELECT g, 'bulk', g FROM generate_series(11, 1200) AS g")
        .await;
    p.rows_when(ORDERS, "1200 rows", |rows| rows.len() == 1000)
        .await;
    // `rows_when` asks for 1000; wait until the last row is in, then check the caps.
    pipa_integration_test::wait::eventually("all 1200 rows", || async {
        let reply = p
            .env
            .table_rows(
                &p.env.admin,
                &p.project,
                &p.source,
                ORDERS,
                Some(5),
                Some(1195),
            )
            .await;
        if reply.status == 200 && reply.rows().len() == 5 {
            Ok(())
        } else {
            Err(reply.body.to_string())
        }
    })
    .await;

    let default = p
        .env
        .table_rows(&p.env.admin, &p.project, &p.source, ORDERS, None, None)
        .await;
    assert_eq!(default.ok().rows().len(), 100);
    let capped = p
        .env
        .table_rows(
            &p.env.admin,
            &p.project,
            &p.source,
            ORDERS,
            Some(100_000),
            None,
        )
        .await;
    assert_eq!(capped.ok().rows().len(), 1000);
}

#[rstest]
#[case::admin(Role::Admin)]
#[case::developer(Role::Developer)]
#[case::viewer(Role::User)]
#[tokio::test]
async fn every_role_of_the_project_may_browse_its_tables(
    pipeline: &'static Pipeline,
    #[case] role: Role,
) {
    let p = pipeline;
    let account = p.env.account(&unique("member"), role, &[&p.project]).await;

    let tables: Tables = list(p, &account.token, &p.project.id).await.ok().parse();
    assert_eq!(tables.tables.len(), 1);
    let rows = p
        .env
        .table_rows(&account.token, &p.project, &p.source, ORDERS, Some(2), None)
        .await
        .ok();
    assert_eq!(rows.rows().len(), 2);
}

#[rstest]
#[case::developer(Role::Developer)]
#[case::viewer(Role::User)]
#[tokio::test]
async fn an_outsider_cannot_browse_a_project_it_is_not_in(
    pipeline: &'static Pipeline,
    #[case] role: Role,
) {
    let p = pipeline;
    let elsewhere = p.env.unique_project().await;
    let outsider = p
        .env
        .account(&unique("outsider"), role, &[&elsewhere])
        .await;

    list(p, &outsider.token, &p.project.id).await.forbidden();
    p.env
        .table_rows(&outsider.token, &p.project, &p.source, ORDERS, None, None)
        .await
        .forbidden();

    // Naming its own project with the other project's source doesn't help either.
    p.env
        .table_rows(&outsider.token, &elsewhere, &p.source, ORDERS, None, None)
        .await
        .bad_request();
}

#[rstest]
#[tokio::test]
async fn a_member_loses_the_tables_when_unassigned(pipeline: &'static Pipeline) {
    let p = pipeline;
    let viewer = p.env.viewer(&[&p.project]).await;
    list(p, &viewer.token, &p.project.id).await.ok();

    p.env
        .backend
        .put(&path::user(&viewer.user.id))
        .bearer(&p.env.admin)
        .json(&pipa_api::UserUpdate {
            project_ids: Some(vec![]),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    list(p, &viewer.token, &p.project.id).await.forbidden();
}

// ---- bad requests ----

#[rstest]
#[tokio::test]
async fn listing_needs_a_valid_project_id(pipeline: &'static Pipeline) {
    let p = pipeline;
    let missing = p
        .env
        .backend
        .get(path::TABLES)
        .bearer(&p.env.admin)
        .send()
        .await;
    assert_eq!(missing.status, 400, "{}", missing.body);
    let malformed = list(p, &p.env.admin, "not-a-uuid").await;
    assert_eq!(malformed.status, 400, "{}", malformed.body);
}

#[rstest]
#[tokio::test]
async fn listing_an_unknown_project_is_empty_for_an_admin_and_forbidden_for_others(
    pipeline: &'static Pipeline,
) {
    let p = pipeline;
    let tables: Tables = list(p, &p.env.admin, UNKNOWN_ID).await.ok().parse();
    assert!(tables.tables.is_empty());
    let developer = p.env.developer(&[&p.project]).await;
    list(p, &developer.token, UNKNOWN_ID).await.forbidden();
}

#[rstest]
#[case::project("not-a-uuid", "source")]
#[case::source("project", "not-a-uuid")]
#[tokio::test]
async fn reading_needs_valid_ids(
    pipeline: &'static Pipeline,
    #[case] project: &str,
    #[case] source: &str,
) {
    let p = pipeline;
    let pick = |value: &str, real: &str| {
        if value == "project" || value == "source" {
            real.to_string()
        } else {
            value.to_string()
        }
    };
    let reply = p
        .env
        .backend
        .post(path::TABLE_ROWS)
        .bearer(&p.env.admin)
        .json(&ReadTableRequest {
            project_id: pick(project, &p.project.id),
            source_id: pick(source, &p.source.id),
            table: ORDERS.to_string(),
            limit: None,
            offset: None,
        })
        .send()
        .await
        .bad_request();
    assert!(reply.error().contains("UUID"), "{}", reply.error());
}

#[rstest]
#[tokio::test]
async fn reading_a_source_that_is_not_the_projects_is_a_bad_request(pipeline: &'static Pipeline) {
    let p = pipeline;
    let reply = p
        .env
        .backend
        .post(path::TABLE_ROWS)
        .bearer(&p.env.admin)
        .json(&ReadTableRequest {
            project_id: p.project.id.clone(),
            source_id: UNKNOWN_ID.to_string(),
            table: ORDERS.to_string(),
            limit: None,
            offset: None,
        })
        .send()
        .await
        .bad_request();
    assert!(reply.error().contains("source_id"), "{}", reply.error());
}

#[rstest]
#[case::unknown_table("public__nope")]
#[case::empty_name("")]
#[case::other_namespace("other_ns.public__orders")]
#[case::sql_in_the_name("public__orders; DROP TABLE x")]
#[tokio::test]
async fn reading_a_table_that_does_not_exist_is_a_bad_request(
    pipeline: &'static Pipeline,
    #[case] table: &str,
) {
    let p = pipeline;
    let reply = p
        .env
        .table_rows(&p.env.admin, &p.project, &p.source, table, None, None)
        .await;
    assert_eq!(reply.status, 400, "{}", reply.body);
}

#[rstest]
#[case::snapshots("public__orders$snapshots")]
#[case::manifests("public__orders$manifests")]
#[tokio::test]
async fn iceberg_metadata_tables_cannot_be_browsed(
    pipeline: &'static Pipeline,
    #[case] table: &str,
) {
    let p = pipeline;
    let reply = p
        .env
        .table_rows(&p.env.admin, &p.project, &p.source, table, None, None)
        .await
        .bad_request();
    assert!(reply.error().contains("metadata"), "{}", reply.error());
}

#[rstest]
#[case::no_table(serde_json::json!({"project_id": "p", "source_id": "s"}))]
#[case::limit_of_the_wrong_type(serde_json::json!({"project_id": "p", "source_id": "s", "table": "t", "limit": "ten"}))]
#[case::negative_limit(serde_json::json!({"project_id": "p", "source_id": "s", "table": "t", "limit": -1}))]
#[case::negative_offset(serde_json::json!({"project_id": "p", "source_id": "s", "table": "t", "offset": -1}))]
#[tokio::test]
async fn a_malformed_read_is_rejected(env: &'static TestEnv, #[case] body: serde_json::Value) {
    let reply = env
        .backend
        .post(path::TABLE_ROWS)
        .bearer(&env.admin)
        .json(&body)
        .send()
        .await;
    assert_eq!(reply.status, 422, "{}", reply.body);
}
