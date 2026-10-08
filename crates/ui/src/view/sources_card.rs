//! View: the Sources card — a paginated list of registered data sources (a table on desktop,
//! cards on mobile) with per-source connection-test and remove actions, plus a "register a
//! data source" form presented as a native `<dialog>` modal (`showModal()`/`close()`) rather
//! than an inline form. Both layouts share the same dialogs.

use leptos::ev::SubmitEvent;
use leptos::html;
use leptos::prelude::*;
use pipa_api::{ConnectionTestOutcome, DataSourceView, DbEngine};

use crate::view::{Pagination, ResponsiveList, TrashIcon};
use crate::viewmodel::SourcesViewModel;

#[component]
pub fn SourcesCard(vm: SourcesViewModel) -> impl IntoView {
    let register_dialog = NodeRef::<html::Dialog>::new();
    let delete_dialog = NodeRef::<html::Dialog>::new();
    let pending_delete = RwSignal::new(Option::<String>::None);

    let open_register = move |_| {
        if let Some(dialog) = register_dialog.get() {
            let _ = dialog.show_modal();
        }
    };

    let on_submit_register = move |ev: SubmitEvent| {
        vm.submit_new(ev);
        if let Some(dialog) = register_dialog.get() {
            dialog.close();
        }
    };

    // Fires on every close, whatever the cause (submit, Cancel, Esc), so the
    // form always starts empty next time it's opened. Leaves `engine` alone, matching
    // `SourcesViewModel::submit_new`'s own reset — the last-picked engine is a sensible
    // default to keep across registrations.
    let on_register_closed = move |_| {
        vm.name.set(String::new());
        vm.host.set(String::new());
        vm.port.set(String::new());
        vm.username.set(String::new());
        vm.password.set(String::new());
        vm.database.set(String::new());
    };

    let on_confirm_delete = move |_| {
        if let Some(id) = pending_delete.get() {
            vm.delete(id);
        }
        if let Some(dialog) = delete_dialog.get() {
            dialog.close();
        }
    };

    view! {
        <section id="sources" class="card card-border bg-base-100 shadow-xl">
            <div class="card-body">
                <div class="flex items-center justify-between">
                    <h2 class="card-title">"Registered data sources"</h2>
                    <button class="btn btn-primary btn-sm" type="button" on:click=open_register>
                        "New Data Source"
                    </button>
                </div>
                <Show
                    when=move || !vm.list.is_empty()
                    fallback=|| {
                        view! { <p class="text-base-content/70">"No data sources registered yet."</p> }
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
                                                <th>"Engine"</th>
                                                <th>"Connection"</th>
                                                <th>"Project"</th>
                                                <th>"Status"</th>
                                                <th class="text-right">"Actions"</th>
                                            </tr>
                                        </thead>
                                        <tbody>
                                            <For
                                                each=move || vm.list.paged()
                                                key=|source| source.id.clone()
                                                children=move |source| {
                                                    view! {
                                                        <SourceRow
                                                            vm=vm
                                                            delete_dialog=delete_dialog
                                                            pending_delete=pending_delete
                                                            source=source
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
                                    each=move || vm.list.paged()
                                    key=|source| source.id.clone()
                                    children=move |source| {
                                        view! {
                                            <SourceItemCard
                                                vm=vm
                                                delete_dialog=delete_dialog
                                                pending_delete=pending_delete
                                                source=source
                                            />
                                        }
                                    }
                                />
                            }
                        }
                    />
                    <div class="card-actions justify-end">
                        <Pagination list=vm.list />
                    </div>
                </Show>
            </div>
        </section>

        <dialog node_ref=register_dialog class="modal" on:close=on_register_closed>
            <div class="modal-box max-h-[85vh] overflow-y-auto">
                <button
                    class="btn btn-sm btn-circle btn-ghost absolute right-2 top-2"
                    type="button"
                    on:click=move |_| {
                        if let Some(dialog) = register_dialog.get() {
                            dialog.close();
                        }
                    }
                >
                    "✕"
                </button>
                <h3 class="text-lg font-bold">"Register a data source"</h3>
                <form class="mt-4 flex flex-col gap-4" on:submit=on_submit_register>
                    <fieldset class="fieldset">
                        <legend class="fieldset-legend">"Name"</legend>
                        <input
                            type="text"
                            class="input w-full"
                            required
                            prop:value=move || vm.name.get()
                            on:input:target=move |ev| vm.name.set(ev.target().value())
                        />
                    </fieldset>
                    <fieldset class="fieldset">
                        <legend class="fieldset-legend">"Engine"</legend>
                        <select
                            class="select w-full"
                            prop:value=move || vm.engine.get().as_str()
                            on:change:target=move |ev| {
                                let value = ev.target().value();
                                vm.engine.set(if value == "mysql" { DbEngine::MySql } else { DbEngine::Postgres });
                            }
                        >
                            <option value="postgres">"PostgreSQL"</option>
                            <option value="mysql">"MySQL"</option>
                        </select>
                    </fieldset>
                    <fieldset class="fieldset">
                        <legend class="fieldset-legend">"Host"</legend>
                        <input
                            type="text"
                            class="input w-full"
                            required
                            prop:value=move || vm.host.get()
                            on:input:target=move |ev| vm.host.set(ev.target().value())
                        />
                    </fieldset>
                    <fieldset class="fieldset">
                        <legend class="fieldset-legend">"Port"</legend>
                        <input
                            type="text"
                            class="input w-full"
                            required
                            placeholder=move || vm.engine.get().default_port().to_string()
                            prop:value=move || vm.port.get()
                            on:input:target=move |ev| vm.port.set(ev.target().value())
                        />
                    </fieldset>
                    <fieldset class="fieldset">
                        <legend class="fieldset-legend">"Username"</legend>
                        <input
                            type="text"
                            class="input w-full"
                            required
                            prop:value=move || vm.username.get()
                            on:input:target=move |ev| vm.username.set(ev.target().value())
                        />
                    </fieldset>
                    <fieldset class="fieldset">
                        <legend class="fieldset-legend">"Password"</legend>
                        <input
                            type="password"
                            class="input w-full"
                            prop:value=move || vm.password.get()
                            on:input:target=move |ev| vm.password.set(ev.target().value())
                        />
                    </fieldset>
                    <fieldset class="fieldset">
                        <legend class="fieldset-legend">"Database"</legend>
                        <input
                            type="text"
                            class="input w-full"
                            required
                            prop:value=move || vm.database.get()
                            on:input:target=move |ev| vm.database.set(ev.target().value())
                        />
                    </fieldset>
                    <div class="modal-action">
                        <button
                            class="btn"
                            type="button"
                            on:click=move |_| {
                                if let Some(dialog) = register_dialog.get() {
                                    dialog.close();
                                }
                            }
                        >
                            "Cancel"
                        </button>
                        <button class="btn btn-primary" type="submit">
                            "Register"
                        </button>
                    </div>
                </form>
            </div>
        </dialog>

        <dialog node_ref=delete_dialog class="modal">
            <div class="modal-box">
                <button
                    class="btn btn-sm btn-circle btn-ghost absolute right-2 top-2"
                    type="button"
                    on:click=move |_| {
                        if let Some(dialog) = delete_dialog.get() {
                            dialog.close();
                        }
                    }
                >
                    "✕"
                </button>
                <h3 class="text-lg font-bold">"Remove data source"</h3>
                <p class="py-4">
                    {move || {
                        pending_delete
                            .get()
                            .and_then(|id| vm.name_of(&id))
                            .map(|name| {
                                format!("Are you sure you want to remove \"{name}\"? This cannot be undone.")
                            })
                            .unwrap_or_default()
                    }}
                </p>
                <div class="modal-action">
                    <button
                        class="btn"
                        type="button"
                        on:click=move |_| {
                            if let Some(dialog) = delete_dialog.get() {
                                dialog.close();
                            }
                        }
                    >
                        "Cancel"
                    </button>
                    <button class="btn btn-error" type="button" on:click=on_confirm_delete>
                        "Remove"
                    </button>
                </div>
            </div>
        </dialog>
    }
}

/// A single data source row. Unlike a project row, a source has no inline edit, so its own
/// fields (name/engine/connection) are safe to read once from `source` — only its project's
/// *name* can change out from under it, which is why that one field is still looked up
/// reactively via `vm.project_label`.
#[component]
fn SourceRow(
    vm: SourcesViewModel,
    delete_dialog: NodeRef<html::Dialog>,
    pending_delete: RwSignal<Option<String>>,
    source: DataSourceView,
) -> impl IntoView {
    let project_id = source.project_id.clone();

    view! {
        <tr>
            <td>
                <strong>{source.name.clone()}</strong>
            </td>
            <td>{SourcesViewModel::engine_label(source.engine)}</td>
            <td>{SourcesViewModel::connection_label(&source)}</td>
            <td>{move || vm.project_label(&project_id).unwrap_or_else(|| "\u{2014}".to_string())}</td>
            <td>
                <TestStatus vm=vm id=source.id.clone() />
            </td>
            <td class="text-right">
                <SourceActions
                    vm=vm
                    delete_dialog=delete_dialog
                    pending_delete=pending_delete
                    id=source.id
                />
            </td>
        </tr>
    }
}

/// The mobile counterpart of [`SourceRow`], reading `source` the same way.
#[component]
fn SourceItemCard(
    vm: SourcesViewModel,
    delete_dialog: NodeRef<html::Dialog>,
    pending_delete: RwSignal<Option<String>>,
    source: DataSourceView,
) -> impl IntoView {
    let project_id = source.project_id.clone();

    view! {
        <div class="card card-border card-sm bg-base-100">
            <div class="card-body">
                <div class="flex items-start justify-between gap-2">
                    <h3 class="card-title break-all">{source.name.clone()}</h3>
                    <span class="badge badge-ghost badge-sm shrink-0">
                        {SourcesViewModel::engine_label(source.engine)}
                    </span>
                </div>
                <p class="font-mono text-sm break-all">{SourcesViewModel::connection_label(&source)}</p>
                <p class="text-sm text-base-content/70">
                    {move || {
                        vm.project_label(&project_id)
                            .map(|name| format!("Project: {name}"))
                            .unwrap_or_else(|| "No project".to_string())
                    }}
                </p>
                <div class="card-actions items-center justify-between">
                    <TestStatus vm=vm id=source.id.clone() />
                    <SourceActions
                        vm=vm
                        delete_dialog=delete_dialog
                        pending_delete=pending_delete
                        id=source.id
                    />
                </div>
            </div>
        </div>
    }
}

/// The badge for a source's last connection test; nothing until it has been tested.
#[component]
fn TestStatus(vm: SourcesViewModel, id: String) -> impl IntoView {
    move || {
        vm.test_result(&id).map(|outcome| match outcome {
            ConnectionTestOutcome::Reachable => {
                view! { <span class="badge badge-success badge-sm">"reachable"</span> }.into_any()
            }
            ConnectionTestOutcome::Unreachable { reason } => view! {
                <span class="badge badge-error badge-sm" title=reason>
                    "unreachable"
                </span>
            }
            .into_any(),
        })
    }
}

/// "Test connection" and remove buttons for one source.
#[component]
fn SourceActions(
    vm: SourcesViewModel,
    delete_dialog: NodeRef<html::Dialog>,
    pending_delete: RwSignal<Option<String>>,
    id: String,
) -> impl IntoView {
    let id_for_test = id.clone();
    let id_for_delete = id;

    let on_test = move |_| vm.test(id_for_test.clone());
    let on_delete = move |_| {
        pending_delete.set(Some(id_for_delete.clone()));
        if let Some(dialog) = delete_dialog.get() {
            let _ = dialog.show_modal();
        }
    };

    view! {
        <div class="join">
            <button class="join-item btn btn-sm" on:click=on_test type="button">
                "Test connection"
            </button>
            <button
                class="join-item btn btn-sm btn-square btn-error btn-soft"
                type="button"
                title="Remove"
                aria-label="Remove"
                on:click=on_delete
            >
                <TrashIcon />
            </button>
        </div>
    }
}
