//! `sqlx`-backed adapter listing the tables and columns of a data source's OLTP database.

use async_trait::async_trait;
use sqlx::{Connection, Row};

use super::connection_tester::{CONNECT_TIMEOUT, mysql_options, postgres_options};
use crate::datasource::domain::{
    ColumnRef, DataSource, DataSourceError, DbEngine, SchemaExplorer, SourceColumn, SourceTable,
    TableRef,
};

/// Every table (Postgres: ordinary and partitioned tables, partitions included — `pgoutput`
/// reports changes under the partition's own name) outside the system schemas, one row per
/// column, in column order, with the column it references through the first (by constraint
/// name) foreign key it is part of. Composite keys pair columns up by position.
const POSTGRES_COLUMNS: &str = "\
    SELECT n.nspname AS schema_name, c.relname AS table_name, a.attname AS column_name, \
           format_type(a.atttypid, a.atttypmod) AS data_type, \
           NOT a.attnotnull AS nullable, \
           COALESCE(a.attnum = ANY (pk.indkey::int2[]), false) AS primary_key, \
           fk.ref_schema, fk.ref_table, fk.ref_column \
    FROM pg_class c \
    JOIN pg_namespace n ON n.oid = c.relnamespace \
    JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped \
    LEFT JOIN pg_index pk ON pk.indrelid = c.oid AND pk.indisprimary \
    LEFT JOIN LATERAL ( \
        SELECT rn.nspname AS ref_schema, rc.relname AS ref_table, ra.attname AS ref_column \
        FROM pg_constraint con \
        CROSS JOIN LATERAL unnest(con.conkey, con.confkey) AS k(attnum, ref_attnum) \
        JOIN pg_class rc ON rc.oid = con.confrelid \
        JOIN pg_namespace rn ON rn.oid = rc.relnamespace \
        JOIN pg_attribute ra ON ra.attrelid = con.confrelid AND ra.attnum = k.ref_attnum \
        WHERE con.conrelid = c.oid AND con.contype = 'f' AND k.attnum = a.attnum \
        ORDER BY con.conname \
        LIMIT 1 \
    ) fk ON true \
    WHERE c.relkind IN ('r', 'p') \
      AND n.nspname <> 'information_schema' AND n.nspname NOT LIKE 'pg\\_%' \
    ORDER BY n.nspname, c.relname, a.attnum";

/// The base tables of the connected database, one row per column, in column order, with the
/// column it references through a foreign key — one row per foreign key the column is in,
/// ordered by constraint name ([`group_into_tables`] keeps the first). The `CAST`s keep MySQL
/// 8's `information_schema` from handing back binary strings, and make the flags plain
/// `BIGINT`s (MySQL has no boolean type).
const MYSQL_COLUMNS: &str = "\
    SELECT CAST(c.TABLE_SCHEMA AS CHAR) AS schema_name, CAST(c.TABLE_NAME AS CHAR) AS table_name, \
           CAST(c.COLUMN_NAME AS CHAR) AS column_name, CAST(c.COLUMN_TYPE AS CHAR) AS data_type, \
           CAST(c.IS_NULLABLE = 'YES' AS SIGNED) AS nullable, \
           CAST(c.COLUMN_KEY = 'PRI' AS SIGNED) AS primary_key, \
           CAST(k.REFERENCED_TABLE_SCHEMA AS CHAR) AS ref_schema, \
           CAST(k.REFERENCED_TABLE_NAME AS CHAR) AS ref_table, \
           CAST(k.REFERENCED_COLUMN_NAME AS CHAR) AS ref_column \
    FROM information_schema.COLUMNS c \
    JOIN information_schema.TABLES t \
      ON t.TABLE_SCHEMA = c.TABLE_SCHEMA AND t.TABLE_NAME = c.TABLE_NAME \
    LEFT JOIN information_schema.KEY_COLUMN_USAGE k \
      ON k.TABLE_SCHEMA = c.TABLE_SCHEMA AND k.TABLE_NAME = c.TABLE_NAME \
     AND k.COLUMN_NAME = c.COLUMN_NAME AND k.REFERENCED_TABLE_NAME IS NOT NULL \
    WHERE c.TABLE_SCHEMA = DATABASE() AND t.TABLE_TYPE = 'BASE TABLE' \
    ORDER BY c.TABLE_SCHEMA, c.TABLE_NAME, c.ORDINAL_POSITION, k.CONSTRAINT_NAME";

/// Lists a data source's tables by briefly connecting to its database with its stored
/// credentials and reading the catalog.
pub struct SqlxSchemaExplorer;

#[async_trait]
impl SchemaExplorer for SqlxSchemaExplorer {
    async fn list_tables(&self, source: &DataSource) -> Result<Vec<SourceTable>, DataSourceError> {
        let columns = match source.engine {
            DbEngine::Postgres => Self::postgres_columns(source).await,
            DbEngine::MySql => Self::mysql_columns(source).await,
        }
        .map_err(DataSourceError::SourceUnavailable)?;
        Ok(group_into_tables(columns))
    }
}

impl SqlxSchemaExplorer {
    async fn postgres_columns(source: &DataSource) -> Result<Vec<ColumnRow>, String> {
        let mut conn = tokio::time::timeout(
            CONNECT_TIMEOUT,
            sqlx::PgConnection::connect_with(&postgres_options(source)),
        )
        .await
        .map_err(|_| "timed out connecting".to_string())?
        .map_err(|err| err.to_string())?;

        let rows = sqlx::query(POSTGRES_COLUMNS)
            .fetch_all(&mut conn)
            .await
            .map_err(|err| err.to_string())?;
        rows.iter()
            .map(|row| {
                Ok(ColumnRow {
                    table: TableRef {
                        schema: row.try_get("schema_name")?,
                        name: row.try_get("table_name")?,
                    },
                    column: SourceColumn {
                        name: row.try_get("column_name")?,
                        data_type: row.try_get("data_type")?,
                        nullable: row.try_get("nullable")?,
                        primary_key: row.try_get("primary_key")?,
                        foreign_key: foreign_key(
                            row.try_get("ref_schema")?,
                            row.try_get("ref_table")?,
                            row.try_get("ref_column")?,
                        ),
                    },
                })
            })
            .collect::<Result<_, sqlx::Error>>()
            .map_err(|err| err.to_string())
    }

    async fn mysql_columns(source: &DataSource) -> Result<Vec<ColumnRow>, String> {
        let mut conn = tokio::time::timeout(
            CONNECT_TIMEOUT,
            sqlx::MySqlConnection::connect_with(&mysql_options(source)),
        )
        .await
        .map_err(|_| "timed out connecting".to_string())?
        .map_err(|err| err.to_string())?;

        let rows = sqlx::query(MYSQL_COLUMNS)
            .fetch_all(&mut conn)
            .await
            .map_err(|err| err.to_string())?;
        rows.iter()
            .map(|row| {
                let flag = |name: &str| row.try_get::<i64, _>(name).map(|value| value != 0);
                Ok(ColumnRow {
                    table: TableRef {
                        schema: row.try_get("schema_name")?,
                        name: row.try_get("table_name")?,
                    },
                    column: SourceColumn {
                        name: row.try_get("column_name")?,
                        data_type: row.try_get("data_type")?,
                        nullable: flag("nullable")?,
                        primary_key: flag("primary_key")?,
                        foreign_key: foreign_key(
                            row.try_get("ref_schema")?,
                            row.try_get("ref_table")?,
                            row.try_get("ref_column")?,
                        ),
                    },
                })
            })
            .collect::<Result<_, sqlx::Error>>()
            .map_err(|err| err.to_string())
    }
}

/// The referenced column read off a row, if the row's column is in a foreign key (the outer
/// join leaves all three `NULL` otherwise).
fn foreign_key(
    schema: Option<String>,
    table: Option<String>,
    column: Option<String>,
) -> Option<ColumnRef> {
    Some(ColumnRef {
        table: TableRef {
            schema: schema?,
            name: table?,
        },
        column: column?,
    })
}

/// One column, with the table it belongs to.
struct ColumnRow {
    table: TableRef,
    column: SourceColumn,
}

/// Folds column rows, sorted by table, into one [`SourceTable`] per table. A column repeated
/// back to back (one row per foreign key it is in) keeps its first row.
fn group_into_tables(rows: Vec<ColumnRow>) -> Vec<SourceTable> {
    let mut tables: Vec<SourceTable> = Vec::new();
    for ColumnRow { table, column } in rows {
        match tables.last_mut() {
            Some(last) if last.table == table => {
                if last
                    .columns
                    .last()
                    .is_none_or(|prev| prev.name != column.name)
                {
                    last.columns.push(column);
                }
            }
            _ => tables.push(SourceTable {
                table,
                columns: vec![column],
            }),
        }
    }
    tables
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(schema: &str, table: &str, column: &str) -> ColumnRow {
        ColumnRow {
            table: TableRef {
                schema: schema.to_string(),
                name: table.to_string(),
            },
            column: SourceColumn {
                name: column.to_string(),
                data_type: "integer".to_string(),
                nullable: false,
                primary_key: false,
                foreign_key: None,
            },
        }
    }

    fn referencing(mut row: ColumnRow, table: &str, column: &str) -> ColumnRow {
        row.column.foreign_key = foreign_key(
            Some("public".to_string()),
            Some(table.to_string()),
            Some(column.to_string()),
        );
        row
    }

    #[test]
    fn groups_consecutive_columns_by_table() {
        let tables = group_into_tables(vec![
            row("public", "customers", "id"),
            row("public", "orders", "id"),
            row("public", "orders", "total"),
            row("sales", "orders", "id"),
        ]);
        let shape: Vec<(String, String, usize)> = tables
            .into_iter()
            .map(|t| (t.table.schema, t.table.name, t.columns.len()))
            .collect();
        assert_eq!(
            shape,
            vec![
                ("public".to_string(), "customers".to_string(), 1),
                ("public".to_string(), "orders".to_string(), 2),
                ("sales".to_string(), "orders".to_string(), 1),
            ]
        );
    }

    /// MySQL returns a column once per foreign key it is in; the first one is kept.
    #[test]
    fn a_column_in_several_foreign_keys_keeps_the_first() {
        let tables = group_into_tables(vec![
            referencing(row("public", "orders", "customer_id"), "customers", "id"),
            referencing(row("public", "orders", "customer_id"), "accounts", "id"),
            row("public", "orders", "total"),
        ]);
        let columns = &tables[0].columns;
        assert_eq!(columns.len(), 2);
        assert_eq!(
            columns[0]
                .foreign_key
                .as_ref()
                .map(|fk| fk.table.name.as_str()),
            Some("customers")
        );
        assert_eq!(columns[1].foreign_key, None);
    }

    #[test]
    fn a_foreign_key_needs_all_three_parts() {
        assert_eq!(foreign_key(None, None, None), None);
        assert_eq!(
            foreign_key(Some("public".to_string()), None, Some("id".to_string())),
            None
        );
    }
}
