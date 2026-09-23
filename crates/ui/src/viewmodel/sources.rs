//! ViewModel: reactive state and commands for the data source registration form and list.

use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::model::{
    ConnectionConfig, ConnectionTestOutcome, DataSourceView, DbEngine, NewDataSource, ProjectView,
};
use crate::viewmodel::PAGE_SIZE;

/// Reactive state for the data source registration form and table, plus the commands that
/// mutate it via [`crate::api`]. Every field is an `RwSignal` handle, so the whole struct is
/// cheap to `Copy` — views hold it by value and read/call straight through it.
#[derive(Copy, Clone)]
pub struct SourcesViewModel {
    pub sources: RwSignal<Vec<DataSourceView>>,
    pub page: RwSignal<usize>,
    pub name: RwSignal<String>,
    pub engine: RwSignal<DbEngine>,
    pub host: RwSignal<String>,
    pub port: RwSignal<String>,
    pub username: RwSignal<String>,
    pub password: RwSignal<String>,
    pub database: RwSignal<String>,
    pub selected_project_id: RwSignal<String>,
    /// The Projects list, shared with [`crate::viewmodel::ProjectsViewModel`] — used for the
    /// project picker and for labeling each row with its project's current name.
    projects: RwSignal<Vec<ProjectView>>,
    /// Shared with the rest of the dashboard, so failures here surface in the same banner.
    status: RwSignal<Option<String>>,
}

impl SourcesViewModel {
    pub fn new(projects: RwSignal<Vec<ProjectView>>, status: RwSignal<Option<String>>) -> Self {
        Self {
            sources: RwSignal::new(Vec::new()),
            page: RwSignal::new(0),
            name: RwSignal::new(String::new()),
            engine: RwSignal::new(DbEngine::Postgres),
            host: RwSignal::new(String::new()),
            port: RwSignal::new(String::new()),
            username: RwSignal::new(String::new()),
            password: RwSignal::new(String::new()),
            database: RwSignal::new(String::new()),
            selected_project_id: RwSignal::new(String::new()),
            projects,
            status,
        }
    }

    pub fn refresh(&self) {
        let sources = self.sources;
        let status = self.status;
        spawn_local(async move {
            match api::list_sources().await {
                Ok(list) => sources.set(list),
                Err(err) => status.set(Some(format!("Failed to load data sources: {err}"))),
            }
        });
    }

    /// The current page's slice, clamped to the last valid page (e.g. after a delete shrinks
    /// the list past the page the user was on).
    pub fn paged(&self) -> Vec<DataSourceView> {
        let all = self.sources.get();
        let total_pages = all.len().div_ceil(PAGE_SIZE).max(1);
        let page = self.page.get().min(total_pages - 1);
        all.into_iter()
            .skip(page * PAGE_SIZE)
            .take(PAGE_SIZE)
            .collect()
    }

    pub fn total(&self) -> Signal<usize> {
        let sources = self.sources;
        Signal::derive(move || sources.get().len())
    }

    /// The projects available for the registration form's project picker.
    pub fn projects(&self) -> Vec<ProjectView> {
        self.projects.get()
    }

    /// The current display name of a source's project, if it has one and it still exists.
    pub fn project_label(&self, project_id: &Option<String>) -> Option<String> {
        let id = project_id.as_ref()?;
        self.projects
            .get()
            .into_iter()
            .find(|p| &p.id == id)
            .map(|p| p.name)
    }

    pub fn submit_new(&self, ev: SubmitEvent) {
        ev.prevent_default();

        let port_value: u16 = match self.port.get().trim().parse() {
            Ok(parsed) => parsed,
            Err(_) => {
                self.status
                    .set(Some("Port must be a valid number.".to_string()));
                return;
            }
        };

        let new_source = NewDataSource {
            name: self.name.get(),
            engine: self.engine.get(),
            connection: ConnectionConfig {
                host: self.host.get(),
                port: port_value,
                username: self.username.get(),
                password: self.password.get(),
                database: self.database.get(),
            },
            project_id: Some(self.selected_project_id.get()).filter(|id| !id.is_empty()),
        };

        let this = *self;
        spawn_local(async move {
            match api::register_source(&new_source).await {
                Ok(_) => {
                    this.status.set(Some("Data source registered.".to_string()));
                    this.name.set(String::new());
                    this.host.set(String::new());
                    this.port.set(String::new());
                    this.username.set(String::new());
                    this.password.set(String::new());
                    this.database.set(String::new());
                    this.selected_project_id.set(String::new());
                    this.refresh();
                }
                Err(err) => this
                    .status
                    .set(Some(format!("Failed to register data source: {err}"))),
            }
        });
    }

    pub fn test(&self, id: String, result: RwSignal<Option<ConnectionTestOutcome>>) {
        let status = self.status;
        spawn_local(async move {
            match api::test_source(&id).await {
                Ok(outcome) => result.set(Some(outcome)),
                Err(err) => status.set(Some(format!("Connection test failed: {err}"))),
            }
        });
    }

    pub fn delete(&self, id: String) {
        let this = *self;
        spawn_local(async move {
            match api::delete_source(&id).await {
                Ok(()) => this.refresh(),
                Err(err) => this
                    .status
                    .set(Some(format!("Failed to remove data source: {err}"))),
            }
        });
    }
}
