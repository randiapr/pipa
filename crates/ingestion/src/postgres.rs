// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! Plain (non-replication) Postgres access shared by the capture and write adapters: opening a
//! connection to a data source and looking up its tables' row keys. Errors are left as
//! `sqlx::Error` for each caller to map into its own layer's error type.

use std::collections::HashMap;

use sqlx::{Connection, PgConnection, Row, postgres::PgConnectOptions};

use crate::datasource::DataSource;

/// A short-lived connection to `source`'s database.
pub async fn connect(source: &DataSource) -> Result<PgConnection, sqlx::Error> {
    let options = PgConnectOptions::new()
        .host(&source.connection.host)
        .port(source.connection.port)
        .username(&source.connection.username)
        .password(&source.connection.password)
        .database(&source.connection.database);
    PgConnection::connect_with(&options).await
}

/// Every table of the connected database with its row key: the replica identity's columns, in
/// index order — the primary key by default, the chosen index for `REPLICA IDENTITY USING
/// INDEX`. Tables without one (no primary key, or `REPLICA IDENTITY FULL`/`NOTHING`) map to
/// nothing.
pub async fn key_columns(
    conn: &mut PgConnection,
) -> Result<HashMap<(String, String), Vec<String>>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT n.nspname AS schema_name, c.relname AS table_name, a.attname AS column_name \
         FROM pg_class c \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         JOIN pg_index i ON i.indrelid = c.oid \
           AND ((c.relreplident = 'd' AND i.indisprimary) \
             OR (c.relreplident = 'i' AND i.indisreplident)) \
         CROSS JOIN LATERAL unnest(i.indkey::int2[]) WITH ORDINALITY AS k(attnum, ord) \
         JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum = k.attnum \
         WHERE c.relkind IN ('r', 'p') \
           AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
         ORDER BY n.nspname, c.relname, k.ord",
    )
    .fetch_all(conn)
    .await?;

    let mut keys: HashMap<(String, String), Vec<String>> = HashMap::new();
    for row in rows {
        keys.entry((row.try_get("schema_name")?, row.try_get("table_name")?))
            .or_default()
            .push(row.try_get("column_name")?);
    }
    Ok(keys)
}

/// `name` as a quoted SQL identifier, safe to splice into a statement whatever it contains.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_quoted_and_escaped() {
        assert_eq!(quote_ident("orders"), "\"orders\"");
        assert_eq!(quote_ident("Order Items"), "\"Order Items\"");
        assert_eq!(quote_ident("a\"b"), "\"a\"\"b\"");
    }
}
