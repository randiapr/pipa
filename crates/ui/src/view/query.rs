// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! View: the SQL query page. Queries only see the selected project's tables. Below the editor,
//! the project's saved queries: load one into the editor, run it, or remove it; the editor's SQL
//! can be saved as a new query or written over the loaded one (both asked for first).

use leptos::ev::SubmitEvent;
use leptos::html;
use leptos::prelude::*;
use pipa_api::SavedQueryView;

use crate::components::{
    Alert, Card, CardTitle, ConfirmDialog, Field, Modal, ModalActions, TextArea, TextInput, Tone,
    TrashIcon, close_dialog, open_dialog,
};
use crate::view::{Pagination, ResponsiveList, RowsView};
use crate::viewmodel::{QueryViewModel, SessionViewModel};

#[component]
pub fn Query() -> impl IntoView {
    let session = expect_context::<SessionViewModel>();
    let vm = QueryViewModel::new(session);
    Effect::new(move |_| vm.refresh_saved());

    let save_dialog = NodeRef::<html::Dialog>::new();
    let update_dialog = NodeRef::<html::Dialog>::new();
    let delete_dialog = NodeRef::<html::Dialog>::new();
    let pending_delete = RwSignal::new(Option::<String>::None);

    let on_submit_save = move |ev: SubmitEvent| {
        vm.save_new(ev);
        close_dialog(save_dialog);
    };

    view! {
        <div class="flex flex-col gap-4">
            <div>
                <h1 class="text-2xl font-bold">"Query"</h1>
                <p class="text-base-content/70">
                    {move || match session.current_project() {
                        Some(project) => format!("Running against the tables of \"{}\".", project.name),
                        None if session.is_admin() => "Running against every table (all projects).".to_string(),
                        None => "Select a project in the navigation bar to query its tables.".to_string(),
                    }}
                </p>
                <p class="text-sm text-base-content/70">
                    "Read-only SQL. Tables are addressed as "
                    <code>"<catalog>.cdc_<source id>.<schema>__<table>"</code>
                    ", e.g. "
                    <code>"SELECT * FROM pipa.cdc_0123….public__orders LIMIT 10"</code>
                    "."
                </p>
            </div>

            <form class="flex flex-col gap-3" on:submit=move |ev| vm.run(ev)>
                <TextArea
                    value=vm.sql
                    required=true
                    placeholder="SELECT …"
                    class="h-32 w-full font-mono"
                />
                <div class="flex flex-wrap gap-2">
                    <button
                        class="btn btn-primary"
                        type="submit"
                        disabled=move || vm.running.get()
                    >
                        {move || if vm.running.get() { "Running…" } else { "Run" }}
                    </button>
                    <Show when=move || vm.can_save()>
                        <button
                            class="btn"
                            type="button"
                            disabled=move || vm.sql.get().trim().is_empty()
                            on:click=move |_| {
                                vm.save_name.set(String::new());
                                open_dialog(save_dialog);
                            }
                        >
                            "Save as new…"
                        </button>
                        {move || {
                            vm.loaded_name()
                                .map(|name| {
                                    view! {
                                        <button
                                            class="btn"
                                            type="button"
                                            disabled=move || vm.sql.get().trim().is_empty()
                                            on:click=move |_| open_dialog(update_dialog)
                                        >
                                            {format!("Update \"{name}\"")}
                                        </button>
                                    }
                                })
                        }}
                    </Show>
                </div>
            </form>

            {move || {
                vm.notice
                    .get()
                    .map(|message| view! { <Alert tone=Tone::Success>{message}</Alert> })
            }}
            {move || {
                vm.error
                    .get()
                    .map(|message| view! { <Alert tone=Tone::Error>{message}</Alert> })
            }}

            <Show when=move || vm.ran.get()>
                <div class="flex flex-col gap-2">
                    <p class="text-sm text-base-content/70">
                        {move || format!("{} row(s)", vm.results.len())}
                    </p>
                    {move || view! { <RowsView rows=vm.results.paged() /> }}
                    <Show when=move || !vm.results.is_empty()>
                        <Pagination list=vm.results />
                    </Show>
                </div>
            </Show>

            <Show when=move || vm.can_save()>
                <SavedQueries vm=vm delete_dialog=delete_dialog pending_delete=pending_delete />
            </Show>
        </div>

        <Modal node_ref=save_dialog title="Save query">
            <form class="mt-4 flex flex-col gap-4" on:submit=on_submit_save>
                <Field
                    legend="Name"
                    hint="Shared with everyone who can develop in this project."
                >
                    <TextInput value=vm.save_name required=true />
                </Field>
                <ModalActions node_ref=save_dialog>
                    <button class="btn btn-primary" type="submit">
                        "Save"
                    </button>
                </ModalActions>
            </form>
        </Modal>

        <ConfirmDialog
            node_ref=update_dialog
            title="Update saved query"
            message=move || {
                format!(
                    "Replace the SQL of \"{}\" with what is in the editor? This cannot be undone.",
                    vm.loaded_name().unwrap_or_default(),
                )
            }
            confirm_label="Update"
            on_confirm=move |_| vm.update_loaded()
        />

        <ConfirmDialog
            node_ref=delete_dialog
            title="Remove saved query"
            message=move || {
                pending_delete
                    .get()
                    .and_then(|id| vm.name_of(&id))
                    .map(|name| {
                        format!(
                            "Are you sure you want to remove \"{name}\"? Everyone in the project loses it. This cannot be undone.",
                        )
                    })
                    .unwrap_or_default()
            }
            confirm_label="Remove"
            danger=true
            on_confirm=move |_| {
                if let Some(id) = pending_delete.get_untracked() {
                    vm.delete(id);
                }
            }
        />
    }
}

/// The saved queries of the selected project, a table on desktop and cards on mobile.
#[component]
fn SavedQueries(
    vm: QueryViewModel,
    delete_dialog: NodeRef<html::Dialog>,
    pending_delete: RwSignal<Option<String>>,
) -> impl IntoView {
    view! {
        <Card class="shadow-xl">
            <CardTitle>"Saved queries"</CardTitle>
            <Show
                when=move || !vm.saved.is_empty()
                fallback=|| {
                    view! {
                        <p class="text-base-content/70">
                            "Nothing saved in this project yet. Write a query and use \"Save as new…\"."
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
                                            <th>"Name"</th>
                                            <th>"SQL"</th>
                                            <th class="text-right">"Actions"</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        <For
                                            each=move || vm.saved.paged()
                                            key=|query| (query.id.clone(), query.updated_at_unix)
                                            children=move |query| {
                                                view! {
                                                    <SavedQueryRow
                                                        vm=vm
                                                        delete_dialog=delete_dialog
                                                        pending_delete=pending_delete
                                                        query=query
                                                    />
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
                                each=move || vm.saved.paged()
                                key=|query| (query.id.clone(), query.updated_at_unix)
                                children=move |query| {
                                    view! {
                                        <SavedQueryItemCard
                                            vm=vm
                                            delete_dialog=delete_dialog
                                            pending_delete=pending_delete
                                            query=query
                                        />
                                    }
                                }
                            />
                        }
                    }
                />
                <Pagination list=vm.saved />
            </Show>
        </Card>
    }
}

#[component]
fn SavedQueryRow(
    vm: QueryViewModel,
    delete_dialog: NodeRef<html::Dialog>,
    pending_delete: RwSignal<Option<String>>,
    query: SavedQueryView,
) -> impl IntoView {
    let preview = QueryViewModel::preview(&query);

    view! {
        <tr>
            <td>
                <strong>{query.name.clone()}</strong>
            </td>
            <td class="max-w-md truncate font-mono text-xs" title=query.sql.clone()>
                {preview}
            </td>
            <td class="text-right">
                <SavedQueryActions
                    vm=vm
                    delete_dialog=delete_dialog
                    pending_delete=pending_delete
                    id=query.id
                />
            </td>
        </tr>
    }
}

/// The mobile counterpart of [`SavedQueryRow`].
#[component]
fn SavedQueryItemCard(
    vm: QueryViewModel,
    delete_dialog: NodeRef<html::Dialog>,
    pending_delete: RwSignal<Option<String>>,
    query: SavedQueryView,
) -> impl IntoView {
    let preview = QueryViewModel::preview(&query);

    view! {
        <Card compact=true>
            <CardTitle level=3 class="break-all">
                {query.name}
            </CardTitle>
            <p class="font-mono text-xs break-all text-base-content/70">{preview}</p>
            <SavedQueryActions
                vm=vm
                delete_dialog=delete_dialog
                pending_delete=pending_delete
                id=query.id
            />
        </Card>
    }
}

/// "Load", "Run" and remove buttons for one saved query.
#[component]
fn SavedQueryActions(
    vm: QueryViewModel,
    delete_dialog: NodeRef<html::Dialog>,
    pending_delete: RwSignal<Option<String>>,
    id: String,
) -> impl IntoView {
    let id_for_load = id.clone();
    let id_for_run = id.clone();
    let id_for_delete = id;

    view! {
        <div class="join">
            <button
                class="join-item btn btn-sm"
                type="button"
                on:click=move |_| vm.load(&id_for_load)
            >
                "Load"
            </button>
            <button
                class="join-item btn btn-sm"
                type="button"
                disabled=move || vm.running.get()
                on:click=move |_| vm.run_saved(&id_for_run)
            >
                "Run"
            </button>
            <button
                class="join-item btn btn-sm btn-square btn-error btn-soft"
                type="button"
                title="Remove"
                aria-label="Remove"
                on:click=move |_| {
                    pending_delete.set(Some(id_for_delete.clone()));
                    open_dialog(delete_dialog);
                }
            >
                <TrashIcon />
            </button>
        </div>
    }
}
