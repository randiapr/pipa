//! `POST /query`: free SQL over the Iceberg tables, for developers and admins, scoped to a
//! project.

use pipa_api::{QueryRequest, Role, path};
use std::sync::OnceLock;

use pipa_integration_test::env::{TestEnv, namespace};
use pipa_integration_test::pipeline::{ORDERS, Pipeline};
use pipa_integration_test::reply::Reply;
use pipa_integration_test::shared;
use rstest::{fixture, rstest};
use serde_json::json;

use crate::common::env;

/// A source with three orders ingested, shared by the tests below: they only read it.
#[fixture]
fn pipeline() -> &'static Pipeline {
    static THREE_ORDERS: OnceLock<Result<Pipeline, String>> = OnceLock::new();
    shared::get_or_init(&THREE_ORDERS, async {
        let p = Pipeline::shared().await;
        p.db.execute("INSERT INTO orders VALUES (1, 'new', 10), (2, 'paid', 20), (3, 'paid', 30)")
            .await;
        p.rows_when(ORDERS, "the three orders", |rows| rows.len() == 3)
            .await;
        p
    })
}

async fn query(p: &Pipeline, token: &str, project: Option<&str>, sql: &str) -> Reply {
    p.env
        .backend
        .post(path::QUERY)
        .bearer(token)
        .json(&QueryRequest {
            sql: sql.to_string(),
            project_id: project.map(str::to_string),
        })
        .send()
        .await
}

fn orders(p: &Pipeline) -> String {
    format!("pipa.{}.{ORDERS}", namespace(&p.source))
}

// ---- positive ----

#[rstest]
#[tokio::test]
async fn sql_runs_over_the_projects_tables(pipeline: &'static Pipeline) {
    let p = pipeline;
    let sql = format!(
        "SELECT status, count(*) AS n, sum(CAST(total AS DOUBLE)) AS sum FROM {} \
         WHERE _op = 'insert' GROUP BY status ORDER BY status",
        orders(p)
    );
    let reply = query(p, &p.env.admin, Some(&p.project.id), &sql).await.ok();
    let rows = reply.rows();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["status"], "new");
    assert_eq!(rows[0]["n"], 1);
    assert_eq!(rows[1]["status"], "paid");
    assert_eq!(rows[1]["n"], 2);
    // `numeric` is stored as text, so arithmetic needs the cast above.
    assert_eq!(rows[1]["sum"], 50.0);
}

#[rstest]
#[tokio::test]
async fn queries_without_tables_work(pipeline: &'static Pipeline) {
    let p = pipeline;
    let reply = query(
        p,
        &p.env.admin,
        Some(&p.project.id),
        "SELECT 1 + 1 AS two, 'x' AS s",
    )
    .await
    .ok();
    assert_eq!(reply.rows(), &vec![json!({"two": 2, "s": "x"})]);
}

#[rstest]
#[tokio::test]
async fn an_empty_result_is_an_empty_list(pipeline: &'static Pipeline) {
    let p = pipeline;
    let sql = format!("SELECT id FROM {} WHERE id < 0", orders(p));
    let reply = query(p, &p.env.admin, Some(&p.project.id), &sql).await.ok();
    assert!(reply.rows().is_empty());
}

#[rstest]
#[tokio::test]
async fn iceberg_metadata_tables_can_be_queried(pipeline: &'static Pipeline) {
    let p = pipeline;
    let sql = format!(
        "SELECT snapshot_id FROM pipa.{}.\"{ORDERS}$snapshots\"",
        namespace(&p.source)
    );
    let reply = query(p, &p.env.admin, Some(&p.project.id), &sql).await.ok();
    assert!(!reply.rows().is_empty());
}

#[rstest]
#[tokio::test]
async fn a_developer_queries_its_own_project(pipeline: &'static Pipeline) {
    let p = pipeline;
    let developer = p.env.developer(&[&p.project]).await;
    let sql = format!("SELECT count(*) AS n FROM {}", orders(p));
    let reply = query(p, &developer.token, Some(&p.project.id), &sql)
        .await
        .ok();
    assert_eq!(reply.rows()[0]["n"], 3);
}

#[rstest]
#[tokio::test]
async fn an_admin_without_a_project_sees_every_project(pipeline: &'static Pipeline) {
    let p = pipeline;
    let sql = format!("SELECT count(*) AS n FROM {}", orders(p));
    // Not scoped: the namespace of a project's source is reachable.
    let reply = query(p, &p.env.admin, None, &sql).await.ok();
    assert_eq!(reply.rows()[0]["n"], 3);
}

#[rstest]
#[tokio::test]
async fn the_same_query_can_be_repeated(pipeline: &'static Pipeline) {
    let p = pipeline;
    let sql = format!("SELECT count(*) AS n FROM {}", orders(p));
    for _ in 0..3 {
        let reply = query(p, &p.env.admin, Some(&p.project.id), &sql).await.ok();
        assert_eq!(reply.rows()[0]["n"], 3);
    }
}

// ---- the project scope ----

#[rstest]
#[tokio::test]
async fn a_query_cannot_reach_another_projects_tables(pipeline: &'static Pipeline) {
    let p = pipeline;
    let other = p.env.unique_project().await;
    let sql = format!("SELECT count(*) AS n FROM {}", orders(p));

    // The admin scoping the query to a different project doesn't see this project's tables.
    let reply = query(p, &p.env.admin, Some(&other.id), &sql)
        .await
        .bad_request();
    assert!(!reply.error().is_empty());

    // Nor does a developer of that other project.
    let developer = p.env.developer(&[&other]).await;
    query(p, &developer.token, Some(&other.id), &sql)
        .await
        .bad_request();
}

#[rstest]
#[tokio::test]
async fn another_projects_tables_stay_out_of_the_listing_of_a_scoped_session(
    pipeline: &'static Pipeline,
) {
    let p = pipeline;
    let other = p.env.unique_project().await;
    let reply = query(
        p,
        &p.env.admin,
        Some(&other.id),
        "SELECT table_schema, table_name FROM pipa.information_schema.tables",
    )
    .await;
    // Either the information schema is not offered at all, or it shows nothing of this project.
    if reply.status == 200 {
        assert!(
            !reply.body.to_string().contains(&namespace(&p.source)),
            "{}",
            reply.body
        );
    }
}

// ---- who may query ----

#[rstest]
#[tokio::test]
async fn a_developer_must_name_a_project(pipeline: &'static Pipeline) {
    let p = pipeline;
    let developer = p.env.developer(&[&p.project]).await;
    query(p, &developer.token, None, "SELECT 1")
        .await
        .forbidden();
}

#[rstest]
#[tokio::test]
async fn a_developer_cannot_query_a_project_it_is_not_in(pipeline: &'static Pipeline) {
    let p = pipeline;
    let elsewhere = p.env.unique_project().await;
    let developer = p.env.developer(&[&elsewhere]).await;
    query(p, &developer.token, Some(&p.project.id), "SELECT 1")
        .await
        .forbidden();
}

#[rstest]
#[tokio::test]
async fn a_viewer_cannot_run_sql_even_in_its_own_project(pipeline: &'static Pipeline) {
    let p = pipeline;
    let viewer = p.env.viewer(&[&p.project]).await;
    assert_eq!(viewer.user.role, Role::User);
    query(p, &viewer.token, Some(&p.project.id), "SELECT 1")
        .await
        .forbidden();
    query(p, &viewer.token, None, "SELECT 1").await.forbidden();
}

// ---- only reading ----

#[rstest]
#[case::create_table("CREATE TABLE evil AS SELECT 1 AS x")]
#[case::create_view("CREATE VIEW evil AS SELECT 1 AS x")]
#[case::create_external_table("CREATE EXTERNAL TABLE evil STORED AS PARQUET LOCATION 's3://x/y'")]
#[case::create_schema("CREATE SCHEMA evil")]
#[case::drop_table("DROP TABLE pipa.public.nothing")]
#[case::insert("INSERT INTO pipa.public.nothing VALUES (1)")]
#[case::update("UPDATE pipa.public.nothing SET a = 1")]
#[case::delete("DELETE FROM pipa.public.nothing")]
#[case::set_variable("SET datafusion.execution.batch_size = 1")]
#[case::copy("COPY (SELECT 1) TO '/tmp/pipa-evil.csv'")]
#[tokio::test]
async fn statements_that_change_things_are_rejected(
    pipeline: &'static Pipeline,
    #[case] sql: &str,
) {
    let p = pipeline;
    let reply = query(p, &p.env.admin, Some(&p.project.id), sql).await;
    assert_eq!(reply.status, 400, "{}", reply.body);
    assert!(!std::path::Path::new("/tmp/pipa-evil.csv").exists());
}

#[rstest]
#[tokio::test]
async fn the_tables_survive_a_rejected_statement(pipeline: &'static Pipeline) {
    let p = pipeline;
    let drop = format!("DROP TABLE {}", orders(p));
    query(p, &p.env.admin, Some(&p.project.id), &drop)
        .await
        .bad_request();
    let sql = format!("SELECT count(*) AS n FROM {}", orders(p));
    assert_eq!(
        query(p, &p.env.admin, Some(&p.project.id), &sql)
            .await
            .ok()
            .rows()[0]["n"],
        3
    );
}

// ---- bad requests ----

#[rstest]
#[case::syntax_error("SELEC 1")]
#[case::unfinished("SELECT FROM WHERE")]
#[case::empty("")]
#[case::blank("   ")]
#[case::unknown_table("SELECT * FROM pipa.public.no_such_table")]
#[case::unknown_column("SELECT no_such_column")]
#[case::unknown_function("SELECT no_such_function(1)")]
#[tokio::test]
async fn an_invalid_query_is_a_bad_request(pipeline: &'static Pipeline, #[case] sql: &str) {
    let p = pipeline;
    let reply = query(p, &p.env.admin, Some(&p.project.id), sql)
        .await
        .bad_request();
    assert!(!reply.error().is_empty());
}

#[rstest]
#[tokio::test]
async fn the_project_id_must_be_a_uuid(pipeline: &'static Pipeline) {
    let p = pipeline;
    let reply = query(p, &p.env.admin, Some("not-a-uuid"), "SELECT 1")
        .await
        .bad_request();
    assert!(reply.error().contains("UUID"), "{}", reply.error());
}

#[rstest]
#[case::no_sql(json!({"project_id": null}))]
#[case::sql_of_the_wrong_type(json!({"sql": 1}))]
#[tokio::test]
async fn a_malformed_query_request_is_rejected(
    env: &'static TestEnv,
    #[case] body: serde_json::Value,
) {
    let reply = env
        .backend
        .post(path::QUERY)
        .bearer(&env.admin)
        .json(&body)
        .send()
        .await;
    assert_eq!(reply.status, 422, "{}", reply.body);
}

#[rstest]
#[tokio::test]
async fn an_unknown_project_gives_an_empty_scope_not_an_error_for_an_admin(env: &'static TestEnv) {
    let reply = env
        .backend
        .post(path::QUERY)
        .bearer(&env.admin)
        .json(&QueryRequest {
            sql: "SELECT 1 AS one".to_string(),
            project_id: Some(unique_uuid()),
        })
        .send()
        .await
        .ok();
    assert_eq!(reply.rows().len(), 1);
}

fn unique_uuid() -> String {
    "0190a1b2-0000-7000-8000-000000000001".to_string()
}
