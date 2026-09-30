//! ViewModel: reactive state and commands for the Users list and its create/edit forms.
//! Admin only — the backend rejects everyone else, and the route is guarded in the view layer.

use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;
use pipa_api::{NewUser, Role, UserUpdate, UserView};

use crate::api;
use crate::viewmodel::{PAGE_SIZE, SessionViewModel, StatusMessage};

#[derive(Copy, Clone)]
pub struct UsersViewModel {
    pub users: RwSignal<Vec<UserView>>,
    pub page: RwSignal<usize>,
    pub username: RwSignal<String>,
    pub password: RwSignal<String>,
    pub role: RwSignal<Role>,
    pub project_ids: RwSignal<Vec<String>>,
    pub editing_id: RwSignal<Option<String>>,
    pub edit_role: RwSignal<Role>,
    pub edit_password: RwSignal<String>,
    pub edit_project_ids: RwSignal<Vec<String>>,
    session: SessionViewModel,
    status: RwSignal<Option<StatusMessage>>,
}

/// The wire spelling of a role, as used by the role `<select>`s.
pub fn role_value(role: Role) -> &'static str {
    match role {
        Role::Admin => "admin",
        Role::User => "user",
    }
}

pub fn parse_role(value: &str) -> Role {
    if value == "admin" {
        Role::Admin
    } else {
        Role::User
    }
}

fn toggle(ids: RwSignal<Vec<String>>, id: String, selected: bool) {
    ids.update(|ids| {
        ids.retain(|existing| existing != &id);
        if selected {
            ids.push(id);
        }
    });
}

impl UsersViewModel {
    pub fn new(session: SessionViewModel, status: RwSignal<Option<StatusMessage>>) -> Self {
        Self {
            users: RwSignal::new(Vec::new()),
            page: RwSignal::new(0),
            username: RwSignal::new(String::new()),
            password: RwSignal::new(String::new()),
            role: RwSignal::new(Role::User),
            project_ids: RwSignal::new(Vec::new()),
            editing_id: RwSignal::new(None),
            edit_role: RwSignal::new(Role::User),
            edit_password: RwSignal::new(String::new()),
            edit_project_ids: RwSignal::new(Vec::new()),
            session,
            status,
        }
    }

    pub fn refresh(&self) {
        let users = self.users;
        let status = self.status;
        spawn_local(async move {
            match api::list_users().await {
                Ok(list) => users.set(list),
                Err(err) => status.set(Some(StatusMessage::Error(format!(
                    "Failed to load users: {err}"
                )))),
            }
        });
    }

    /// The current page's slice, clamped to the last valid page.
    pub fn paged(&self) -> Vec<UserView> {
        let all = self.users.get();
        let total_pages = all.len().div_ceil(PAGE_SIZE).max(1);
        let page = self.page.get().min(total_pages - 1);
        all.into_iter()
            .skip(page * PAGE_SIZE)
            .take(PAGE_SIZE)
            .collect()
    }

    pub fn total(&self) -> Signal<usize> {
        let users = self.users;
        Signal::derive(move || users.get().len())
    }

    pub fn find(&self, id: &str) -> Option<UserView> {
        self.users.get().into_iter().find(|user| user.id == id)
    }

    /// Whether `id` is the account currently signed in.
    pub fn is_self(&self, id: &str) -> bool {
        self.session
            .user
            .with(|user| user.as_ref().is_some_and(|user| user.id == id))
    }

    /// The names of the projects `user` is assigned to, for display.
    pub fn project_names(&self, user: &UserView) -> Vec<String> {
        let projects = self.session.projects.get();
        user.project_ids
            .iter()
            .filter_map(|id| projects.iter().find(|project| &project.id == id))
            .map(|project| project.name.clone())
            .collect()
    }

    pub fn toggle_new_project(&self, id: String, selected: bool) {
        toggle(self.project_ids, id, selected);
    }

    pub fn toggle_edit_project(&self, id: String, selected: bool) {
        toggle(self.edit_project_ids, id, selected);
    }

    pub fn reset_new(&self) {
        self.username.set(String::new());
        self.password.set(String::new());
        self.role.set(Role::User);
        self.project_ids.set(Vec::new());
    }

    pub fn submit_new(&self, ev: SubmitEvent) {
        ev.prevent_default();

        let new_user = NewUser {
            username: self.username.get(),
            password: self.password.get(),
            role: self.role.get(),
            project_ids: self.project_ids.get(),
        };

        let this = *self;
        spawn_local(async move {
            match api::create_user(&new_user).await {
                Ok(_) => {
                    this.status
                        .set(Some(StatusMessage::Success("User created.".to_string())));
                    this.reset_new();
                    this.refresh();
                }
                Err(err) => this.status.set(Some(StatusMessage::Error(format!(
                    "Failed to create user: {err}"
                )))),
            }
        });
    }

    pub fn start_edit(&self, id: String) {
        if let Some(current) = self.find(&id) {
            self.edit_role.set(current.role);
            self.edit_project_ids.set(current.project_ids);
            self.edit_password.set(String::new());
        }
        self.editing_id.set(Some(id));
    }

    pub fn save_edit(&self, id: String) {
        let update = UserUpdate {
            role: Some(self.edit_role.get()),
            password: Some(self.edit_password.get()).filter(|password| !password.is_empty()),
            project_ids: Some(self.edit_project_ids.get()),
        };

        let this = *self;
        spawn_local(async move {
            match api::update_user(&id, &update).await {
                Ok(_) => {
                    this.editing_id.set(None);
                    this.status
                        .set(Some(StatusMessage::Success("User updated.".to_string())));
                    this.refresh();
                    if this.is_self(&id) {
                        // Their own role or projects may have just changed.
                        this.session.refresh_user();
                    }
                }
                Err(err) => this.status.set(Some(StatusMessage::Error(format!(
                    "Failed to update user: {err}"
                )))),
            }
        });
    }

    pub fn cancel_edit(&self) {
        self.editing_id.set(None);
        self.edit_password.set(String::new());
    }

    pub fn delete(&self, id: String) {
        let this = *self;
        spawn_local(async move {
            match api::delete_user(&id).await {
                Ok(()) => {
                    this.status
                        .set(Some(StatusMessage::Success("User removed.".to_string())));
                    this.refresh();
                }
                Err(err) => this.status.set(Some(StatusMessage::Error(format!(
                    "Failed to remove user: {err}"
                )))),
            }
        });
    }
}
