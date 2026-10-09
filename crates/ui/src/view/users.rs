//! View: the Users page (admin only) — a paginated list of accounts (a table on desktop, cards
//! on mobile), with create and edit each presented as a [`Modal`], like the Projects card.
//! Saving an edit and deleting each ask for confirmation first ([`ConfirmDialog`]).

use leptos::ev::SubmitEvent;
use leptos::html;
use leptos::prelude::*;
use pipa_api::Role;

use crate::components::{
    Badge, Card, CardActions, CardTitle, Checkbox, ConfirmDialog, EditIcon, Field, Modal,
    ModalActions, Select, TextInput, Tone, TrashIcon, close_dialog, open_dialog,
};
use crate::view::{Pagination, ResponsiveList, StatusToast, auto_dismiss};
use crate::viewmodel::{SessionViewModel, StatusMessage, UsersViewModel, parse_role, role_value};

#[component]
pub fn Users() -> impl IntoView {
    let session = expect_context::<SessionViewModel>();
    let status = RwSignal::new(None::<StatusMessage>);
    let vm = UsersViewModel::new(session, status);
    Effect::new(move |_| vm.refresh());
    // A brand-new admin may not have loaded the project list yet.
    Effect::new(move |_| session.refresh_projects());
    auto_dismiss(status);

    let create_dialog = NodeRef::<html::Dialog>::new();
    let edit_dialog = NodeRef::<html::Dialog>::new();
    let confirm_edit_dialog = NodeRef::<html::Dialog>::new();
    let delete_dialog = NodeRef::<html::Dialog>::new();
    let pending_delete = RwSignal::new(Option::<String>::None);

    let on_submit_create = move |ev: SubmitEvent| {
        vm.submit_new(ev);
        close_dialog(create_dialog);
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
        <div class="flex flex-col gap-6">
            <div class="flex items-center justify-between">
                <div>
                    <h1 class="text-2xl font-bold">"Users"</h1>
                    <p class="text-base-content/70">
                        "Manage accounts, their roles and which projects each can access."
                    </p>
                </div>
                <button
                    class="btn btn-primary btn-sm"
                    type="button"
                    on:click=move |_| open_dialog(create_dialog)
                >
                    "New User"
                </button>
            </div>

            <StatusToast status=status />

            <Show
                when=move || !vm.list.is_empty()
                fallback=|| view! { <p class="text-base-content/70">"No users yet."</p> }
            >
                <ResponsiveList
                    table=move || {
                        view! {
                            <div class="overflow-x-auto">
                                <table class="table">
                                    <thead>
                                        <tr>
                                            <th>"Username"</th>
                                            <th>"Role"</th>
                                            <th>"Projects"</th>
                                            <th class="text-right">"Actions"</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        <For
                                            each=move || vm.list.paged()
                                            key=|user| user.id.clone()
                                            children=move |user| {
                                                view! {
                                                    <UserRow
                                                        vm=vm
                                                        edit_dialog=edit_dialog
                                                        delete_dialog=delete_dialog
                                                        pending_delete=pending_delete
                                                        id=user.id
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
                                key=|user| user.id.clone()
                                children=move |user| {
                                    view! {
                                        <UserItemCard
                                            vm=vm
                                            edit_dialog=edit_dialog
                                            delete_dialog=delete_dialog
                                            pending_delete=pending_delete
                                            id=user.id
                                        />
                                    }
                                }
                            />
                        }
                    }
                />
                <div class="flex justify-end">
                    <Pagination list=vm.list />
                </div>
            </Show>
        </div>

        <Modal node_ref=create_dialog title="Create user" on_close=move |_| vm.reset_new()>
            <form class="mt-4 flex flex-col gap-4" on:submit=on_submit_create>
                <Field legend="Username">
                    <TextInput value=vm.username required=true autocomplete="off" />
                </Field>
                <Field legend="Password" hint="At least 8 characters.">
                    <TextInput
                        value=vm.password
                        kind="password"
                        required=true
                        minlength=8
                        autocomplete="new-password"
                    />
                </Field>
                <Field legend="Role">
                    <RoleSelect role=vm.role />
                </Field>
                <ProjectChecklist
                    selected=vm.project_ids
                    is_admin=Signal::derive(move || vm.role.get() == Role::Admin)
                    on_toggle=Callback::new(move |(id, on)| vm.toggle_new_project(id, on))
                />
                <ModalActions node_ref=create_dialog>
                    <button class="btn btn-primary" type="submit">
                        "Create"
                    </button>
                </ModalActions>
            </form>
        </Modal>

        <Modal
            node_ref=edit_dialog
            title=move || {
                vm.editing_id
                    .get()
                    .and_then(|id| vm.find(&id))
                    .map(|user| format!("Edit {}", user.username))
                    .unwrap_or_else(|| "Edit user".to_string())
            }
            on_close=move |_| vm.cancel_edit()
        >
            <form class="mt-4 flex flex-col gap-4" on:submit=on_submit_edit>
                <Field legend="New password">
                    <TextInput
                        value=vm.edit_password
                        kind="password"
                        minlength=8
                        autocomplete="new-password"
                        placeholder="Leave empty to keep the current one"
                    />
                </Field>
                <Field legend="Role">
                    <RoleSelect role=vm.edit_role />
                </Field>
                <ProjectChecklist
                    selected=vm.edit_project_ids
                    is_admin=Signal::derive(move || vm.edit_role.get() == Role::Admin)
                    on_toggle=Callback::new(move |(id, on)| vm.toggle_edit_project(id, on))
                />
                <ModalActions node_ref=edit_dialog>
                    <button class="btn btn-primary" type="submit">
                        "Save"
                    </button>
                </ModalActions>
            </form>
        </Modal>

        <ConfirmDialog
            node_ref=confirm_edit_dialog
            title="Save user"
            message=move || {
                let name = vm
                    .editing_id
                    .get()
                    .and_then(|id| vm.find(&id))
                    .map(|user| user.username)
                    .unwrap_or_default();
                let password = if vm.edit_password.get().is_empty() {
                    ""
                } else {
                    " This also sets a new password."
                };
                format!(
                    "Save the changes to \"{name}\"? Their role and projects apply from their next request.{password}",
                )
            }
            confirm_label="Save"
            on_confirm=on_confirm_edit
        />

        <ConfirmDialog
            node_ref=delete_dialog
            title="Delete user"
            message=move || {
                pending_delete
                    .get()
                    .and_then(|id| vm.find(&id))
                    .map(|user| {
                        format!(
                            "Are you sure you want to delete \"{}\"? This cannot be undone.",
                            user.username,
                        )
                    })
                    .unwrap_or_default()
            }
            confirm_label="Delete"
            danger=true
            on_confirm=on_confirm_delete
        />
    }
}

/// The role picker shared by the create and edit forms.
#[component]
fn RoleSelect(role: RwSignal<Role>) -> impl IntoView {
    let options = [
        (Role::User, "User (view tables only)"),
        (Role::Developer, "Developer"),
        (Role::Admin, "Admin"),
    ]
    .map(|(role, label)| (role_value(role).to_string(), label.to_string()))
    .to_vec();

    view! {
        <Select
            value=Signal::derive(move || role_value(role.get()).to_string())
            options=options
            on_change=move |value: String| role.set(parse_role(&value))
        />
    }
}

/// Checkboxes for the projects an account may access. Meaningless for admins, who see every
/// project, so it is replaced by a note while the `Admin` role is selected.
#[component]
fn ProjectChecklist(
    selected: RwSignal<Vec<String>>,
    #[prop(into)] is_admin: Signal<bool>,
    on_toggle: Callback<(String, bool)>,
) -> impl IntoView {
    let session = expect_context::<SessionViewModel>();

    view! {
        <Field legend="Projects">
            <Show
                when=move || !is_admin.get()
                fallback=|| {
                    view! { <p class="text-base-content/70">"Admins can access every project."</p> }
                }
            >
                <Show
                    when=move || !session.projects.get().is_empty()
                    fallback=|| view! { <p class="text-base-content/70">"No projects exist yet."</p> }
                >
                    <div class="flex max-h-48 flex-col gap-2 overflow-y-auto">
                        <For
                            each=move || session.projects.get()
                            key=|project| (project.id.clone(), project.name.clone())
                            children=move |project| {
                                let id = project.id.clone();
                                let id_for_checked = project.id.clone();
                                view! {
                                    <label class="label cursor-pointer justify-start gap-3">
                                        <Checkbox
                                            small=true
                                            checked=Signal::derive(move || {
                                                selected.with(|ids| ids.contains(&id_for_checked))
                                            })
                                            on_change=move |on| on_toggle.run((id.clone(), on))
                                        />
                                        <span>{project.name}</span>
                                    </label>
                                }
                            }
                        />
                    </div>
                </Show>
            </Show>
        </Field>
    }
}

/// A single user row. Fields are looked up from `vm.list` by `id` on every render, so an edit
/// is reflected in place (see `ProjectRow` for why).
#[component]
fn UserRow(
    vm: UsersViewModel,
    edit_dialog: NodeRef<html::Dialog>,
    delete_dialog: NodeRef<html::Dialog>,
    pending_delete: RwSignal<Option<String>>,
    id: String,
) -> impl IntoView {
    let id_for_name = id.clone();
    let id_for_role = id.clone();
    let id_for_projects = id.clone();

    view! {
        <tr>
            <td>
                <UserName vm=vm id=id_for_name />
            </td>
            <td>
                <RoleBadge vm=vm id=id_for_role />
            </td>
            <td>{move || vm.find(&id_for_projects).map(|user| vm.projects_label(&user)).unwrap_or_default()}</td>
            <td class="text-right">
                <UserActions
                    vm=vm
                    edit_dialog=edit_dialog
                    delete_dialog=delete_dialog
                    pending_delete=pending_delete
                    id=id
                />
            </td>
        </tr>
    }
}

/// The mobile counterpart of [`UserRow`], looked up by `id` the same way.
#[component]
fn UserItemCard(
    vm: UsersViewModel,
    edit_dialog: NodeRef<html::Dialog>,
    delete_dialog: NodeRef<html::Dialog>,
    pending_delete: RwSignal<Option<String>>,
    id: String,
) -> impl IntoView {
    let id_for_name = id.clone();
    let id_for_role = id.clone();
    let id_for_projects = id.clone();

    view! {
        <Card compact=true>
            <div class="flex items-start justify-between gap-2">
                <CardTitle level=3 class="break-all">
                    <UserName vm=vm id=id_for_name />
                </CardTitle>
                <RoleBadge vm=vm id=id_for_role />
            </div>
            <p class="text-sm text-base-content/70">
                {move || {
                    vm.find(&id_for_projects)
                        .map(|user| format!("Projects: {}", vm.projects_label(&user)))
                        .unwrap_or_default()
                }}
            </p>
            <CardActions>
                <UserActions
                    vm=vm
                    edit_dialog=edit_dialog
                    delete_dialog=delete_dialog
                    pending_delete=pending_delete
                    id=id
                />
            </CardActions>
        </Card>
    }
}

/// A user's name, marked "you" for the signed-in account.
#[component]
fn UserName(vm: UsersViewModel, id: String) -> impl IntoView {
    let id_for_self = id.clone();

    view! {
        {move || vm.find(&id).map(|user| user.username).unwrap_or_default()}
        <Show when=move || vm.is_self(&id_for_self)>
            <Badge tone=Tone::Ghost small=true class="ml-2">
                "you"
            </Badge>
        </Show>
    }
}

#[component]
fn RoleBadge(vm: UsersViewModel, id: String) -> impl IntoView {
    move || {
        vm.find(&id).map(|user| {
            view! {
                <Badge tone=role_tone(user.role) small=true>
                    {role_value(user.role)}
                </Badge>
            }
        })
    }
}

/// Each role's badge color.
fn role_tone(role: Role) -> Tone {
    match role {
        Role::Admin => Tone::Primary,
        Role::Developer => Tone::Secondary,
        Role::User => Tone::Neutral,
    }
}

/// Edit/delete buttons for one user. Deleting yourself is disabled.
#[component]
fn UserActions(
    vm: UsersViewModel,
    edit_dialog: NodeRef<html::Dialog>,
    delete_dialog: NodeRef<html::Dialog>,
    pending_delete: RwSignal<Option<String>>,
    id: String,
) -> impl IntoView {
    let id_for_edit = id.clone();
    let id_for_delete = id.clone();
    let id_for_delete_disabled = id;

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
                disabled=move || vm.is_self(&id_for_delete_disabled)
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
