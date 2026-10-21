// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! [`MaintenanceScheduler`]: maintains the target tables of the sources this instance captures,
//! one table at a time, on an interval.
//!
//! Only owned sources are maintained, so a table's compaction runs in the same process as its
//! writer and two instances don't compact the same table — except briefly around a handoff,
//! which is safe (a compaction whose inputs another commit removed is dropped) but wasteful,
//! so ownership is checked again before each table.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use crate::datasource::DataSource;
use crate::maintenance::domain::{MaintenanceReport, TableMaintainer};

pub struct MaintenanceScheduler {
    maintainer: Arc<dyn TableMaintainer>,
    interval: Duration,
}

impl MaintenanceScheduler {
    pub fn new(maintainer: Arc<dyn TableMaintainer>, interval: Duration) -> Self {
        Self {
            maintainer,
            interval,
        }
    }

    /// Runs forever, every `interval` maintaining the tables of the sources `owned` holds. The
    /// first run is one interval after the start, leaving startup to the captures.
    pub async fn run(self, owned: watch::Receiver<Vec<DataSource>>) {
        let mut ticker = tokio::time::interval(self.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            self.run_once(&owned).await;
        }
    }

    /// Maintains every table of every owned source once, sequentially, which bounds the memory
    /// and IO maintenance takes. A table that fails is logged and retried at the next run; the
    /// others go on.
    pub async fn run_once(&self, owned: &watch::Receiver<Vec<DataSource>>) -> MaintenanceReport {
        let mut total = MaintenanceReport::default();
        let sources = owned.borrow().clone();
        for source in sources {
            let tables = match self.maintainer.tables(&source).await {
                Ok(tables) => tables,
                Err(err) => {
                    tracing::warn!(id = %source.id, error = %err, "could not list a source's tables for maintenance");
                    continue;
                }
            };
            for table in tables {
                if !owned.borrow().iter().any(|owned| owned.id == source.id) {
                    tracing::info!(id = %source.id, "source no longer captured here, stopping its maintenance");
                    break;
                }
                match self.maintainer.maintain(&table).await {
                    Ok(report) if report.is_empty() => {}
                    Ok(report) => {
                        tracing::info!(namespace = %table.namespace, table = %table.table, "maintained: {report}");
                        total.files_rewritten += report.files_rewritten;
                        total.bytes_rewritten += report.bytes_rewritten;
                        total.files_written += report.files_written;
                        total.compactions += report.compactions;
                        total.snapshots_expired += report.snapshots_expired;
                        total.files_deleted += report.files_deleted;
                        total.orphans_deleted += report.orphans_deleted;
                    }
                    Err(err) => {
                        tracing::warn!(namespace = %table.namespace, table = %table.table, error = %err, "table maintenance failed, retrying at the next run");
                    }
                }
            }
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;
    use crate::datasource::{ConnectionConfig, DataSourceId, DbEngine};
    use crate::maintenance::domain::MaintenanceError;
    use crate::write::domain::{TargetTable, namespace_for_source};

    fn source() -> DataSource {
        DataSource {
            id: DataSourceId(uuid::Uuid::now_v7()),
            name: "s".to_string(),
            engine: DbEngine::Postgres,
            connection: ConnectionConfig {
                host: "h".to_string(),
                port: 5432,
                database: "d".to_string(),
                username: "u".to_string(),
                password: "p".to_string(),
            },
            ingested_tables: None,
        }
    }

    /// Two tables per source; maintaining one named `fails` errors, and maintaining any table
    /// of `drop_on_maintain` takes that source away from `owned`.
    struct FakeMaintainer {
        maintained: Mutex<Vec<String>>,
        owned: watch::Sender<Vec<DataSource>>,
        drop_on_maintain: Option<DataSourceId>,
    }

    #[async_trait]
    impl TableMaintainer for FakeMaintainer {
        async fn tables(&self, source: &DataSource) -> Result<Vec<TargetTable>, MaintenanceError> {
            Ok(["fails", "ok"]
                .into_iter()
                .map(|table| TargetTable {
                    namespace: namespace_for_source(source),
                    table: table.to_string(),
                })
                .collect())
        }

        async fn maintain(
            &self,
            table: &TargetTable,
        ) -> Result<MaintenanceReport, MaintenanceError> {
            self.maintained
                .lock()
                .unwrap()
                .push(format!("{}.{}", table.namespace, table.table));
            if let Some(dropped) = self.drop_on_maintain {
                self.owned
                    .send_modify(|owned| owned.retain(|source| source.id != dropped));
            }
            if table.table == "fails" {
                return Err(MaintenanceError::Io("boom".to_string()));
            }
            Ok(MaintenanceReport {
                compactions: 1,
                ..Default::default()
            })
        }
    }

    #[tokio::test]
    async fn every_table_of_every_owned_source_is_maintained_despite_failures() {
        let (a, b) = (source(), source());
        let (owned_tx, owned) = watch::channel(vec![a.clone(), b.clone()]);
        let maintainer = Arc::new(FakeMaintainer {
            maintained: Mutex::new(Vec::new()),
            owned: owned_tx,
            drop_on_maintain: None,
        });
        let scheduler = MaintenanceScheduler::new(maintainer.clone(), Duration::from_secs(1));

        let report = scheduler.run_once(&owned).await;

        assert_eq!(report.compactions, 2, "the failing tables add nothing");
        let maintained = maintainer.maintained.lock().unwrap().clone();
        let (na, nb) = (namespace_for_source(&a), namespace_for_source(&b));
        assert_eq!(
            maintained,
            [
                format!("{na}.fails"),
                format!("{na}.ok"),
                format!("{nb}.fails"),
                format!("{nb}.ok")
            ]
        );
    }

    #[tokio::test]
    async fn a_source_that_moves_away_stops_being_maintained() {
        let (a, b) = (source(), source());
        let (owned_tx, owned) = watch::channel(vec![a.clone(), b.clone()]);
        let maintainer = Arc::new(FakeMaintainer {
            maintained: Mutex::new(Vec::new()),
            owned: owned_tx,
            drop_on_maintain: Some(a.id),
        });
        let scheduler = MaintenanceScheduler::new(maintainer.clone(), Duration::from_secs(1));

        scheduler.run_once(&owned).await;

        let maintained = maintainer.maintained.lock().unwrap().clone();
        let (na, nb) = (namespace_for_source(&a), namespace_for_source(&b));
        // a moved away while its first table was being maintained; b is still ours.
        assert_eq!(
            maintained,
            [
                format!("{na}.fails"),
                format!("{nb}.fails"),
                format!("{nb}.ok")
            ]
        );
    }
}
