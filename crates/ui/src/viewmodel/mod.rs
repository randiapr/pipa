//! ViewModel layer: reactive state plus the commands that mutate it via [`crate::api`].
//!
//! Each `*ViewModel` struct bundles `RwSignal` handles (cheap to `Copy`, since an `RwSignal`
//! is just a small handle) with the methods that read/write them and call [`crate::api`].
//! Views hold a ViewModel by value, read its signals to render, and call its methods in
//! response to user input — they never call `crate::api` directly and hold no state of
//! their own beyond purely ephemeral UI state (e.g. which dialog is open). Anything a list row
//! shows lives here rather than in the row component, since each row renders twice over — as a
//! table row on desktop and a card on mobile (see [`LayoutViewModel`]) — and must look the same
//! in both, including across a resize that swaps one for the other.

mod explorer;
mod layout;
mod paging;
mod projects;
mod query;
mod session;
mod sources;
mod tables;
mod users;

use leptos::prelude::*;

pub use explorer::{ExplorerViewModel, table_ref};
pub use layout::LayoutViewModel;
pub use paging::{PAGE_SIZE_OPTIONS, PageSize, PagedList};
pub use projects::ProjectsViewModel;
pub use query::{QueryViewModel, cell_text, columns_of};
pub use session::SessionViewModel;
pub use sources::SourcesViewModel;
pub use tables::TablesViewModel;
pub use users::{UsersViewModel, parse_role, role_value};

/// A dashboard-wide status message: whether it reports a success or an error (the view picks
/// the alert's color from that), and its text.
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
}

/// Composition root for the app's ViewModels and the status message they share. Used by
/// every route (`Landing`, `ProjectsPage`, `SourcesPage`) that needs project/source data — each route
/// constructs its own instance and fetches independently on mount. Must be created below
/// `App`, which provides the shared [`SessionViewModel`] through context.
#[derive(Copy, Clone)]
pub struct AppViewModel {
    pub status: RwSignal<Option<StatusMessage>>,
    pub projects: ProjectsViewModel,
    pub sources: SourcesViewModel,
}

impl AppViewModel {
    pub fn new() -> Self {
        let session = expect_context::<SessionViewModel>();
        let status = RwSignal::new(None);
        let projects = ProjectsViewModel::new(session.projects, status);
        let sources = SourcesViewModel::new(session, status);
        Self {
            status,
            projects,
            sources,
        }
    }

    /// Reloads projects and the data sources of the selected project.
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
