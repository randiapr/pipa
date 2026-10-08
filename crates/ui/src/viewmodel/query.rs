//! ViewModel: the SQL query page. Queries run against the selected project's tables only.

use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::viewmodel::{PagedList, SessionViewModel};

#[derive(Copy, Clone)]
pub struct QueryViewModel {
    pub sql: RwSignal<String>,
    /// The rows of the last successful run, a page at a time.
    pub results: PagedList<serde_json::Value>,
    /// Whether `results` holds a run's rows (even none) — false before the first run and
    /// after a failed one.
    pub ran: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
    pub running: RwSignal<bool>,
    session: SessionViewModel,
}

impl QueryViewModel {
    pub fn new(session: SessionViewModel) -> Self {
        Self {
            sql: RwSignal::new(String::new()),
            results: PagedList::new(),
            ran: RwSignal::new(false),
            error: RwSignal::new(None),
            running: RwSignal::new(false),
            session,
        }
    }

    pub fn run(&self, ev: SubmitEvent) {
        ev.prevent_default();

        let project_id = self.session.current_project_id.get_untracked();
        if project_id.is_none() && !self.session.is_admin() {
            self.error.set(Some(
                "Select a project first to query its tables.".to_string(),
            ));
            return;
        }

        let this = *self;
        let sql = this.sql.get();
        this.running.set(true);
        this.error.set(None);
        spawn_local(async move {
            match api::query(&sql, project_id.as_deref()).await {
                Ok(serde_json::Value::Array(rows)) => this.show(rows),
                Ok(other) => this.show(vec![other]),
                Err(err) => {
                    this.results.reset(Vec::new());
                    this.ran.set(false);
                    this.error.set(Some(err));
                }
            }
            this.running.set(false);
        });
    }

    fn show(&self, rows: Vec<serde_json::Value>) {
        self.results.reset(rows);
        self.ran.set(true);
    }
}

/// Column names of a result, in order of first appearance across its rows.
pub fn columns_of(rows: &[serde_json::Value]) -> Vec<String> {
    let mut columns: Vec<String> = Vec::new();
    for row in rows {
        if let Some(object) = row.as_object() {
            for key in object.keys() {
                if !columns.contains(key) {
                    columns.push(key.clone());
                }
            }
        }
    }
    columns
}

/// A result cell as plain text: strings unquoted, `null` empty, anything else as JSON.
pub fn cell_text(value: Option<&serde_json::Value>) -> String {
    match value {
        None | Some(serde_json::Value::Null) => String::new(),
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}
