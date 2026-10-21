// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! Several ingestion instances: who captures which source, and what happens when one goes away.
//! Two instances must never both read the same source (a replication slot admits one consumer),
//! and a source must not stay unowned when its instance dies.

use std::time::Duration;

use pipa_api::{DataSourceView, ProjectView};
use pipa_integration_test::env::{TestEnv, unique};
use pipa_integration_test::ingestion::{Ingestion, IngestionConfig};
use pipa_integration_test::pipeline::{ORDERS, SCHEMA};
use pipa_integration_test::postgres::{self, Database};
use pipa_integration_test::redis_db::RedisDb;
use pipa_integration_test::wait;
use rstest::rstest;
use uuid::Uuid;

use crate::common::own_env;

struct World {
    env: TestEnv,
    project: ProjectView,
    db: Database,
    redis: RedisDb,
}

async fn world(env: TestEnv) -> World {
    let db = Database::create().await;
    db.execute(SCHEMA).await;
    World {
        project: env.unique_project().await,
        redis: RedisDb::new().await,
        env,
        db,
    }
}

impl World {
    /// A source of the world's database capturing `public.orders`.
    async fn source(&self) -> DataSourceView {
        let source = self
            .env
            .source(Some(&self.project), &unique("src"), &self.db)
            .await;
        self.env.ingest(&source, &[("public", "orders")]).await;
        source
    }

    fn instance(&self, id: &str) -> IngestionConfig {
        IngestionConfig {
            redis_url: Some(self.redis.url.clone()),
            instance_id: Some(id.to_string()),
            ..IngestionConfig::new(&self.env.bucket)
        }
    }

    /// Waits until `source`'s lease is held and returns its holder.
    async fn owner(&self, source: &DataSourceView) -> String {
        let key = format!("pipa:ingestion:lease:{}", source.id);
        wait::eventually(&format!("a lease on {}", source.id), || async {
            self.redis
                .get(&key)
                .await
                .ok_or_else(|| "no lease".to_string())
        })
        .await
    }

    /// Waits until `source`'s lease is held by someone other than `old`.
    async fn new_owner(&self, source: &DataSourceView, old: &str) -> String {
        let key = format!("pipa:ingestion:lease:{}", source.id);
        wait::eventually(&format!("{} to leave {}", old, source.id), || async {
            match self.redis.get(&key).await {
                Some(holder) if holder != old => Ok(holder),
                other => Err(format!("{other:?}")),
            }
        })
        .await
    }
}

// ---- with Redis ----

#[rstest]
#[tokio::test]
async fn an_instance_announces_itself_and_leases_its_sources(#[future] own_env: TestEnv) {
    let w = world(own_env.await).await;
    let source = w.source().await;
    let _instance = Ingestion::start(w.instance("solo")).await.unwrap();

    assert_eq!(w.owner(&source).await, "solo");
    let lease = format!("pipa:ingestion:lease:{}", source.id);
    let ttl = w.redis.ttl_ms(&lease).await;
    assert!(
        (1..=3_000).contains(&ttl),
        "the lease lives three intervals, got {ttl} ms"
    );
    assert_eq!(
        w.redis.get("pipa:ingestion:member:solo").await.as_deref(),
        Some("1")
    );
}

#[rstest]
#[tokio::test]
async fn sources_are_shared_between_instances_and_each_has_one_owner(#[future] own_env: TestEnv) {
    let w = world(own_env.await).await;
    let mut sources = Vec::new();
    for _ in 0..12 {
        sources.push(w.source().await);
    }
    let _first = Ingestion::start(w.instance("one")).await.unwrap();
    let _second = Ingestion::start(w.instance("two")).await.unwrap();

    let mut owners = Vec::new();
    for source in &sources {
        owners.push(w.owner(source).await);
    }
    // Let the second instance's arrival settle: sources move until the split is stable.
    tokio::time::sleep(Duration::from_secs(6)).await;
    let mut owners_after = Vec::new();
    for source in &sources {
        owners_after.push(w.owner(source).await);
    }
    for (index, owner) in owners_after.iter().enumerate() {
        assert!(owner == "one" || owner == "two", "source {index}: {owner}");
    }
    assert!(
        owners_after.iter().any(|owner| owner == "one"),
        "{owners_after:?}"
    );
    assert!(
        owners_after.iter().any(|owner| owner == "two"),
        "{owners_after:?}"
    );

    // One slot per source: nobody opened a second consumer.
    let slots = postgres::replication_slots().await;
    for source in &sources {
        let name = postgres::slot_name(&source.id);
        assert_eq!(
            slots.iter().filter(|slot| **slot == name).count(),
            1,
            "{name}"
        );
    }
}

#[rstest]
#[tokio::test]
async fn capture_goes_on_exactly_once_when_the_owner_is_stopped(#[future] own_env: TestEnv) {
    let w = world(own_env.await).await;
    let source = w.source().await;
    let mut one = Ingestion::start(w.instance("one")).await.unwrap();
    let mut two = Ingestion::start(w.instance("two")).await.unwrap();
    let owner = w.owner(&source).await;
    pipa_integration_test::pipeline::wait_for_slot(&source).await;

    for id in 1..=10 {
        w.db.execute(&format!("INSERT INTO orders VALUES ({id}, 'before', {id})"))
            .await;
    }
    w.env
        .wait_for_rows(&w.project, &source, ORDERS, "the first ten rows", |rows| {
            rows.len() == 10
        })
        .await;

    // Stop the owner the way an orchestrator does; the other takes the source over.
    let (owning, other) = if owner == "one" {
        (&mut one, "two")
    } else {
        (&mut two, "one")
    };
    owning.stop().await;
    assert_eq!(w.new_owner(&source, &owner).await, other);

    for id in 11..=20 {
        w.db.execute(&format!("INSERT INTO orders VALUES ({id}, 'after', {id})"))
            .await;
    }
    w.env
        .wait_for_rows(&w.project, &source, ORDERS, "all twenty rows", |rows| {
            rows.len() == 20
        })
        .await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let changelog = w
        .env
        .query(
            &w.project,
            &format!(
                "SELECT id FROM pipa.{}.\"{ORDERS}\"",
                pipa_integration_test::env::namespace(&source)
            ),
        )
        .await
        .ok();
    assert_eq!(
        changelog.rows().len(),
        20,
        "changes were lost or duplicated in the handoff"
    );
}

#[rstest]
#[tokio::test]
async fn a_crashed_owner_loses_its_sources_once_its_lease_lapses(#[future] own_env: TestEnv) {
    let w = world(own_env.await).await;
    let source = w.source().await;
    let mut one = Ingestion::start(w.instance("one")).await.unwrap();
    let mut two = Ingestion::start(w.instance("two")).await.unwrap();
    let owner = w.owner(&source).await;
    pipa_integration_test::pipeline::wait_for_slot(&source).await;
    w.db.execute("INSERT INTO orders VALUES (1, 'before', 1)")
        .await;
    w.env
        .wait_for_rows(&w.project, &source, ORDERS, "the first row", |rows| {
            rows.len() == 1
        })
        .await;

    // No chance to release anything: the other instance has to wait the lease out.
    let (owning, other) = if owner == "one" {
        (&mut one, "two")
    } else {
        (&mut two, "one")
    };
    owning.crash().await;
    assert_eq!(w.new_owner(&source, &owner).await, other);

    w.db.execute("INSERT INTO orders VALUES (2, 'after', 2)")
        .await;
    let rows = w
        .env
        .wait_for_rows(
            &w.project,
            &source,
            ORDERS,
            "the row after the crash",
            |rows| rows.len() == 2,
        )
        .await;
    assert_eq!(rows.len(), 2);
}

#[rstest]
#[tokio::test]
async fn a_new_instance_takes_nothing_from_a_healthy_one_it_does_not_win(
    #[future] own_env: TestEnv,
) {
    let w = world(own_env.await).await;
    let source = w.source().await;
    let mut one = Ingestion::start(w.instance("one")).await.unwrap();
    assert_eq!(w.owner(&source).await, "one");

    // A second instance arrives. Whichever of the two the source hashes to, it has exactly one
    // owner at every moment.
    let _two = Ingestion::start(w.instance("two")).await.unwrap();
    for _ in 0..8 {
        let holder = w.owner(&source).await;
        assert!(holder == "one" || holder == "two");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(one.is_running());
}

#[rstest]
#[tokio::test]
async fn with_redis_down_nothing_is_captured_but_ingestion_stays_up(#[future] own_env: TestEnv) {
    let w = world(own_env.await).await;
    let source = w.source().await;
    let instance = Ingestion::start(IngestionConfig {
        redis_url: Some("redis://127.0.0.1:1".to_string()),
        instance_id: Some("lonely".to_string()),
        ..IngestionConfig::new(&w.env.bucket)
    })
    .await
    .unwrap();

    let slot = postgres::slot_name(&source.id);
    let instance = std::cell::RefCell::new(instance);
    let instance = &instance;
    wait::stays(
        "no capture without coordination",
        Duration::from_secs(6),
        || async {
            if postgres::replication_slots().await.contains(&slot) {
                Err("a slot was created".to_string())
            } else if !instance.borrow_mut().is_running() {
                Err(format!(
                    "ingestion exited:\n{}",
                    instance.borrow().log_tail(20)
                ))
            } else {
                Ok(())
            }
        },
    )
    .await;
}

// ---- without Redis: static partitioning ----

/// Which of `count` shards owns `source` (the rule `INGESTION_SHARD_*` documents).
fn shard_of(source: &DataSourceView, count: u128) -> u32 {
    (Uuid::parse_str(&source.id).unwrap().as_u128() % count) as u32
}

#[rstest]
#[tokio::test]
async fn a_shard_captures_only_the_sources_it_owns(#[future] own_env: TestEnv) {
    let w = world(own_env.await).await;
    let source = w.source().await;
    let owner = shard_of(&source, 2);
    let stranger = 1 - owner;
    let shard = |index| IngestionConfig {
        shard: Some((index, 2)),
        ..IngestionConfig::new(&w.env.bucket)
    };

    // The shard that does not own the source leaves it alone, however long it runs.
    let wrong = std::cell::RefCell::new(Ingestion::start(shard(stranger)).await.unwrap());
    let wrong = &wrong;
    let slot = postgres::slot_name(&source.id);
    wait::stays(
        "the wrong shard staying away",
        Duration::from_secs(6),
        || async {
            if postgres::replication_slots().await.contains(&slot) {
                Err("it opened the slot".to_string())
            } else if !wrong.borrow_mut().is_running() {
                Err("it exited".to_string())
            } else {
                Ok(())
            }
        },
    )
    .await;

    // The owner picks it up.
    let _right = Ingestion::start(shard(owner)).await.unwrap();
    pipa_integration_test::pipeline::wait_for_slot(&source).await;
    w.db.execute("INSERT INTO orders VALUES (1, 'mine', 1)")
        .await;
    w.env
        .wait_for_rows(&w.project, &source, ORDERS, "the row", |rows| {
            rows.len() == 1
        })
        .await;
}

#[rstest]
#[tokio::test]
async fn a_single_instance_without_redis_captures_everything(#[future] own_env: TestEnv) {
    let w = world(own_env.await).await;
    let source = w.source().await;
    let _only = w.env.ingestion().await;
    pipa_integration_test::pipeline::wait_for_slot(&source).await;
    w.db.execute("INSERT INTO orders VALUES (1, 'x', 1)").await;
    w.env
        .wait_for_rows(&w.project, &source, ORDERS, "the row", |rows| {
            rows.len() == 1
        })
        .await;
}
