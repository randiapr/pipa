// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! Snapshot expiry, and deleting the files only expired snapshots referenced.
//!
//! iceberg's `expire_snapshots` action only rewrites metadata; the files are left to "a
//! higher-level maintenance operation", which this is. A file is deleted only when the snapshots
//! that were expired referenced it and none of the remaining ones mentions it at all — decided
//! from the table's metadata before and after the expiry commit, never from a listing — so a
//! file any retained snapshot can still read is never touched.
//!
//! "Mentions" includes removal records: RustFS's catalog refuses a commit whose manifests name
//! a file that doesn't exist, removed or not, and the writer's appends carry a compaction's
//! removal records forward. So a compaction's inputs outlive it until the next compaction has
//! dropped its records and the snapshots that still carried them have expired.
//!
//! Expiring loses no data here: every target table is an append-only changelog, so every
//! change is still a row of the current snapshot. Only time travel to old table states goes.

use std::collections::HashSet;

use iceberg::table::Table;
use iceberg::transaction::{ApplyTransactionAction, Transaction};
use iceberg::{Catalog, TableIdent};

use super::compaction::delete_files;
use super::files::referenced_files;
use crate::maintenance::domain::{ExpiryPolicy, MaintenanceError, MaintenanceReport};

/// Expires `ident`'s snapshots past `policy` and deletes the files nothing references any more,
/// adding what it did to `report`.
pub async fn expire(
    catalog: &dyn Catalog,
    policy: &ExpiryPolicy,
    ident: &TableIdent,
    report: &mut MaintenanceReport,
) -> Result<(), MaintenanceError> {
    let before = catalog
        .load_table(ident)
        .await
        .map_err(|err| MaintenanceError::Catalog(err.to_string()))?;
    let cutoff_ms = chrono::Utc::now().timestamp_millis()
        - i64::try_from(policy.max_snapshot_age.as_millis()).unwrap_or(i64::MAX);
    if !has_expirable(&before, policy, cutoff_ms) {
        return Ok(());
    }

    let tx = Transaction::new(&before);
    let tx = tx
        .expire_snapshots()
        .expire_older_than_ms(cutoff_ms)
        .retain_last(policy.min_snapshots_to_keep)
        .apply(tx)?;
    let after = tx
        .commit(catalog)
        .await
        .map_err(|err| MaintenanceError::Commit(err.to_string()))?;

    let retained: HashSet<i64> = after
        .metadata()
        .snapshots()
        .map(|snapshot| snapshot.snapshot_id())
        .collect();
    let expired: Vec<_> = before
        .metadata()
        .snapshots()
        .filter(|snapshot| !retained.contains(&snapshot.snapshot_id()))
        .cloned()
        .collect();
    if expired.is_empty() {
        return Ok(());
    }
    report.snapshots_expired += expired.len();

    // Both sides must be read in full before anything goes: a manifest that can't be read
    // could be what keeps a file alive.
    let gone = match referenced_files(&before, &expired).await {
        Ok(files) => files,
        Err(err) => {
            tracing::warn!(table = %ident, error = %err, "expired snapshots, but could not list their files; leaving them for the orphan sweep");
            return Ok(());
        }
    };
    let kept = referenced_files(
        &after,
        &after.metadata().snapshots().cloned().collect::<Vec<_>>(),
    )
    .await?;
    let unreferenced: Vec<&String> = gone.difference(&kept).collect();
    report.files_deleted += delete_files(&after, unreferenced.into_iter()).await;
    Ok(())
}

/// Whether expiry would remove anything: some snapshot is older than the cutoff and isn't
/// among the most recent ones always kept. Saves a metadata commit on every quiet run.
fn has_expirable(table: &Table, policy: &ExpiryPolicy, cutoff_ms: i64) -> bool {
    let snapshots = table.metadata().snapshots().len();
    snapshots > policy.min_snapshots_to_keep
        && table
            .metadata()
            .snapshots()
            .any(|snapshot| snapshot.timestamp_ms() < cutoff_ms)
}
