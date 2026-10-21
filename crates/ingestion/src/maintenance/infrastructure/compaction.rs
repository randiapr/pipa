// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! Compaction: rewrites runs of small data files into files of the target size, committed as an
//! Iceberg `replace` snapshot built by hand (see [`super::commit`] for why).
//!
//! The rows, their order within each rewritten run, and the table's checkpoint are unchanged:
//! the replace snapshot carries the checkpoint of the snapshot it replaces, so dedup after a
//! compaction reads exactly what it read before. A compaction racing the writer (or another
//! instance's compaction, around a handoff) is rebuilt on the new state and retried; one
//! whose input files are gone by then is dropped, and its output files deleted.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use futures::{StreamExt, TryStreamExt};
use iceberg::arrow::ArrowReaderBuilder;
use iceberg::spec::{
    DataFile, FormatVersion, MAIN_BRANCH, ManifestContentType, ManifestEntryRef, ManifestFile,
    ManifestListWriter, ManifestWriter, ManifestWriterBuilder, Operation, Snapshot, SnapshotRef,
    SnapshotReference, SnapshotRetention, SnapshotSummaryCollector, Summary,
};
use iceberg::table::Table;
use iceberg::{Catalog, Runtime, TableIdent, TableRequirement, TableUpdate};

use super::commit::{CommitError, SnapshotCommitter};
use super::files::live_data_files;
use crate::maintenance::domain::{
    CandidateFile, CompactionPolicy, MaintenanceError, MaintenanceReport, plan_compaction,
};
use crate::write::domain::Checkpoint;
use crate::write::infrastructure::{
    COMMIT_POSITION_PROPERTY, COMMIT_SOURCE_ID_PROPERTY, IcebergFileWriter,
    LEGACY_POSITION_PROPERTY, latest_checkpoint, new_data_file_writer,
};

/// Snapshot summary property naming the maintenance operation that made a snapshot.
pub const MAINTENANCE_PROPERTY: &str = "pipa.maintenance";

/// How often a compaction that keeps losing the race to other commits is rebuilt and retried.
const COMMIT_ATTEMPTS: u32 = 5;

/// Whether maintenance knows how to rewrite `table`: format v2 (what pipa creates), never
/// partitioned, and with its latest data in the current schema.
pub fn supports(table: &Table) -> bool {
    let metadata = table.metadata();
    metadata.format_version() == FormatVersion::V2
        && metadata
            .partition_specs_iter()
            .all(|spec| spec.is_unpartitioned())
        && metadata.current_snapshot().is_none_or(|snapshot| {
            snapshot
                .schema_id()
                .is_none_or(|id| id == metadata.current_schema_id())
        })
}

/// Compacts `ident`'s small data files, adding what it did to `report`.
pub async fn compact(
    catalog: &dyn Catalog,
    committer: &dyn SnapshotCommitter,
    policy: &CompactionPolicy,
    ident: &TableIdent,
    report: &mut MaintenanceReport,
) -> Result<(), MaintenanceError> {
    let table = load(catalog, ident).await?;
    if !supports(&table) {
        tracing::debug!(table = %ident, "not compacting a table of a layout pipa doesn't write");
        return Ok(());
    }
    let Some(snapshot) = table.metadata().current_snapshot() else {
        return Ok(());
    };
    let Some(live) = live_data_files(&table, snapshot).await? else {
        tracing::debug!(table = %ident, "not compacting a table with delete files");
        return Ok(());
    };
    let candidates: Vec<CandidateFile> = live
        .values()
        .map(|entry| CandidateFile {
            path: entry.file_path().to_string(),
            size_bytes: entry.data_file().file_size_in_bytes(),
            sequence_number: entry.sequence_number().unwrap_or_default(),
        })
        .collect();

    for bin in plan_compaction(&candidates, policy) {
        compact_bin(catalog, committer, ident, &bin, report).await?;
    }
    Ok(())
}

/// Rewrites one bin's files into new ones and replaces them in the table.
async fn compact_bin(
    catalog: &dyn Catalog,
    committer: &dyn SnapshotCommitter,
    ident: &TableIdent,
    bin: &[CandidateFile],
    report: &mut MaintenanceReport,
) -> Result<(), MaintenanceError> {
    // Read from the table as it is now: an earlier bin's commit moved it on.
    let table = load(catalog, ident).await?;
    let Some(snapshot) = table.metadata().current_snapshot() else {
        return Ok(());
    };
    let inputs: HashSet<String> = bin.iter().map(|file| file.path.clone()).collect();
    let Some(live) = live_data_files(&table, snapshot).await? else {
        return Ok(());
    };
    if !inputs.iter().all(|path| live.contains_key(path)) {
        return Ok(());
    }
    let input_rows: u64 = inputs.iter().map(|path| live[path].record_count()).sum();

    let output = rewrite(&table, snapshot.snapshot_id(), bin).await?;
    let output_rows: u64 = output.iter().map(DataFile::record_count).sum();
    if output_rows != input_rows {
        delete_files(&table, output.iter().map(DataFile::file_path)).await;
        return Err(MaintenanceError::Io(format!(
            "compaction read {output_rows} rows from files holding {input_rows}; nothing was replaced"
        )));
    }

    match commit_replace(
        catalog,
        committer,
        ident,
        &inputs,
        &output,
        snapshot.sequence_number(),
    )
    .await
    {
        Ok(true) => {
            report.compactions += 1;
            report.files_rewritten += inputs.len();
            report.bytes_rewritten += bin.iter().map(|file| file.size_bytes).sum::<u64>();
            report.files_written += output.len();
            Ok(())
        }
        Ok(false) => {
            delete_files(&table, output.iter().map(DataFile::file_path)).await;
            Ok(())
        }
        // Whether the commit landed is unknown, so its files may be in the table now: leave
        // them. If it didn't land they are orphans, which the sweep removes once old enough.
        Err(err) => Err(err),
    }
}

/// Reads `bin` (live data files of snapshot `snapshot_id`, in commit order) and writes their
/// rows, in that order, into new data files of up to the target size.
async fn rewrite(
    table: &Table,
    snapshot_id: i64,
    bin: &[CandidateFile],
) -> Result<Vec<DataFile>, MaintenanceError> {
    let order: HashMap<&str, usize> = bin
        .iter()
        .enumerate()
        .map(|(index, file)| (file.path.as_str(), index))
        .collect();
    let tasks = table
        .scan()
        .snapshot_id(snapshot_id)
        .select_all()
        .build()?
        .plan_files()
        .await?;
    let mut tasks: Vec<_> = tasks
        .try_filter(|task| std::future::ready(order.contains_key(task.data_file_path.as_str())))
        .try_collect()
        .await?;
    tasks.sort_by_key(|task| (order[task.data_file_path.as_str()], task.start));

    // One file at a time, so the rows come out in the order the files were committed.
    let mut batches = ArrowReaderBuilder::new(table.file_io().clone(), Runtime::current())
        .with_data_file_concurrency_limit(1)
        .build()
        .read(futures::stream::iter(tasks.into_iter().map(Ok)).boxed())?
        .stream();
    let mut writer: Box<dyn IcebergFileWriter> =
        new_data_file_writer(table, "pipa-compact").await?;
    let written = async {
        while let Some(batch) = batches.try_next().await? {
            writer.write(batch).await?;
        }
        writer.close().await
    }
    .await;
    Ok(written?)
}

/// Commits a `replace` snapshot swapping `inputs` for `output`, rebuilding it on the table's
/// latest state whenever another commit got in first. `Ok(false)` when it gave up without
/// changing anything (an input is no longer live, or the race was lost too often); the
/// caller then owns `output` again.
async fn commit_replace(
    catalog: &dyn Catalog,
    committer: &dyn SnapshotCommitter,
    ident: &TableIdent,
    inputs: &HashSet<String>,
    output: &[DataFile],
    data_sequence_number: i64,
) -> Result<bool, MaintenanceError> {
    for attempt in 0..COMMIT_ATTEMPTS {
        let table = load(catalog, ident).await?;
        let Some(base) = table.metadata().current_snapshot() else {
            return Ok(false);
        };
        let Some(staged) =
            stage_replace(&table, base, inputs, output, data_sequence_number, attempt).await?
        else {
            return Ok(false);
        };

        match committer
            .commit(ident, staged.requirements, staged.updates)
            .await
        {
            Ok(()) => return Ok(true),
            Err(CommitError::Conflict(reason)) => {
                tracing::debug!(table = %ident, attempt, %reason, "compaction lost a commit race, rebuilding it");
                delete_files(&table, staged.metadata_files.iter()).await;
                tokio::time::sleep(Duration::from_millis(100 << attempt)).await;
            }
            Err(CommitError::Rejected(reason)) => {
                delete_files(&table, staged.metadata_files.iter()).await;
                return Err(MaintenanceError::Commit(reason));
            }
            Err(err @ CommitError::Unknown(_)) => {
                return Err(MaintenanceError::Commit(err.to_string()));
            }
        }
    }
    tracing::info!(table = %ident, "compaction kept losing commit races, leaving it for the next run");
    Ok(false)
}

/// A replace snapshot written out and ready to commit.
struct StagedReplace {
    requirements: Vec<TableRequirement>,
    updates: Vec<TableUpdate>,
    /// The manifests and manifest list written for it, to delete if it doesn't land.
    metadata_files: Vec<String>,
}

/// Writes the manifests and manifest list of a snapshot that is `base` with `inputs` replaced
/// by `output`. `None` when an input isn't live in `base`.
async fn stage_replace(
    table: &Table,
    base: &SnapshotRef,
    inputs: &HashSet<String>,
    output: &[DataFile],
    data_sequence_number: i64,
    attempt: u32,
) -> Result<Option<StagedReplace>, MaintenanceError> {
    let metadata = table.metadata();
    let snapshot_id = new_snapshot_id(table);
    let commit_id = uuid::Uuid::now_v7().simple();
    let mut metadata_files = Vec::new();
    let mut manifest_count = 0;
    let mut new_manifest =
        |metadata_files: &mut Vec<String>| -> Result<ManifestWriter, MaintenanceError> {
            manifest_count += 1;
            let path = format!(
                "{}/metadata/{commit_id}-m{manifest_count}.avro",
                metadata.location()
            );
            metadata_files.push(path.clone());
            Ok(ManifestWriterBuilder::new(
                table.file_io().new_output(&path)?,
                Some(snapshot_id),
                metadata.current_schema().clone(),
                metadata.default_partition_spec().as_ref().clone(),
            )
            .build_v2_data())
        };

    let list = table.manifest_list_reader(base).load().await?;
    let mut manifests: Vec<ManifestFile> = Vec::new();
    let mut removed: Vec<ManifestEntryRef> = Vec::new();
    for manifest_file in list.entries() {
        if manifest_file.content != ManifestContentType::Data {
            manifests.push(manifest_file.clone());
            continue;
        }
        // An earlier compaction's removal records: dropped here, as Iceberg's own rewrites drop
        // them. This is the only place they leave the table's current state — the writer's
        // appends carry them forward — and until they have, expiry can't delete the files they
        // name (see `expiry`).
        if !manifest_file.has_added_files() && !manifest_file.has_existing_files() {
            continue;
        }
        let manifest = manifest_file.load_manifest(table.file_io()).await?;
        let touched = manifest
            .entries()
            .iter()
            .any(|entry| entry.is_alive() && inputs.contains(entry.file_path()));
        if !touched {
            manifests.push(manifest_file.clone());
            continue;
        }
        // Rewritten without the inputs, everything else as it was.
        let mut kept = Vec::new();
        for entry in manifest.entries().iter().filter(|entry| entry.is_alive()) {
            if inputs.contains(entry.file_path()) {
                removed.push(entry.clone());
            } else {
                kept.push(entry);
            }
        }
        if kept.is_empty() {
            continue;
        }
        let mut writer = new_manifest(&mut metadata_files)?;
        for entry in kept {
            writer.add_existing_file(
                entry.data_file().clone(),
                entry
                    .snapshot_id()
                    .unwrap_or(manifest_file.added_snapshot_id),
                sequence_number_of(entry)?,
                entry.file_sequence_number,
            )?;
        }
        manifests.push(writer.write_manifest_file().await?);
    }
    if removed.len() != inputs.len() {
        delete_files(table, metadata_files.iter()).await;
        return Ok(None);
    }

    // The inputs' removal records, in a manifest of their own so the next compaction can drop
    // them whole. RustFS's catalog requires them (a replace must name what it removes), and
    // checks on every later commit that each file they name still exists.
    let mut writer = new_manifest(&mut metadata_files)?;
    for entry in &removed {
        writer.add_delete_file(
            entry.data_file().clone(),
            sequence_number_of(entry)?,
            entry.file_sequence_number,
        )?;
    }
    manifests.push(writer.write_manifest_file().await?);
    let removed: Vec<DataFile> = removed
        .iter()
        .map(|entry| entry.data_file().clone())
        .collect();

    // The new files keep the data sequence number of the snapshot they were read from, like
    // Iceberg's own rewrites (`use-starting-sequence-number`): to any delete applying to the
    // rows they hold, they are as old as those rows.
    let mut writer = new_manifest(&mut metadata_files)?;
    for file in output {
        writer.add_file(file.clone(), data_sequence_number)?;
    }
    manifests.push(writer.write_manifest_file().await?);

    let sequence_number = metadata.next_sequence_number();
    let list_path = format!(
        "{}/metadata/snap-{snapshot_id}-{attempt}-{commit_id}.avro",
        metadata.location()
    );
    metadata_files.push(list_path.clone());
    let mut list_writer = ManifestListWriter::v2(
        table.file_io().new_output(&list_path)?.writer().await?,
        snapshot_id,
        Some(base.snapshot_id()),
        sequence_number,
    );
    list_writer.add_manifests(manifests.into_iter())?;
    list_writer.close().await?;

    let snapshot = Snapshot::builder()
        .with_snapshot_id(snapshot_id)
        .with_parent_snapshot_id(Some(base.snapshot_id()))
        .with_sequence_number(sequence_number)
        .with_timestamp_ms(chrono::Utc::now().timestamp_millis())
        .with_manifest_list(list_path)
        .with_summary(replace_summary(table, base, &removed, output))
        .with_schema_id(metadata.current_schema_id())
        .build();

    Ok(Some(StagedReplace {
        requirements: vec![
            TableRequirement::UuidMatch {
                uuid: metadata.uuid(),
            },
            TableRequirement::RefSnapshotIdMatch {
                r#ref: MAIN_BRANCH.to_string(),
                snapshot_id: Some(base.snapshot_id()),
            },
        ],
        updates: vec![
            TableUpdate::AddSnapshot { snapshot },
            TableUpdate::SetSnapshotRef {
                ref_name: MAIN_BRANCH.to_string(),
                reference: SnapshotReference::new(
                    snapshot_id,
                    SnapshotRetention::branch(None, None, None),
                ),
            },
        ],
        metadata_files,
    }))
}

/// The summary of the replace snapshot on top of `base`: what it added and removed, the
/// table totals carried forward, and — what dedup depends on — `table`'s checkpoint.
fn replace_summary(
    table: &Table,
    base: &Snapshot,
    removed: &[DataFile],
    added: &[DataFile],
) -> Summary {
    let metadata = table.metadata();
    let mut collector = SnapshotSummaryCollector::default();
    for file in added {
        collector.add_file(
            file,
            metadata.current_schema().clone(),
            metadata.default_partition_spec().clone(),
        );
    }
    for file in removed {
        collector.remove_file(
            file,
            metadata.current_schema().clone(),
            metadata.default_partition_spec().clone(),
        );
    }
    let mut properties = collector.build();

    let previous = &base.summary().additional_properties;
    let count = |properties: &HashMap<String, String>, key: &str| {
        properties
            .get(key)
            .and_then(|value| value.parse::<u64>().ok())
    };
    for (total, added, removed) in [
        ("total-data-files", "added-data-files", "deleted-data-files"),
        ("total-records", "added-records", "deleted-records"),
        ("total-files-size", "added-files-size", "removed-files-size"),
    ] {
        if let Some(previous_total) = count(previous, total) {
            let value = (previous_total + count(&properties, added).unwrap_or(0))
                .saturating_sub(count(&properties, removed).unwrap_or(0));
            properties.insert(total.to_string(), value.to_string());
        }
    }
    for unchanged in [
        "total-delete-files",
        "total-position-deletes",
        "total-equality-deletes",
    ] {
        if let Some(value) = previous.get(unchanged) {
            properties.insert(unchanged.to_string(), value.clone());
        }
    }

    match latest_checkpoint(table) {
        Some(Checkpoint::Commit(position)) => {
            properties.insert(COMMIT_POSITION_PROPERTY.to_string(), position);
        }
        Some(Checkpoint::LegacyChange(position)) => {
            properties.insert(LEGACY_POSITION_PROPERTY.to_string(), position);
        }
        None => {}
    }
    if let Some(source_id) = previous.get(COMMIT_SOURCE_ID_PROPERTY) {
        properties.insert(COMMIT_SOURCE_ID_PROPERTY.to_string(), source_id.clone());
    }
    properties.insert(MAINTENANCE_PROPERTY.to_string(), "compaction".to_string());

    Summary {
        operation: Operation::Replace,
        additional_properties: properties,
    }
}

fn sequence_number_of(entry: &ManifestEntryRef) -> Result<i64, MaintenanceError> {
    entry.sequence_number().ok_or_else(|| {
        MaintenanceError::Io(format!("{} has no sequence number", entry.file_path()))
    })
}

/// A positive snapshot id no snapshot of `table` has (Iceberg's own scheme).
fn new_snapshot_id(table: &Table) -> i64 {
    loop {
        let (high, low) = uuid::Uuid::now_v7().as_u64_pair();
        let id = ((high ^ low) & i64::MAX as u64) as i64;
        if id != 0 && table.metadata().snapshot_by_id(id).is_none() {
            return id;
        }
    }
}

async fn load(catalog: &dyn Catalog, ident: &TableIdent) -> Result<Table, MaintenanceError> {
    catalog
        .load_table(ident)
        .await
        .map_err(|err| MaintenanceError::Catalog(err.to_string()))
}

/// Deletes `paths`, best effort: whatever is left behind the orphan sweep removes later.
pub async fn delete_files(table: &Table, paths: impl Iterator<Item = impl AsRef<str>>) -> usize {
    let mut deleted = 0;
    for path in paths {
        match table.file_io().delete(path.as_ref()).await {
            Ok(()) => deleted += 1,
            Err(err) => {
                tracing::warn!(path = path.as_ref(), error = %err, "could not delete a file")
            }
        }
    }
    deleted
}
