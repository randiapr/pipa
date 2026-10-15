//! A change in Postgres, all the way to being read through `pipa-backend`.

use std::time::Duration;

use pipa_api::path;
use pipa_integration_test::env::{TestEnv, unique};
use pipa_integration_test::pipeline::{ORDERS, Pipeline, statuses, wait_for_slot};
use pipa_integration_test::postgres::Database;
use pipa_integration_test::wait;
use rstest::{fixture, rstest};
use serde_json::{Value, json};

use crate::common::own_env;

/// A source of its own on the shared stack: the common case.
#[fixture]
async fn pipeline() -> Pipeline {
    Pipeline::shared().await
}

/// A backend and ingestion instance of its own, for the tests that stop or crash ingestion.
#[fixture]
async fn own_pipeline(#[future] own_env: TestEnv) -> Pipeline {
    Pipeline::start(own_env.await).await
}

fn ops(rows: &[Value]) -> Vec<(String, i64)> {
    let mut found: Vec<(String, i64)> = rows
        .iter()
        .map(|row| {
            (
                row["_op"].as_str().unwrap().to_string(),
                row["id"].as_i64().unwrap(),
            )
        })
        .collect();
    found.sort();
    found
}

// ---- the three kinds of change ----

#[rstest]
#[tokio::test]
async fn inserted_rows_arrive_as_current_rows(#[future] pipeline: Pipeline) {
    let p = pipeline.await;
    p.db.execute("INSERT INTO orders VALUES (1, 'new', 10.50), (2, 'new', 20.00)")
        .await;

    let rows = p
        .rows_when(ORDERS, "both inserted rows", |rows| rows.len() == 2)
        .await;
    assert_eq!(statuses(&rows), vec![(1, "new".into()), (2, "new".into())]);
    // The browser shows the row, not the changelog bookkeeping.
    for column in ["_op", "_position", "_source_id", "_commit_timestamp_us"] {
        assert!(
            rows[0].get(column).is_none(),
            "{column} leaked: {}",
            rows[0]
        );
    }
    assert_eq!(rows[0]["id"], 1);
}

#[rstest]
#[tokio::test]
async fn an_update_replaces_the_current_row_and_the_changelog_keeps_both(
    #[future] pipeline: Pipeline,
) {
    let p = pipeline.await;
    p.db.execute("INSERT INTO orders VALUES (1, 'new', 10.50)")
        .await;
    p.rows_when(ORDERS, "the insert", |rows| rows.len() == 1)
        .await;

    p.db.execute("UPDATE orders SET status = 'paid', total = 11.00 WHERE id = 1")
        .await;
    let rows = p
        .rows_when(ORDERS, "the update", |rows| {
            rows.len() == 1 && rows[0]["status"] == "paid"
        })
        .await;
    assert_eq!(rows[0]["id"], 1);

    let sql = p.changelog_sql("_op, id, status", ORDERS);
    let changelog = p
        .env
        .wait_for_query(&p.project, &sql, "both changes in the changelog", |rows| {
            rows.len() == 2
        })
        .await;
    assert_eq!(
        ops(&changelog),
        vec![("insert".into(), 1), ("update".into(), 1)]
    );
}

#[rstest]
#[tokio::test]
async fn a_delete_drops_the_row_but_stays_in_the_changelog(#[future] pipeline: Pipeline) {
    let p = pipeline.await;
    p.db.execute("INSERT INTO orders VALUES (1, 'new', 1), (2, 'new', 2)")
        .await;
    p.rows_when(ORDERS, "the inserts", |rows| rows.len() == 2)
        .await;

    p.db.execute("DELETE FROM orders WHERE id = 1").await;
    let rows = p
        .rows_when(ORDERS, "the delete", |rows| rows.len() == 1)
        .await;
    assert_eq!(rows[0]["id"], 2);

    let sql = p.changelog_sql("_op, id, status", ORDERS);
    let changelog = p
        .env
        .wait_for_query(&p.project, &sql, "the delete in the changelog", |rows| {
            rows.len() == 3
        })
        .await;
    assert_eq!(
        ops(&changelog),
        vec![
            ("delete".into(), 1),
            ("insert".into(), 1),
            ("insert".into(), 2)
        ]
    );
}

#[rstest]
#[tokio::test]
async fn a_row_deleted_and_inserted_again_is_current_again(#[future] pipeline: Pipeline) {
    let p = pipeline.await;
    p.db.execute("INSERT INTO orders VALUES (1, 'first', 1)")
        .await;
    p.db.execute("DELETE FROM orders WHERE id = 1").await;
    p.db.execute("INSERT INTO orders VALUES (1, 'second', 2)")
        .await;

    let rows = p
        .rows_when(ORDERS, "the re-inserted row", |rows| {
            rows.len() == 1 && rows[0]["status"] == "second"
        })
        .await;
    assert_eq!(rows[0]["id"], 1);
}

#[rstest]
#[tokio::test]
async fn several_updates_of_a_row_leave_the_last_one(#[future] pipeline: Pipeline) {
    let p = pipeline.await;
    p.db.execute("INSERT INTO orders VALUES (1, 's0', 0)").await;
    for step in 1..=5 {
        p.db.execute(&format!(
            "UPDATE orders SET status = 's{step}' WHERE id = 1"
        ))
        .await;
    }
    let rows = p
        .rows_when(ORDERS, "the last update", |rows| {
            rows.len() == 1 && rows[0]["status"] == "s5"
        })
        .await;
    assert_eq!(rows.len(), 1);
    let sql = p.changelog_sql("id", ORDERS);
    p.env
        .wait_for_query(&p.project, &sql, "all six changes", |rows| rows.len() == 6)
        .await;
}

// ---- transactions ----

#[rstest]
#[tokio::test]
async fn a_large_transaction_lands_whole(#[future] pipeline: Pipeline) {
    let p = pipeline.await;
    p.db.execute("INSERT INTO orders SELECT g, 'bulk', g FROM generate_series(1, 500) AS g")
        .await;
    let rows = p
        .rows_when(ORDERS, "all 500 rows", |rows| rows.len() == 500)
        .await;
    assert_eq!(rows.len(), 500);
}

#[rstest]
#[tokio::test]
async fn a_rolled_back_transaction_leaves_no_trace(#[future] pipeline: Pipeline) {
    let p = pipeline.await;
    p.db.execute("BEGIN; INSERT INTO orders VALUES (1, 'ghost', 1); ROLLBACK;")
        .await;
    p.db.execute("INSERT INTO orders VALUES (2, 'real', 2)")
        .await;

    let rows = p
        .rows_when(ORDERS, "the committed row", |rows| !rows.is_empty())
        .await;
    assert_eq!(statuses(&rows), vec![(2, "real".into())]);
}

#[rstest]
#[tokio::test]
async fn a_transaction_across_tables_lands_in_each() {
    let p = Pipeline::shared_with(
        pipa_integration_test::pipeline::SCHEMA,
        &[("public", "orders"), ("public", "customers")],
    )
    .await;
    p.db.execute(
        "BEGIN;
         INSERT INTO customers VALUES (1, 'a@example.com');
         INSERT INTO orders VALUES (1, 'new', 5);
         COMMIT;",
    )
    .await;
    p.rows_when(ORDERS, "the order", |rows| rows.len() == 1)
        .await;
    let customers = p
        .rows_when("public__customers", "the customer", |rows| rows.len() == 1)
        .await;
    assert_eq!(customers[0]["email"], "a@example.com");
}

// ---- values ----

#[rstest]
#[tokio::test]
async fn nulls_and_awkward_text_survive(#[future] pipeline: Pipeline) {
    let p = pipeline.await;
    p.db.execute("INSERT INTO orders VALUES (1, NULL, NULL)")
        .await;
    p.db.execute(
        "INSERT INTO orders VALUES (2, E'quote '' \"double\" , comma\\nnewline ünïcode 🚀', 0)",
    )
    .await;

    let rows = p
        .rows_when(ORDERS, "both rows", |rows| rows.len() == 2)
        .await;
    assert!(
        rows[0]["status"].is_null() && rows[0]["total"].is_null(),
        "{}",
        rows[0]
    );
    assert_eq!(
        rows[1]["status"],
        "quote ' \"double\" , comma\nnewline ünïcode 🚀"
    );
}

#[rstest]
#[tokio::test]
async fn the_column_types_of_the_source_are_kept() {
    let p = Pipeline::shared_with(
        "CREATE TABLE typed (
            id bigint PRIMARY KEY,
            flag boolean,
            amount numeric(12, 3),
            ratio double precision,
            label varchar(20),
            happened timestamptz,
            day date,
            payload jsonb
        )",
        &[("public", "typed")],
    )
    .await;
    p.db.execute(
        "INSERT INTO typed VALUES
         (1, true, 12.345, 0.5, 'x', '2026-01-02 03:04:05+00', '2026-01-02', '{\"a\": [1, 2]}')",
    )
    .await;
    let rows = p
        .rows_when("public__typed", "the row", |rows| rows.len() == 1)
        .await;
    let row = &rows[0];
    assert_eq!(row["id"], 1);
    assert_eq!(row["flag"], true);
    assert_eq!(row["ratio"], 0.5);
    assert_eq!(row["label"], "x");
    assert!(row["amount"].to_string().contains("12.345"), "{row}");
    assert!(row["happened"].to_string().contains("2026-01-02"), "{row}");
    assert!(row["day"].to_string().contains("2026-01-02"), "{row}");
    assert!(row["payload"].to_string().contains('a'), "{row}");
}

#[rstest]
#[tokio::test]
async fn table_and_schema_names_are_sanitized_into_the_iceberg_name() {
    let p = Pipeline::shared_with(
        "CREATE SCHEMA \"Sales\";
         CREATE TABLE \"Sales\".\"Order Items\" (id integer PRIMARY KEY, sku text)",
        &[("Sales", "Order Items")],
    )
    .await;
    p.db.execute("INSERT INTO \"Sales\".\"Order Items\" VALUES (1, 'abc')")
        .await;
    let rows = p
        .rows_when("sales__order_items", "the row", |rows| rows.len() == 1)
        .await;
    assert_eq!(rows[0]["sku"], "abc");
}

#[rstest]
#[tokio::test]
async fn a_table_without_a_primary_key_is_read_as_stored() {
    let p = Pipeline::shared_with(
        "CREATE TABLE audit_log (kind text, detail text)",
        &[("public", "audit_log")],
    )
    .await;
    p.db.execute("INSERT INTO audit_log VALUES ('login', 'a'), ('login', 'a'), ('logout', 'b')")
        .await;
    // No key to collapse on: identical rows stay separate rows.
    let rows = p
        .rows_when("public__audit_log", "all three rows", |rows| {
            rows.len() == 3
        })
        .await;
    assert_eq!(rows.len(), 3);
}

#[rstest]
#[tokio::test]
async fn a_composite_key_identifies_a_row() {
    let p = Pipeline::shared_with(
        "CREATE TABLE lines (order_id integer, line integer, qty integer, PRIMARY KEY (order_id, line))",
        &[("public", "lines")],
    )
    .await;
    p.db.execute("INSERT INTO lines VALUES (1, 1, 1), (1, 2, 1), (2, 1, 1)")
        .await;
    p.db.execute("UPDATE lines SET qty = 9 WHERE order_id = 1 AND line = 2")
        .await;
    let rows = p
        .rows_when("public__lines", "the update", |rows| {
            rows.len() == 3 && rows.iter().any(|row| row["qty"] == 9)
        })
        .await;
    assert_eq!(rows.iter().filter(|row| row["qty"] == 9).count(), 1);
}

// ---- which tables are captured ----

#[rstest]
#[tokio::test]
async fn tables_that_are_not_chosen_are_not_ingested(#[future] pipeline: Pipeline) {
    let p = pipeline.await;
    p.db.execute("INSERT INTO customers VALUES (1, 'secret@example.com')")
        .await;
    p.db.execute("INSERT INTO orders VALUES (1, 'new', 1)")
        .await;
    p.rows_when(ORDERS, "the order", |rows| rows.len() == 1)
        .await;

    wait::stays(
        "the customers table staying out",
        Duration::from_secs(4),
        || async {
            let reply = p
                .env
                .backend
                .get(&format!("{}?project_id={}", path::TABLES, p.project.id))
                .bearer(&p.env.admin)
                .send()
                .await;
            let tables = reply.body["tables"].to_string();
            if tables.contains("customers") {
                Err(tables)
            } else {
                Ok(())
            }
        },
    )
    .await;
    // Not even by asking for it by name.
    let reply = p
        .env
        .table_rows(
            &p.env.admin,
            &p.project,
            &p.source,
            "public__customers",
            None,
            None,
        )
        .await;
    assert_eq!(reply.status, 400, "{}", reply.body);
}

#[rstest]
#[tokio::test]
async fn nothing_is_ingested_until_tables_are_chosen() {
    let p = Pipeline::shared_with(pipa_integration_test::pipeline::SCHEMA, &[]).await;
    p.db.execute("INSERT INTO orders VALUES (1, 'new', 1)")
        .await;
    p.db.execute("INSERT INTO customers VALUES (1, 'a@example.com')")
        .await;

    wait::stays("an empty table list", Duration::from_secs(4), || async {
        let reply = p
            .env
            .backend
            .get(&format!("{}?project_id={}", path::TABLES, p.project.id))
            .bearer(&p.env.admin)
            .send()
            .await;
        if reply.body["tables"].as_array().is_some_and(Vec::is_empty) {
            Ok(())
        } else {
            Err(reply.body.to_string())
        }
    })
    .await;
}

#[rstest]
#[tokio::test]
async fn a_table_chosen_later_is_ingested_from_then_on(#[future] pipeline: Pipeline) {
    let p = pipeline.await;
    p.db.execute("INSERT INTO customers VALUES (1, 'before@example.com')")
        .await;
    p.db.execute("INSERT INTO orders VALUES (1, 'new', 1)")
        .await;
    p.rows_when(ORDERS, "the order", |rows| rows.len() == 1)
        .await;

    p.env
        .ingest(&p.source, &[("public", "orders"), ("public", "customers")])
        .await;
    // The choice is picked up on ingestion's next refresh (1 s here), without a restart; keep
    // inserting until the first row after that lands.
    let mut next = 2;
    let pipeline = &p;
    let rows = wait::eventually("the customers table to be captured", || {
        let sql = format!("INSERT INTO customers VALUES ({next}, 'after{next}@example.com')");
        next += 1;
        async move {
            pipeline.db.execute(&sql).await;
            let reply = pipeline
                .env
                .table_rows(
                    &pipeline.env.admin,
                    &pipeline.project,
                    &pipeline.source,
                    "public__customers",
                    None,
                    None,
                )
                .await;
            if reply.status == 200 && !reply.rows().is_empty() {
                Ok(reply.rows().clone())
            } else {
                Err(reply.body.to_string())
            }
        }
    })
    .await;
    // What happened before the choice is not backfilled.
    assert!(
        rows.iter().all(|row| row["email"] != "before@example.com"),
        "{rows:?}"
    );
}

#[rstest]
#[tokio::test]
async fn a_table_that_is_unchosen_stops_being_ingested(#[future] pipeline: Pipeline) {
    let p = pipeline.await;
    p.db.execute("INSERT INTO orders VALUES (1, 'kept', 1)")
        .await;
    p.rows_when(ORDERS, "the first order", |rows| rows.len() == 1)
        .await;

    p.env.ingest(&p.source, &[]).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    p.db.execute("INSERT INTO orders VALUES (2, 'dropped', 2)")
        .await;

    wait::stays(
        "orders staying as it was",
        Duration::from_secs(4),
        || async {
            let reply = p
                .env
                .table_rows(&p.env.admin, &p.project, &p.source, ORDERS, None, None)
                .await;
            let rows = reply.rows();
            if rows.len() == 1 {
                Ok(())
            } else {
                Err(format!("{rows:?}"))
            }
        },
    )
    .await;
}

// ---- ingestion coming and going ----

#[rstest]
#[tokio::test]
async fn changes_made_while_ingestion_was_down_are_caught_up_once(
    #[future] own_pipeline: Pipeline,
) {
    let mut p = own_pipeline.await;
    p.db.execute("INSERT INTO orders VALUES (1, 'before', 1)")
        .await;
    p.rows_when(ORDERS, "the first row", |rows| rows.len() == 1)
        .await;

    p.ingestion().stop().await;
    p.db.execute("INSERT INTO orders VALUES (2, 'while-down', 2), (3, 'while-down', 3)")
        .await;
    p.db.execute("UPDATE orders SET status = 'changed' WHERE id = 1")
        .await;
    let next = p.env.ingestion().await;
    p.set_ingestion(next);

    let rows = p
        .rows_when(ORDERS, "the missed changes", |rows| {
            rows.len() == 3 && rows.iter().any(|row| row["status"] == "changed")
        })
        .await;
    assert_eq!(rows.len(), 3);

    // Exactly once: three inserts and one update, however often the WAL was redelivered.
    let sql = p.changelog_sql("_op, id", ORDERS);
    let changelog = p
        .env
        .wait_for_query(&p.project, &sql, "the whole changelog", |rows| {
            rows.len() >= 4
        })
        .await;
    assert_eq!(
        ops(&changelog),
        vec![
            ("insert".into(), 1),
            ("insert".into(), 2),
            ("insert".into(), 3),
            ("update".into(), 1)
        ]
    );
}

#[rstest]
#[tokio::test]
async fn a_crashed_ingestion_loses_and_duplicates_nothing(#[future] own_pipeline: Pipeline) {
    let mut p = own_pipeline.await;
    for id in 1..=20 {
        p.db.execute(&format!("INSERT INTO orders VALUES ({id}, 'a', {id})"))
            .await;
    }
    // No warning: whatever was in flight is redelivered from the replication slot.
    p.ingestion().crash().await;
    for id in 21..=40 {
        p.db.execute(&format!("INSERT INTO orders VALUES ({id}, 'b', {id})"))
            .await;
    }
    let next = p.env.ingestion().await;
    p.set_ingestion(next);

    p.rows_when(ORDERS, "all 40 rows", |rows| rows.len() == 40)
        .await;
    let sql = p.changelog_sql("id", ORDERS);
    // Give any duplicate time to show up, then count.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let changelog = p.env.query(&p.project, &sql).await.ok();
    assert_eq!(changelog.rows().len(), 40, "duplicated or lost changes");
}

#[rstest]
#[tokio::test]
async fn ingestion_survives_a_source_it_cannot_reach(#[future] own_pipeline: Pipeline) {
    let mut p = own_pipeline.await;
    // A second source of the project whose password is wrong...
    let mut broken = pipa_integration_test::env::new_source(
        Some(&p.project),
        &unique("broken"),
        &Database::create().await,
    );
    broken.connection.password = "wrong".to_string();
    let broken: pipa_api::DataSourceData = p
        .env
        .backend
        .post(path::DATASOURCES)
        .bearer(&p.env.admin)
        .json(&broken)
        .send()
        .await
        .created()
        .parse();
    p.env
        .ingest(&broken.datasource, &[("public", "orders")])
        .await;
    tokio::time::sleep(Duration::from_secs(4)).await;

    // ...does not stop the healthy one, or ingestion itself.
    assert!(p.ingestion().is_running(), "{}", p.ingestion().log_tail(30));
    p.db.execute("INSERT INTO orders VALUES (1, 'fine', 1)")
        .await;
    p.rows_when(ORDERS, "the healthy source's row", |rows| rows.len() == 1)
        .await;
}

#[rstest]
#[tokio::test]
async fn a_removed_source_stops_being_captured_without_stopping_ingestion(
    #[future] own_pipeline: Pipeline,
) {
    let mut p = own_pipeline.await;
    p.db.execute("INSERT INTO orders VALUES (1, 'new', 1)")
        .await;
    p.rows_when(ORDERS, "the row", |rows| rows.len() == 1).await;

    p.env
        .backend
        .delete(&path::datasource(&p.source.id))
        .bearer(&p.env.admin)
        .send()
        .await
        .deleted();
    tokio::time::sleep(Duration::from_secs(3)).await;
    p.db.execute("INSERT INTO orders VALUES (2, 'unseen', 2)")
        .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(p.ingestion().is_running(), "{}", p.ingestion().log_tail(30));

    // A source registered afterwards is still picked up.
    let db = Database::create().await;
    db.execute("CREATE TABLE t (id integer PRIMARY KEY)").await;
    let next = p.env.source(Some(&p.project), &unique("next"), &db).await;
    p.env.ingest(&next, &[("public", "t")]).await;
    wait_for_slot(&next).await;
    db.execute("INSERT INTO t VALUES (1)").await;
    p.env
        .wait_for_rows(
            &p.project,
            &next,
            "public__t",
            "the new source's row",
            |rows| rows.len() == 1,
        )
        .await;
}

// ---- the iceberg side ----

#[rstest]
#[tokio::test]
async fn the_changelog_columns_describe_each_change(#[future] pipeline: Pipeline) {
    let p = pipeline.await;
    p.db.execute("INSERT INTO orders VALUES (1, 'new', 1)")
        .await;
    p.rows_when(ORDERS, "the row", |rows| rows.len() == 1).await;

    let sql = p.changelog_sql(
        "_op, _source_id, _position, _commit_timestamp_us, id",
        ORDERS,
    );
    let changelog = p.env.query(&p.project, &sql).await.ok();
    let row = &changelog.rows()[0];
    assert_eq!(row["_op"], "insert");
    assert_eq!(row["_source_id"], json!(p.source.id));
    assert!(
        row["_position"]
            .as_str()
            .is_some_and(|position| position.contains('/'))
    );
    assert!(row["_commit_timestamp_us"].as_i64().unwrap() > 1_700_000_000_000_000);
}
