//! [`IcebergTableMaintainer`] against a real Iceberg table: a `MemoryCatalog` over the local
//! filesystem, written to through the real [`IcebergChangelogWriter`].
//!
//! The REST commit itself can't run here, so [`CatalogCommitter`] stands in for it: it checks
//! the requirements and applies the updates exactly as a REST catalog would, then swaps the
//! table's metadata in the `MemoryCatalog`.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path as FsPath, PathBuf};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use arrow_array::{Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema as ArrowSchema};
use async_trait::async_trait;
use futures::TryStreamExt;
use iceberg::arrow::arrow_schema_to_schema_auto_assign_ids;
use iceberg::io::LocalFsStorageFactory;
use iceberg::memory::{MEMORY_CATALOG_WAREHOUSE, MemoryCatalog, MemoryCatalogBuilder};
use iceberg::spec::Operation as SnapshotOperation;
use iceberg::transaction::{ApplyTransactionAction, Transaction};
use iceberg::{
    Catalog, CatalogBuilder, MetadataLocation, NamespaceIdent, TableCreation, TableIdent,
    TableRequirement, TableUpdate,
};
use tokio::sync::Mutex;

use super::commit::{CommitError, SnapshotCommitter};
use super::files::referenced_files;
use super::{IcebergTableMaintainer, OrphanSweeper};
use crate::capture::backfill::SNAPSHOT_POSITION;
use crate::capture::domain::{ChangeEvent, ColumnValue, Operation};
use crate::datasource::{ConnectionConfig, DataSource, DataSourceId, DbEngine, TableRef};
use crate::maintenance::domain::{
    CompactionPolicy, ExpiryPolicy, MaintenanceReport, TableMaintainer,
};
use crate::write::domain::{
    BackfillState, COMMIT_TIMESTAMP_COLUMN, Checkpoint, IcebergWriter, OP_COLUMN, POSITION_COLUMN,
    SOURCE_ID_COLUMN, TargetTable, namespace_for_source,
};
use crate::write::infrastructure::IcebergChangelogWriter;

type Hook = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Applies commits to a [`MemoryCatalog`] the way a REST catalog does. `before_next_commit`
/// runs just before the next commit is checked — a commit landing between a compaction's
/// planning and its commit.
struct CatalogCommitter {
    catalog: Arc<MemoryCatalog>,
    before_next_commit: Mutex<Option<Hook>>,
    conflicts: Mutex<usize>,
}

#[async_trait]
impl SnapshotCommitter for CatalogCommitter {
    async fn commit(
        &self,
        ident: &TableIdent,
        requirements: Vec<TableRequirement>,
        updates: Vec<TableUpdate>,
    ) -> Result<(), CommitError> {
        if let Some(hook) = self.before_next_commit.lock().await.take() {
            hook.await;
        }
        let fail = |err: iceberg::Error| CommitError::Rejected(err.to_string());
        let table = self.catalog.load_table(ident).await.map_err(fail)?;
        for requirement in &requirements {
            if let Err(err) = requirement.check(Some(table.metadata())) {
                *self.conflicts.lock().await += 1;
                return Err(CommitError::Conflict(err.to_string()));
            }
        }
        let current_location = table.metadata_location().expect("loaded tables have one");
        let mut builder = table
            .metadata()
            .clone()
            .into_builder(Some(current_location.to_string()));
        for update in updates {
            builder = update.apply(builder).map_err(fail)?;
        }
        let metadata = builder.build().map_err(fail)?.metadata;
        // RustFS's own checks of a new snapshot.
        if let Some(snapshot) = metadata.current_snapshot() {
            let summary = snapshot.summary();
            if summary.operation == SnapshotOperation::Replace
                && !summary
                    .additional_properties
                    .contains_key("deleted-data-files")
            {
                return Err(CommitError::Rejected(
                    "row-level snapshot operation requires deleted data files".to_string(),
                ));
            }
            let staged = iceberg::table::Table::builder()
                .metadata(metadata.clone())
                .identifier(ident.clone())
                .file_io(table.file_io().clone())
                .runtime(iceberg::Runtime::current())
                .build()
                .map_err(fail)?;
            for path in referenced_files(&staged, std::slice::from_ref(snapshot))
                .await
                .unwrap()
            {
                if !table.file_io().exists(&path).await.map_err(fail)? {
                    return Err(CommitError::Rejected(format!(
                        "manifest referenced data file is missing: {path}"
                    )));
                }
            }
        }
        let location = MetadataLocation::from_str(current_location)
            .map_err(fail)?
            .with_next_version();
        metadata
            .write_to(table.file_io(), &location)
            .await
            .map_err(fail)?;
        self.catalog.drop_table(ident).await.map_err(fail)?;
        self.catalog
            .register_table(ident, location.to_string())
            .await
            .map_err(fail)?;
        Ok(())
    }
}

struct Fixture {
    dir: PathBuf,
    catalog: Arc<MemoryCatalog>,
    committer: Arc<CatalogCommitter>,
    writer: Arc<IcebergChangelogWriter>,
    source: DataSource,
    target: TargetTable,
    ident: TableIdent,
    batches: u64,
}

impl Fixture {
    async fn new() -> Self {
        // Canonical, so paths match what a filesystem listing returns (macOS's temp dir is a
        // symlink).
        let dir = std::env::temp_dir().join(format!(
            "pipa-maintenance-{}",
            uuid::Uuid::now_v7().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(dir).unwrap();
        let catalog = Arc::new(
            MemoryCatalogBuilder::default()
                .with_storage_factory(Arc::new(LocalFsStorageFactory))
                .load(
                    "test",
                    HashMap::from([(
                        MEMORY_CATALOG_WAREHOUSE.to_string(),
                        dir.to_str().unwrap().to_string(),
                    )]),
                )
                .await
                .unwrap(),
        );

        let source = DataSource {
            id: DataSourceId(uuid::Uuid::now_v7()),
            name: "test".to_string(),
            engine: DbEngine::Postgres,
            connection: ConnectionConfig {
                host: "unused".to_string(),
                port: 5432,
                username: "u".to_string(),
                password: "p".to_string(),
                database: "d".to_string(),
            },
            ingested_tables: Some(Vec::new()),
        };
        let target = TargetTable {
            namespace: namespace_for_source(&source),
            table: "public__orders".to_string(),
        };
        let namespace = NamespaceIdent::new(target.namespace.clone());
        catalog
            .create_namespace(&namespace, HashMap::new())
            .await
            .unwrap();
        // The table the writer would create from the source's columns; created up front, so the
        // writer never has to reach a Postgres to introspect them.
        let schema = arrow_schema_to_schema_auto_assign_ids(&ArrowSchema::new(vec![
            Field::new(OP_COLUMN, DataType::Utf8, false),
            Field::new(SOURCE_ID_COLUMN, DataType::Utf8, false),
            Field::new(POSITION_COLUMN, DataType::Utf8, false),
            Field::new(COMMIT_TIMESTAMP_COLUMN, DataType::Int64, false),
            Field::new("id", DataType::Int32, true),
            Field::new("name", DataType::Utf8, true),
        ]))
        .unwrap();
        catalog
            .create_table(
                &namespace,
                TableCreation::builder()
                    .name(target.table.clone())
                    .schema(schema)
                    .build(),
            )
            .await
            .unwrap();

        let ident = TableIdent::new(namespace, target.table.clone());
        Self {
            dir,
            committer: Arc::new(CatalogCommitter {
                catalog: Arc::clone(&catalog),
                before_next_commit: Mutex::new(None),
                conflicts: Mutex::new(0),
            }),
            writer: Arc::new(IcebergChangelogWriter::new(catalog.clone())),
            catalog,
            source,
            target,
            ident,
            batches: 0,
        }
    }

    /// The events of batch `n`: three inserts, as one transaction committed at `commit_position(n)`.
    fn batch(n: u64) -> Vec<ChangeEvent> {
        (0..3)
            .map(|row| ChangeEvent {
                schema: "public".to_string(),
                table: "orders".to_string(),
                operation: Operation::Insert {
                    after: vec![
                        ColumnValue {
                            name: "id".to_string(),
                            value: Some((n * 3 + row).to_string()),
                        },
                        ColumnValue {
                            name: "name".to_string(),
                            value: Some(format!("row {row} of batch {n}")),
                        },
                    ],
                },
                position: position(n * 16 + row),
                commit_position: commit_position(n),
                commit_timestamp_unix_micros: n as i64,
            })
            .collect()
    }

    async fn commit_batches(&mut self, count: u64) {
        for _ in 0..count {
            self.batches += 1;
            let n = self.batches;
            self.writer
                .commit_batch(
                    &self.source,
                    &self.target,
                    &Self::batch(n),
                    &commit_position(n),
                )
                .await
                .unwrap();
        }
    }

    /// A commit of batch `n` to run in [`CatalogCommitter::before_next_commit`].
    fn racing_commit(&mut self) -> Hook {
        self.batches += 1;
        let n = self.batches;
        let (writer, source, target) = (
            Arc::clone(&self.writer),
            self.source.clone(),
            self.target.clone(),
        );
        Box::pin(async move {
            writer
                .commit_batch(&source, &target, &Fixture::batch(n), &commit_position(n))
                .await
                .unwrap();
        })
    }

    fn maintainer(
        &self,
        expiry: ExpiryPolicy,
        orphans: Option<OrphanSweeper>,
    ) -> IcebergTableMaintainer {
        IcebergTableMaintainer::new(
            self.catalog.clone(),
            self.committer.clone(),
            CompactionPolicy {
                target_file_bytes: 128 * 1024 * 1024,
                small_file_bytes: 96 * 1024 * 1024,
                min_input_files: 4,
                max_input_bytes: 1024 * 1024 * 1024,
            },
            expiry,
            orphans,
        )
    }

    async fn maintain(&self, maintainer: &IcebergTableMaintainer) -> MaintenanceReport {
        maintainer.maintain(&self.target).await.unwrap()
    }

    async fn live_files(&self) -> Vec<String> {
        let table = self.catalog.load_table(&self.ident).await.unwrap();
        table
            .scan()
            .select_all()
            .build()
            .unwrap()
            .plan_files()
            .await
            .unwrap()
            .map_ok(|task| task.data_file_path)
            .try_collect()
            .await
            .unwrap()
    }

    /// Every row's `_position`, sorted.
    async fn positions(&self) -> Vec<String> {
        let table = self.catalog.load_table(&self.ident).await.unwrap();
        let batches: Vec<RecordBatch> = table
            .scan()
            .select([POSITION_COLUMN])
            .build()
            .unwrap()
            .to_arrow()
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        let mut positions: Vec<String> = batches
            .iter()
            .flat_map(|batch| {
                let column = batch
                    .column(0)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap()
                    .clone();
                (0..column.len()).map(move |i| column.value(i).to_string())
            })
            .collect();
        positions.sort();
        positions
    }

    /// The `_position` of every row of the first `batches` batches, sorted.
    fn expected_positions(batches: u64) -> Vec<String> {
        let mut positions: Vec<String> = (1..=batches)
            .flat_map(|n| (0..3).map(move |row| position(n * 16 + row)))
            .collect();
        positions.sort();
        positions
    }

    /// What RustFS's catalog checks on every commit: each file the current snapshot's manifests
    /// mention — removal records included — exists.
    async fn assert_every_mentioned_file_exists(&self) {
        let table = self.catalog.load_table(&self.ident).await.unwrap();
        let current = table.metadata().current_snapshot().unwrap().clone();
        let on_disk = self.files_on_disk();
        for path in referenced_files(&table, &[current]).await.unwrap() {
            assert!(
                on_disk.contains(&path),
                "{path} is mentioned but was deleted"
            );
        }
    }

    async fn checkpoint(&self) -> Option<Checkpoint> {
        self.writer
            .last_committed_position(&self.target)
            .await
            .unwrap()
    }

    /// Every file under the table's location.
    fn files_on_disk(&self) -> HashSet<String> {
        fn walk(dir: &FsPath, files: &mut HashSet<String>) {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, files);
                } else {
                    files.insert(path.to_str().unwrap().to_string());
                }
            }
        }
        let mut files = HashSet::new();
        walk(&self.dir, &mut files);
        files
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn position(n: u64) -> String {
    format!("0/{n:08X}")
}

fn commit_position(n: u64) -> String {
    format!("0/{:08X}", n * 16 + 15)
}

fn keep_everything() -> ExpiryPolicy {
    ExpiryPolicy {
        max_snapshot_age: Duration::from_secs(24 * 3600),
        min_snapshots_to_keep: 1,
    }
}

fn expire_everything_old() -> ExpiryPolicy {
    ExpiryPolicy {
        max_snapshot_age: Duration::ZERO,
        min_snapshots_to_keep: 1,
    }
}

#[tokio::test]
async fn compaction_replaces_small_files_keeping_every_row_and_the_checkpoint() {
    let mut fixture = Fixture::new().await;
    fixture.commit_batches(20).await;
    assert_eq!(fixture.live_files().await.len(), 20);
    let checkpoint = fixture.checkpoint().await;
    assert_eq!(checkpoint, Some(Checkpoint::Commit(commit_position(20))));

    let report = fixture
        .maintain(&fixture.maintainer(keep_everything(), None))
        .await;

    assert_eq!(report.compactions, 1);
    assert_eq!(report.files_rewritten, 20);
    assert_eq!(report.files_written, 1);
    assert_eq!(fixture.live_files().await.len(), 1);
    assert_eq!(fixture.positions().await, Fixture::expected_positions(20));
    assert_eq!(
        fixture.checkpoint().await,
        checkpoint,
        "dedup must read what it read before"
    );

    let table = fixture.catalog.load_table(&fixture.ident).await.unwrap();
    let summary = table.metadata().current_snapshot().unwrap().summary();
    assert_eq!(summary.operation, SnapshotOperation::Replace);
    assert_eq!(
        summary.additional_properties["pipa.maintenance"],
        "compaction"
    );
    assert_eq!(summary.additional_properties["total-records"], "60");
    assert_eq!(summary.additional_properties["total-data-files"], "1");

    // Ingestion carries on on top of it.
    fixture.commit_batches(1).await;
    assert_eq!(fixture.live_files().await.len(), 2);
    assert_eq!(fixture.positions().await, Fixture::expected_positions(21));
    assert_eq!(
        fixture.checkpoint().await,
        Some(Checkpoint::Commit(commit_position(21)))
    );
}

#[tokio::test]
async fn too_few_small_files_are_left_alone() {
    let mut fixture = Fixture::new().await;
    fixture.commit_batches(3).await;

    let report = fixture
        .maintain(&fixture.maintainer(keep_everything(), None))
        .await;

    assert!(report.is_empty(), "{report}");
    assert_eq!(fixture.live_files().await.len(), 3);
}

#[tokio::test]
async fn a_commit_landing_mid_compaction_is_kept_and_its_checkpoint_wins() {
    let mut fixture = Fixture::new().await;
    fixture.commit_batches(8).await;
    let racing = fixture.racing_commit();
    *fixture.committer.before_next_commit.lock().await = Some(racing);

    let report = fixture
        .maintain(&fixture.maintainer(keep_everything(), None))
        .await;

    assert_eq!(report.compactions, 1);
    assert_eq!(*fixture.committer.conflicts.lock().await, 1, "rebased once");
    // The compacted file plus the racing commit's.
    assert_eq!(fixture.live_files().await.len(), 2);
    assert_eq!(fixture.positions().await, Fixture::expected_positions(9));
    assert_eq!(
        fixture.checkpoint().await,
        Some(Checkpoint::Commit(commit_position(9)))
    );
}

#[tokio::test]
async fn expiry_deletes_exactly_the_files_no_remaining_snapshot_mentions() {
    let mut fixture = Fixture::new().await;
    fixture.commit_batches(10).await;
    let first_ten: Vec<String> = fixture.live_files().await;
    fixture
        .maintain(&fixture.maintainer(keep_everything(), None))
        .await;
    tokio::time::sleep(Duration::from_millis(5)).await;

    // The replace is current, and its removal records still name the ten files: they stay.
    let report = fixture
        .maintain(&fixture.maintainer(expire_everything_old(), None))
        .await;
    assert_eq!(
        report.snapshots_expired, 10,
        "every append; the replace is current"
    );
    let on_disk = fixture.files_on_disk();
    for path in &first_ten {
        assert!(
            on_disk.contains(path),
            "{path} is still named by a removal record"
        );
    }

    // Ingestion carries the records forward; the next compaction drops them, and the files
    // go once the snapshots that carried them have expired.
    fixture.commit_batches(4).await;
    fixture.assert_every_mentioned_file_exists().await;
    let report = fixture
        .maintain(&fixture.maintainer(keep_everything(), None))
        .await;
    assert_eq!(report.compactions, 1);
    tokio::time::sleep(Duration::from_millis(5)).await;
    let report = fixture
        .maintain(&fixture.maintainer(expire_everything_old(), None))
        .await;

    let table = fixture.catalog.load_table(&fixture.ident).await.unwrap();
    assert_eq!(table.metadata().snapshots().len(), 1);
    assert!(report.files_deleted >= first_ten.len());
    let on_disk = fixture.files_on_disk();
    for path in &first_ten {
        assert!(!on_disk.contains(path), "{path} should be gone");
    }
    fixture.assert_every_mentioned_file_exists().await;
    assert_eq!(fixture.positions().await, Fixture::expected_positions(14));
    assert_eq!(
        fixture.checkpoint().await,
        Some(Checkpoint::Commit(commit_position(14)))
    );

    // And ingestion carries on.
    fixture.commit_batches(1).await;
    fixture.assert_every_mentioned_file_exists().await;
    assert_eq!(fixture.positions().await, Fixture::expected_positions(15));
}

#[tokio::test]
async fn the_orphan_sweep_deletes_only_old_unreferenced_files_pipa_writes() {
    let mut fixture = Fixture::new().await;
    fixture.commit_batches(2).await;
    let table = fixture.catalog.load_table(&fixture.ident).await.unwrap();
    let location = PathBuf::from(table.metadata().location());
    let stray_data = location.join("data/stray.parquet");
    let stray_manifest = location.join("metadata/stray-m0.avro");
    let other_file = location.join("data/notes.txt");
    for path in [&stray_data, &stray_manifest, &other_file] {
        std::fs::write(path, b"stray").unwrap();
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
    let before = fixture.files_on_disk();

    let sweeper = OrphanSweeper::new(
        Arc::new(object_store::local::LocalFileSystem::new()),
        "",
        Duration::ZERO,
    );
    let report = fixture
        .maintain(&fixture.maintainer(keep_everything(), Some(sweeper)))
        .await;

    assert_eq!(report.orphans_deleted, 2);
    let after = fixture.files_on_disk();
    let removed: HashSet<&String> = before.difference(&after).collect();
    assert_eq!(
        removed,
        HashSet::from([
            &stray_data.to_str().unwrap().to_string(),
            &stray_manifest.to_str().unwrap().to_string()
        ])
    );
    assert!(after.contains(other_file.to_str().unwrap()));
    assert_eq!(fixture.positions().await, Fixture::expected_positions(2));
}

#[tokio::test]
async fn an_unreferenced_file_younger_than_the_age_survives_the_sweep() {
    let mut fixture = Fixture::new().await;
    fixture.commit_batches(1).await;
    let table = fixture.catalog.load_table(&fixture.ident).await.unwrap();
    let in_flight = PathBuf::from(table.metadata().location()).join("data/in-flight.parquet");
    std::fs::write(&in_flight, b"written, not committed yet").unwrap();

    let sweeper = OrphanSweeper::new(
        Arc::new(object_store::local::LocalFileSystem::new()),
        "",
        Duration::from_secs(3600),
    );
    let report = fixture
        .maintain(&fixture.maintainer(keep_everything(), Some(sweeper)))
        .await;

    assert_eq!(report.orphans_deleted, 0);
    assert!(in_flight.exists());
}

#[tokio::test]
async fn a_snapshot_without_a_checkpoint_falls_back_to_its_ancestors() {
    let mut fixture = Fixture::new().await;
    fixture.commit_batches(2).await;
    // A commit by something that doesn't carry pipa's checkpoint forward.
    let table = fixture.catalog.load_table(&fixture.ident).await.unwrap();
    let tx = Transaction::new(&table);
    let tx = tx
        .fast_append()
        .set_snapshot_properties(HashMap::from([("other".to_string(), "engine".to_string())]))
        .apply(tx)
        .unwrap();
    let table = tx.commit(fixture.catalog.as_ref()).await.unwrap();
    let current = table.metadata().current_snapshot().unwrap();
    assert!(crate::write::infrastructure::checkpoint_of(current).is_none());

    assert_eq!(
        fixture.checkpoint().await,
        Some(Checkpoint::Commit(commit_position(2)))
    );
}

/// A backfill page lands in a single commit together with the state it got to, and carries
/// the checkpoint forward; a last step without rows only records the state.
#[tokio::test]
async fn a_backfill_page_lands_with_its_state_and_keeps_the_checkpoint() {
    let mut fixture = Fixture::new().await;
    fixture.commit_batches(1).await;
    let checkpoint = fixture.checkpoint().await;
    let snapshots = |table: &iceberg::table::Table| table.metadata().snapshots().count();
    let before = snapshots(&fixture.catalog.load_table(&fixture.ident).await.unwrap());
    assert_eq!(
        fixture
            .writer
            .backfill_state(&fixture.target)
            .await
            .unwrap(),
        None
    );

    let table = TableRef {
        schema: "public".to_string(),
        name: "orders".to_string(),
    };
    let rows: Vec<ChangeEvent> = (100..103)
        .map(|id| ChangeEvent {
            schema: "public".to_string(),
            table: "orders".to_string(),
            operation: Operation::Read {
                after: vec![
                    ColumnValue {
                        name: "id".to_string(),
                        value: Some(id.to_string()),
                    },
                    ColumnValue {
                        name: "name".to_string(),
                        value: Some(format!("existing {id}")),
                    },
                ],
            },
            position: SNAPSHOT_POSITION.to_string(),
            commit_position: SNAPSHOT_POSITION.to_string(),
            commit_timestamp_unix_micros: 0,
        })
        .collect();
    let running = BackfillState::Running {
        after: vec!["102".to_string()],
    };
    fixture
        .writer
        .commit_backfill(&fixture.source, &fixture.target, &table, &rows, &running)
        .await
        .unwrap();

    assert_eq!(
        fixture
            .writer
            .backfill_state(&fixture.target)
            .await
            .unwrap(),
        Some(running)
    );
    assert_eq!(fixture.checkpoint().await, checkpoint);
    let loaded = fixture.catalog.load_table(&fixture.ident).await.unwrap();
    assert_eq!(snapshots(&loaded), before + 1);
    let mut expected = Fixture::expected_positions(1);
    expected.extend(std::iter::repeat_n(SNAPSHOT_POSITION.to_string(), 3));
    expected.sort();
    assert_eq!(fixture.positions().await, expected);

    fixture
        .writer
        .commit_backfill(
            &fixture.source,
            &fixture.target,
            &table,
            &[],
            &BackfillState::Done,
        )
        .await
        .unwrap();
    assert_eq!(
        fixture
            .writer
            .backfill_state(&fixture.target)
            .await
            .unwrap(),
        Some(BackfillState::Done)
    );
    let loaded = fixture.catalog.load_table(&fixture.ident).await.unwrap();
    assert_eq!(snapshots(&loaded), before + 1);
    assert_eq!(fixture.checkpoint().await, checkpoint);
}
