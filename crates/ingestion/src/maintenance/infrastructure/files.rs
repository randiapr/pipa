// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! Walking a table's snapshots down to the files they reference.

use std::collections::{HashMap, HashSet};

use iceberg::spec::{DataContentType, ManifestContentType, ManifestEntryRef, SnapshotRef};
use iceberg::table::Table;

use crate::maintenance::domain::MaintenanceError;

/// The data files `snapshot` reads, by path. `None` when it also reads delete files: pipa
/// never writes any, and a table someone else added them to isn't one compaction rewrites.
pub async fn live_data_files(
    table: &Table,
    snapshot: &SnapshotRef,
) -> Result<Option<HashMap<String, ManifestEntryRef>>, MaintenanceError> {
    let list = table.manifest_list_reader(snapshot).load().await?;
    let mut live = HashMap::new();
    for manifest_file in list.entries() {
        let manifest = manifest_file.load_manifest(table.file_io()).await?;
        for entry in manifest.entries().iter().filter(|entry| entry.is_alive()) {
            if manifest_file.content != ManifestContentType::Data
                || entry.content_type() != DataContentType::Data
            {
                return Ok(None);
            }
            live.insert(entry.file_path().to_string(), entry.clone());
        }
    }
    Ok(Some(live))
}

/// Every file `snapshots` mention: their manifest lists, manifests and the data/delete files
/// those list, removal records included (RustFS checks those exist too; see `expiry`).
/// Manifests shared between snapshots are read once.
pub async fn referenced_files(
    table: &Table,
    snapshots: &[SnapshotRef],
) -> Result<HashSet<String>, MaintenanceError> {
    let mut files = HashSet::new();
    let mut read_manifests = HashSet::new();
    for snapshot in snapshots {
        files.insert(snapshot.manifest_list().to_string());
        let list = table.manifest_list_reader(snapshot).load().await?;
        for manifest_file in list.entries() {
            files.insert(manifest_file.manifest_path.clone());
            if read_manifests.insert(manifest_file.manifest_path.clone()) {
                let manifest = manifest_file.load_manifest(table.file_io()).await?;
                for entry in manifest.entries() {
                    files.insert(entry.file_path().to_string());
                }
            }
        }
    }
    Ok(files)
}
