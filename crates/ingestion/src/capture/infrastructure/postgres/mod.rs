//! Postgres adapter for [`CdcSource`]: streams row changes via logical replication (WAL),
//! decoded with [`decode`].

mod decode;

use std::collections::HashMap;

use async_trait::async_trait;
use decode::{
    DecodedChange, RelationInfo, decode_delete, decode_insert, decode_relation, decode_update,
};
use pgwire_replication::{Lsn, ReplicationClient, ReplicationConfig, ReplicationEvent};
use sqlx::{Connection, PgConnection, postgres::PgConnectOptions};
use tokio::sync::mpsc;

use crate::capture::domain::{CaptureError, CdcSource, ChangeEvent, Operation};
use crate::datasource::DataSource;

const CHANGE_CHANNEL_CAPACITY: usize = 256;

/// Streams changes out of a Postgres data source's write-ahead log via `pgoutput` logical
/// replication.
///
/// On first use for a given data source, provisions the replication objects it needs (a
/// `FOR ALL TABLES` publication and a logical replication slot, both named after the data
/// source's id) if they don't already exist — mirroring how most CDC connectors bootstrap
/// themselves rather than requiring the operator to run `CREATE PUBLICATION`/`pg_create_
/// logical_replication_slot` by hand.
pub struct PostgresWalSource;

impl PostgresWalSource {
    pub fn new() -> Self {
        Self
    }

    async fn ensure_replication_objects(
        &self,
        source: &DataSource,
        publication: &str,
        slot: &str,
    ) -> Result<(), CaptureError> {
        let options = PgConnectOptions::new()
            .host(&source.connection.host)
            .port(source.connection.port)
            .username(&source.connection.username)
            .password(&source.connection.password)
            .database(&source.connection.database);

        let mut conn = PgConnection::connect_with(&options)
            .await
            .map_err(|err| CaptureError::Connect(err.to_string()))?;

        // Identifiers can't be bind parameters; `publication` is built by us from a fixed
        // prefix + UUID hex (`replication_publication_name`), never from attacker input, so
        // asserting the dynamic SQL string is safe here is sound.
        let create_publication =
            sqlx::AssertSqlSafe(format!("CREATE PUBLICATION {publication} FOR ALL TABLES"));
        match sqlx::query(create_publication).execute(&mut conn).await {
            Ok(_) => {}
            Err(err) if is_duplicate_object(&err) => {}
            Err(err) => return Err(CaptureError::Setup(err.to_string())),
        }

        let slot_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_replication_slots WHERE slot_name = $1)",
        )
        .bind(slot)
        .fetch_one(&mut conn)
        .await
        .map_err(|err| CaptureError::Setup(err.to_string()))?;

        if !slot_exists {
            sqlx::query("SELECT pg_create_logical_replication_slot($1, 'pgoutput')")
                .bind(slot)
                .execute(&mut conn)
                .await
                .map_err(|err| CaptureError::Setup(err.to_string()))?;
        }

        Ok(())
    }
}

impl Default for PostgresWalSource {
    fn default() -> Self {
        Self::new()
    }
}

fn is_duplicate_object(err: &sqlx::Error) -> bool {
    // SQLSTATE 42710 (duplicate_object) — covers both an existing publication and an
    // existing replication slot.
    err.as_database_error()
        .and_then(|db_err| db_err.code())
        .as_deref()
        == Some("42710")
}

fn replication_slot_name(source: &DataSource) -> String {
    format!("pipa_cdc_{}", source.id.0.simple())
}

fn replication_publication_name(source: &DataSource) -> String {
    format!("pipa_cdc_{}", source.id.0.simple())
}

#[async_trait]
impl CdcSource for PostgresWalSource {
    async fn stream_changes(
        &self,
        source: &DataSource,
    ) -> Result<mpsc::Receiver<Result<ChangeEvent, CaptureError>>, CaptureError> {
        let slot = replication_slot_name(source);
        let publication = replication_publication_name(source);

        self.ensure_replication_objects(source, &publication, &slot)
            .await?;

        let config = ReplicationConfig::new(
            source.connection.host.clone(),
            source.connection.username.clone(),
            source.connection.password.clone(),
            source.connection.database.clone(),
            slot,
            publication,
        )
        .with_port(source.connection.port);

        let mut client = ReplicationClient::connect(config)
            .await
            .map_err(|err| CaptureError::Connect(err.to_string()))?;

        let (tx, rx) = mpsc::channel(CHANGE_CHANNEL_CAPACITY);

        tokio::spawn(async move {
            let mut relations: HashMap<u32, RelationInfo> = HashMap::new();
            let mut commit_timestamp_unix_micros = 0i64;

            loop {
                let event = match client.recv().await {
                    Ok(Some(event)) => event,
                    Ok(None) => break,
                    Err(err) => {
                        let _ = tx.send(Err(CaptureError::Stream(err.to_string()))).await;
                        break;
                    }
                };

                match event {
                    ReplicationEvent::Begin {
                        commit_time_micros, ..
                    } => {
                        commit_timestamp_unix_micros = commit_time_micros;
                    }
                    ReplicationEvent::Commit { end_lsn, .. } => {
                        client.update_applied_lsn(end_lsn);
                    }
                    ReplicationEvent::KeepAlive { wal_end, .. } => {
                        client.update_applied_lsn(wal_end);
                    }
                    ReplicationEvent::XLogData { wal_end, data, .. } => {
                        if data.is_empty() {
                            continue;
                        }
                        let tag = data[0];
                        let payload = &data[1..];

                        let decoded: Option<Result<DecodedChange, decode::DecodeError>> = match tag
                        {
                            b'R' => match decode_relation(payload) {
                                Ok((oid, info)) => {
                                    relations.insert(oid, info);
                                    None
                                }
                                Err(err) => Some(Err(err)),
                            },
                            b'I' => Some(decode_insert(payload, &relations)),
                            b'U' => Some(decode_update(payload, &relations)),
                            b'D' => Some(decode_delete(payload, &relations)),
                            // Truncate, Origin, Type, and streaming-transaction markers
                            // aren't captured in this MVP.
                            _ => None,
                        };

                        if let Some(decoded) = decoded {
                            let outcome = decoded
                                .map_err(|err| CaptureError::Decode(err.to_string()))
                                .map(|change| {
                                    to_change_event(
                                        change,
                                        &relations,
                                        wal_end,
                                        commit_timestamp_unix_micros,
                                    )
                                });

                            let sent = match outcome {
                                Ok(Some(event)) => tx.send(Ok(event)).await,
                                Ok(None) => Ok(()),
                                Err(err) => tx.send(Err(err)).await,
                            };
                            if sent.is_err() {
                                break;
                            }
                        }

                        client.update_applied_lsn(wal_end);
                    }
                    ReplicationEvent::Message { .. } => {}
                    ReplicationEvent::StoppedAt { .. } => break,
                }
            }

            let _ = client.shutdown().await;
        });

        Ok(rx)
    }
}

fn to_change_event(
    change: DecodedChange,
    relations: &HashMap<u32, RelationInfo>,
    wal_end: Lsn,
    commit_timestamp_unix_micros: i64,
) -> Option<ChangeEvent> {
    let (relation_oid, operation) = match change {
        DecodedChange::Insert {
            relation_oid,
            after,
        } => (relation_oid, Operation::Insert { after }),
        DecodedChange::Update {
            relation_oid,
            before,
            after,
        } => (relation_oid, Operation::Update { before, after }),
        DecodedChange::Delete {
            relation_oid,
            before,
        } => (relation_oid, Operation::Delete { before }),
    };

    let relation = relations.get(&relation_oid)?;

    Some(ChangeEvent {
        schema: relation.namespace.clone(),
        table: relation.name.clone(),
        operation,
        position: wal_end.to_string(),
        commit_timestamp_unix_micros,
    })
}

#[cfg(test)]
mod live_tests {
    use std::time::Duration;

    use uuid::Uuid;

    use super::*;
    use crate::capture::domain::Operation;
    use crate::datasource::{ConnectionConfig, DataSourceId, DbEngine};

    /// Exercises the full stack — slot/publication bootstrap, replication connect,
    /// pgoutput decoding — against a live Postgres with logical replication enabled.
    ///
    /// Ignored by default: it needs a reachable Postgres started with
    /// `-c wal_level=logical -c max_replication_slots=4 -c max_wal_senders=4` and a
    /// `testdb` database containing `CREATE TABLE orders (id int primary key, total numeric)`.
    /// Run explicitly with:
    /// `cargo test -p pipa-ingestion --lib -- --ignored postgres_wal_source_streams_real_changes`
    #[tokio::test]
    #[ignore]
    async fn postgres_wal_source_streams_real_changes() {
        let source = DataSource {
            id: DataSourceId(Uuid::new_v4()),
            name: "smoke-test".to_string(),
            engine: DbEngine::Postgres,
            connection: ConnectionConfig {
                host: std::env::var("PIPA_TEST_PG_HOST")
                    .unwrap_or_else(|_| "127.0.0.1".to_string()),
                port: std::env::var("PIPA_TEST_PG_PORT")
                    .ok()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(55432),
                username: "postgres".to_string(),
                password: "postgres".to_string(),
                database: "testdb".to_string(),
            },
        };

        let cdc_source = PostgresWalSource::new();
        let mut changes = cdc_source
            .stream_changes(&source)
            .await
            .expect("stream_changes should establish the replication connection");

        // Give the replication slot a moment to fully attach before writing.
        tokio::time::sleep(Duration::from_millis(500)).await;

        let options = PgConnectOptions::new()
            .host(&source.connection.host)
            .port(source.connection.port)
            .username(&source.connection.username)
            .password(&source.connection.password)
            .database(&source.connection.database);
        let mut conn = PgConnection::connect_with(&options)
            .await
            .expect("connect for writes");

        sqlx::query("INSERT INTO orders (id, total) VALUES (1, 9.99)")
            .execute(&mut conn)
            .await
            .expect("insert");
        sqlx::query("UPDATE orders SET total = 19.99 WHERE id = 1")
            .execute(&mut conn)
            .await
            .expect("update");
        sqlx::query("DELETE FROM orders WHERE id = 1")
            .execute(&mut conn)
            .await
            .expect("delete");

        let mut seen = Vec::new();
        for _ in 0..3 {
            let event = tokio::time::timeout(Duration::from_secs(10), changes.recv())
                .await
                .expect("timed out waiting for a change event")
                .expect("change channel closed unexpectedly")
                .expect("capture reported an error");
            seen.push(event);
        }

        assert_eq!(seen[0].table, "orders");
        assert_eq!(seen[0].schema, "public");
        assert!(matches!(seen[0].operation, Operation::Insert { .. }));
        assert!(matches!(seen[1].operation, Operation::Update { .. }));
        assert!(matches!(seen[2].operation, Operation::Delete { .. }));
    }
}
