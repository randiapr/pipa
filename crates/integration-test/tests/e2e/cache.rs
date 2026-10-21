// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! The query/table-list cache: in memory by default, shared through Redis with `REDIS_URL`.
//! `now()` is the probe: a cached result keeps the value it was computed with.

use std::time::Duration;

use pipa_api::{QueryRequest, path};
use pipa_integration_test::backend::{Backend, BackendConfig};
use pipa_integration_test::env::TestEnv;
use pipa_integration_test::pipeline::{ORDERS, Pipeline, wait_for_backfill};
use pipa_integration_test::redis_db::RedisDb;
use pipa_integration_test::reply::Reply;
use rstest::rstest;

const QUERY_KEYS: &str = "pipa:cache:v1:query:*";
const TABLES_KEYS: &str = "pipa:cache:v1:tables:*";

/// A backend whose caches live `ttl` seconds, in `redis` if given.
async fn cached_env(redis: Option<&RedisDb>, ttl: u64) -> TestEnv {
    TestEnv::with(|config| {
        config.redis_url = redis.map(|redis| redis.url.clone());
        config.query_ttl_secs = ttl;
        config.tables_ttl_secs = ttl;
    })
    .await
}

/// Keys of `pattern` that appeared since `before` (the harness' own readiness probe is cached too,
/// so a test counts what it added rather than all there is).
async fn added(redis: &RedisDb, pattern: &str, before: &[String]) -> Vec<String> {
    redis
        .keys(pattern)
        .await
        .into_iter()
        .filter(|key| !before.contains(key))
        .collect()
}

async fn run(backend: &Backend, token: &str, project: Option<&str>, sql: &str) -> Reply {
    backend
        .post(path::QUERY)
        .bearer(token)
        .json(&QueryRequest {
            sql: sql.to_string(),
            project_id: project.map(str::to_string),
        })
        .send()
        .await
}

/// The value of `now()` as the backend answers it.
async fn now(backend: &Backend, token: &str, project: Option<&str>) -> String {
    now_as(backend, token, project, "t").await
}

async fn now_as(backend: &Backend, token: &str, project: Option<&str>, alias: &str) -> String {
    let reply = run(backend, token, project, &format!("SELECT now() AS {alias}"))
        .await
        .ok();
    reply.rows()[0][alias].to_string()
}

// ---- positive ----

#[rstest]
#[tokio::test]
async fn a_query_result_is_cached_in_redis_for_its_ttl() {
    let redis = RedisDb::new().await;
    let env = cached_env(Some(&redis), 30).await;
    let before = redis.keys(QUERY_KEYS).await;

    let first = now(&env.backend, &env.admin, None).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let second = now(&env.backend, &env.admin, None).await;
    assert_eq!(first, second, "the second answer was computed again");

    let keys = added(&redis, QUERY_KEYS, &before).await;
    assert_eq!(keys.len(), 1, "{keys:?}");
    // The key is a digest: the SQL isn't readable from it.
    assert!(!keys[0].to_lowercase().contains("now"), "{}", keys[0]);
    let ttl = redis.ttl_ms(&keys[0]).await;
    assert!((1..=30_000).contains(&ttl), "ttl {ttl} ms");
}

#[rstest]
#[tokio::test]
async fn without_redis_a_backend_caches_in_memory() {
    let env = cached_env(None, 30).await;
    let first = now(&env.backend, &env.admin, None).await;
    let second = now(&env.backend, &env.admin, None).await;
    assert_eq!(first, second);
}

#[rstest]
#[tokio::test]
async fn backends_share_a_cache_through_redis_but_not_in_memory() {
    let redis = RedisDb::new().await;
    let env = cached_env(Some(&redis), 30).await;
    let sibling = Backend::start(env.backend.config.clone()).await.unwrap();

    let first = now(&env.backend, &env.admin, None).await;
    let from_sibling = now(&sibling, &env.admin, None).await;
    assert_eq!(
        first, from_sibling,
        "the sibling did not use the shared entry"
    );

    // In memory, each backend has its own.
    let memory = cached_env(None, 30).await;
    let other = Backend::start(memory.backend.config.clone()).await.unwrap();
    let here = now(&memory.backend, &memory.admin, None).await;
    let there = now(&other, &memory.admin, None).await;
    assert_ne!(here, there);
}

#[rstest]
#[tokio::test]
async fn different_sql_has_different_entries() {
    let redis = RedisDb::new().await;
    let env = cached_env(Some(&redis), 30).await;
    let before = redis.keys(QUERY_KEYS).await;
    let a = now_as(&env.backend, &env.admin, None, "a").await;
    let b = now_as(&env.backend, &env.admin, None, "b").await;
    assert_ne!(a, b);
    assert_eq!(added(&redis, QUERY_KEYS, &before).await.len(), 2);
}

#[rstest]
#[tokio::test]
async fn the_project_scope_is_part_of_the_key() {
    let redis = RedisDb::new().await;
    let env = cached_env(Some(&redis), 30).await;
    let first = env.unique_project().await;
    let second = env.unique_project().await;
    let before = redis.keys(QUERY_KEYS).await;

    // Without a source, both scopes are the same empty set; sources make them differ. So give
    // each project one.
    for project in [&first, &second] {
        let db = pipa_integration_test::postgres::Database::create().await;
        env.source(Some(project), &pipa_integration_test::env::unique("s"), &db)
            .await;
    }

    let a = now(&env.backend, &env.admin, Some(&first.id)).await;
    let b = now(&env.backend, &env.admin, Some(&second.id)).await;
    let all = now(&env.backend, &env.admin, None).await;
    assert_ne!(a, b, "one project was answered with the other's entry");
    assert_ne!(a, all);
    assert_eq!(added(&redis, QUERY_KEYS, &before).await.len(), 3);
}

#[rstest]
#[tokio::test]
async fn an_entry_is_shared_by_everyone_allowed_to_ask() {
    let redis = RedisDb::new().await;
    let env = cached_env(Some(&redis), 30).await;
    let project = env.unique_project().await;
    let developer = env.developer(&[&project]).await;

    let by_admin = now(&env.backend, &env.admin, Some(&project.id)).await;
    let by_developer = now(&env.backend, &developer.token, Some(&project.id)).await;
    assert_eq!(by_admin, by_developer);
}

// ---- negative ----

#[rstest]
#[tokio::test]
async fn a_cached_result_is_never_served_to_someone_not_allowed_to_ask() {
    let redis = RedisDb::new().await;
    let env = cached_env(Some(&redis), 30).await;
    let project = env.unique_project().await;
    let elsewhere = env.unique_project().await;
    let viewer = env.viewer(&[&project]).await;
    let outsider = env.developer(&[&elsewhere]).await;

    let sql = "SELECT now() AS t";
    run(&env.backend, &env.admin, Some(&project.id), sql)
        .await
        .ok();
    // The entry exists, and still: no.
    run(&env.backend, &viewer.token, Some(&project.id), sql)
        .await
        .forbidden();
    run(&env.backend, &outsider.token, Some(&project.id), sql)
        .await
        .forbidden();
    run(&env.backend, &outsider.token, None, sql)
        .await
        .forbidden();
}

#[rstest]
#[tokio::test]
async fn failed_queries_are_not_cached() {
    let redis = RedisDb::new().await;
    let env = cached_env(Some(&redis), 30).await;
    let before = redis.keys(QUERY_KEYS).await;
    run(&env.backend, &env.admin, None, "SELEC 1")
        .await
        .bad_request();
    run(
        &env.backend,
        &env.admin,
        None,
        "SELECT * FROM pipa.nope.nothing",
    )
    .await
    .bad_request();
    assert!(added(&redis, QUERY_KEYS, &before).await.is_empty());
}

#[rstest]
#[tokio::test]
async fn a_ttl_of_zero_turns_the_cache_off() {
    let redis = RedisDb::new().await;
    let env = cached_env(Some(&redis), 0).await;
    let first = now(&env.backend, &env.admin, None).await;
    let second = now(&env.backend, &env.admin, None).await;
    assert_ne!(first, second);
    assert!(redis.keys("pipa:cache:*").await.is_empty());
}

#[rstest]
#[tokio::test]
async fn an_entry_over_the_size_limit_is_not_stored() {
    let redis = RedisDb::new().await;
    let env = TestEnv::with(|config| {
        config.redis_url = Some(redis_url(&redis));
        config.query_ttl_secs = 30;
        config.extra_env = vec![("CACHE_MAX_ENTRY_BYTES".to_string(), "10".to_string())];
    })
    .await;
    let before = redis.keys(QUERY_KEYS).await;
    let first = now(&env.backend, &env.admin, None).await;
    let second = now(&env.backend, &env.admin, None).await;
    assert_ne!(first, second);
    assert!(added(&redis, QUERY_KEYS, &before).await.is_empty());
}

fn redis_url(redis: &RedisDb) -> String {
    redis.url.clone()
}

#[rstest]
#[tokio::test]
async fn an_unreachable_redis_costs_speed_not_availability() {
    // Nothing listens on port 1. Every query is answered, just never from a cache.
    let env = TestEnv::with(|config| {
        config.redis_url = Some("redis://127.0.0.1:1".to_string());
        config.query_ttl_secs = 30;
        config.tables_ttl_secs = 30;
    })
    .await;
    let first = now(&env.backend, &env.admin, None).await;
    let second = now(&env.backend, &env.admin, None).await;
    assert_ne!(first, second);

    let project = env.unique_project().await;
    env.backend
        .get(&format!("{}?project_id={}", path::TABLES, project.id))
        .bearer(&env.admin)
        .send()
        .await
        .ok();
}

#[rstest]
#[tokio::test]
async fn a_malformed_redis_url_stops_the_backend_from_starting() {
    let bucket = pipa_integration_test::env::new_bucket().await;
    let log = Backend::start_expecting_failure(BackendConfig {
        redis_url: Some("this is not a url".to_string()),
        ..BackendConfig::new(&bucket)
    })
    .await
    .unwrap();
    assert!(log.to_lowercase().contains("redis"), "{log}");
}

// ---- the table list ----

#[rstest]
#[tokio::test]
async fn the_table_list_is_cached_but_reading_a_table_is_not() {
    let redis = RedisDb::new().await;
    let p = Pipeline::start(cached_env(Some(&redis), 30).await).await;
    let list = || async {
        let reply = p
            .env
            .backend
            .get(&format!("{}?project_id={}", path::TABLES, p.project.id))
            .bearer(&p.env.admin)
            .send()
            .await
            .ok();
        reply.body["tables"].as_array().unwrap().len()
    };

    // Listed (and cached) with just the orders table, which its backfill created.
    assert_eq!(list().await, 1);
    p.db.execute("INSERT INTO orders VALUES (1, 'new', 1)")
        .await;
    // Reads always see the data as soon as it is there...
    p.rows_when(ORDERS, "the row", |rows| rows.len() == 1).await;
    p.db.execute("UPDATE orders SET status = 'paid' WHERE id = 1")
        .await;
    p.rows_when(ORDERS, "the update", |rows| rows[0]["status"] == "paid")
        .await;
    // ...and a table chosen now is there too...
    p.env
        .ingest(&p.source, &[("public", "orders"), ("public", "customers")])
        .await;
    wait_for_backfill(&p.env, &p.project, &p.source, &[("public", "customers")]).await;

    // ...while the list keeps the answer it had, until its ttl runs out.
    assert_eq!(list().await, 1, "the list was not served from the cache");
    assert_eq!(redis.keys(TABLES_KEYS).await.len(), 1);
}
