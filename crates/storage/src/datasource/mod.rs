//! Bounded context: registering and testing OLTP data sources for CDC capture.
//!
//! Organized with a clean-architecture split so persistence/connectivity technology stays
//! swappable behind ports the application layer defines:
//! - [`domain`] — the `DataSource` aggregate, its value objects, and the `DataSourceRepository`
//!   / `ConnectionTester` ports.
//! - [`application`] — `DataSourceService`, the use cases orchestrating those ports.
//! - [`infrastructure`] — concrete adapters: an object-store-backed repository and a
//!   `sqlx`-backed connection tester.

pub mod application;
pub mod domain;
pub mod infrastructure;

pub use application::DataSourceService;
pub use domain::{
    ConnectionConfig, ConnectionTestOutcome, ConnectionTester, DataSource, DataSourceError,
    DataSourceId, DataSourceRepository, DbEngine, NewDataSource,
};
