//! ViewModel: reactive state and commands for the Projects list and its create/edit forms.

use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::model::{NewProject, ProjectUpdate, ProjectView};
use crate::viewmodel::{StatusMessage, PAGE_SIZE};

/// Reactive state for the Projects feature, plus the commands that mutate it via
/// [`crate::api`]. Every field is an `RwSignal` handle, so the whole struct is cheap to
/// `Copy` — views hold it by value and read/call straight through it.
#[derive(Copy, Clone)]
pub struct ProjectsViewModel {
    pub projects: RwSignal<Vec<ProjectView>>,
    pub page: RwSignal<usize>,
    pub name: RwSignal<String>,
    pub description: RwSignal<String>,
    pub editing_id: RwSignal<Option<String>>,
    pub edit_name: RwSignal<String>,
    pub edit_description: RwSignal<String>,
    /// Shared with the rest of the dashboard, so failures here surface in the same banner.
    status: RwSignal<Option<StatusMessage>>,
}

impl ProjectsViewModel {
    pub fn new(status: RwSignal<Option<StatusMessage>>) -> Self {
        Self {
            projects: RwSignal::new(Vec::new()),
            page: RwSignal::new(0),
            name: RwSignal::new(String::new()),
            description: RwSignal::new(String::new()),
            editing_id: RwSignal::new(None),
            edit_name: RwSignal::new(String::new()),
            edit_description: RwSignal::new(String::new()),
            status,
        }
    }

    pub fn refresh(&self) {
        let projects = self.projects;
        let status = self.status;
        spawn_local(async move {
            match api::list_projects().await {
                Ok(list) => projects.set(list),
                Err(err) => status.set(Some(StatusMessage::Error(format!("Failed to load projects: {err}")))),
            }
        });
    }

    /// The current page's slice, clamped to the last valid page (e.g. after a delete shrinks
    /// the list past the page the user was on).
    pub fn paged(&self) -> Vec<ProjectView> {
        let all = self.projects.get();
        let total_pages = all.len().div_ceil(PAGE_SIZE).max(1);
        let page = self.page.get().min(total_pages - 1);
        all.into_iter()
            .skip(page * PAGE_SIZE)
            .take(PAGE_SIZE)
            .collect()
    }

    pub fn total(&self) -> Signal<usize> {
        let projects = self.projects;
        Signal::derive(move || projects.get().len())
    }

    /// Looks up a project's current display name by id — used by the Sources view to label
    /// rows without holding a stale copy of the name.
    pub fn name_of(&self, id: &str) -> Option<String> {
        self.projects
            .get()
            .into_iter()
            .find(|p| p.id == id)
            .map(|p| p.name)
    }

    pub fn submit_new(&self, ev: SubmitEvent) {
        ev.prevent_default();

        let new_project = NewProject {
            name: self.name.get(),
            description: Some(self.description.get()).filter(|d| !d.trim().is_empty()),
        };

        let this = *self;
        spawn_local(async move {
            match api::register_project(&new_project).await {
                Ok(_) => {
                    this.status
                        .set(Some(StatusMessage::Success("Project created.".to_string())));
                    this.name.set(String::new());
                    this.description.set(String::new());
                    this.refresh();
                }
                Err(err) => this
                    .status
                    .set(Some(StatusMessage::Error(format!("Failed to create project: {err}")))),
            }
        });
    }

    pub fn start_edit(&self, id: String) {
        if let Some(current) = self.projects.get().into_iter().find(|p| p.id == id) {
            self.edit_name.set(current.name);
            self.edit_description
                .set(current.description.unwrap_or_default());
        }
        self.editing_id.set(Some(id));
    }

    pub fn save_edit(&self, id: String) {
        let update = ProjectUpdate {
            name: self.edit_name.get(),
            description: Some(self.edit_description.get()).filter(|d| !d.trim().is_empty()),
        };

        let this = *self;
        spawn_local(async move {
            match api::update_project(&id, &update).await {
                Ok(_) => {
                    this.editing_id.set(None);
                    this.refresh();
                }
                Err(err) => this
                    .status
                    .set(Some(StatusMessage::Error(format!("Failed to update project: {err}")))),
            }
        });
    }

    pub fn cancel_edit(&self) {
        self.editing_id.set(None);
    }

    pub fn delete(&self, id: String) {
        let this = *self;
        spawn_local(async move {
            match api::delete_project(&id).await {
                Ok(()) => {
                    this.status
                        .set(Some(StatusMessage::Success("Project removed.".to_string())));
                    this.refresh();
                }
                Err(err) => this
                    .status
                    .set(Some(StatusMessage::Error(format!("Failed to remove project: {err}")))),
            }
        });
    }
}
