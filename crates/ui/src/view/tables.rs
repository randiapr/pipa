//! View: the Tables page. Lists the selected project's Iceberg tables and shows one table's rows,
//! a page at a time. Read-only, so it is open to every role, including the view-only `user`.

use leptos::prelude::*;

use crate::view::{RowsTable, STATUS_TOAST_DURATION};
use crate::viewmodel::{SessionViewModel, StatusMessage, TABLE_PAGE_ROWS, TablesViewModel};

#[component]
pub fn Tables() -> impl IntoView {
    let session = expect_context::<SessionViewModel>();
    let status = RwSignal::new(Option::<StatusMessage>::None);
    let vm = TablesViewModel::new(session, status);

    // Reloads when the selected project (or the signed-in user) changes.
    Effect::new(move |_| vm.refresh());
    // Auto-dismiss the status toast so it doesn't linger on screen forever.
    Effect::new(move |_| {
        if status.get().is_some() {
            set_timeout(move || status.set(None), STATUS_TOAST_DURATION);
        }
    });

    view! {
        <div class="flex flex-col gap-6">
            <div>
                <h1 class="text-2xl font-bold">"Tables"</h1>
                <p class="text-base-content/70">
                    {move || match session.current_project() {
                        Some(project) => format!("Browse the tables of \"{}\" (read-only).", project.name),
                        None => "Select a project in the navigation bar to browse its tables.".to_string(),
                    }}
                </p>
            </div>

            {move || {
                status
                    .get()
                    .map(|msg| {
                        view! {
                            <div class="toast toast-top toast-end">
                                <div role="alert" class=msg.alert_class()>
                                    <span>{msg.text().to_string()}</span>
                                </div>
                            </div>
                        }
                    })
            }}

            <Show
                when=move || !vm.tables.get().is_empty()
                fallback=move || {
                    view! {
                        <p class="text-base-content/70">
                            {move || {
                                if session.current_project_id.get().is_none() || vm.loading.get() {
                                    ""
                                } else {
                                    "No tables yet. They appear once a data source of this project has captured changes."
                                }
                            }}
                        </p>
                    }
                }
            >
                <div class="overflow-x-auto">
                    <table class="table">
                        <thead>
                            <tr>
                                <th>"Table"</th>
                                <th>"Data source"</th>
                                <th class="text-right">"Actions"</th>
                            </tr>
                        </thead>
                        <tbody>
                            <For
                                each=move || vm.tables.get()
                                key=|table| (table.source_id.clone(), table.name.clone())
                                children=move |table| {
                                    let to_open = table.clone();
                                    view! {
                                        <tr>
                                            <td class="font-mono text-sm">{table.name}</td>
                                            <td>{table.source_name}</td>
                                            <td class="text-right">
                                                <button
                                                    class="btn btn-sm btn-primary"
                                                    type="button"
                                                    on:click=move |_| vm.open_table(to_open.clone())
                                                >
                                                    "View"
                                                </button>
                                            </td>
                                        </tr>
                                    }
                                }
                            />
                        </tbody>
                    </table>
                </div>
            </Show>

            {move || {
                vm.open
                    .get()
                    .map(|table| {
                        view! {
                            <div class="flex flex-col gap-3">
                                <div class="flex items-center justify-between">
                                    <h2 class="text-lg font-semibold font-mono">{table.name}</h2>
                                    <button class="btn btn-sm btn-ghost" type="button" on:click=move |_| vm.close()>
                                        "Close"
                                    </button>
                                </div>
                                {move || {
                                    vm.error
                                        .get()
                                        .map(|message| {
                                            view! {
                                                <div role="alert" class="alert alert-error">
                                                    <span>{message}</span>
                                                </div>
                                            }
                                        })
                                }}
                                <Show when=move || vm.loading.get()>
                                    <span class="loading loading-spinner"></span>
                                </Show>
                                {move || vm.rows.get().map(|rows| view! { <RowsTable rows=rows /> })}
                                <div class="flex items-center justify-between">
                                    <span class="text-sm text-base-content/70">
                                        {move || {
                                            let start = vm.offset.get() + 1;
                                            format!("Rows from {start} ({TABLE_PAGE_ROWS} per page)")
                                        }}
                                    </span>
                                    <div class="join">
                                        <button
                                            class="join-item btn btn-sm"
                                            type="button"
                                            disabled=move || !vm.has_prev() || vm.loading.get()
                                            on:click=move |_| vm.prev()
                                        >
                                            "\u{ab} Previous"
                                        </button>
                                        <button
                                            class="join-item btn btn-sm"
                                            type="button"
                                            disabled=move || !vm.has_next() || vm.loading.get()
                                            on:click=move |_| vm.next()
                                        >
                                            "Next \u{bb}"
                                        </button>
                                    </div>
                                </div>
                            </div>
                        }
                    })
            }}
        </div>
    }
}
