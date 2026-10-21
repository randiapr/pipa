// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! Domain layer: the `SavedQuery` aggregate and the `SavedQueryRepository` port the application
//! layer depends on. A saved query is a named SQL text kept for a project, so analytics queries
//! don't have to be retyped; running one is an ordinary `POST /query`.

use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::project::ProjectId;

/// Longest name, in characters.
pub const MAX_NAME_CHARS: usize = 100;
/// Largest SQL text, in bytes. Analytics queries with a few CTEs are a few KiB.
pub const MAX_SQL_BYTES: usize = 64 * 1024;

/// Identity of a saved query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SavedQueryId(pub Uuid);

impl SavedQueryId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for SavedQueryId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SavedQueryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A saved query (aggregate root), kept for one project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedQuery {
    pub id: SavedQueryId,
    pub project_id: ProjectId,
    pub name: String,
    pub sql: String,
    pub created_at_unix: u64,
    pub updated_at_unix: u64,
}

/// Fields needed to save a new query, before an identity is assigned.
#[derive(Debug, Clone)]
pub struct NewSavedQuery {
    pub project_id: ProjectId,
    pub name: String,
    pub sql: String,
}

/// Fields that can be changed on an existing saved query. It stays in its project.
#[derive(Debug, Clone)]
pub struct SavedQueryUpdate {
    pub name: String,
    pub sql: String,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

/// Trims `name` and checks `name` and `sql`, returning the name to store.
fn validate(name: &str, sql: &str) -> Result<String, SavedQueryError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(SavedQueryError::InvalidField(
            "name must not be empty".to_string(),
        ));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(SavedQueryError::InvalidField(format!(
            "name must be at most {MAX_NAME_CHARS} characters"
        )));
    }
    if sql.trim().is_empty() {
        return Err(SavedQueryError::InvalidField(
            "sql must not be empty".to_string(),
        ));
    }
    if sql.len() > MAX_SQL_BYTES {
        return Err(SavedQueryError::InvalidField(format!(
            "sql must be at most {MAX_SQL_BYTES} bytes"
        )));
    }
    Ok(name.to_string())
}

impl SavedQuery {
    /// Validates and constructs a new `SavedQuery`, assigning it a fresh identity.
    pub fn register(new: NewSavedQuery) -> Result<Self, SavedQueryError> {
        let name = validate(&new.name, &new.sql)?;
        let now = now_unix();
        Ok(Self {
            id: SavedQueryId::new(),
            project_id: new.project_id,
            name,
            sql: new.sql,
            created_at_unix: now,
            updated_at_unix: now,
        })
    }

    /// Validates and applies an update to this saved query.
    pub fn apply_update(&mut self, update: SavedQueryUpdate) -> Result<(), SavedQueryError> {
        self.name = validate(&update.name, &update.sql)?;
        self.sql = update.sql;
        self.updated_at_unix = now_unix();
        Ok(())
    }
}

/// Errors surfaced by the saved query domain and its use cases.
#[derive(Debug, thiserror::Error)]
pub enum SavedQueryError {
    #[error("invalid saved query: {0}")]
    InvalidField(String),
    #[error("a saved query named \"{0}\" already exists in this project")]
    DuplicateName(String),
    #[error("saved query {0} was not found")]
    NotFound(SavedQueryId),
    #[error("saved query storage error: {0}")]
    Storage(String),
}

/// Port: persistence for `SavedQuery` aggregates, implemented by an infrastructure adapter.
#[async_trait]
pub trait SavedQueryRepository: Send + Sync {
    async fn save(&self, saved_query: &SavedQuery) -> Result<(), SavedQueryError>;
    async fn find_by_id(&self, id: SavedQueryId) -> Result<Option<SavedQuery>, SavedQueryError>;
    async fn list(&self) -> Result<Vec<SavedQuery>, SavedQueryError>;
    async fn delete(&self, id: SavedQueryId) -> Result<(), SavedQueryError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new(name: &str, sql: &str) -> NewSavedQuery {
        NewSavedQuery {
            project_id: ProjectId::new(),
            name: name.to_string(),
            sql: sql.to_string(),
        }
    }

    #[test]
    fn the_name_is_trimmed_and_the_sql_kept_as_written() {
        let query = SavedQuery::register(new("  Daily volume ", "SELECT 1\n")).unwrap();
        assert_eq!(query.name, "Daily volume");
        assert_eq!(query.sql, "SELECT 1\n");
    }

    #[test]
    fn rejects_an_empty_name_or_sql() {
        for (name, sql) in [
            ("", "SELECT 1"),
            ("   ", "SELECT 1"),
            ("a", ""),
            ("a", " \n"),
        ] {
            assert!(matches!(
                SavedQuery::register(new(name, sql)),
                Err(SavedQueryError::InvalidField(_))
            ));
        }
    }

    #[test]
    fn rejects_a_name_or_sql_over_the_limits() {
        let long_name = "n".repeat(MAX_NAME_CHARS + 1);
        let long_sql = "s".repeat(MAX_SQL_BYTES + 1);
        assert!(SavedQuery::register(new(&long_name, "SELECT 1")).is_err());
        assert!(SavedQuery::register(new("a", &long_sql)).is_err());
        assert!(SavedQuery::register(new(&"n".repeat(MAX_NAME_CHARS), "SELECT 1")).is_ok());
    }

    #[test]
    fn an_update_changes_name_and_sql_but_not_the_project() {
        let mut query = SavedQuery::register(new("a", "SELECT 1")).unwrap();
        let project = query.project_id;
        query
            .apply_update(SavedQueryUpdate {
                name: " b ".to_string(),
                sql: "SELECT 2".to_string(),
            })
            .unwrap();
        assert_eq!((query.name.as_str(), query.sql.as_str()), ("b", "SELECT 2"));
        assert_eq!(query.project_id, project);
    }

    #[test]
    fn an_invalid_update_leaves_the_query_as_it_was() {
        let mut query = SavedQuery::register(new("a", "SELECT 1")).unwrap();
        let result = query.apply_update(SavedQueryUpdate {
            name: "b".to_string(),
            sql: " ".to_string(),
        });
        assert!(result.is_err());
        assert_eq!((query.name.as_str(), query.sql.as_str()), ("a", "SELECT 1"));
    }
}
