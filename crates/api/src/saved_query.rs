// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! `/saved-queries` request and response bodies.

use serde::{Deserialize, Serialize};

use crate::envelope::BaseResponse;

/// `POST /saved-queries` request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewSavedQuery {
    /// Project the query belongs to; everyone who can develop in it sees the query.
    pub project_id: String,
    pub name: String,
    pub sql: String,
}

/// `PUT /saved-queries/{id}` request body. A query stays in its project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedQueryUpdate {
    pub name: String,
    pub sql: String,
}

/// A saved query as returned by the API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedQueryView {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub sql: String,
    pub created_at_unix: u64,
    pub updated_at_unix: u64,
}

/// Payload of `GET /saved-queries`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedQueries {
    pub saved_queries: Vec<SavedQueryView>,
}

/// Payload of `POST /saved-queries` and `PUT /saved-queries/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedQueryData {
    pub saved_query: SavedQueryView,
}

pub type SavedQueriesResponse = BaseResponse<SavedQueries>;
pub type SavedQueryResponse = BaseResponse<SavedQueryData>;
