//! Postgres adapter for [`SnapshotSource`]: reads a table's existing rows in key order, a page at
//! a time, with plain `SELECT`s.

use async_trait::async_trait;
use sqlx::{PgConnection, Row};

use crate::capture::domain::{CaptureError, ColumnValue, SnapshotRead, SnapshotSource};
use crate::datasource::{DataSource, TableRef};
use crate::postgres::{self, quote_ident};

/// Reads a Postgres data source's tables with keyset pagination over each table's row key (its
/// replica identity, the same key the changelog's current rows are collapsed by). Each page is
/// its own short statement, so no lock or long-running snapshot is held on the source.
pub struct PostgresSnapshotSource;

impl PostgresSnapshotSource {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PostgresSnapshotSource {
    fn default() -> Self {
        Self::new()
    }
}

/// A column of the table being read, with the SQL type a key value is cast back to.
struct Column {
    name: String,
    sql_type: String,
}

/// The alias the table being read goes by in [`chunk_query`].
const SOURCE_ALIAS: &str = "src";

fn snapshot_error(err: sqlx::Error) -> CaptureError {
    CaptureError::Snapshot(err.to_string())
}

/// `table`'s columns in order, as the change log carries them: generated columns are left out,
/// like `pgoutput` leaves them out.
async fn columns(conn: &mut PgConnection, table: &TableRef) -> Result<Vec<Column>, CaptureError> {
    let rows = sqlx::query(
        "SELECT a.attname AS name, format_type(a.atttypid, a.atttypmod) AS sql_type \
         FROM pg_attribute a \
         JOIN pg_class c ON c.oid = a.attrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = $1 AND c.relname = $2 \
           AND a.attnum > 0 AND NOT a.attisdropped AND a.attgenerated = '' \
         ORDER BY a.attnum",
    )
    .bind(&table.schema)
    .bind(&table.name)
    .fetch_all(&mut *conn)
    .await
    .map_err(snapshot_error)?;
    rows.into_iter()
        .map(|row| {
            Ok(Column {
                name: row.try_get("name").map_err(snapshot_error)?,
                sql_type: row.try_get("sql_type").map_err(snapshot_error)?,
            })
        })
        .collect()
}

/// The page query: every column as text, rows after the bound key (`$1…$n`, text cast back to
/// each key column's type) when `continued`, in key order, at most `limit` of them.
///
/// The key columns are qualified with the table's alias: the text columns selected carry the
/// same names, and an unqualified `ORDER BY "id"` would sort by those — as text, so `10` before
/// `2` — while the `WHERE` compared typed values, skipping rows between pages.
fn chunk_query(
    table: &TableRef,
    columns: &[Column],
    keys: &[&Column],
    continued: bool,
    limit: usize,
) -> String {
    let select = columns
        .iter()
        .map(|column| format!("{}::text", quote_ident(&column.name)))
        .collect::<Vec<_>>()
        .join(", ");
    let key_list = keys
        .iter()
        .map(|key| format!("{SOURCE_ALIAS}.{}", quote_ident(&key.name)))
        .collect::<Vec<_>>()
        .join(", ");
    let filter = if continued {
        let bounds = keys
            .iter()
            .enumerate()
            .map(|(index, key)| format!("${}::text::{}", index + 1, key.sql_type))
            .collect::<Vec<_>>()
            .join(", ");
        format!(" WHERE ({key_list}) > ({bounds})")
    } else {
        String::new()
    };
    format!(
        "SELECT {select} FROM ONLY {}.{} AS {SOURCE_ALIAS}{filter} ORDER BY {key_list} LIMIT {limit}",
        quote_ident(&table.schema),
        quote_ident(&table.name),
    )
}

#[async_trait]
impl SnapshotSource for PostgresSnapshotSource {
    async fn list_tables(&self, source: &DataSource) -> Result<Vec<TableRef>, CaptureError> {
        let mut conn = postgres::connect(source)
            .await
            .map_err(|err| CaptureError::Connect(err.to_string()))?;
        // Ordinary tables only, partitions included: a partitioned table's changes are
        // published under its partitions' names, and that is where its rows are read too.
        let rows = sqlx::query(
            "SELECT n.nspname AS schema_name, c.relname AS table_name \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.relkind = 'r' \
               AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
               AND n.nspname NOT LIKE 'pg_toast%' \
             ORDER BY n.nspname, c.relname",
        )
        .fetch_all(&mut conn)
        .await
        .map_err(snapshot_error)?;
        rows.into_iter()
            .map(|row| {
                Ok(TableRef {
                    schema: row.try_get("schema_name").map_err(snapshot_error)?,
                    name: row.try_get("table_name").map_err(snapshot_error)?,
                })
            })
            .collect()
    }

    async fn read_chunk(
        &self,
        source: &DataSource,
        table: &TableRef,
        after: Option<&[String]>,
        limit: usize,
    ) -> Result<SnapshotRead, CaptureError> {
        let mut conn = postgres::connect(source)
            .await
            .map_err(|err| CaptureError::Connect(err.to_string()))?;
        let Some(key_names) = postgres::key_columns(&mut conn)
            .await
            .map_err(snapshot_error)?
            .remove(&(table.schema.clone(), table.name.clone()))
        else {
            return Ok(SnapshotRead::Keyless);
        };
        let columns = columns(&mut conn, table).await?;
        let keys = key_names
            .iter()
            .map(|name| {
                columns
                    .iter()
                    .find(|column| &column.name == name)
                    .ok_or_else(|| {
                        CaptureError::Snapshot(format!(
                            "key column {name:?} of {}.{} is not a stored column",
                            table.schema, table.name
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(after) = after
            && after.len() != keys.len()
        {
            return Err(CaptureError::Snapshot(format!(
                "resume key {after:?} doesn't match the key {key_names:?} of {}.{}",
                table.schema, table.name
            )));
        }

        // Identifiers are quoted by `quote_ident` and types come from the database's own
        // `format_type`; the only values are bound parameters.
        let sql = sqlx::AssertSqlSafe(chunk_query(table, &columns, &keys, after.is_some(), limit));
        let mut query = sqlx::query(sql);
        for value in after.unwrap_or_default() {
            query = query.bind(value.as_str());
        }
        let fetched = query.fetch_all(&mut conn).await.map_err(snapshot_error)?;

        let mut rows = Vec::with_capacity(fetched.len());
        for row in &fetched {
            let values = columns
                .iter()
                .enumerate()
                .map(|(index, column)| {
                    Ok(ColumnValue {
                        name: column.name.clone(),
                        value: row
                            .try_get::<Option<String>, _>(index)
                            .map_err(snapshot_error)?,
                    })
                })
                .collect::<Result<Vec<_>, CaptureError>>()?;
            rows.push(values);
        }
        let last_key = rows.last().map(|values: &Vec<ColumnValue>| {
            keys.iter()
                .map(|key| {
                    values
                        .iter()
                        .find(|value| value.name == key.name)
                        .and_then(|value| value.value.clone())
                        // A replica identity's columns are NOT NULL.
                        .unwrap_or_default()
                })
                .collect()
        });
        Ok(SnapshotRead::Chunk { rows, last_key })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str, sql_type: &str) -> Column {
        Column {
            name: name.to_string(),
            sql_type: sql_type.to_string(),
        }
    }

    #[test]
    fn the_first_page_reads_from_the_start_in_key_order() {
        let table = TableRef {
            schema: "public".to_string(),
            name: "orders".to_string(),
        };
        let columns = [column("id", "integer"), column("total", "numeric")];
        assert_eq!(
            chunk_query(&table, &columns, &[&columns[0]], false, 100),
            "SELECT \"id\"::text, \"total\"::text FROM ONLY \"public\".\"orders\" AS src \
             ORDER BY src.\"id\" LIMIT 100"
        );
    }

    #[test]
    fn a_later_page_continues_after_a_composite_key() {
        let table = TableRef {
            schema: "Sales".to_string(),
            name: "Order Items".to_string(),
        };
        let columns = [
            column("order_id", "bigint"),
            column("line", "integer"),
            column("sku", "character varying(20)"),
        ];
        assert_eq!(
            chunk_query(&table, &columns, &[&columns[0], &columns[1]], true, 5),
            "SELECT \"order_id\"::text, \"line\"::text, \"sku\"::text \
             FROM ONLY \"Sales\".\"Order Items\" AS src \
             WHERE (src.\"order_id\", src.\"line\") > ($1::text::bigint, $2::text::integer) \
             ORDER BY src.\"order_id\", src.\"line\" LIMIT 5"
        );
    }
}
