//! The whole path a change takes, set up: a project with a Postgres data source, its tables
//! chosen, and an ingestion instance capturing them into Iceberg.

use pipa_api::{DataSourceView, ProjectView};
use serde_json::Value;

use crate::env::{TestEnv, unique};
use crate::ingestion::{Ingestion, IngestionConfig};
use crate::postgres::{self, Database};
use crate::wait;

/// The schema most CDC tests use.
pub const SCHEMA: &str = "
    CREATE TABLE orders (id integer PRIMARY KEY, status text, total numeric(10, 2));
    CREATE TABLE customers (id integer PRIMARY KEY, email text NOT NULL);
";

/// The Iceberg table `public.orders` lands in.
pub const ORDERS: &str = "public__orders";

/// The backend a [`Pipeline`] runs against: its own, or the shared one.
pub enum EnvRef {
    Owned(Box<TestEnv>),
    Shared(&'static TestEnv),
}

impl std::ops::Deref for EnvRef {
    type Target = TestEnv;

    fn deref(&self) -> &TestEnv {
        match self {
            EnvRef::Owned(env) => env,
            EnvRef::Shared(env) => env,
        }
    }
}

pub struct Pipeline {
    pub env: EnvRef,
    pub project: ProjectView,
    pub db: Database,
    pub source: DataSourceView,
    ingestion: Option<Ingestion>,
}

impl Pipeline {
    /// A pipeline of its own, backend and ingestion included: for tests that stop, crash or
    /// restart ingestion or that need data nobody else touches. [`SCHEMA`], with `public.orders`
    /// chosen for ingestion.
    pub async fn start(env: TestEnv) -> Self {
        Self::start_with(env, SCHEMA, &[("public", "orders")]).await
    }

    /// Like [`Self::start`], with a schema and tables of your own.
    pub async fn start_with(env: TestEnv, schema_sql: &str, tables: &[(&str, &str)]) -> Self {
        let ingestion = env.ingestion().await;
        Self::assemble(
            EnvRef::Owned(Box::new(env)),
            schema_sql,
            tables,
            Some(ingestion),
        )
        .await
    }

    /// Like [`Self::start`], with the ingestion instance configured by `configure`.
    pub async fn start_configured(
        env: TestEnv,
        configure: impl FnOnce(&mut IngestionConfig),
    ) -> Self {
        let ingestion = env.ingestion_with(configure).await;
        Self::assemble(
            EnvRef::Owned(Box::new(env)),
            SCHEMA,
            &[("public", "orders")],
            Some(ingestion),
        )
        .await
    }

    /// A project, database and source of its own on the shared stack, captured by the shared
    /// ingestion instance. [`SCHEMA`], with `public.orders` chosen for ingestion.
    pub async fn shared() -> Self {
        Self::shared_with(SCHEMA, &[("public", "orders")]).await
    }

    /// Like [`Self::shared`], with a schema and tables of your own.
    pub async fn shared_with(schema_sql: &str, tables: &[(&str, &str)]) -> Self {
        let env = EnvRef::Shared(crate::shared::env_with_ingestion());
        Self::assemble(env, schema_sql, tables, None).await
    }

    /// Creates `schema_sql` in a new database, registers it in a new project, chooses `tables`,
    /// and waits until ingestion has attached to the source (changes made before that are not
    /// captured: a new replication slot starts from where it is created).
    async fn assemble(
        env: EnvRef,
        schema_sql: &str,
        tables: &[(&str, &str)],
        ingestion: Option<Ingestion>,
    ) -> Self {
        let db = Database::create().await;
        db.execute(schema_sql).await;
        let project = env.unique_project().await;
        let source = env.source(Some(&project), &unique("source"), &db).await;
        env.ingest(&source, tables).await;
        wait_for_slot(&source).await;
        Self {
            env,
            project,
            db,
            source,
            ingestion,
        }
    }

    /// The ingestion instance of a pipeline that has one of its own.
    pub fn ingestion(&mut self) -> &mut Ingestion {
        self.ingestion
            .as_mut()
            .expect("this pipeline runs on the shared ingestion instance")
    }

    /// Replaces the pipeline's own ingestion instance (after stopping or crashing it).
    pub fn set_ingestion(&mut self, ingestion: Ingestion) {
        self.ingestion = Some(ingestion);
    }

    /// The current rows of `table`, once `done` holds for them.
    pub async fn rows_when(
        &self,
        table: &str,
        what: &str,
        done: impl Fn(&[Value]) -> bool,
    ) -> Vec<Value> {
        self.env
            .wait_for_rows(&self.project, &self.source, table, what, done)
            .await
    }

    /// `SELECT … FROM <this source's namespace>.<table>`: the changelog, via `POST /query`.
    pub fn changelog_sql(&self, columns: &str, table: &str) -> String {
        format!(
            "SELECT {columns} FROM pipa.{}.\"{table}\"",
            crate::env::namespace(&self.source)
        )
    }
}

/// Waits until `pipa-ingestion` is capturing `source`: it has created the replication slot, and
/// has had time to apply the tables chosen for it.
///
/// A table choice reaches a running capture on ingestion's next refresh of the registered sources
/// (every second in tests), and changes to a table not yet chosen are dropped as they arrive. So
/// a change made right after the slot appears could be lost if the choice was made a moment
/// after ingestion first read the source. One refresh interval later it cannot be.
pub async fn wait_for_slot(source: &DataSourceView) {
    let slot = postgres::slot_name(&source.id);
    wait::eventually(&format!("the replication slot {slot}"), || async {
        let slots = postgres::replication_slots().await;
        if slots.contains(&slot) {
            Ok(())
        } else {
            Err(format!("{slots:?}"))
        }
    })
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
}

/// `(id, status)` of every row of an `orders` result, sorted by id.
pub fn statuses(rows: &[Value]) -> Vec<(i64, String)> {
    let mut found: Vec<(i64, String)> = rows
        .iter()
        .map(|row| {
            (
                row["id"].as_i64().expect("an integer id"),
                row["status"].as_str().unwrap_or("<null>").to_string(),
            )
        })
        .collect();
    found.sort();
    found
}
