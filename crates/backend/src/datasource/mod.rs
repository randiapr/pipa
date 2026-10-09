//! Bounded context: registering, testing and exploring OLTP data sources for CDC capture, and
//! choosing which of their tables are ingested.
//!
//! Organized with a clean-architecture split so persistence/connectivity technology stays
//! swappable behind ports the application layer defines:
//! - [`domain`] — the `DataSource` aggregate, its value objects, and the `DataSourceRepository`
//!   / `ConnectionTester` / `SchemaExplorer` ports.
//! - [`application`] — `DataSourceService`, the use cases orchestrating those ports.
//! - [`infrastructure`] — concrete adapters: an object-store-backed repository and a
//!   `sqlx`-backed connection tester and schema explorer.

pub mod application;
pub mod domain;
pub mod infrastructure;

pub use application::DataSourceService;
pub use domain::{DataSourceError, DataSourceId};
