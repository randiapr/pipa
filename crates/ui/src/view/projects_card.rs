//! View: the Projects card — a paginated list of existing projects (a table on desktop, cards
//! on mobile), with create and edit each presented as a native `<dialog>` modal
//! (`showModal()`/`close()`), not inline forms. Both layouts share the same dialogs.

use leptos::ev::SubmitEvent;
use leptos::html;
use leptos::prelude::*;

use crate::view::{EditIcon, Pagination, ResponsiveList, TrashIcon};
use crate::viewmodel::{ProjectsViewModel, SessionViewModel};

#[component]
pub fn ProjectsCard(vm: ProjectsViewModel) -> impl IntoView {
    // Creating, editing and deleting projects is admin-only (the backend enforces it too).
    let session = expect_context::<SessionViewModel>();
    let create_dialog = NodeRef::<html::Dialog>::new();
    let edit_dialog = NodeRef::<html::Dialog>::new();
    let delete_dialog = NodeRef::<html::Dialog>::new();
    let pending_delete = RwSignal::new(Option::<String>::None);

    let open_create = move |_| {
        if let Some(dialog) = create_dialog.get() {
            let _ = dialog.show_modal();
        }
    };

    let on_submit_create = move |ev: SubmitEvent| {
        vm.submit_new(ev);
        if let Some(dialog) = create_dialog.get() {
            dialog.close();
        }
    };

    // Fires on every close, whatever the cause (submit, Cancel, Esc), so the
    // form always starts empty next time it's opened.
    let on_create_closed = move |_| {
        vm.name.set(String::new());
        vm.description.set(String::new());
    };

    let on_submit_edit = move |ev: SubmitEvent| {
        ev.prevent_default();
        if let Some(id) = vm.editing_id.get() {
            vm.save_edit(id);
        }
        if let Some(dialog) = edit_dialog.get() {
            dialog.close();
        }
    };

    // Fires on every close, whatever the cause, so a dismissed edit doesn't leave a project
    // stuck looking "in progress" (`vm.is_editing` stays keyed to a row otherwise).
    let on_edit_closed = move |_| vm.cancel_edit();

    let on_confirm_delete = move |_| {
        if let Some(id) = pending_delete.get() {
            vm.delete(id);
        }
        if let Some(dialog) = delete_dialog.get() {
            dialog.close();
        }
    };

    view! {
        <section id="projects" class="card card-border bg-base-100 shadow-xl">
            <div class="card-body">
                <div class="flex items-center justify-between">
                    <h2 class="card-title">"Projects"</h2>
                    <Show when=move || session.is_admin()>
                        <button class="btn btn-primary btn-sm" type="button" on:click=open_create>
                            "New Project"
                        </button>
                    </Show>
                </div>

                <Show
                    when=move || !vm.list.is_empty()
                    fallback=|| view! { <p class="text-base-content/70">"No projects yet."</p> }
                >
                    <ResponsiveList
                        table=move || {
                            view! {
                                <div class="overflow-x-auto">
                                    <table class="table">
                                        <thead>
                                            <tr>
                                                <th>"Name"</th>
                                                <th>"Description"</th>
                                                <Show when=move || session.is_admin()>
                                                    <th class="text-right">"Actions"</th>
                                                </Show>
                                            </tr>
                                        </thead>
                                        <tbody>
                                            <For
                                                each=move || vm.list.paged()
                                                key=|project| project.id.clone()
                                                children=move |project| {
                                                    view! {
                                                        <ProjectRow
                                                            vm=vm
                                                            edit_dialog=edit_dialog
                                                            delete_dialog=delete_dialog
                                                            pending_delete=pending_delete
                                                            id=project.id
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
                                    key=|project| project.id.clone()
                                    children=move |project| {
                                        view! {
                                            <ProjectItemCard
                                                vm=vm
                                                edit_dialog=edit_dialog
                                                delete_dialog=delete_dialog
                                                pending_delete=pending_delete
                                                id=project.id
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

        <dialog node_ref=create_dialog class="modal" on:close=on_create_closed>
            <div class="modal-box max-h-[85vh] overflow-y-auto">
                <button
                    class="btn btn-sm btn-circle btn-ghost absolute right-2 top-2"
                    type="button"
                    on:click=move |_| {
                        if let Some(dialog) = create_dialog.get() {
                            dialog.close();
                        }
                    }
                >
                    "✕"
                </button>
                <h3 class="text-lg font-bold">"Create project"</h3>
                <form class="mt-4 flex flex-col gap-4" on:submit=on_submit_create>
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
                        <legend class="fieldset-legend">"Description"</legend>
                        <input
                            type="text"
                            class="input w-full"
                            prop:value=move || vm.description.get()
                            on:input:target=move |ev| vm.description.set(ev.target().value())
                        />
                    </fieldset>
                    <div class="modal-action">
                        <button
                            class="btn"
                            type="button"
                            on:click=move |_| {
                                if let Some(dialog) = create_dialog.get() {
                                    dialog.close();
                                }
                            }
                        >
                            "Cancel"
                        </button>
                        <button class="btn btn-primary" type="submit">
                            "Create"
                        </button>
                    </div>
                </form>
            </div>
        </dialog>

        <dialog node_ref=edit_dialog class="modal" on:close=on_edit_closed>
            <div class="modal-box max-h-[85vh] overflow-y-auto">
                <button
                    class="btn btn-sm btn-circle btn-ghost absolute right-2 top-2"
                    type="button"
                    on:click=move |_| {
                        if let Some(dialog) = edit_dialog.get() {
                            dialog.close();
                        }
                    }
                >
                    "✕"
                </button>
                <h3 class="text-lg font-bold">"Edit project"</h3>
                <form class="mt-4 flex flex-col gap-4" on:submit=on_submit_edit>
                    <fieldset class="fieldset">
                        <legend class="fieldset-legend">"Name"</legend>
                        <input
                            type="text"
                            class="input w-full"
                            required
                            prop:value=move || vm.edit_name.get()
                            on:input:target=move |ev| vm.edit_name.set(ev.target().value())
                        />
                    </fieldset>
                    <fieldset class="fieldset">
                        <legend class="fieldset-legend">"Description"</legend>
                        <input
                            type="text"
                            class="input w-full"
                            prop:value=move || vm.edit_description.get()
                            on:input:target=move |ev| vm.edit_description.set(ev.target().value())
                        />
                    </fieldset>
                    <div class="modal-action">
                        <button
                            class="btn"
                            type="button"
                            on:click=move |_| {
                                if let Some(dialog) = edit_dialog.get() {
                                    dialog.close();
                                }
                            }
                        >
                            "Cancel"
                        </button>
                        <button class="btn btn-primary" type="submit">
                            "Save"
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
                <h3 class="text-lg font-bold">"Delete project"</h3>
                <p class="py-4">
                    {move || {
                        pending_delete
                            .get()
                            .and_then(|id| vm.name_of(&id))
                            .map(|name| {
                                format!("Are you sure you want to delete \"{name}\"? This cannot be undone.")
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
                        "Delete"
                    </button>
                </div>
            </div>
        </dialog>
    }
}

/// A single project row. Fields are looked up from `vm.list` by `id` on every render
/// (rather than captured once from the row's initial data) so an edit elsewhere is reflected
/// immediately — `<For>` keys rows by id and reuses the DOM node across an edit, so it never
/// recreates this component just because the underlying project changed.
#[component]
fn ProjectRow(
    vm: ProjectsViewModel,
    edit_dialog: NodeRef<html::Dialog>,
    delete_dialog: NodeRef<html::Dialog>,
    pending_delete: RwSignal<Option<String>>,
    id: String,
) -> impl IntoView {
    let session = expect_context::<SessionViewModel>();
    let id_for_name = id.clone();
    let id_for_description = id.clone();

    view! {
        <tr>
            <td>{move || vm.name_of(&id_for_name).unwrap_or_default()}</td>
            <td>{move || vm.description_of(&id_for_description).unwrap_or_default()}</td>
            <Show when=move || session.is_admin()>
                <td class="text-right">
                    <ProjectActions
                        vm=vm
                        edit_dialog=edit_dialog
                        delete_dialog=delete_dialog
                        pending_delete=pending_delete
                        id=id.clone()
                    />
                </td>
            </Show>
        </tr>
    }
}

/// The mobile counterpart of [`ProjectRow`], looked up by `id` the same way.
#[component]
fn ProjectItemCard(
    vm: ProjectsViewModel,
    edit_dialog: NodeRef<html::Dialog>,
    delete_dialog: NodeRef<html::Dialog>,
    pending_delete: RwSignal<Option<String>>,
    id: String,
) -> impl IntoView {
    let session = expect_context::<SessionViewModel>();
    let id_for_name = id.clone();
    let id_for_description = id.clone();

    view! {
        <div class="card card-border card-sm bg-base-100">
            <div class="card-body">
                <h3 class="card-title">{move || vm.name_of(&id_for_name).unwrap_or_default()}</h3>
                <p class="text-base-content/70">
                    {move || {
                        vm.description_of(&id_for_description)
                            .unwrap_or_else(|| "No description".to_string())
                    }}
                </p>
                <Show when=move || session.is_admin()>
                    <div class="card-actions justify-end">
                        <ProjectActions
                            vm=vm
                            edit_dialog=edit_dialog
                            delete_dialog=delete_dialog
                            pending_delete=pending_delete
                            id=id.clone()
                        />
                    </div>
                </Show>
            </div>
        </div>
    }
}

/// Edit/delete buttons for one project (admin only — callers decide whether to show them).
#[component]
fn ProjectActions(
    vm: ProjectsViewModel,
    edit_dialog: NodeRef<html::Dialog>,
    delete_dialog: NodeRef<html::Dialog>,
    pending_delete: RwSignal<Option<String>>,
    id: String,
) -> impl IntoView {
    let id_for_edit = id.clone();
    let id_for_delete = id;

    view! {
        <div class="join">
            <button
                class="join-item btn btn-sm btn-square"
                type="button"
                title="Edit"
                aria-label="Edit"
                on:click=move |_| {
                    vm.start_edit(id_for_edit.clone());
                    if let Some(dialog) = edit_dialog.get() {
                        let _ = dialog.show_modal();
                    }
                }
            >
                <EditIcon />
            </button>
            <button
                class="join-item btn btn-sm btn-square btn-error btn-soft"
                type="button"
                title="Delete"
                aria-label="Delete"
                on:click=move |_| {
                    pending_delete.set(Some(id_for_delete.clone()));
                    if let Some(dialog) = delete_dialog.get() {
                        let _ = dialog.show_modal();
                    }
                }
            >
                <TrashIcon />
            </button>
        </div>
    }
}
