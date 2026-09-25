//! Bounded context: grouping data sources into projects.
//!
//! Organized with the same clean-architecture split as [`crate::datasource`]:
//! - [`domain`] — the `Project` aggregate and the `ProjectRepository` port.
//! - [`application`] — `ProjectService`, the use cases orchestrating that port.
//! - [`infrastructure`] — a concrete object-store-backed repository adapter.

pub mod application;
pub mod domain;
pub mod infrastructure;

pub use application::ProjectService;
pub use domain::{NewProject, Project, ProjectError, ProjectId, ProjectRepository, ProjectUpdate};
