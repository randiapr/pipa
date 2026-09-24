//! View: the Projects card — a paginated table of existing projects, with create and edit
//! each presented as a native `<dialog>` modal (`showModal()`/`close()`), not inline forms.

use leptos::ev::SubmitEvent;
use leptos::html;
use leptos::prelude::*;

use crate::view::Pagination;
use crate::viewmodel::ProjectsViewModel;

#[component]
pub fn ProjectsCard(vm: ProjectsViewModel) -> impl IntoView {
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
                    <button class="btn btn-primary btn-sm" type="button" on:click=open_create>
                        "New Project"
                    </button>
                </div>

                <Show
                    when=move || !vm.projects.get().is_empty()
                    fallback=|| view! { <p class="text-base-content/70">"No projects yet."</p> }
                >
                    <div class="overflow-x-auto">
                        <table class="table">
                            <thead>
                                <tr>
                                    <th>"Name"</th>
                                    <th>"Description"</th>
                                    <th class="text-right">"Actions"</th>
                                </tr>
                            </thead>
                            <tbody>
                                <For
                                    each=move || vm.paged()
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
                    <div class="card-actions justify-end">
                        <Pagination page=vm.page total=vm.total() />
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

/// A single project row. Fields are looked up from `vm.projects` by `id` on every render
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
    let id_for_edit = id.clone();
    let id_for_delete = id.clone();
    let id_for_name = id.clone();
    let id_for_description = id;

    view! {
        <tr>
            <td>{move || vm.name_of(&id_for_name).unwrap_or_default()}</td>
            <td>{move || {
                vm.projects
                    .get()
                    .into_iter()
                    .find(|p| p.id == id_for_description)
                    .and_then(|p| p.description)
                    .unwrap_or_default()
            }}</td>
            <td class="text-right">
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
                        <svg
                            xmlns="http://www.w3.org/2000/svg"
                            fill="none"
                            viewBox="0 0 24 24"
                            class="h-4 w-4 stroke-current"
                        >
                            <path
                                stroke-linecap="round"
                                stroke-linejoin="round"
                                stroke-width="2"
                                d="M16.862 4.487l1.687-1.688a1.875 1.875 0 112.652 2.652L10.582 16.07a4.5 4.5 0 01-1.897 1.13L6 18l.8-2.685a4.5 4.5 0 011.13-1.897l8.932-8.931zm0 0L19.5 7.125"
                            ></path>
                        </svg>
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
                        <svg
                            xmlns="http://www.w3.org/2000/svg"
                            fill="none"
                            viewBox="0 0 24 24"
                            class="h-4 w-4 stroke-current"
                        >
                            <path
                                stroke-linecap="round"
                                stroke-linejoin="round"
                                stroke-width="2"
                                d="M14.74 9l-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 01-2.244 2.077H8.084a2.25 2.25 0 01-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 00-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 013.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 00-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 00-7.5 0"
                            ></path>
                        </svg>
                    </button>
                </div>
            </td>
        </tr>
    }
}
