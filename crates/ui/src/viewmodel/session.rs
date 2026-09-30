//! ViewModel: who is signed in, and which project the dashboard is currently scoped to.
//!
//! One instance is created in `App` and shared through Leptos context (`expect_context`), since
//! the nav bar, the route guards and every page need it. The login token lives in
//! `localStorage` (where [`crate::api`] reads it from); the signed-in user is only known after
//! login, or after `/auth/me` confirms a stored token on page load.

use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;
use pipa_api::{LoginRequest, ProjectView, Role, UserView};

use crate::api;
use crate::storage;

#[derive(Copy, Clone)]
pub struct SessionViewModel {
    /// The signed-in user; `None` when signed out.
    pub user: RwSignal<Option<UserView>>,
    /// The projects the signed-in user may see: all of them for an admin, only the assigned ones
    /// otherwise. Shared with [`crate::viewmodel::ProjectsViewModel`], so creating or deleting a
    /// project updates the nav-bar switcher too.
    pub projects: RwSignal<Vec<ProjectView>>,
    /// The project pages are scoped to. `None` means "all projects", which only an admin may
    /// pick.
    pub current_project_id: RwSignal<Option<String>>,
    /// False until we know whether a token found in storage is still valid, so route guards
    /// don't bounce a returning user to the login page while `/auth/me` is in flight.
    pub checked: RwSignal<bool>,
    pub username: RwSignal<String>,
    pub password: RwSignal<String>,
    pub login_error: RwSignal<Option<String>>,
    pub logging_in: RwSignal<bool>,
    /// True once `projects` holds a real answer from the backend rather than the initial empty
    /// placeholder, so an empty list isn't mistaken for "no projects".
    projects_loaded: RwSignal<bool>,
}

impl SessionViewModel {
    pub fn new() -> Self {
        let has_token = storage::get(storage::TOKEN_KEY).is_some();
        Self {
            user: RwSignal::new(None),
            projects: RwSignal::new(Vec::new()),
            current_project_id: RwSignal::new(storage::get(storage::PROJECT_KEY)),
            checked: RwSignal::new(!has_token),
            username: RwSignal::new(String::new()),
            password: RwSignal::new(String::new()),
            login_error: RwSignal::new(None),
            logging_in: RwSignal::new(false),
            projects_loaded: RwSignal::new(false),
        }
    }

    /// Wires the session up: drops it whenever the backend answers 401, keeps the current
    /// project valid and persisted, and confirms a stored token. Call once, from `App`.
    pub fn init(&self) {
        let this = *self;
        api::on_unauthorized(move || this.clear());

        // Keep the selection valid as the project list or the user changes: a deleted project,
        // or an unassignment, must not stay selected, and a non-admin always needs one.
        Effect::new(move |_| {
            let projects = this.projects.get();
            let is_admin = this.is_admin();
            if this.user.with(Option::is_none) || !this.projects_loaded.get() {
                return;
            }
            let current = this.current_project_id.get_untracked();
            let valid = match &current {
                Some(id) => projects.iter().any(|project| &project.id == id),
                None => is_admin,
            };
            if !valid {
                let fallback = if is_admin {
                    None
                } else {
                    projects.first().map(|project| project.id.clone())
                };
                this.current_project_id.set(fallback);
            }
        });

        Effect::new(move |_| match this.current_project_id.get() {
            Some(id) => storage::set(storage::PROJECT_KEY, &id),
            None => storage::remove(storage::PROJECT_KEY),
        });

        if storage::get(storage::TOKEN_KEY).is_some() {
            spawn_local(async move {
                match api::me().await {
                    Ok(user) => {
                        this.user.set(Some(user));
                        this.refresh_projects();
                    }
                    Err(_) => this.clear(),
                }
                this.checked.set(true);
            });
        }
    }

    pub fn is_authenticated(&self) -> bool {
        self.user.with(Option::is_some)
    }

    pub fn is_admin(&self) -> bool {
        self.user
            .with(|user| user.as_ref().is_some_and(|user| user.role == Role::Admin))
    }

    /// The project pages are scoped to, if any and it still exists.
    pub fn current_project(&self) -> Option<ProjectView> {
        let id = self.current_project_id.get()?;
        self.projects
            .with(|projects| projects.iter().find(|project| project.id == id).cloned())
    }

    pub fn select_project(&self, id: Option<String>) {
        self.current_project_id.set(id);
    }

    pub fn refresh_projects(&self) {
        let this = *self;
        spawn_local(async move {
            match api::list_projects().await {
                Ok(list) => {
                    this.projects.set(list);
                    this.projects_loaded.set(true);
                }
                Err(err) => log::warn!("failed to load projects: {err}"),
            }
        });
    }

    /// Re-reads the signed-in user, e.g. after their own role or projects were edited.
    pub fn refresh_user(&self) {
        let this = *self;
        spawn_local(async move {
            if let Ok(user) = api::me().await {
                this.user.set(Some(user));
                this.refresh_projects();
            }
        });
    }

    pub fn login(&self, ev: SubmitEvent) {
        ev.prevent_default();
        let credentials = LoginRequest {
            username: self.username.get(),
            password: self.password.get(),
        };
        let this = *self;
        this.logging_in.set(true);
        this.login_error.set(None);
        spawn_local(async move {
            match api::login(&credentials).await {
                Ok(data) => {
                    // The token must be in storage before anything else calls the API.
                    storage::set(storage::TOKEN_KEY, &data.token);
                    this.password.set(String::new());
                    this.projects_loaded.set(false);
                    this.user.set(Some(data.user));
                    this.checked.set(true);
                    this.refresh_projects();
                }
                Err(err) => this.login_error.set(Some(err)),
            }
            this.logging_in.set(false);
        });
    }

    pub fn logout(&self) {
        self.clear();
    }

    /// Forgets the token, the user and everything scoped to them.
    fn clear(&self) {
        storage::remove(storage::TOKEN_KEY);
        self.user.set(None);
        self.projects.set(Vec::new());
        self.projects_loaded.set(false);
        self.current_project_id.set(None);
        self.checked.set(true);
    }
}

impl Default for SessionViewModel {
    fn default() -> Self {
        Self::new()
    }
}
