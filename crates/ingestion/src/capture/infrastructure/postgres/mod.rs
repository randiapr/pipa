// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! Postgres adapter for [`CdcSource`]: streams row changes via logical replication (WAL),
//! decoded with [`decode`].

mod decode;
mod snapshot;

use std::collections::HashMap;

use async_trait::async_trait;
use decode::{
    DecodedChange, RelationInfo, decode_delete, decode_insert, decode_relation, decode_update,
};
use pgwire_replication::{Lsn, ReplicationClient, ReplicationConfig, ReplicationEvent};
use sqlx::PgConnection;
use tokio::sync::mpsc;

use crate::capture::domain::{
    CaptureError, CdcSource, ChangeEvent, ChangeStream, Operation, StreamItem,
};
use crate::datasource::DataSource;

pub use snapshot::PostgresSnapshotSource;

const CHANGE_CHANNEL_CAPACITY: usize = 256;
const CONFIRM_CHANNEL_CAPACITY: usize = 16;

/// Microseconds from the Unix epoch (1970-01-01) to the Postgres epoch (2000-01-01), which is
/// what `pgoutput` timestamps such as a transaction's commit time count from.
const POSTGRES_EPOCH_UNIX_MICROS: i64 = 946_684_800_000_000;

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
        let mut conn = crate::postgres::connect(source)
            .await
            .map_err(|err| CaptureError::Connect(err.to_string()))?;
        ensure_publication(&mut conn, publication).await?;
        ensure_slot(&mut conn, slot).await
    }
}

/// Creates `publication` for all tables unless it exists.
async fn ensure_publication(
    conn: &mut PgConnection,
    publication: &str,
) -> Result<(), CaptureError> {
    // Identifiers can't be bind parameters; `publication` is built by us from a fixed
    // prefix + UUID hex (`replication_publication_name`), never from attacker input, so
    // asserting the dynamic SQL string is safe here is sound.
    let create_publication =
        sqlx::AssertSqlSafe(format!("CREATE PUBLICATION {publication} FOR ALL TABLES"));
    match sqlx::query(create_publication).execute(conn).await {
        Ok(_) => Ok(()),
        Err(err) if is_duplicate_object(&err) => Ok(()),
        Err(err) => Err(CaptureError::Setup(err.to_string())),
    }
}

/// Creates the `pgoutput` replication slot `slot` unless it exists.
async fn ensure_slot(conn: &mut PgConnection, slot: &str) -> Result<(), CaptureError> {
    let slot_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pg_replication_slots WHERE slot_name = $1)",
    )
    .bind(slot)
    .fetch_one(&mut *conn)
    .await
    .map_err(|err| CaptureError::Setup(err.to_string()))?;
    if !slot_exists {
        sqlx::query("SELECT pg_create_logical_replication_slot($1, 'pgoutput')")
            .bind(slot)
            .execute(conn)
            .await
            .map_err(|err| CaptureError::Setup(err.to_string()))?;
    }
    Ok(())
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
        resume_from: Option<&str>,
    ) -> Result<ChangeStream, CaptureError> {
        let slot = replication_slot_name(source);
        let publication = replication_publication_name(source);

        self.ensure_replication_objects(source, &publication, &slot)
            .await?;
        let client =
            start_replication(replication_config(source, slot, publication, resume_from)?).await?;

        let (tx, rx) = mpsc::channel(CHANGE_CHANNEL_CAPACITY);
        let (confirm_tx, confirm_rx) = mpsc::channel::<String>(CONFIRM_CHANNEL_CAPACITY);
        tokio::spawn(forward_stream(client, tx, confirm_rx));

        Ok(ChangeStream {
            events: rx,
            confirm: confirm_tx,
        })
    }
}

/// The replication connection settings for `source`'s `slot` and `publication`, starting at
/// `resume_from` when given (the slot's own position otherwise).
fn replication_config(
    source: &DataSource,
    slot: String,
    publication: String,
    resume_from: Option<&str>,
) -> Result<ReplicationConfig, CaptureError> {
    let config = ReplicationConfig::new(
        source.connection.host.clone(),
        source.connection.username.clone(),
        source.connection.password.clone(),
        source.connection.database.clone(),
        slot,
        publication,
    )
    .with_port(source.connection.port)
    .with_application_name("pipa-ingestion");

    let Some(resume_from) = resume_from else {
        return Ok(config);
    };
    let start_lsn = resume_from.parse::<Lsn>().map_err(|err| {
        CaptureError::Connect(format!("invalid resume position {resume_from:?}: {err}"))
    })?;
    Ok(config.with_start_lsn(start_lsn))
}

/// Connects for replication. `connect` returns before the server has accepted
/// `START_REPLICATION`; this waits for it, so a refusal (slot in use, missing privilege) fails
/// here and a stream that is "open" is.
async fn start_replication(config: ReplicationConfig) -> Result<ReplicationClient, CaptureError> {
    let mut client = ReplicationClient::connect(config)
        .await
        .map_err(|err| CaptureError::Connect(err.to_string()))?;
    client
        .wait_started()
        .await
        .map_err(|err| CaptureError::Connect(err.to_string()))?;
    Ok(client)
}

/// The background half of a [`ChangeStream`]: hands what `client` receives, decoded by a
/// [`WalDecoder`], to `tx`, and confirms the positions that come back on `confirm_rx`. Ends when
/// the stream does, or when either side of the channel is gone.
async fn forward_stream(
    mut client: ReplicationClient,
    tx: mpsc::Sender<Result<StreamItem, CaptureError>>,
    mut confirm_rx: mpsc::Receiver<String>,
) {
    let mut decoder = WalDecoder::default();
    loop {
        let step = tokio::select! {
            biased;

            confirmed = confirm_rx.recv() => confirm(&mut client, confirmed),
            event = client.recv() => decoder.receive(event.map_err(|err| err.to_string())),
        };
        if !step.deliver(&tx).await {
            break;
        }
    }
    let _ = client.shutdown().await;
}

/// Advances the slot to a position the caller reports as durably persisted.
///
/// Advancing the slot (`update_applied_lsn`) is driven exclusively from here —
/// `pgwire-replication`'s doc comment for that method says to call it "only once you have
/// durably persisted all events up to `lsn`". `Commit` and between-transaction `KeepAlive`s
/// only tell the caller how far the stream has been delivered.
fn confirm(client: &mut ReplicationClient, confirmed: Option<String>) -> Step {
    // The caller (CaptureOrchestrator) is gone; streaming isn't useful without anyone to confirm
    // positions, so stop.
    let Some(position) = confirmed else {
        return Step::STOP;
    };
    match position.parse::<Lsn>() {
        Ok(lsn) => client.update_applied_lsn(lsn),
        Err(err) => {
            tracing::error!(
                position = %position,
                error = %err,
                "received an unparseable confirm position, ignoring",
            );
        }
    }
    Step::IDLE
}

/// What the stream's background task does after one received event or confirm: hand the
/// reader an item (`send`), stop, or both (a failed stream reports why, then ends).
#[derive(Debug)]
struct Step {
    send: Option<Result<StreamItem, CaptureError>>,
    stop: bool,
}

impl Step {
    const IDLE: Self = Self {
        send: None,
        stop: false,
    };
    const STOP: Self = Self {
        send: None,
        stop: true,
    };

    fn send(item: Result<StreamItem, CaptureError>) -> Self {
        Self {
            send: Some(item),
            stop: false,
        }
    }

    fn progress(position: Lsn) -> Self {
        Self::send(Ok(StreamItem::Progress {
            position: position.to_string(),
        }))
    }

    fn decode_error(err: decode::DecodeError) -> Self {
        Self::send(Err(CaptureError::Decode(err.to_string())))
    }

    /// Carries the step out on `tx`. Whether the stream goes on: not once it stops, nor once
    /// the reader is gone.
    async fn deliver(self, tx: &mpsc::Sender<Result<StreamItem, CaptureError>>) -> bool {
        if let Some(item) = self.send
            && tx.send(item).await.is_err()
        {
            return false;
        }
        !self.stop
    }
}

/// The decoding half of a replication stream, apart from its I/O: tracks the relations and the
/// transaction being received, and turns each event into the [`Step`] to take.
#[derive(Default)]
struct WalDecoder {
    relations: HashMap<u32, RelationInfo>,
    commit_timestamp_unix_micros: i64,
    /// The transaction being received: its commit position (from `Begin`), until its `Commit`
    /// arrives.
    open_transaction: Option<Lsn>,
}

impl WalDecoder {
    /// What to do with the outcome of one `recv`: the stream ends cleanly on `None`, and on an
    /// error after reporting it.
    fn receive(&mut self, received: Result<Option<ReplicationEvent>, String>) -> Step {
        match received {
            Ok(Some(event)) => self.event(event),
            Ok(None) => Step::STOP,
            Err(err) => Step {
                send: Some(Err(CaptureError::Stream(err))),
                stop: true,
            },
        }
    }

    fn event(&mut self, event: ReplicationEvent) -> Step {
        match event {
            ReplicationEvent::Begin {
                final_lsn,
                commit_time_micros,
                ..
            } => {
                self.commit_timestamp_unix_micros = postgres_to_unix_micros(commit_time_micros);
                self.open_transaction = Some(final_lsn);
                Step::IDLE
            }
            ReplicationEvent::Commit { end_lsn, .. } => {
                self.open_transaction = None;
                Step::progress(end_lsn)
            }
            // A keepalive carries the position the server has decoded and sent up to: every
            // transaction committed before it has been delivered. Inside a transaction that
            // would say nothing about the rest of it, so only between transactions.
            ReplicationEvent::KeepAlive { wal_end, .. } if self.open_transaction.is_none() => {
                Step::progress(wal_end)
            }
            ReplicationEvent::KeepAlive { .. } => Step::IDLE,
            ReplicationEvent::XLogData { wal_end, data, .. } => self.xlog_data(wal_end, &data),
            ReplicationEvent::Message { .. } => Step::IDLE,
            ReplicationEvent::StoppedAt { .. } => Step::STOP,
        }
    }

    /// One `pgoutput` message: a relation is remembered, a row change is handed on.
    fn xlog_data(&mut self, wal_end: Lsn, data: &[u8]) -> Step {
        let Some((&tag, payload)) = data.split_first() else {
            return Step::IDLE;
        };
        let decoded = match tag {
            b'R' => return self.relation(payload),
            b'I' => decode_insert(payload, &self.relations),
            b'U' => decode_update(payload, &self.relations),
            b'D' => decode_delete(payload, &self.relations),
            // Truncate, Origin, Type, and streaming-transaction markers aren't captured in this
            // MVP.
            _ => return Step::IDLE,
        };
        match decoded {
            Ok(change) => to_change_event(
                change,
                &self.relations,
                wal_end,
                self.open_transaction.unwrap_or(wal_end),
                self.commit_timestamp_unix_micros,
            )
            .map_or(Step::IDLE, |event| {
                Step::send(Ok(StreamItem::Change(event)))
            }),
            Err(err) => Step::decode_error(err),
        }
    }

    fn relation(&mut self, payload: &[u8]) -> Step {
        match decode_relation(payload) {
            Ok((oid, info)) => {
                self.relations.insert(oid, info);
                Step::IDLE
            }
            Err(err) => Step::decode_error(err),
        }
    }
}

/// Converts a `pgoutput` timestamp (microseconds since 2000-01-01) to microseconds since the Unix
/// epoch.
fn postgres_to_unix_micros(postgres_micros: i64) -> i64 {
    postgres_micros.saturating_add(POSTGRES_EPOCH_UNIX_MICROS)
}

fn to_change_event(
    change: DecodedChange,
    relations: &HashMap<u32, RelationInfo>,
    wal_end: Lsn,
    commit_lsn: Lsn,
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
        commit_position: commit_lsn.to_string(),
        commit_timestamp_unix_micros,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datasource::{ConnectionConfig, DataSourceId, DbEngine};

    fn lsn(text: &str) -> Lsn {
        text.parse().unwrap()
    }

    fn cstr(text: &str) -> Vec<u8> {
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(0);
        bytes
    }

    /// A `pgoutput` Relation message for `public.orders (id, total)`.
    fn relation(oid: u32) -> Vec<u8> {
        let mut bytes = vec![b'R'];
        bytes.extend_from_slice(&oid.to_be_bytes());
        bytes.extend(cstr("public"));
        bytes.extend(cstr("orders"));
        bytes.push(b'd');
        bytes.extend_from_slice(&2u16.to_be_bytes());
        for column in ["id", "total"] {
            bytes.push(0);
            bytes.extend(cstr(column));
            bytes.extend_from_slice(&23i32.to_be_bytes());
            bytes.extend_from_slice(&(-1i32).to_be_bytes());
        }
        bytes
    }

    fn tuple(values: &[&str]) -> Vec<u8> {
        let mut bytes = (values.len() as u16).to_be_bytes().to_vec();
        for value in values {
            bytes.push(b't');
            bytes.extend_from_slice(&(value.len() as u32).to_be_bytes());
            bytes.extend_from_slice(value.as_bytes());
        }
        bytes
    }

    /// A row change message: `tag`, the relation, then each `(marker, tuple)`.
    fn change(tag: u8, oid: u32, tuples: &[(u8, &[&str])]) -> Vec<u8> {
        let mut bytes = vec![tag];
        bytes.extend_from_slice(&oid.to_be_bytes());
        for (marker, values) in tuples {
            bytes.push(*marker);
            bytes.extend(tuple(values));
        }
        bytes
    }

    fn xlog(wal_end: &str, data: Vec<u8>) -> ReplicationEvent {
        ReplicationEvent::XLogData {
            wal_start: lsn(wal_end),
            wal_end: lsn(wal_end),
            server_time_micros: 0,
            data: data.into(),
        }
    }

    fn begin(final_lsn: &str) -> ReplicationEvent {
        ReplicationEvent::Begin {
            final_lsn: lsn(final_lsn),
            xid: 7,
            commit_time_micros: 0,
        }
    }

    fn keepalive(wal_end: &str) -> ReplicationEvent {
        ReplicationEvent::KeepAlive {
            wal_end: lsn(wal_end),
            reply_requested: false,
            server_time_micros: 0,
        }
    }

    fn idle(step: &Step) -> bool {
        step.send.is_none() && !step.stop
    }

    fn progress(step: &Step) -> Option<String> {
        match step {
            Step {
                send: Some(Ok(StreamItem::Progress { position })),
                stop: false,
            } => Some(position.clone()),
            _ => None,
        }
    }

    fn changed(step: Step) -> ChangeEvent {
        match step {
            Step {
                send: Some(Ok(StreamItem::Change(event))),
                stop: false,
            } => event,
            other => panic!("expected a change, got {other:?}"),
        }
    }

    fn decode_failed(step: &Step) -> bool {
        matches!(
            step,
            Step {
                send: Some(Err(CaptureError::Decode(_))),
                stop: false,
            }
        )
    }

    #[test]
    fn a_transaction_is_delivered_with_its_commit_position_then_progress() {
        let mut decoder = WalDecoder::default();
        assert!(idle(&decoder.event(xlog("0/10", relation(16412)))));
        assert!(idle(&decoder.event(begin("0/40"))));

        let insert =
            changed(decoder.event(xlog("0/20", change(b'I', 16412, &[(b'N', &["1", "9.99"])]))));
        assert_eq!(
            (insert.schema.as_str(), insert.table.as_str()),
            ("public", "orders")
        );
        assert!(matches!(insert.operation, Operation::Insert { .. }));
        assert_eq!(insert.position, lsn("0/20").to_string());
        assert_eq!(insert.commit_position, lsn("0/40").to_string());
        assert_eq!(
            insert.commit_timestamp_unix_micros,
            POSTGRES_EPOCH_UNIX_MICROS
        );

        let update = changed(decoder.event(xlog(
            "0/28",
            change(
                b'U',
                16412,
                &[(b'K', &["1", "9.99"]), (b'N', &["1", "19.99"])],
            ),
        )));
        assert!(matches!(
            update.operation,
            Operation::Update {
                before: Some(_),
                ..
            }
        ));
        let delete = changed(decoder.event(xlog(
            "0/30",
            change(b'D', 16412, &[(b'K', &["1", "19.99"])]),
        )));
        assert!(matches!(delete.operation, Operation::Delete { .. }));
        assert_eq!(delete.commit_position, lsn("0/40").to_string());

        let commit = decoder.event(ReplicationEvent::Commit {
            lsn: lsn("0/40"),
            end_lsn: lsn("0/48"),
            commit_time_micros: 0,
        });
        assert_eq!(progress(&commit), Some(lsn("0/48").to_string()));
    }

    /// Mid-transaction a keepalive says nothing about the rest of the transaction.
    #[test]
    fn keepalives_report_progress_only_between_transactions() {
        let mut decoder = WalDecoder::default();
        assert_eq!(
            progress(&decoder.event(keepalive("0/10"))),
            Some(lsn("0/10").to_string())
        );
        decoder.event(begin("0/40"));
        assert!(idle(&decoder.event(keepalive("0/20"))));
    }

    #[test]
    fn undecodable_messages_are_reported_and_the_stream_goes_on() {
        let mut decoder = WalDecoder::default();
        // A change to a relation never described.
        assert!(decode_failed(
            &decoder.event(xlog("0/10", change(b'I', 1, &[(b'N', &["1"])]),))
        ));
        // A truncated relation message.
        assert!(decode_failed(
            &decoder.event(xlog("0/18", vec![b'R', 0, 0]))
        ));
    }

    #[test]
    fn messages_that_are_not_captured_are_skipped() {
        let mut decoder = WalDecoder::default();
        assert!(idle(&decoder.event(xlog("0/10", Vec::new()))));
        assert!(idle(&decoder.event(xlog("0/10", vec![b'T', 0, 0, 0, 1]))));
        assert!(idle(&decoder.event(ReplicationEvent::Message {
            transactional: false,
            lsn: lsn("0/10"),
            prefix: "p".to_string(),
            content: Vec::new().into(),
        })));
    }

    #[test]
    fn the_stream_stops_when_it_ends_and_reports_why_it_failed() {
        let mut decoder = WalDecoder::default();
        let ended = decoder.receive(Ok(None));
        assert!(ended.send.is_none() && ended.stop);
        let stopped = decoder.receive(Ok(Some(ReplicationEvent::StoppedAt {
            reached: lsn("0/10"),
        })));
        assert!(stopped.send.is_none() && stopped.stop);
        let failed = decoder.receive(Err("connection reset".to_string()));
        assert!(matches!(
            failed,
            Step {
                send: Some(Err(CaptureError::Stream(_))),
                stop: true,
            }
        ));
        assert!(progress(&decoder.receive(Ok(Some(keepalive("0/20"))))).is_some());
    }

    #[tokio::test]
    async fn a_step_is_delivered_until_it_stops_or_the_reader_is_gone() {
        let (tx, mut rx) = mpsc::channel(4);
        assert!(Step::IDLE.deliver(&tx).await);
        assert!(Step::progress(lsn("0/10")).deliver(&tx).await);
        assert!(matches!(
            rx.recv().await,
            Some(Ok(StreamItem::Progress { .. }))
        ));
        assert!(!Step::STOP.deliver(&tx).await);

        drop(rx);
        assert!(!Step::progress(lsn("0/20")).deliver(&tx).await);
    }

    #[test]
    fn a_resume_position_must_be_an_lsn() {
        let source = DataSource {
            id: DataSourceId(uuid::Uuid::now_v7()),
            name: "test".to_string(),
            engine: DbEngine::Postgres,
            connection: ConnectionConfig {
                host: "db".to_string(),
                port: 5432,
                username: "u".to_string(),
                password: "p".to_string(),
                database: "d".to_string(),
            },
            ingested_tables: Some(Vec::new()),
        };
        let config = |resume_from| {
            replication_config(
                &source,
                "slot".to_string(),
                "publication".to_string(),
                resume_from,
            )
        };
        assert!(config(None).is_ok());
        assert!(config(Some("0/16B6C50")).is_ok());
        assert!(matches!(
            config(Some("nope")),
            Err(CaptureError::Connect(_))
        ));
    }

    #[test]
    fn converts_postgres_timestamps_to_unix_micros() {
        // 2000-01-01T00:00:00Z, the Postgres epoch itself.
        assert_eq!(postgres_to_unix_micros(0), 946_684_800_000_000);
        // A commit at 2026-10-09T01:43:52.919964Z, as `pgoutput` sends it.
        assert_eq!(
            postgres_to_unix_micros(844_825_432_919_964),
            1_791_510_232_919_964
        );
    }
}

#[cfg(test)]
mod live_tests {
    use std::time::Duration;

    use uuid::Uuid;

    use sqlx::{Connection, PgConnection, postgres::PgConnectOptions};

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
            id: DataSourceId(Uuid::now_v7()),
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
            ingested_tables: Some(Vec::new()),
        };

        let cdc_source = PostgresWalSource::new();
        let mut stream = cdc_source
            .stream_changes(&source, None)
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
        while seen.len() < 3 {
            let item = tokio::time::timeout(Duration::from_secs(10), stream.events.recv())
                .await
                .expect("timed out waiting for a change event")
                .expect("change channel closed unexpectedly")
                .expect("capture reported an error");
            let StreamItem::Change(event) = item else {
                continue;
            };
            stream
                .confirm
                .send(event.commit_position.clone())
                .await
                .expect("confirm channel should still be open");
            seen.push(event);
        }

        assert_eq!(seen[0].table, "orders");
        assert_eq!(seen[0].schema, "public");
        assert!(matches!(seen[0].operation, Operation::Insert { .. }));
        assert!(matches!(seen[1].operation, Operation::Update { .. }));
        assert!(matches!(seen[2].operation, Operation::Delete { .. }));
        // Three autocommit statements: three transactions, delivered in commit order.
        let commits: Vec<Lsn> = seen
            .iter()
            .map(|event| event.commit_position.parse().unwrap())
            .collect();
        assert!(commits.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
