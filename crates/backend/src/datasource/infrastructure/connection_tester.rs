//! `sqlx`-backed adapter for verifying OLTP data source connectivity.

use std::time::Duration;

use async_trait::async_trait;
use sqlx::{Connection, Executor, mysql::MySqlConnectOptions, postgres::PgConnectOptions};

use crate::datasource::domain::{ConnectionTestOutcome, ConnectionTester, DataSource, DbEngine};

pub(super) const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Verifies data source connectivity by briefly connecting to the OLTP database and running
/// a trivial query, using the engine-appropriate `sqlx` driver.
pub struct SqlxConnectionTester;

#[async_trait]
impl ConnectionTester for SqlxConnectionTester {
    async fn test(&self, source: &DataSource) -> ConnectionTestOutcome {
        let result = match source.engine {
            DbEngine::Postgres => Self::test_postgres(source).await,
            DbEngine::MySql => Self::test_mysql(source).await,
        };

        match result {
            Ok(()) => ConnectionTestOutcome::Reachable,
            Err(reason) => ConnectionTestOutcome::Unreachable { reason },
        }
    }
}

impl SqlxConnectionTester {
    async fn test_postgres(source: &DataSource) -> Result<(), String> {
        let options = postgres_options(source);
        let mut conn =
            tokio::time::timeout(CONNECT_TIMEOUT, sqlx::PgConnection::connect_with(&options))
                .await
                .map_err(|_| "timed out connecting".to_string())?
                .map_err(|err| err.to_string())?;

        conn.execute("SELECT 1")
            .await
            .map_err(|err| err.to_string())?;
        Ok(())
    }

    async fn test_mysql(source: &DataSource) -> Result<(), String> {
        let options = mysql_options(source);
        let mut conn = tokio::time::timeout(
            CONNECT_TIMEOUT,
            sqlx::MySqlConnection::connect_with(&options),
        )
        .await
        .map_err(|_| "timed out connecting".to_string())?
        .map_err(|err| err.to_string())?;

        conn.execute("SELECT 1")
            .await
            .map_err(|err| err.to_string())?;
        Ok(())
    }
}

pub(super) fn postgres_options(source: &DataSource) -> PgConnectOptions {
    PgConnectOptions::new()
        .host(&source.connection.host)
        .port(source.connection.port)
        .username(&source.connection.username)
        .password(&source.connection.password)
        .database(&source.connection.database)
}

pub(super) fn mysql_options(source: &DataSource) -> MySqlConnectOptions {
    MySqlConnectOptions::new()
        .host(&source.connection.host)
        .port(source.connection.port)
        .username(&source.connection.username)
        .password(&source.connection.password)
        .database(&source.connection.database)
}
