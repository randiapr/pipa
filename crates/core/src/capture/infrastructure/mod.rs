//! Infrastructure layer: concrete [`super::domain::CdcSource`] adapters, one per OLTP engine.

mod postgres;

pub use postgres::PostgresWalSource;
