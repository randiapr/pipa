// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! ViewModel: the SQL query page. Queries run against the selected project's tables only. The
//! page also keeps the project's saved queries: named SQL texts shared by the project's
//! developers. Running one is an ordinary run of its SQL.

use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;
use pipa_api::SavedQueryView;

use crate::api;
use crate::viewmodel::{PagedList, SessionViewModel};

/// How much of a saved query's SQL its list entry previews, in characters.
const PREVIEW_CHARS: usize = 80;

#[derive(Copy, Clone)]
pub struct QueryViewModel {
    pub sql: RwSignal<String>,
    /// The rows of the last successful run, a page at a time.
    pub results: PagedList<serde_json::Value>,
    /// Whether `results` holds a run's rows (even none) — false before the first run and
    /// after a failed one.
    pub ran: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
    /// What the last saved-query change did, e.g. "Saved \"Daily volume\"."
    pub notice: RwSignal<Option<String>>,
    pub running: RwSignal<bool>,
    /// The selected project's saved queries.
    pub saved: PagedList<SavedQueryView>,
    /// The name field of the "save as new" dialog.
    pub save_name: RwSignal<String>,
    /// The saved query last loaded into the editor, which "update" overwrites.
    loaded: RwSignal<Option<String>>,
    session: SessionViewModel,
}

impl QueryViewModel {
    pub fn new(session: SessionViewModel) -> Self {
        Self {
            sql: RwSignal::new(String::new()),
            results: PagedList::new(),
            ran: RwSignal::new(false),
            error: RwSignal::new(None),
            notice: RwSignal::new(None),
            running: RwSignal::new(false),
            saved: PagedList::new(),
            save_name: RwSignal::new(String::new()),
            loaded: RwSignal::new(None),
            session,
        }
    }

    pub fn run(&self, ev: SubmitEvent) {
        ev.prevent_default();
        self.execute();
    }

    fn execute(&self) {
        let project_id = self.session.current_project_id.get_untracked();
        if project_id.is_none() && !self.session.is_admin() {
            self.error.set(Some(
                "Select a project first to query its tables.".to_string(),
            ));
            return;
        }

        let this = *self;
        let sql = this.sql.get_untracked();
        this.running.set(true);
        this.error.set(None);
        this.notice.set(None);
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

    /// Whether the selected project can hold saved queries: one must be selected, and the
    /// backend keeps them from view-only users.
    pub fn can_save(&self) -> bool {
        self.session.can_develop() && self.session.current_project_id.get().is_some()
    }

    /// Reloads the selected project's saved queries. Reads the selection reactively, so calling
    /// this from an `Effect` re-runs it when the project is switched (which also detaches the
    /// editor from the previous project's query).
    pub fn refresh_saved(&self) {
        let project_id = self.session.current_project_id.get();
        self.loaded.set(None);
        let saved = self.saved;
        let error = self.error;
        let Some(project_id) = project_id.filter(|_| self.session.can_develop()) else {
            saved.reset(Vec::new());
            return;
        };
        spawn_local(async move {
            match api::list_saved_queries(&project_id).await {
                Ok(list) => saved.reset(list),
                Err(err) => error.set(Some(format!("Failed to load saved queries: {err}"))),
            }
        });
    }

    /// A saved query's name by id — for the dialogs, which only hold an id.
    pub fn name_of(&self, id: &str) -> Option<String> {
        self.saved.items.with(|items| {
            items
                .iter()
                .find(|query| query.id == id)
                .map(|q| q.name.clone())
        })
    }

    /// The name of the saved query in the editor, if it came from one.
    pub fn loaded_name(&self) -> Option<String> {
        self.loaded.get().and_then(|id| self.name_of(&id))
    }

    /// Puts a saved query's SQL in the editor.
    pub fn load(&self, id: &str) {
        let found = self
            .saved
            .items
            .with(|items| items.iter().find(|query| query.id == id).cloned());
        if let Some(query) = found {
            self.sql.set(query.sql);
            self.loaded.set(Some(query.id));
            self.error.set(None);
            self.notice.set(None);
        }
    }

    /// Loads a saved query and runs it.
    pub fn run_saved(&self, id: &str) {
        self.load(id);
        self.execute();
    }

    /// Saves the editor's SQL as a new query named `save_name`, and makes it the loaded one.
    pub fn save_new(&self, ev: SubmitEvent) {
        ev.prevent_default();
        let Some(project_id) = self.session.current_project_id.get_untracked() else {
            self.error
                .set(Some("Select a project first to save a query.".to_string()));
            return;
        };
        let this = *self;
        let name = this.save_name.get_untracked();
        let sql = this.sql.get_untracked();
        this.error.set(None);
        spawn_local(async move {
            match api::create_saved_query(&project_id, &name, &sql).await {
                Ok(saved) => {
                    this.notice.set(Some(format!("Saved \"{}\".", saved.name)));
                    this.save_name.set(String::new());
                    this.reload_keeping(saved.id);
                }
                Err(err) => this
                    .error
                    .set(Some(format!("Failed to save the query: {err}"))),
            }
        });
    }

    /// Overwrites the loaded saved query's SQL with the editor's.
    pub fn update_loaded(&self) {
        let (Some(id), Some(name)) = (self.loaded.get_untracked(), self.loaded_name()) else {
            return;
        };
        let this = *self;
        let sql = this.sql.get_untracked();
        this.error.set(None);
        spawn_local(async move {
            match api::update_saved_query(&id, &name, &sql).await {
                Ok(saved) => {
                    this.notice
                        .set(Some(format!("Updated \"{}\".", saved.name)));
                    this.reload_keeping(saved.id);
                }
                Err(err) => this
                    .error
                    .set(Some(format!("Failed to update the query: {err}"))),
            }
        });
    }

    pub fn delete(&self, id: String) {
        let this = *self;
        let name = self.name_of(&id);
        this.error.set(None);
        spawn_local(async move {
            match api::delete_saved_query(&id).await {
                Ok(()) => {
                    this.notice.set(Some(match name {
                        Some(name) => format!("Removed \"{name}\"."),
                        None => "Saved query removed.".to_string(),
                    }));
                    if this.loaded.get_untracked().as_deref() == Some(id.as_str()) {
                        this.loaded.set(None);
                    }
                    this.reload_keeping_loaded();
                }
                Err(err) => this
                    .error
                    .set(Some(format!("Failed to remove the query: {err}"))),
            }
        });
    }

    /// Reloads the list after a change and keeps `id` as the loaded query.
    fn reload_keeping(&self, id: String) {
        self.loaded.set(Some(id));
        self.reload_keeping_loaded();
    }

    /// Reloads the list without touching which query is loaded (unlike [`Self::refresh_saved`],
    /// which is for a project switch).
    fn reload_keeping_loaded(&self) {
        let Some(project_id) = self.session.current_project_id.get_untracked() else {
            return;
        };
        let saved = self.saved;
        let error = self.error;
        spawn_local(async move {
            match api::list_saved_queries(&project_id).await {
                Ok(list) => saved.reset(list),
                Err(err) => error.set(Some(format!("Failed to load saved queries: {err}"))),
            }
        });
    }

    /// The first non-blank line of a saved query's SQL, cut to a list-sized preview.
    pub fn preview(query: &SavedQueryView) -> String {
        let line = query
            .sql
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or_default();
        if line.chars().count() > PREVIEW_CHARS {
            let cut: String = line.chars().take(PREVIEW_CHARS).collect();
            format!("{cut}…")
        } else {
            line.to_string()
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(sql: &str) -> SavedQueryView {
        SavedQueryView {
            id: "id".to_string(),
            project_id: "project".to_string(),
            name: "name".to_string(),
            sql: sql.to_string(),
            created_at_unix: 0,
            updated_at_unix: 0,
        }
    }

    #[test]
    fn a_preview_is_the_first_non_blank_line_trimmed() {
        let query = saved("\n  \n   WITH tx AS (\n  SELECT 1\n)");
        assert_eq!(QueryViewModel::preview(&query), "WITH tx AS (");
    }

    #[test]
    fn a_long_first_line_is_cut_with_an_ellipsis() {
        let query = saved(&"x".repeat(PREVIEW_CHARS + 20));
        let preview = QueryViewModel::preview(&query);
        assert_eq!(preview.chars().count(), PREVIEW_CHARS + 1);
        assert!(preview.ends_with('…'));
        assert_eq!(
            QueryViewModel::preview(&saved(&"x".repeat(PREVIEW_CHARS))).len(),
            PREVIEW_CHARS
        );
    }
}
