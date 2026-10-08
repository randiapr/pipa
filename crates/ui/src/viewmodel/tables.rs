//! ViewModel: the Tables page — the Iceberg tables of the selected project and a paged,
//! read-only view of one table's rows. Open to every role with access to the project; this is
//! how the view-only `user` role sees data.

use leptos::prelude::*;
use leptos::task::spawn_local;
use pipa_api::TableView;

use crate::api;
use crate::viewmodel::{SessionViewModel, StatusMessage};

/// Rows fetched per page of a table.
pub const TABLE_PAGE_ROWS: usize = 100;

#[derive(Copy, Clone)]
pub struct TablesViewModel {
    pub tables: RwSignal<Vec<TableView>>,
    /// The table whose rows are shown below the list.
    pub open: RwSignal<Option<TableView>>,
    /// The current page of the open table; `None` while loading or before one is opened.
    pub rows: RwSignal<Option<Vec<serde_json::Value>>>,
    /// Rows skipped before the current page.
    pub offset: RwSignal<usize>,
    pub loading: RwSignal<bool>,
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
        self.load(self.offset.get_untracked() + TABLE_PAGE_ROWS);
    }

    pub fn prev(&self) {
        self.load(self.offset.get_untracked().saturating_sub(TABLE_PAGE_ROWS));
    }

    pub fn has_prev(&self) -> bool {
        self.offset.get() > 0
    }

    /// A full page suggests there may be more rows after it.
    pub fn has_next(&self) -> bool {
        self.rows.with(|rows| {
            rows.as_ref()
                .is_some_and(|rows| rows.len() >= TABLE_PAGE_ROWS)
        })
    }

    fn load(&self, offset: usize) {
        let (Some(project_id), Some(table)) = (
            self.session.current_project_id.get_untracked(),
            self.open.get_untracked(),
        ) else {
            return;
        };
        let this = *self;
        this.offset.set(offset);
        this.rows.set(None);
        this.error.set(None);
        this.loading.set(true);
        spawn_local(async move {
            match api::read_table(&project_id, &table, TABLE_PAGE_ROWS, offset).await {
                Ok(serde_json::Value::Array(rows)) => this.rows.set(Some(rows)),
                Ok(other) => this.rows.set(Some(vec![other])),
                Err(err) => this.error.set(Some(err)),
            }
            this.loading.set(false);
        });
    }
}
