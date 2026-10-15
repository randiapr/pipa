//! A Postgres database of a test's own, on the shared server: the OLTP source whose changes
//! `pipa-ingestion` captures.

use pipa_api::ConnectionConfig;
use sqlx::{AssertSqlSafe, Connection, PgConnection, Row};
use uuid::Uuid;

use crate::infra::{self, PG_PASSWORD, PG_USER};

pub struct Database {
    pub name: String,
}

impl Database {
    /// Creates an empty database with a name of its own.
    pub async fn create() -> Self {
        let name = format!("t_{}", Uuid::now_v7().simple());
        let mut admin = PgConnection::connect(&infra::admin_url(infra::endpoints()))
            .await
            .expect("connecting to the Postgres maintenance database");
        sqlx::query(AssertSqlSafe(format!("CREATE DATABASE \"{name}\"")))
            .execute(&mut admin)
            .await
            .expect("creating the test database");
        Self { name }
    }

    /// The parameters a client registers this database as a data source with.
    pub fn connection(&self) -> ConnectionConfig {
        let endpoints = infra::endpoints();
        ConnectionConfig {
            host: endpoints.postgres_host.clone(),
            port: endpoints.postgres_port,
            username: PG_USER.to_string(),
            password: PG_PASSWORD.to_string(),
            database: self.name.clone(),
        }
    }

    async fn connect(&self) -> PgConnection {
        let endpoints = infra::endpoints();
        PgConnection::connect(&format!(
            "postgres://{PG_USER}:{PG_PASSWORD}@{}:{}/{}",
            endpoints.postgres_host, endpoints.postgres_port, self.name
        ))
        .await
        .expect("connecting to the test database")
    }

    /// Runs one or more `;`-separated statements, in one transaction if there are several.
    pub async fn execute(&self, sql: &str) {
        let mut connection = self.connect().await;
        sqlx::raw_sql(AssertSqlSafe(sql.to_string()))
            .execute(&mut connection)
            .await
            .unwrap_or_else(|err| panic!("{sql}: {err}"));
    }

    /// The first column of every row of `sql`, as text.
    pub async fn strings(&self, sql: &str) -> Vec<String> {
        let mut connection = self.connect().await;
        sqlx::query(AssertSqlSafe(sql.to_string()))
            .fetch_all(&mut connection)
            .await
            .unwrap_or_else(|err| panic!("{sql}: {err}"))
            .iter()
            .map(|row| row.get::<String, _>(0))
            .collect()
    }
}

/// Names of the replication slots on the server, from the maintenance database (slots are
/// server-wide in `pg_replication_slots`).
pub async fn replication_slots() -> Vec<String> {
    let mut admin = PgConnection::connect(&infra::admin_url(infra::endpoints()))
        .await
        .expect("connecting to the Postgres maintenance database");
    sqlx::query("SELECT slot_name::text FROM pg_replication_slots")
        .fetch_all(&mut admin)
        .await
        .expect("listing replication slots")
        .iter()
        .map(|row| row.get::<String, _>(0))
        .collect()
}

/// The replication slot `pipa-ingestion` keeps for a data source.
pub fn slot_name(source_id: &str) -> String {
    format!("pipa_cdc_{}", source_id.replace('-', ""))
}
