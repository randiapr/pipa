//! View: the Projects card — a paginated list of existing projects (a table on desktop, cards
//! on mobile), with create and edit each presented as a [`Modal`], not inline forms. Both
//! layouts share the same dialogs. Saving an edit and deleting each ask for confirmation first
//! ([`ConfirmDialog`]).

use leptos::ev::SubmitEvent;
use leptos::html;
use leptos::prelude::*;

use crate::components::{
    Card, CardActions, CardTitle, ConfirmDialog, EditIcon, Field, Modal, ModalActions, TextInput,
    TrashIcon, close_dialog, open_dialog,
};
use crate::view::{Pagination, ResponsiveList};
use crate::viewmodel::{ProjectsViewModel, SessionViewModel};

#[component]
pub fn ProjectsCard(vm: ProjectsViewModel) -> impl IntoView {
    // Creating, editing and deleting projects is admin-only (the backend enforces it too).
    let session = expect_context::<SessionViewModel>();
    let create_dialog = NodeRef::<html::Dialog>::new();
    let edit_dialog = NodeRef::<html::Dialog>::new();
    let confirm_edit_dialog = NodeRef::<html::Dialog>::new();
    let delete_dialog = NodeRef::<html::Dialog>::new();
    let pending_delete = RwSignal::new(Option::<String>::None);

    let on_submit_create = move |ev: SubmitEvent| {
        vm.submit_new(ev);
        close_dialog(create_dialog);
    };

    // Runs on every close, whatever the cause (submit, Cancel, Esc), so the form always
    // starts empty next time it's opened.
    let on_create_closed = move |_| {
        vm.name.set(String::new());
        vm.description.set(String::new());
    };

    // Submitting the form only asks; the save happens once that is confirmed.
    let on_submit_edit = move |ev: SubmitEvent| {
        ev.prevent_default();
        open_dialog(confirm_edit_dialog);
    };
    let on_confirm_edit = move |_| {
        if let Some(id) = vm.editing_id.get_untracked() {
            vm.save_edit(id);
        }
        close_dialog(edit_dialog);
    };

    let on_confirm_delete = move |_| {
        if let Some(id) = pending_delete.get_untracked() {
            vm.delete(id);
        }
    };

    view! {
        <Card attr:id="projects" class="shadow-xl">
            <div class="flex items-center justify-between">
                <CardTitle>"Projects"</CardTitle>
                <Show when=move || session.is_admin()>
                    <button
                        class="btn btn-primary btn-sm"
                        type="button"
                        on:click=move |_| open_dialog(create_dialog)
                    >
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
                <CardActions>
                    <Pagination list=vm.list />
                </CardActions>
            </Show>
        </Card>

        <Modal node_ref=create_dialog title="Create project" on_close=on_create_closed>
            <form class="mt-4 flex flex-col gap-4" on:submit=on_submit_create>
                <Field legend="Name">
                    <TextInput value=vm.name required=true />
                </Field>
                <Field legend="Description">
                    <TextInput value=vm.description />
                </Field>
                <ModalActions node_ref=create_dialog>
                    <button class="btn btn-primary" type="submit">
                        "Create"
                    </button>
                </ModalActions>
            </form>
        </Modal>

        // Runs on every close, whatever the cause, so a dismissed edit doesn't leave a project
        // stuck looking "in progress" (`vm.is_editing` stays keyed to a row otherwise).
        <Modal node_ref=edit_dialog title="Edit project" on_close=move |_| vm.cancel_edit()>
            <form class="mt-4 flex flex-col gap-4" on:submit=on_submit_edit>
                <Field legend="Name">
                    <TextInput value=vm.edit_name required=true />
                </Field>
                <Field legend="Description">
                    <TextInput value=vm.edit_description />
                </Field>
                <ModalActions node_ref=edit_dialog>
                    <button class="btn btn-primary" type="submit">
                        "Save"
                    </button>
                </ModalActions>
            </form>
        </Modal>

        <ConfirmDialog
            node_ref=confirm_edit_dialog
            title="Save project"
            message=move || format!("Save the changes to \"{}\"?", vm.edit_name.get().trim())
            confirm_label="Save"
            on_confirm=on_confirm_edit
        />

        <ConfirmDialog
            node_ref=delete_dialog
            title="Delete project"
            message=move || {
                pending_delete
                    .get()
                    .and_then(|id| vm.name_of(&id))
                    .map(|name| {
                        format!("Are you sure you want to delete \"{name}\"? This cannot be undone.")
                    })
                    .unwrap_or_default()
            }
            confirm_label="Delete"
            danger=true
            on_confirm=on_confirm_delete
        />
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
    // Read from inside the `Show` below, which may render it more than once.
    let id = StoredValue::new(id);

    view! {
        <Card compact=true>
            <CardTitle level=3>{move || vm.name_of(&id_for_name).unwrap_or_default()}</CardTitle>
            <p class="text-base-content/70">
                {move || {
                    vm.description_of(&id_for_description)
                        .unwrap_or_else(|| "No description".to_string())
                }}
            </p>
            <Show when=move || session.is_admin()>
                <CardActions>
                    <ProjectActions
                        vm=vm
                        edit_dialog=edit_dialog
                        delete_dialog=delete_dialog
                        pending_delete=pending_delete
                        id=id.get_value()
                    />
                </CardActions>
            </Show>
        </Card>
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
                    open_dialog(edit_dialog);
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
                    open_dialog(delete_dialog);
                }
            >
                <TrashIcon />
            </button>
        </div>
    }
}
