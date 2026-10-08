//! View: a query or table result — a row count above a table with one column per key.

use leptos::prelude::*;

use crate::viewmodel::{cell_text, columns_of};

/// Renders `rows` (JSON objects) as a table, or "No rows." when there are none.
#[component]
pub fn RowsTable(rows: Vec<serde_json::Value>) -> impl IntoView {
    if rows.is_empty() {
        return view! { <p class="text-base-content/70">"No rows."</p> }.into_any();
    }
    let columns = columns_of(&rows);
    let count = rows.len();
    view! {
        <div class="flex flex-col gap-2">
            <p class="text-sm text-base-content/70">{format!("{count} row(s)")}</p>
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
        </div>
    }
    .into_any()
}
