//! View: one page of a query or table result — a table with one column per key on desktop, or
//! one card per row listing `column: value` pairs on mobile. Callers render the row count and
//! the pager around it, since they know the whole result's size and how it is paged.

use std::sync::Arc;

use leptos::prelude::*;

use crate::components::Card;
use crate::view::ResponsiveList;
use crate::viewmodel::{cell_text, columns_of};

/// Renders `rows` (JSON objects), or "No rows." when there are none.
#[component]
pub fn RowsView(rows: Vec<serde_json::Value>) -> impl IntoView {
    if rows.is_empty() {
        return view! { <p class="text-base-content/70">"No rows."</p> }.into_any();
    }
    let columns = Arc::new(columns_of(&rows));
    let rows = Arc::new(rows);
    let (table_rows, table_columns) = (Arc::clone(&rows), Arc::clone(&columns));

    view! {
        <ResponsiveList
            table=move || view! { <RowsTable rows=Arc::clone(&table_rows) columns=Arc::clone(&table_columns) /> }
            cards=move || view! { <RowsCards rows=Arc::clone(&rows) columns=Arc::clone(&columns) /> }
        />
    }
    .into_any()
}

#[component]
fn RowsTable(rows: Arc<Vec<serde_json::Value>>, columns: Arc<Vec<String>>) -> impl IntoView {
    view! {
        <div class="overflow-x-auto">
            <table class="table table-zebra table-sm">
                <thead>
                    <tr>
                        {columns
                            .iter()
                            .map(|column| view! { <th>{column.clone()}</th> })
                            .collect_view()}
                    </tr>
                </thead>
                <tbody>
                    {rows
                        .iter()
                        .map(|row| {
                            view! {
                                <tr>
                                    {columns
                                        .iter()
                                        .map(|column| {
                                            view! { <td>{cell_text(row.get(column))}</td> }
                                        })
                                        .collect_view()}
                                </tr>
                            }
                        })
                        .collect_view()}
                </tbody>
            </table>
        </div>
    }
}

#[component]
fn RowsCards(rows: Arc<Vec<serde_json::Value>>, columns: Arc<Vec<String>>) -> impl IntoView {
    rows.iter()
        .map(|row| {
            // Owned up front: the card's children are a closure that must not borrow `row`.
            let cells: Vec<(String, String)> = columns
                .iter()
                .map(|column| (column.clone(), cell_text(row.get(column))))
                .collect();
            view! {
                <Card compact=true>
                    <dl class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-sm">
                        {cells
                            .into_iter()
                            .map(|(column, value)| {
                                view! {
                                    <dt class="font-mono text-base-content/70">{column}</dt>
                                    <dd class="break-all">{value}</dd>
                                }
                            })
                            .collect_view()}
                    </dl>
                </Card>
            }
        })
        .collect_view()
}
