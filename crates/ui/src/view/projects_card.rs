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

    // Fires on every close, whatever the cause (submit, Cancel, backdrop click, Esc), so the
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

    view! {
        <section id="projects" class="card bg-base-100 shadow-sm">
            <div class="card-body">
                <div class="flex items-center justify-between">
                    <h2 class="card-title">"Projects"</h2>
                    <button class="btn btn-primary btn-sm" type="button" on:click=open_create>
                        "New project"
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
                                        view! { <ProjectRow vm=vm edit_dialog=edit_dialog id=project.id /> }
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
            <form method="dialog" class="modal-backdrop">
                <button>"close"</button>
            </form>
        </dialog>

        <dialog node_ref=edit_dialog class="modal" on:close=on_edit_closed>
            <div class="modal-box max-h-[85vh] overflow-y-auto">
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
            <form method="dialog" class="modal-backdrop">
                <button>"close"</button>
            </form>
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
                        class="join-item btn btn-sm"
                        type="button"
                        on:click=move |_| {
                            vm.start_edit(id_for_edit.clone());
                            if let Some(dialog) = edit_dialog.get() {
                                let _ = dialog.show_modal();
                            }
                        }
                    >
                        "Edit"
                    </button>
                    <button
                        class="join-item btn btn-sm btn-error btn-soft"
                        type="button"
                        on:click=move |_| vm.delete(id_for_delete.clone())
                    >
                        "Delete"
                    </button>
                </div>
            </td>
        </tr>
    }
}
