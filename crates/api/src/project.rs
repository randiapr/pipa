//! `/projects` request and response bodies.

use serde::{Deserialize, Serialize};

use crate::envelope::BaseResponse;

/// `POST /projects` request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewProject {
    pub name: String,
    pub description: Option<String>,
}

/// `PUT /projects/{id}` request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectUpdate {
    pub name: String,
    pub description: Option<String>,
}

/// A project as returned by the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectView {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at_unix: u64,
}

/// Payload of `GET /projects`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Projects {
    pub projects: Vec<ProjectView>,
}

/// Payload of `POST /projects`, `GET /projects/{id}` and `PUT /projects/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectData {
    pub project: ProjectView,
}

pub type ProjectsResponse = BaseResponse<Projects>;
pub type ProjectResponse = BaseResponse<ProjectData>;
