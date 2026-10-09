//! View: the Tables page. Lists the selected project's Iceberg tables as collapses, one per
//! table, and shows the open one's rows inside it, a page at a time. Read-only, so it is open to every
//! role. For everyone, rows are each table's current rows without the changelog columns
//! (`TablesViewModel::visible_rows`), and the Iceberg metadata tables aren't listed
//! (`TablesViewModel::visible_tables`).

use leptos::prelude::*;
use pipa_api::TableView;

use crate::view::{PageSizePicker, RowsView, STATUS_TOAST_DURATION};
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
                when=move || !vm.visible_tables().is_empty()
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
                <div class="flex flex-col gap-2">
                    <For
                        each=move || vm.visible_tables()
                        key=|table| (table.source_id.clone(), table.name.clone())
                        children=move |table| view! { <TableCollapse vm=vm table=table /> }
                    />
                </div>
            </Show>
        </div>
    }
}

/// One table as a daisyUI collapse: its name and data source as the title, its rows inside.
/// Only the table in `vm.open` is expanded (forced with `collapse-open`/`collapse-close`, so
/// the ViewModel stays the single source of truth); opening another closes it.
#[component]
fn TableCollapse(vm: TablesViewModel, table: TableView) -> impl IntoView {
    let key = (table.source_id.clone(), table.name.clone());
    let is_open = Memo::new(move |_| {
        vm.open.with(|open| {
            open.as_ref()
                .is_some_and(|open| (&open.source_id, &open.name) == (&key.0, &key.1))
        })
    });
    let to_open = table.clone();
    let toggle = move || {
        if is_open.get_untracked() {
            vm.close();
        } else {
            vm.open_table(to_open.clone());
        }
    };

    view! {
        <div
            class="collapse collapse-arrow border border-base-300 bg-base-100"
            class:collapse-open=move || is_open.get()
            class:collapse-close=move || !is_open.get()
        >
            <div
                class="collapse-title cursor-pointer ps-12 pe-4 after:start-5 after:end-auto"
                role="button"
                tabindex="0"
                aria-expanded=move || is_open.get().to_string()
                on:click={
                    let toggle = toggle.clone();
                    move |_| toggle()
                }
                on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                    if ev.key() == "Enter" || ev.key() == " " {
                        ev.prevent_default();
                        toggle();
                    }
                }
            >
                <div class="flex flex-wrap items-baseline gap-x-3">
                    <span class="font-mono font-semibold break-all">{table.name}</span>
                    <span class="text-sm text-base-content/70">{table.source_name}</span>
                </div>
            </div>
            <div class="collapse-content">
                <Show when=move || is_open.get()>
                    <TableRows vm=vm />
                </Show>
            </div>
        </div>
    }
}

/// The open table's current page, with its error, loading state and pager.
#[component]
fn TableRows(vm: TablesViewModel) -> impl IntoView {
    view! {
        <div class="flex flex-col gap-3">
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
            {move || vm.visible_rows().map(|rows| view! { <RowsView rows=rows /> })}
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
}
