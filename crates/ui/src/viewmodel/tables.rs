//! ViewModel: the Tables page — the Iceberg tables of the selected project and a paged,
//! read-only view of one table's rows. Open to every role with access to the project; this is
//! how the view-only `user` role sees data.

use leptos::prelude::*;
use leptos::task::spawn_local;
use pipa_api::TableView;

use crate::api;
use crate::viewmodel::{PageSize, SessionViewModel, StatusMessage};

/// Suffixes of the metadata tables iceberg-datafusion lists next to every Iceberg table
/// (`orders$snapshots`, `orders$manifests`). Not data, so the Tables page doesn't list them.
const METADATA_TABLE_SUFFIXES: [&str; 2] = ["$snapshots", "$manifests"];

/// Columns `pipa-ingestion` adds to every changelog row (`crates/ingestion/src/write/domain.rs`).
/// They describe the change, not the data, so the Tables page doesn't show them.
const CHANGELOG_COLUMNS: [&str; 4] = ["_op", "_source_id", "_position", "_commit_timestamp_us"];

fn is_metadata_table(name: &str) -> bool {
    METADATA_TABLE_SUFFIXES
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

#[derive(Copy, Clone)]
pub struct TablesViewModel {
    /// Every table the project's data sources expose, as `GET /tables` returns them; the page
    /// lists [`Self::visible_tables`].
    pub tables: RwSignal<Vec<TableView>>,
    /// The table whose rows are shown below the list.
    pub open: RwSignal<Option<TableView>>,
    /// The current page of the open table; `None` while loading or before one is opened.
    pub rows: RwSignal<Option<Vec<serde_json::Value>>>,
    /// Rows skipped before the current page.
    pub offset: RwSignal<usize>,
    pub loading: RwSignal<bool>,
    /// Rows fetched per page: the picker's choice on desktop (at most 100), fewer on mobile.
    pub page_size: PageSize,
    pub error: RwSignal<Option<String>>,
    session: SessionViewModel,
    status: RwSignal<Option<StatusMessage>>,
}

impl TablesViewModel {
    pub fn new(session: SessionViewModel, status: RwSignal<Option<StatusMessage>>) -> Self {
        Self {
            tables: RwSignal::new(Vec::new()),
            open: RwSignal::new(None),
            rows: RwSignal::new(None),
            offset: RwSignal::new(0),
            loading: RwSignal::new(false),
            page_size: PageSize::new(),
            error: RwSignal::new(None),
            session,
            status,
        }
    }

    /// Reloads the selected project's tables. Reads the selection reactively, so calling this
    /// from an `Effect` re-runs it when the project is switched. Whatever was open belonged to
    /// the previous project (or may be gone), so it is closed.
    pub fn refresh(&self) {
        self.close();
        self.tables.set(Vec::new());
        let Some(project_id) = self.session.current_project_id.get() else {
            return;
        };
        let this = *self;
        this.loading.set(true);
        spawn_local(async move {
            match api::list_tables(&project_id).await {
                Ok(list) => this.tables.set(list),
                Err(err) => this.status.set(Some(StatusMessage::Error(format!(
                    "Failed to load tables: {err}"
                )))),
            }
            this.loading.set(false);
        });
    }

    /// The tables to list: the data tables, without their Iceberg metadata tables, for every
    /// role. The backend already leaves those out; this is display only.
    pub fn visible_tables(&self) -> Vec<TableView> {
        self.tables.with(|tables| {
            tables
                .iter()
                .filter(|table| !is_metadata_table(&table.name))
                .cloned()
                .collect()
        })
    }

    /// The open table's current page as shown, without the changelog columns
    /// ([`CHANGELOG_COLUMNS`]) for every role. The backend already returns current rows without
    /// them; this also covers a table it reads as stored (no recorded row key).
    pub fn visible_rows(&self) -> Option<Vec<serde_json::Value>> {
        let rows = self.rows.get()?;
        Some(rows.into_iter().map(without_changelog_columns).collect())
    }

    pub fn close(&self) {
        self.open.set(None);
        self.rows.set(None);
        self.error.set(None);
        self.offset.set(0);
    }

    /// Shows the first page of `table`.
    pub fn open_table(&self, table: TableView) {
        self.open.set(Some(table));
        self.load(0);
    }

    pub fn next(&self) {
        self.load(self.offset.get_untracked() + self.page_size.get_untracked());
    }

    pub fn prev(&self) {
        self.load(
            self.offset
                .get_untracked()
                .saturating_sub(self.page_size.get_untracked()),
        );
    }

    /// Fetches the open table's page again from the same row, e.g. after the page size changed.
    pub fn reload(&self) {
        self.load(self.offset.get_untracked());
    }

    pub fn has_prev(&self) -> bool {
        self.offset.get() > 0
    }

    /// A full page suggests there may be more rows after it.
    pub fn has_next(&self) -> bool {
        let size = self.page_size.get();
        self.rows
            .with(|rows| rows.as_ref().is_some_and(|rows| rows.len() >= size))
    }

    fn load(&self, offset: usize) {
        let (Some(project_id), Some(table)) = (
            self.session.current_project_id.get_untracked(),
            self.open.get_untracked(),
        ) else {
            return;
        };
        let this = *self;
        let limit = this.page_size.get_untracked();
        this.offset.set(offset);
        this.rows.set(None);
        this.error.set(None);
        this.loading.set(true);
        spawn_local(async move {
            match api::read_table(&project_id, &table, limit, offset).await {
                Ok(serde_json::Value::Array(rows)) => this.rows.set(Some(rows)),
                Ok(other) => this.rows.set(Some(vec![other])),
                Err(err) => this.error.set(Some(err)),
            }
            this.loading.set(false);
        });
    }
}

fn without_changelog_columns(mut row: serde_json::Value) -> serde_json::Value {
    if let Some(columns) = row.as_object_mut() {
        for column in CHANGELOG_COLUMNS {
            columns.remove(column);
        }
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_iceberg_metadata_tables() {
        assert!(is_metadata_table("public__orders$snapshots"));
        assert!(is_metadata_table("public__orders$manifests"));
        assert!(!is_metadata_table("public__orders"));
        // `$` is legal in a Postgres table name; only the metadata suffixes count.
        assert!(!is_metadata_table("public__price$list"));
    }

    #[test]
    fn strips_only_the_changelog_columns() {
        let row = serde_json::json!({
            "_op": "update",
            "_source_id": "01a11ed5",
            "_position": "0/1CC23E0",
            "_commit_timestamp_us": 1791518059872411_i64,
            "id": 1,
            "tier": "silver",
            "_note": "kept: not a changelog column",
        });
        assert_eq!(
            without_changelog_columns(row),
            serde_json::json!({ "id": 1, "tier": "silver", "_note": "kept: not a changelog column" })
        );
    }
}
