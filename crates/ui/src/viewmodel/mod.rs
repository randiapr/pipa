//! ViewModel layer: reactive state plus the commands that mutate it via [`crate::api`].
//!
//! Each `*ViewModel` struct bundles `RwSignal` handles (cheap to `Copy`, since an `RwSignal`
//! is just a small handle) with the methods that read/write them and call [`crate::api`].
//! Views hold a ViewModel by value, read its signals to render, and call its methods in
//! response to user input — they never call `crate::api` directly and hold no state of
//! their own beyond purely ephemeral, per-row UI state (see `view::sources_card::SourceRow`).

mod projects;
mod sources;

use leptos::prelude::*;

pub use projects::ProjectsViewModel;
pub use sources::SourcesViewModel;

/// Rows shown per page in a paginated data table.
pub const PAGE_SIZE: usize = 5;

/// A dashboard-wide status message, carrying enough to pick the right daisyUI `alert` variant
/// (`alert-success`/`alert-error`) rather than always rendering the same neutral alert.
#[derive(Clone, PartialEq, Eq)]
pub enum StatusMessage {
    Success(String),
    Error(String),
}

impl StatusMessage {
    pub fn text(&self) -> &str {
        match self {
            Self::Success(text) | Self::Error(text) => text,
        }
    }

    /// The daisyUI `alert` variant class for this message's kind.
    pub fn alert_class(&self) -> &'static str {
        match self {
            Self::Success(_) => "alert alert-success",
            Self::Error(_) => "alert alert-error",
        }
    }
}

/// Composition root for the app's ViewModels and the status message they share. Used by
/// every route (`Landing`, `Dashboard`) that needs project/source data — each route
/// constructs its own instance and fetches independently on mount.
#[derive(Copy, Clone)]
pub struct AppViewModel {
    pub status: RwSignal<Option<StatusMessage>>,
    pub projects: ProjectsViewModel,
    pub sources: SourcesViewModel,
}

impl AppViewModel {
    pub fn new() -> Self {
        let status = RwSignal::new(None);
        let projects = ProjectsViewModel::new(status);
        let sources = SourcesViewModel::new(projects.projects, status);
        Self {
            status,
            projects,
            sources,
        }
    }

    pub fn refresh_all(&self) {
        self.projects.refresh();
        self.sources.refresh();
    }
}

impl Default for AppViewModel {
    fn default() -> Self {
        Self::new()
    }
}
