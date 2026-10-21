// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! Orphan sweep: deletes files under a table's location that no snapshot references — what a
//! commit that never landed leaves behind (a crash between writing a batch and committing it,
//! a compaction whose commit state was unknown).
//!
//! Only the file kinds pipa writes are considered (Parquet data files under `data/`, Avro
//! manifests and manifest lists under `metadata/`, never the catalog's own `*.metadata.json`),
//! and only once older than the configured age: a file a commit in flight has written is
//! unreferenced until that commit lands, and the age is what keeps it safe.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use futures::TryStreamExt;
use iceberg::table::Table;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt};

use super::files::referenced_files;
use crate::maintenance::domain::{MaintenanceError, MaintenanceReport};

/// Lists and deletes table files through the object store tables live in.
pub struct OrphanSweeper {
    store: Arc<dyn ObjectStore>,
    /// What table locations start with in `store` — `s3://{bucket}/` for the RustFS bucket.
    /// A table located elsewhere isn't swept.
    location_prefix: String,
    min_age: Duration,
}

impl OrphanSweeper {
    pub fn new(
        store: Arc<dyn ObjectStore>,
        location_prefix: impl Into<String>,
        min_age: Duration,
    ) -> Self {
        Self {
            store,
            location_prefix: location_prefix.into(),
            min_age,
        }
    }

    /// `location` as a path in the store, if it is in it.
    fn store_path(&self, location: &str) -> Option<Path> {
        location
            .strip_prefix(&self.location_prefix)
            .and_then(|rest| Path::parse(rest.trim_start_matches('/')).ok())
    }

    /// Deletes `table`'s orphans, adding them to `report`.
    pub async fn sweep(
        &self,
        table: &Table,
        report: &mut MaintenanceReport,
    ) -> Result<(), MaintenanceError> {
        let Some(root) = self.store_path(table.metadata().location()) else {
            tracing::debug!(
                table = %table.identifier(),
                location = table.metadata().location(),
                "not sweeping a table located outside the object store"
            );
            return Ok(());
        };

        // Read before listing: a file committed after this point is younger than `min_age`.
        let referenced: HashSet<Path> = referenced_files(
            table,
            &table.metadata().snapshots().cloned().collect::<Vec<_>>(),
        )
        .await?
        .iter()
        .filter_map(|location| self.store_path(location))
        .chain(
            table
                .metadata()
                .statistics_iter()
                .filter_map(|statistics| self.store_path(&statistics.statistics_path)),
        )
        .collect();
        let cutoff = chrono::Utc::now()
            - chrono::Duration::from_std(self.min_age).unwrap_or(chrono::Duration::MAX);

        for (directory, extension) in [("data", ".parquet"), ("metadata", ".avro")] {
            let prefix = root.clone().join(directory);
            let mut listing = self.store.list(Some(&prefix));
            while let Some(object) = listing
                .try_next()
                .await
                .map_err(|err| MaintenanceError::Io(err.to_string()))?
            {
                if object.last_modified >= cutoff
                    || !object.location.as_ref().ends_with(extension)
                    || referenced.contains(&object.location)
                {
                    continue;
                }
                match self.store.delete(&object.location).await {
                    Ok(()) => {
                        tracing::info!(path = %object.location, "deleted an orphaned file");
                        report.orphans_deleted += 1;
                    }
                    Err(err) => {
                        tracing::warn!(path = %object.location, error = %err, "could not delete an orphaned file");
                    }
                }
            }
        }
        Ok(())
    }
}
