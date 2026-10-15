//! `/datasources`: registering OLTP sources, testing the connection, exploring their tables and
//! choosing what to ingest, and who may do any of it.

use pipa_api::{
    ConnectionConfig, ConnectionTest, ConnectionTestOutcome, DataSourceData, DataSources, DbEngine,
    IngestedTablesUpdate, NewDataSource, SourceTableRef, SourceTables, path,
};
use pipa_integration_test::env::{TestEnv, new_source, unique};
use pipa_integration_test::postgres::Database;
use rstest::rstest;
use serde_json::json;

use crate::common::{env, own_env};

const UNKNOWN_ID: &str = "0190a1b2-0000-7000-8000-000000000000";

const SCHEMA: &str = "
    CREATE TABLE customers (id serial PRIMARY KEY, email text NOT NULL, nickname text);
    CREATE TABLE orders (
        id integer PRIMARY KEY,
        customer_id integer NOT NULL REFERENCES customers (id),
        total numeric(10, 2)
    );
    CREATE SCHEMA audit;
    CREATE TABLE audit.events (kind text, at timestamptz);
";

async fn database() -> Database {
    let database = Database::create().await;
    database.execute(SCHEMA).await;
    database
}

// ---- registering ----

#[rstest]
#[tokio::test]
async fn a_data_source_is_registered_in_a_project(env: &'static TestEnv) {
    let project = env.unique_project().await;
    let database = Database::create().await;
    let name = unique("orders-db");

    let created: DataSourceData = env
        .backend
        .post(path::DATASOURCES)
        .bearer(&env.admin)
        .json(&new_source(Some(&project), &name, &database))
        .send()
        .await
        .created()
        .parse();
    let source = created.datasource;
    assert_eq!(source.name, name);
    assert_eq!(source.engine, DbEngine::Postgres);
    assert_eq!(source.project_id.as_deref(), Some(project.id.as_str()));
    assert_eq!(source.connection.database, database.name);
    assert!(source.registered_at_unix > 0);
    // Nothing is captured until tables are chosen.
    assert!(source.ingested_tables.is_empty());

    let fetched: DataSourceData = env
        .backend
        .get(&path::datasource(&source.id))
        .bearer(&env.admin)
        .send()
        .await
        .ok()
        .parse();
    assert_eq!(fetched.datasource.id, source.id);
}

#[rstest]
#[tokio::test]
async fn an_admin_can_register_a_source_without_a_project(env: &'static TestEnv) {
    let database = Database::create().await;
    let source = env.source(None, &unique("loose"), &database).await;
    assert_eq!(source.project_id, None);
}

#[rstest]
#[tokio::test]
async fn a_mysql_source_can_be_registered(env: &'static TestEnv) {
    let project = env.unique_project().await;
    let created: DataSourceData = env
        .backend
        .post(path::DATASOURCES)
        .bearer(&env.admin)
        .json(&NewDataSource {
            name: unique("mysql"),
            engine: DbEngine::MySql,
            connection: ConnectionConfig {
                host: "127.0.0.1".to_string(),
                port: 3306,
                username: "root".to_string(),
                password: "secret".to_string(),
                database: "shop".to_string(),
            },
            project_id: Some(project.id.clone()),
        })
        .send()
        .await
        .created()
        .parse();
    assert_eq!(created.datasource.engine, DbEngine::MySql);
}

#[rstest]
#[tokio::test]
async fn sources_are_listed_and_filtered_by_project(env: &'static TestEnv) {
    let first = env.unique_project().await;
    let second = env.unique_project().await;
    let database = Database::create().await;
    let in_first = env.source(Some(&first), &unique("a"), &database).await;
    let in_second = env.source(Some(&second), &unique("b"), &database).await;

    let all: DataSources = env
        .backend
        .get(path::DATASOURCES)
        .bearer(&env.admin)
        .send()
        .await
        .ok()
        .parse();
    let ids: Vec<_> = all
        .datasources
        .iter()
        .map(|source| source.id.as_str())
        .collect();
    assert!(ids.contains(&in_first.id.as_str()) && ids.contains(&in_second.id.as_str()));

    let only_first: DataSources = env
        .backend
        .get(&format!("{}?project_id={}", path::DATASOURCES, first.id))
        .bearer(&env.admin)
        .send()
        .await
        .ok()
        .parse();
    let ids: Vec<_> = only_first
        .datasources
        .iter()
        .map(|source| source.id.as_str())
        .collect();
    assert_eq!(ids, vec![in_first.id.as_str()]);
}

#[rstest]
#[tokio::test]
async fn a_deleted_source_is_gone(#[future] own_env: TestEnv) {
    let env = own_env.await;
    let project = env.unique_project().await;
    let source = env
        .source(Some(&project), &unique("doomed"), &Database::create().await)
        .await;
    env.backend
        .delete(&path::datasource(&source.id))
        .bearer(&env.admin)
        .send()
        .await
        .deleted();
    env.backend
        .get(&path::datasource(&source.id))
        .bearer(&env.admin)
        .send()
        .await
        .not_found();
    // Deleting again is a not-found, unlike a project.
    env.backend
        .delete(&path::datasource(&source.id))
        .bearer(&env.admin)
        .send()
        .await
        .not_found();
}

// ---- registering: validation ----

fn source_with(database: &Database, change: impl FnOnce(&mut NewDataSource)) -> NewDataSource {
    let mut source = new_source(None, &unique("invalid"), database);
    change(&mut source);
    source
}

#[rstest]
#[case::empty_name(|s: &mut NewDataSource| s.name = String::new())]
#[case::blank_name(|s: &mut NewDataSource| s.name = "  ".to_string())]
#[case::empty_host(|s: &mut NewDataSource| s.connection.host = String::new())]
#[case::empty_username(|s: &mut NewDataSource| s.connection.username = String::new())]
#[case::empty_database(|s: &mut NewDataSource| s.connection.database = String::new())]
#[case::port_zero(|s: &mut NewDataSource| s.connection.port = 0)]
#[tokio::test]
async fn an_invalid_data_source_is_rejected(
    env: &'static TestEnv,
    #[case] change: fn(&mut NewDataSource),
) {
    let database = Database::create().await;
    env.backend
        .post(path::DATASOURCES)
        .bearer(&env.admin)
        .json(&source_with(&database, change))
        .send()
        .await
        .bad_request();
}

#[rstest]
#[case::unknown_engine(json!({"engine": "oracle"}))]
#[case::port_out_of_range(json!({"connection": {"host": "h", "port": 70000, "username": "u", "password": "p", "database": "d"}}))]
#[case::negative_port(json!({"connection": {"host": "h", "port": -1, "username": "u", "password": "p", "database": "d"}}))]
#[case::missing_connection(json!({"connection": null}))]
#[case::project_id_not_a_uuid(json!({"project_id": "not-a-uuid"}))]
#[tokio::test]
async fn a_malformed_data_source_is_rejected(
    env: &'static TestEnv,
    #[case] overrides: serde_json::Value,
) {
    let mut body = json!({
        "name": unique("malformed"),
        "engine": "postgres",
        "connection": {"host": "h", "port": 5432, "username": "u", "password": "p", "database": "d"},
    });
    for (key, value) in overrides.as_object().unwrap() {
        body[key] = value.clone();
    }
    let reply = env
        .backend
        .post(path::DATASOURCES)
        .bearer(&env.admin)
        .json(&body)
        .send()
        .await;
    assert!(
        matches!(reply.status, 400 | 422),
        "{} {}",
        reply.status,
        reply.body
    );
}

#[rstest]
#[case::get("GET", "")]
#[case::delete("DELETE", "")]
#[case::test("POST", "/test")]
#[case::tables("GET", "/tables")]
#[case::choose("PUT", "/tables")]
#[tokio::test]
async fn an_unknown_source_is_not_found(
    env: &'static TestEnv,
    #[case] method: &str,
    #[case] suffix: &str,
) {
    env.backend
        .request(
            method.parse().unwrap(),
            &format!("{}{suffix}", path::datasource(UNKNOWN_ID)),
        )
        .bearer(&env.admin)
        .json(&IngestedTablesUpdate { tables: vec![] })
        .send()
        .await
        .not_found();
}

#[rstest]
#[tokio::test]
async fn a_source_id_that_is_not_a_uuid_is_rejected(env: &'static TestEnv) {
    let reply = env
        .backend
        .get(&path::datasource("not-a-uuid"))
        .bearer(&env.admin)
        .send()
        .await;
    assert_eq!(reply.status, 400, "{}", reply.body);
}

// ---- testing the connection ----

#[rstest]
#[tokio::test]
async fn a_reachable_source_tests_as_reachable(env: &'static TestEnv) {
    let project = env.unique_project().await;
    let source = env
        .source(Some(&project), &unique("up"), &database().await)
        .await;
    let outcome: ConnectionTest = env
        .backend
        .post(&path::datasource_test(&source.id))
        .bearer(&env.admin)
        .send()
        .await
        .ok()
        .parse();
    assert_eq!(outcome.connection_test, ConnectionTestOutcome::Reachable);
}

#[rstest]
#[case::wrong_password(|c: &mut ConnectionConfig| c.password = "wrong".to_string())]
#[case::unknown_database(|c: &mut ConnectionConfig| c.database = "no_such_database".to_string())]
#[case::unknown_user(|c: &mut ConnectionConfig| c.username = "nobody".to_string())]
#[case::nothing_listening(|c: &mut ConnectionConfig| c.port = 1)]
#[tokio::test]
async fn an_unreachable_source_tests_as_unreachable_with_a_reason(
    env: &'static TestEnv,
    #[case] break_it: fn(&mut ConnectionConfig),
) {
    let project = env.unique_project().await;
    let database = Database::create().await;
    let mut registration = new_source(Some(&project), &unique("down"), &database);
    break_it(&mut registration.connection);
    // Registering never connects, so a broken source can still be registered...
    let source = env
        .backend
        .post(path::DATASOURCES)
        .bearer(&env.admin)
        .json(&registration)
        .send()
        .await
        .created()
        .parse::<DataSourceData>()
        .datasource;

    // ...and the test is a successful request that reports the failure.
    let outcome: ConnectionTest = env
        .backend
        .post(&path::datasource_test(&source.id))
        .bearer(&env.admin)
        .send()
        .await
        .ok()
        .parse();
    match outcome.connection_test {
        ConnectionTestOutcome::Unreachable { reason } => assert!(!reason.is_empty()),
        ConnectionTestOutcome::Reachable => panic!("a broken source tested as reachable"),
    }
}

// ---- exploring tables ----

#[rstest]
#[tokio::test]
async fn the_explorer_lists_tables_columns_keys_and_what_is_ingested(env: &'static TestEnv) {
    let project = env.unique_project().await;
    let source = env
        .source(Some(&project), &unique("explore"), &database().await)
        .await;
    env.ingest(&source, &[("public", "orders")]).await;

    let tables: SourceTables = env
        .backend
        .get(&path::datasource_tables(&source.id))
        .bearer(&env.admin)
        .send()
        .await
        .ok()
        .parse();
    let find = |schema: &str, name: &str| {
        tables
            .tables
            .iter()
            .find(|table| table.schema == schema && table.name == name)
            .unwrap_or_else(|| panic!("{schema}.{name} not listed: {:?}", tables.tables))
    };

    let orders = find("public", "orders");
    assert!(orders.ingested);
    let id = orders.columns.iter().find(|c| c.name == "id").unwrap();
    assert!(id.primary_key && !id.nullable);
    let customer = orders
        .columns
        .iter()
        .find(|c| c.name == "customer_id")
        .unwrap();
    let reference = customer.foreign_key.as_ref().expect("a foreign key");
    assert_eq!(
        (
            reference.schema.as_str(),
            reference.table.as_str(),
            reference.column.as_str()
        ),
        ("public", "customers", "id")
    );
    let total = orders.columns.iter().find(|c| c.name == "total").unwrap();
    assert!(total.nullable && !total.primary_key);
    assert!(total.data_type.contains("numeric"), "{}", total.data_type);

    let customers = find("public", "customers");
    assert!(!customers.ingested);
    assert!(
        customers
            .columns
            .iter()
            .any(|c| c.name == "nickname" && c.nullable)
    );

    // Another schema is listed too; system schemas are not.
    assert!(!find("audit", "events").ingested);
    assert!(
        tables
            .tables
            .iter()
            .all(|table| table.schema != "pg_catalog" && table.schema != "information_schema")
    );
}

#[rstest]
#[tokio::test]
async fn the_explorer_of_an_unreachable_source_is_a_bad_gateway(env: &'static TestEnv) {
    let project = env.unique_project().await;
    let mut registration = new_source(Some(&project), &unique("down"), &Database::create().await);
    registration.connection.password = "wrong".to_string();
    let source = env
        .backend
        .post(path::DATASOURCES)
        .bearer(&env.admin)
        .json(&registration)
        .send()
        .await
        .created()
        .parse::<DataSourceData>()
        .datasource;
    env.backend
        .get(&path::datasource_tables(&source.id))
        .bearer(&env.admin)
        .send()
        .await
        .bad_gateway();
}

// ---- choosing tables ----

#[rstest]
#[tokio::test]
async fn the_ingested_tables_are_replaced_as_a_whole(env: &'static TestEnv) {
    let project = env.unique_project().await;
    let source = env
        .source(Some(&project), &unique("choice"), &database().await)
        .await;
    let choose = |tables: &[(&str, &str)]| {
        let tables = tables
            .iter()
            .map(|(schema, name)| SourceTableRef {
                schema: schema.to_string(),
                name: name.to_string(),
            })
            .collect();
        env.backend
            .put(&path::datasource_tables(&source.id))
            .bearer(&env.admin)
            .json(&IngestedTablesUpdate { tables })
            .send()
    };

    let chosen: DataSourceData = choose(&[("public", "orders"), ("audit", "events")])
        .await
        .ok()
        .parse();
    let names: Vec<_> = chosen
        .datasource
        .ingested_tables
        .iter()
        .map(|table| format!("{}.{}", table.schema, table.name))
        .collect();
    assert_eq!(names, vec!["audit.events", "public.orders"]);

    // A later choice replaces the earlier one; duplicates collapse.
    let chosen: DataSourceData = choose(&[("public", "customers"), ("public", "customers")])
        .await
        .ok()
        .parse();
    assert_eq!(chosen.datasource.ingested_tables.len(), 1);
    assert_eq!(chosen.datasource.ingested_tables[0].name, "customers");

    // The choice is stored, and an empty one stops ingesting everything.
    let fetched: DataSourceData = env
        .backend
        .get(&path::datasource(&source.id))
        .bearer(&env.admin)
        .send()
        .await
        .ok()
        .parse();
    assert_eq!(fetched.datasource.ingested_tables.len(), 1);
    let none: DataSourceData = choose(&[]).await.ok().parse();
    assert!(none.datasource.ingested_tables.is_empty());
}

#[rstest]
#[case::empty_schema("", "orders")]
#[case::empty_name("public", "")]
#[case::blank_name("public", "  ")]
#[tokio::test]
async fn a_table_without_a_schema_or_name_is_rejected(
    env: &'static TestEnv,
    #[case] schema: &str,
    #[case] name: &str,
) {
    let project = env.unique_project().await;
    let source = env
        .source(Some(&project), &unique("choice"), &database().await)
        .await;
    env.ingest(&source, &[("public", "orders")]).await;
    env.backend
        .put(&path::datasource_tables(&source.id))
        .bearer(&env.admin)
        .json(&IngestedTablesUpdate {
            tables: vec![SourceTableRef {
                schema: schema.to_string(),
                name: name.to_string(),
            }],
        })
        .send()
        .await
        .bad_request();
    // The earlier choice stands.
    let fetched: DataSourceData = env
        .backend
        .get(&path::datasource(&source.id))
        .bearer(&env.admin)
        .send()
        .await
        .ok()
        .parse();
    assert_eq!(fetched.datasource.ingested_tables.len(), 1);
}

#[rstest]
#[tokio::test]
async fn a_table_choice_is_not_checked_against_the_database(env: &'static TestEnv) {
    let project = env.unique_project().await;
    let source = env
        .source(Some(&project), &unique("choice"), &database().await)
        .await;
    // It may not exist yet, or the source may be down just now.
    env.ingest(&source, &[("public", "not_there_yet")]).await;
}

#[rstest]
#[tokio::test]
async fn a_table_choice_needs_a_tables_list(env: &'static TestEnv) {
    let project = env.unique_project().await;
    let source = env
        .source(Some(&project), &unique("choice"), &database().await)
        .await;
    let reply = env
        .backend
        .put(&path::datasource_tables(&source.id))
        .bearer(&env.admin)
        .json(&json!({}))
        .send()
        .await;
    assert_eq!(reply.status, 422, "{}", reply.body);
}

// ---- who may do what ----

#[rstest]
#[tokio::test]
async fn a_viewer_cannot_touch_data_sources_at_all(env: &'static TestEnv) {
    let project = env.unique_project().await;
    let database = Database::create().await;
    let source = env
        .source(Some(&project), &unique("hidden"), &database)
        .await;
    let viewer = env.viewer(&[&project]).await;
    let token = &viewer.token;
    let one = path::datasource(&source.id);

    env.backend
        .get(path::DATASOURCES)
        .bearer(token)
        .send()
        .await
        .forbidden();
    env.backend
        .get(&format!("{}?project_id={}", path::DATASOURCES, project.id))
        .bearer(token)
        .send()
        .await
        .forbidden();
    env.backend
        .post(path::DATASOURCES)
        .bearer(token)
        .json(&new_source(Some(&project), &unique("by-viewer"), &database))
        .send()
        .await
        .forbidden();
    env.backend.get(&one).bearer(token).send().await.forbidden();
    env.backend
        .delete(&one)
        .bearer(token)
        .send()
        .await
        .forbidden();
    env.backend
        .post(&path::datasource_test(&source.id))
        .bearer(token)
        .send()
        .await
        .forbidden();
    env.backend
        .get(&path::datasource_tables(&source.id))
        .bearer(token)
        .send()
        .await
        .forbidden();
    env.backend
        .put(&path::datasource_tables(&source.id))
        .bearer(token)
        .json(&IngestedTablesUpdate { tables: vec![] })
        .send()
        .await
        .forbidden();

    // The source is still there.
    env.backend.get(&one).bearer(&env.admin).send().await.ok();
}

#[rstest]
#[tokio::test]
async fn a_developer_works_inside_its_own_projects(#[future] own_env: TestEnv) {
    let env = own_env.await;
    let project = env.unique_project().await;
    let developer = env.developer(&[&project]).await;
    let database = database().await;
    let token = &developer.token;

    let source: DataSourceData = env
        .backend
        .post(path::DATASOURCES)
        .bearer(token)
        .json(&new_source(Some(&project), &unique("dev-db"), &database))
        .send()
        .await
        .created()
        .parse();
    let id = source.datasource.id;

    env.backend
        .get(&path::datasource(&id))
        .bearer(token)
        .send()
        .await
        .ok();
    env.backend
        .post(&path::datasource_test(&id))
        .bearer(token)
        .send()
        .await
        .ok();
    env.backend
        .get(&path::datasource_tables(&id))
        .bearer(token)
        .send()
        .await
        .ok();
    env.backend
        .put(&path::datasource_tables(&id))
        .bearer(token)
        .json(&IngestedTablesUpdate { tables: vec![] })
        .send()
        .await
        .ok();
    env.backend
        .delete(&path::datasource(&id))
        .bearer(token)
        .send()
        .await
        .deleted();
}

#[rstest]
#[tokio::test]
async fn a_developer_cannot_reach_another_projects_sources(env: &'static TestEnv) {
    let mine = env.unique_project().await;
    let theirs = env.unique_project().await;
    let developer = env.developer(&[&mine]).await;
    let database = Database::create().await;
    let foreign = env
        .source(Some(&theirs), &unique("foreign"), &database)
        .await;
    let loose = env.source(None, &unique("loose"), &database).await;
    let token = &developer.token;

    for source in [&foreign, &loose] {
        env.backend
            .get(&path::datasource(&source.id))
            .bearer(token)
            .send()
            .await
            .forbidden();
        env.backend
            .delete(&path::datasource(&source.id))
            .bearer(token)
            .send()
            .await
            .forbidden();
        env.backend
            .post(&path::datasource_test(&source.id))
            .bearer(token)
            .send()
            .await
            .forbidden();
        env.backend
            .get(&path::datasource_tables(&source.id))
            .bearer(token)
            .send()
            .await
            .forbidden();
        env.backend
            .put(&path::datasource_tables(&source.id))
            .bearer(token)
            .json(&IngestedTablesUpdate { tables: vec![] })
            .send()
            .await
            .forbidden();
        // Still there.
        env.backend
            .get(&path::datasource(&source.id))
            .bearer(&env.admin)
            .send()
            .await
            .ok();
    }

    // Registering into someone else's project, or into none, is refused as well.
    for project in [Some(&theirs), None] {
        env.backend
            .post(path::DATASOURCES)
            .bearer(token)
            .json(&new_source(project, &unique("sneaky"), &database))
            .send()
            .await
            .forbidden();
    }
}

#[rstest]
#[tokio::test]
async fn a_developers_listing_holds_only_its_own_projects_sources(env: &'static TestEnv) {
    let mine = env.unique_project().await;
    let theirs = env.unique_project().await;
    let developer = env.developer(&[&mine]).await;
    let database = Database::create().await;
    let own = env.source(Some(&mine), &unique("own"), &database).await;
    env.source(Some(&theirs), &unique("foreign"), &database)
        .await;
    env.source(None, &unique("loose"), &database).await;

    let listed: DataSources = env
        .backend
        .get(path::DATASOURCES)
        .bearer(&developer.token)
        .send()
        .await
        .ok()
        .parse();
    let ids: Vec<_> = listed
        .datasources
        .iter()
        .map(|source| source.id.as_str())
        .collect();
    assert_eq!(ids, vec![own.id.as_str()]);

    // Asking for another project's sources by name is refused rather than answered with nothing.
    env.backend
        .get(&format!("{}?project_id={}", path::DATASOURCES, theirs.id))
        .bearer(&developer.token)
        .send()
        .await
        .forbidden();
}

#[rstest]
#[tokio::test]
async fn a_developer_loses_access_when_unassigned(env: &'static TestEnv) {
    let project = env.unique_project().await;
    let developer = env.developer(&[&project]).await;
    let source = env
        .source(Some(&project), &unique("db"), &Database::create().await)
        .await;
    env.backend
        .get(&path::datasource(&source.id))
        .bearer(&developer.token)
        .send()
        .await
        .ok();

    env.backend
        .put(&path::user(&developer.user.id))
        .bearer(&env.admin)
        .json(&pipa_api::UserUpdate {
            project_ids: Some(vec![]),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    env.backend
        .get(&path::datasource(&source.id))
        .bearer(&developer.token)
        .send()
        .await
        .forbidden();
}

/// Listing reads every source one by one; a source deleted meanwhile must not fail the list.
#[rstest]
#[tokio::test]
async fn listing_sources_while_others_are_deleted_never_fails(env: &'static TestEnv) {
    let project = env.unique_project().await;
    let database = Database::create().await;
    let mut sources = Vec::new();
    for _ in 0..30 {
        sources.push(
            env.source(Some(&project), &unique("doomed"), &database)
                .await,
        );
    }

    let deleters: Vec<_> = sources
        .into_iter()
        .map(|source| {
            tokio::spawn(async move {
                env.backend
                    .delete(&path::datasource(&source.id))
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
                .get(path::DATASOURCES)
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
