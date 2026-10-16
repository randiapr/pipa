//! Concrete [`TableMaintainer`]: compaction, snapshot expiry and the orphan sweep over the
//! Iceberg REST catalog and the object store the tables live in.

mod commit;
mod compaction;
mod expiry;
mod files;
mod orphans;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use iceberg::{Catalog, NamespaceIdent, TableIdent};

pub use commit::{RestSnapshotCommitter, SnapshotCommitter};
pub use orphans::OrphanSweeper;

use crate::datasource::DataSource;
use crate::maintenance::domain::{
    CompactionPolicy, ExpiryPolicy, MaintenanceError, MaintenanceReport, TableMaintainer,
};
use crate::write::domain::{TargetTable, namespace_for_source};

/// How often a table's location is listed for orphans at most: listing is the costly part of
/// maintenance and orphans are rare, so it doesn't happen on every run.
const ORPHAN_SWEEP_INTERVAL: Duration = Duration::from_secs(3600);

/// [`TableMaintainer`] for pipa's Iceberg target tables. Each run of a table compacts it first
/// (so the files it replaces start ageing out), then expires snapshots past the retention (which
/// frees the files of earlier compactions once nothing reads them), then sweeps orphans.
pub struct IcebergTableMaintainer {
    catalog: Arc<dyn Catalog>,
    committer: Arc<dyn SnapshotCommitter>,
    compaction: CompactionPolicy,
    expiry: ExpiryPolicy,
    orphans: Option<OrphanSweeper>,
    /// When each table was last swept for orphans, by this process.
    last_swept: Mutex<HashMap<TableIdent, Instant>>,
}

impl IcebergTableMaintainer {
    pub fn new(
        catalog: Arc<dyn Catalog>,
        committer: Arc<dyn SnapshotCommitter>,
        compaction: CompactionPolicy,
        expiry: ExpiryPolicy,
        orphans: Option<OrphanSweeper>,
    ) -> Self {
        Self {
            catalog,
            committer,
            compaction,
            expiry,
            orphans,
            last_swept: Mutex::new(HashMap::new()),
        }
    }

    fn ident(table: &TargetTable) -> TableIdent {
        TableIdent::new(
            NamespaceIdent::new(table.namespace.clone()),
            table.table.clone(),
        )
    }

    /// Whether `ident` is due an orphan sweep, marking it swept if so.
    fn sweep_due(&self, ident: &TableIdent) -> bool {
        let mut last_swept = self.last_swept.lock().expect("never poisoned");
        let now = Instant::now();
        let due = last_swept
            .get(ident)
            .is_none_or(|last| now.duration_since(*last) >= ORPHAN_SWEEP_INTERVAL);
        if due {
            last_swept.insert(ident.clone(), now);
        }
        due
    }
}

#[async_trait]
impl TableMaintainer for IcebergTableMaintainer {
    async fn tables(&self, source: &DataSource) -> Result<Vec<TargetTable>, MaintenanceError> {
        let namespace = NamespaceIdent::new(namespace_for_source(source));
        let catalog_error = |err: iceberg::Error| MaintenanceError::Catalog(err.to_string());
        if !self
            .catalog
            .namespace_exists(&namespace)
            .await
            .map_err(catalog_error)?
        {
            return Ok(Vec::new());
        }
        Ok(self
            .catalog
            .list_tables(&namespace)
            .await
            .map_err(catalog_error)?
            .into_iter()
            .map(|ident| TargetTable {
                namespace: namespace_for_source(source),
                table: ident.name().to_string(),
            })
            .collect())
    }

    async fn maintain(&self, table: &TargetTable) -> Result<MaintenanceReport, MaintenanceError> {
        let ident = Self::ident(table);
        let mut report = MaintenanceReport::default();
        compaction::compact(
            self.catalog.as_ref(),
            self.committer.as_ref(),
            &self.compaction,
            &ident,
            &mut report,
        )
        .await?;
        expiry::expire(self.catalog.as_ref(), &self.expiry, &ident, &mut report).await?;
        if let Some(sweeper) = self.orphans.as_ref().filter(|_| self.sweep_due(&ident)) {
            let current = self
                .catalog
                .load_table(&ident)
                .await
                .map_err(|err| MaintenanceError::Catalog(err.to_string()))?;
            sweeper.sweep(&current, &mut report).await?;
        }
        Ok(report)
    }
}
