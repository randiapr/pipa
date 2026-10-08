//! View: the Tables page. Lists the selected project's Iceberg tables (a table on desktop, cards
//! on mobile) and shows one table's rows, a page at a time. Read-only, so it is open to every
//! role, including the view-only `user`.

use leptos::prelude::*;
use pipa_api::TableView;

use crate::view::{PageSizePicker, ResponsiveList, RowsView, STATUS_TOAST_DURATION};
use crate::viewmodel::{SessionViewModel, StatusMessage, TablesViewModel};

#[component]
pub fn Tables() -> impl IntoView {
    let session = expect_context::<SessionViewModel>();
    let status = RwSignal::new(Option::<StatusMessage>::None);
    let vm = TablesViewModel::new(session, status);

    // Reloads when the selected project (or the signed-in user) changes.
    Effect::new(move |_| vm.refresh());
    // Fetches the open table's page again, from the same row, when the page size changes
    // (picked on desktop, or a resize swapping layouts).
    Effect::watch(
        move || vm.page_size.get(),
        move |_, _, _| vm.reload(),
        false,
    );
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
                <ResponsiveList
                    table=move || {
                        view! {
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
                                                            <ViewButton vm=vm table=to_open />
                                                        </td>
                                                    </tr>
                                                }
                                            }
                                        />
                                    </tbody>
                                </table>
                            </div>
                        }
                    }
                    cards=move || {
                        view! {
                            <For
                                each=move || vm.tables.get()
                                key=|table| (table.source_id.clone(), table.name.clone())
                                children=move |table| {
                                    let to_open = table.clone();
                                    view! {
                                        <div class="card card-border card-sm bg-base-100">
                                            <div class="card-body">
                                                <h3 class="card-title font-mono break-all">{table.name}</h3>
                                                <p class="text-sm text-base-content/70">
                                                    {format!("Data source: {}", table.source_name)}
                                                </p>
                                                <div class="card-actions justify-end">
                                                    <ViewButton vm=vm table=to_open />
                                                </div>
                                            </div>
                                        </div>
                                    }
                                }
                            />
                        }
                    }
                />
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
                                {move || vm.rows.get().map(|rows| view! { <RowsView rows=rows /> })}
                                <div class="flex flex-wrap items-center justify-between gap-3">
                                    <span class="text-sm text-base-content/70">
                                        {move || {
                                            let start = vm.offset.get() + 1;
                                            match vm.rows.with(|rows| rows.as_ref().map(Vec::len)) {
                                                Some(count) if count > 0 => {
                                                    format!("Rows {start}\u{2013}{}", start + count - 1)
                                                }
                                                _ => format!("Rows from {start}"),
                                            }
                                        }}
                                    </span>
                                    <div class="flex flex-wrap items-center gap-3">
                                        <PageSizePicker size=vm.page_size />
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
                            </div>
                        }
                    })
            }}
        </div>
    }
}

/// Opens `table`'s rows below the list.
#[component]
fn ViewButton(vm: TablesViewModel, table: TableView) -> impl IntoView {
    view! {
        <button class="btn btn-sm btn-primary" type="button" on:click=move |_| vm.open_table(table.clone())>
            "View"
        </button>
    }
}
