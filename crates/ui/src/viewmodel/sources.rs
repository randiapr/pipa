//! ViewModel: reactive state and commands for the data source registration form and list.

use std::collections::HashMap;

use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;
use pipa_api::{
    ConnectionConfig, ConnectionTestOutcome, DataSourceView, DbEngine, NewDataSource, ProjectView,
};

use crate::api;
use crate::viewmodel::{PagedList, SessionViewModel, StatusMessage};

/// Reactive state for the data source registration form and table, plus the commands that
/// mutate it via [`crate::api`]. Every field is an `RwSignal` handle, so the whole struct is
/// cheap to `Copy` — views hold it by value and read/call straight through it.
#[derive(Copy, Clone)]
pub struct SourcesViewModel {
    pub list: PagedList<DataSourceView>,
    /// The latest connection-test outcome per source id. Kept here rather than in a row so it
    /// shows in both the table and the card layout, and survives a resize that swaps them.
    pub test_results: RwSignal<HashMap<String, ConnectionTestOutcome>>,
    pub name: RwSignal<String>,
    pub engine: RwSignal<DbEngine>,
    pub host: RwSignal<String>,
    pub port: RwSignal<String>,
    pub username: RwSignal<String>,
    pub password: RwSignal<String>,
    pub database: RwSignal<String>,
    /// The Projects list, shared with [`crate::viewmodel::ProjectsViewModel`] — used for
    /// labeling each row with its project's current name.
    projects: RwSignal<Vec<ProjectView>>,
    /// Which project the dashboard is scoped to.
    session: SessionViewModel,
    /// Shared with the rest of the dashboard, so failures here surface in the same banner.
    status: RwSignal<Option<StatusMessage>>,
}

impl SourcesViewModel {
    pub fn new(session: SessionViewModel, status: RwSignal<Option<StatusMessage>>) -> Self {
        Self {
            list: PagedList::new(),
            test_results: RwSignal::new(HashMap::new()),
            name: RwSignal::new(String::new()),
            engine: RwSignal::new(DbEngine::Postgres),
            host: RwSignal::new(String::new()),
            port: RwSignal::new(String::new()),
            username: RwSignal::new(String::new()),
            password: RwSignal::new(String::new()),
            database: RwSignal::new(String::new()),
            projects: session.projects,
            session,
            status,
        }
    }

    /// Reloads the data sources of the selected project (every accessible one when none is
    /// selected). Reads the selection reactively, so calling this from an `Effect` re-runs it
    /// when the project is switched.
    pub fn refresh(&self) {
        self.load(self.session.current_project_id.get());
    }

    /// Reloads every data source the user may see, whatever project is selected. For overviews
    /// that show all projects side by side.
    pub fn refresh_unscoped(&self) {
        self.load(None);
    }

    fn load(&self, project_id: Option<String>) {
        let sources = self.list.items;
        let status = self.status;
        // Data sources carry connection details, so the backend refuses them to plain users.
        if !self.session.can_develop() {
            sources.set(Vec::new());
            return;
        }
        spawn_local(async move {
            match api::list_sources(project_id.as_deref()).await {
                Ok(list) => sources.set(list),
                Err(err) => status.set(Some(StatusMessage::Error(format!(
                    "Failed to load data sources: {err}"
                )))),
            }
        });
    }

    /// Looks up a data source's current display name by id — used by the delete confirmation
    /// dialog, which only holds an id.
    pub fn name_of(&self, id: &str) -> Option<String> {
        self.list
            .items
            .get()
            .into_iter()
            .find(|s| s.id == id)
            .map(|s| s.name)
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

    /// Where a source connects to, as `host:port/database`.
    pub fn connection_label(source: &DataSourceView) -> String {
        let connection = &source.connection;
        format!(
            "{}:{}/{}",
            connection.host, connection.port, connection.database
        )
    }

    /// How many tables a source ingests.
    pub fn ingested_label(source: &DataSourceView) -> String {
        match source.ingested_tables.len() {
            0 => "no tables".to_string(),
            1 => "1 table".to_string(),
            count => format!("{count} tables"),
        }
    }

    pub fn engine_label(engine: DbEngine) -> &'static str {
        match engine {
            DbEngine::Postgres => "PostgreSQL",
            DbEngine::MySql => "MySQL",
        }
    }

    pub fn submit_new(&self, ev: SubmitEvent) {
        ev.prevent_default();

        let port_value: u16 = match self.port.get().trim().parse() {
            Ok(parsed) => parsed,
            Err(_) => {
                self.status.set(Some(StatusMessage::Error(
                    "Port must be a valid number.".to_string(),
                )));
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
            // Registered into whichever project the nav-bar switcher has selected.
            project_id: self.session.current_project_id.get_untracked(),
        };

        let this = *self;
        spawn_local(async move {
            match api::register_source(&new_source).await {
                Ok(_) => {
                    this.status.set(Some(StatusMessage::Success(
                        "Data source registered.".to_string(),
                    )));
                    this.name.set(String::new());
                    this.host.set(String::new());
                    this.port.set(String::new());
                    this.username.set(String::new());
                    this.password.set(String::new());
                    this.database.set(String::new());
                    this.refresh();
                }
                Err(err) => this.status.set(Some(StatusMessage::Error(format!(
                    "Failed to register data source: {err}"
                )))),
            }
        });
    }

    /// The outcome of the last connection test of `id`, if it has been tested.
    pub fn test_result(&self, id: &str) -> Option<ConnectionTestOutcome> {
        self.test_results.with(|results| results.get(id).cloned())
    }

    pub fn test(&self, id: String) {
        let results = self.test_results;
        let status = self.status;
        spawn_local(async move {
            match api::test_source(&id).await {
                Ok(outcome) => results.update(|results| {
                    results.insert(id, outcome);
                }),
                Err(err) => status.set(Some(StatusMessage::Error(format!(
                    "Connection test failed: {err}"
                )))),
            }
        });
    }

    pub fn delete(&self, id: String) {
        let this = *self;
        spawn_local(async move {
            match api::delete_source(&id).await {
                Ok(()) => {
                    this.status.set(Some(StatusMessage::Success(
                        "Data source removed.".to_string(),
                    )));
                    this.refresh();
                }
                Err(err) => this.status.set(Some(StatusMessage::Error(format!(
                    "Failed to remove data source: {err}"
                )))),
            }
        });
    }
}
