//! View: the Users page (admin only) — a paginated table of accounts, with create and edit each
//! presented as a native `<dialog>` modal, like the Projects card.

use leptos::ev::SubmitEvent;
use leptos::html;
use leptos::prelude::*;
use pipa_api::Role;

use crate::view::{Pagination, STATUS_TOAST_DURATION};
use crate::viewmodel::{SessionViewModel, StatusMessage, UsersViewModel, parse_role, role_value};

#[component]
pub fn Users() -> impl IntoView {
    let session = expect_context::<SessionViewModel>();
    let status = RwSignal::new(None::<StatusMessage>);
    let vm = UsersViewModel::new(session, status);
    Effect::new(move |_| vm.refresh());
    // A brand-new admin may not have loaded the project list yet.
    Effect::new(move |_| session.refresh_projects());

    // Auto-dismiss the status toast so it doesn't linger on screen forever.
    Effect::new(move |_| {
        if status.get().is_some() {
            set_timeout(move || status.set(None), STATUS_TOAST_DURATION);
        }
    });

    let create_dialog = NodeRef::<html::Dialog>::new();
    let edit_dialog = NodeRef::<html::Dialog>::new();
    let delete_dialog = NodeRef::<html::Dialog>::new();
    let pending_delete = RwSignal::new(Option::<String>::None);

    let close = move |dialog: NodeRef<html::Dialog>| {
        if let Some(dialog) = dialog.get() {
            dialog.close();
        }
    };

    let on_submit_create = move |ev: SubmitEvent| {
        vm.submit_new(ev);
        close(create_dialog);
    };
    let on_submit_edit = move |ev: SubmitEvent| {
        ev.prevent_default();
        if let Some(id) = vm.editing_id.get() {
            vm.save_edit(id);
        }
        close(edit_dialog);
    };
    let on_confirm_delete = move |_| {
        if let Some(id) = pending_delete.get() {
            vm.delete(id);
        }
        close(delete_dialog);
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
                    on:click=move |_| {
                        if let Some(dialog) = create_dialog.get() {
                            let _ = dialog.show_modal();
                        }
                    }
                >
                    "New User"
                </button>
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
                when=move || !vm.users.get().is_empty()
                fallback=|| view! { <p class="text-base-content/70">"No users yet."</p> }
            >
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
                                each=move || vm.paged()
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
                <div class="flex justify-end">
                    <Pagination page=vm.page total=vm.total() />
                </div>
            </Show>
        </div>

        <dialog node_ref=create_dialog class="modal" on:close=move |_| vm.reset_new()>
            <div class="modal-box max-h-[85vh] overflow-y-auto">
                <button
                    class="btn btn-sm btn-circle btn-ghost absolute right-2 top-2"
                    type="button"
                    on:click=move |_| close(create_dialog)
                >
                    "✕"
                </button>
                <h3 class="text-lg font-bold">"Create user"</h3>
                <form class="mt-4 flex flex-col gap-4" on:submit=on_submit_create>
                    <fieldset class="fieldset">
                        <legend class="fieldset-legend">"Username"</legend>
                        <input
                            type="text"
                            class="input w-full"
                            required
                            autocomplete="off"
                            prop:value=move || vm.username.get()
                            on:input:target=move |ev| vm.username.set(ev.target().value())
                        />
                    </fieldset>
                    <fieldset class="fieldset">
                        <legend class="fieldset-legend">"Password"</legend>
                        <input
                            type="password"
                            class="input w-full"
                            required
                            minlength="8"
                            autocomplete="new-password"
                            prop:value=move || vm.password.get()
                            on:input:target=move |ev| vm.password.set(ev.target().value())
                        />
                        <p class="label">"At least 8 characters."</p>
                    </fieldset>
                    <fieldset class="fieldset">
                        <legend class="fieldset-legend">"Role"</legend>
                        <select
                            class="select w-full"
                            prop:value=move || role_value(vm.role.get())
                            on:change:target=move |ev| vm.role.set(parse_role(&ev.target().value()))
                        >
                            <option value="user">"User (view tables only)"</option>
                            <option value="developer">"Developer"</option>
                            <option value="admin">"Admin"</option>
                        </select>
                    </fieldset>
                    <ProjectChecklist
                        selected=vm.project_ids
                        is_admin=Signal::derive(move || vm.role.get() == Role::Admin)
                        on_toggle=Callback::new(move |(id, on)| vm.toggle_new_project(id, on))
                    />
                    <div class="modal-action">
                        <button class="btn" type="button" on:click=move |_| close(create_dialog)>
                            "Cancel"
                        </button>
                        <button class="btn btn-primary" type="submit">
                            "Create"
                        </button>
                    </div>
                </form>
            </div>
        </dialog>

        <dialog node_ref=edit_dialog class="modal" on:close=move |_| vm.cancel_edit()>
            <div class="modal-box max-h-[85vh] overflow-y-auto">
                <button
                    class="btn btn-sm btn-circle btn-ghost absolute right-2 top-2"
                    type="button"
                    on:click=move |_| close(edit_dialog)
                >
                    "✕"
                </button>
                <h3 class="text-lg font-bold">
                    {move || {
                        vm.editing_id
                            .get()
                            .and_then(|id| vm.find(&id))
                            .map(|user| format!("Edit {}", user.username))
                            .unwrap_or_else(|| "Edit user".to_string())
                    }}
                </h3>
                <form class="mt-4 flex flex-col gap-4" on:submit=on_submit_edit>
                    <fieldset class="fieldset">
                        <legend class="fieldset-legend">"New password"</legend>
                        <input
                            type="password"
                            class="input w-full"
                            minlength="8"
                            autocomplete="new-password"
                            placeholder="Leave empty to keep the current one"
                            prop:value=move || vm.edit_password.get()
                            on:input:target=move |ev| vm.edit_password.set(ev.target().value())
                        />
                    </fieldset>
                    <fieldset class="fieldset">
                        <legend class="fieldset-legend">"Role"</legend>
                        <select
                            class="select w-full"
                            prop:value=move || role_value(vm.edit_role.get())
                            on:change:target=move |ev| {
                                vm.edit_role.set(parse_role(&ev.target().value()))
                            }
                        >
                            <option value="user">"User (view tables only)"</option>
                            <option value="developer">"Developer"</option>
                            <option value="admin">"Admin"</option>
                        </select>
                    </fieldset>
                    <ProjectChecklist
                        selected=vm.edit_project_ids
                        is_admin=Signal::derive(move || vm.edit_role.get() == Role::Admin)
                        on_toggle=Callback::new(move |(id, on)| vm.toggle_edit_project(id, on))
                    />
                    <div class="modal-action">
                        <button class="btn" type="button" on:click=move |_| close(edit_dialog)>
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
                    on:click=move |_| close(delete_dialog)
                >
                    "✕"
                </button>
                <h3 class="text-lg font-bold">"Delete user"</h3>
                <p class="py-4">
                    {move || {
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
                    }}
                </p>
                <div class="modal-action">
                    <button class="btn" type="button" on:click=move |_| close(delete_dialog)>
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
        <fieldset class="fieldset">
            <legend class="fieldset-legend">"Projects"</legend>
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
                                        <input
                                            type="checkbox"
                                            class="checkbox checkbox-sm"
                                            prop:checked=move || {
                                                selected.with(|ids| ids.contains(&id_for_checked))
                                            }
                                            on:change:target=move |ev| {
                                                on_toggle.run((id.clone(), ev.target().checked()))
                                            }
                                        />
                                        <span>{project.name}</span>
                                    </label>
                                }
                            }
                        />
                    </div>
                </Show>
            </Show>
        </fieldset>
    }
}

/// A single user row. Fields are looked up from `vm.users` by `id` on every render, so an edit
/// is reflected in place (see `ProjectRow` for why).
#[component]
fn UserRow(
    vm: UsersViewModel,
    edit_dialog: NodeRef<html::Dialog>,
    delete_dialog: NodeRef<html::Dialog>,
    pending_delete: RwSignal<Option<String>>,
    id: String,
) -> impl IntoView {
    let id_for_edit = id.clone();
    let id_for_delete = id.clone();
    let id_for_delete_disabled = id.clone();
    let id_for_self = id.clone();
    let id_for_name = id.clone();
    let id_for_role = id.clone();
    let id_for_projects = id;

    view! {
        <tr>
            <td>
                {move || vm.find(&id_for_name).map(|user| user.username).unwrap_or_default()}
                <Show when=move || vm.is_self(&id_for_self)>
                    <span class="badge badge-ghost badge-sm ml-2">"you"</span>
                </Show>
            </td>
            <td>
                {move || {
                    match vm.find(&id_for_role).map(|user| user.role) {
                        Some(Role::Admin) => {
                            view! { <span class="badge badge-primary badge-sm">"admin"</span> }.into_any()
                        }
                        Some(Role::Developer) => {
                            view! { <span class="badge badge-secondary badge-sm">"developer"</span> }.into_any()
                        }
                        Some(Role::User) => {
                            view! { <span class="badge badge-neutral badge-sm">"user"</span> }.into_any()
                        }
                        None => ().into_any(),
                    }
                }}
            </td>
            <td>
                {move || {
                    let Some(user) = vm.find(&id_for_projects) else {
                        return String::new();
                    };
                    if user.role == Role::Admin {
                        return "All projects".to_string();
                    }
                    let names = vm.project_names(&user);
                    if names.is_empty() { "\u{2014}".to_string() } else { names.join(", ") }
                }}
            </td>
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
                        disabled=move || vm.is_self(&id_for_delete_disabled)
                        on:click={
                            let id = id_for_delete.clone();
                            move |_| {
                                pending_delete.set(Some(id.clone()));
                                if let Some(dialog) = delete_dialog.get() {
                                    let _ = dialog.show_modal();
                                }
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
