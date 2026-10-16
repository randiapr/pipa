//! Table maintenance in `pipa-ingestion`: small data files compacted and old snapshots expired,
//! while every row — and ingestion itself — carries on unchanged.

use std::time::Duration;

use pipa_integration_test::env::TestEnv;
use pipa_integration_test::pipeline::{ORDERS, Pipeline, statuses};
use rstest::rstest;
use serde_json::Value;

use crate::common::own_env;

/// A pipeline whose ingestion maintains its tables every second: compacting from four small
/// files on, and keeping only the current snapshot once the others are a second old.
async fn maintained_pipeline(env: TestEnv) -> Pipeline {
    Pipeline::start_configured(env, |config| {
        config.extra_env = [
            ("INGESTION_MAINTENANCE_INTERVAL_SECS", "1"),
            ("INGESTION_COMPACTION_MIN_INPUT_FILES", "4"),
            ("INGESTION_SNAPSHOT_MAX_AGE_SECS", "1"),
            ("INGESTION_SNAPSHOT_MIN_TO_KEEP", "1"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    })
    .await
}

/// The `total-data-files` a `$snapshots` row's summary records.
fn data_files(snapshot: &Value) -> u64 {
    snapshot["summary"]["total-data-files"]
        .as_str()
        .and_then(|count| count.parse().ok())
        .unwrap_or(u64::MAX)
}

#[rstest]
#[tokio::test]
async fn small_files_are_compacted_and_old_snapshots_expired_without_changing_a_row(
    #[future] own_env: TestEnv,
) {
    let p = maintained_pipeline(own_env.await).await;
    // One transaction per batch, so each lands as a file of its own.
    for id in 1..=6 {
        p.db.execute(&format!("INSERT INTO orders VALUES ({id}, 'new', {id})"))
            .await;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    p.rows_when(ORDERS, "the six orders", |rows| rows.len() == 6)
        .await;

    // The six one-file commits are compacted (a commit can land after a compaction, so the last
    // ones may still be files of their own), and every snapshot but the current one expires.
    let snapshots = p.changelog_sql("operation, summary", &format!("{ORDERS}$snapshots"));
    p.env
        .wait_for_query(
            &p.project,
            &snapshots,
            "a compacted table with one snapshot",
            |rows| rows.len() == 1 && data_files(&rows[0]) < 6,
        )
        .await;

    let rows = p
        .rows_when(ORDERS, "the six orders, compacted", |rows| rows.len() == 6)
        .await;
    assert_eq!(
        statuses(&rows),
        (1..=6)
            .map(|id| (id, "new".to_string()))
            .collect::<Vec<_>>()
    );

    // Ingestion carries on on top of it, and its checkpoint survived: nothing comes twice.
    p.db.execute("UPDATE orders SET status = 'paid' WHERE id = 1")
        .await;
    p.rows_when(ORDERS, "order 1 paid", |rows| {
        rows.iter()
            .any(|row| row["id"] == 1 && row["status"] == "paid")
    })
    .await;
    let changelog = p.changelog_sql("_op, id", ORDERS);
    p.env
        .wait_for_query(
            &p.project,
            &changelog,
            "six inserts and one update",
            |rows| rows.len() == 7,
        )
        .await;
}
