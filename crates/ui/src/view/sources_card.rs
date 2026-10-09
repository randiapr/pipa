//! View: the Sources card — a paginated list of registered data sources (a table on desktop,
//! cards on mobile) with per-source explore (the tables explorer, where the tables to ingest are
//! chosen), connection-test and remove actions, plus a "register a data source" form presented
//! as a [`Modal`] rather than an inline form. Both layouts share the same dialogs; removing asks
//! for confirmation first ([`ConfirmDialog`]).

use leptos::ev::SubmitEvent;
use leptos::html;
use leptos::prelude::*;
use pipa_api::{ConnectionTestOutcome, DataSourceView, DbEngine};

use crate::components::{
    Badge, Card, CardActions, CardTitle, ConfirmDialog, Field, Modal, ModalActions, Select,
    TextInput, Tone, TrashIcon, close_dialog, open_dialog,
};
use crate::view::{Pagination, ResponsiveList};
use crate::viewmodel::SourcesViewModel;

#[component]
pub fn SourcesCard(vm: SourcesViewModel) -> impl IntoView {
    let register_dialog = NodeRef::<html::Dialog>::new();
    let delete_dialog = NodeRef::<html::Dialog>::new();
    let pending_delete = RwSignal::new(Option::<String>::None);

    let on_submit_register = move |ev: SubmitEvent| {
        vm.submit_new(ev);
        close_dialog(register_dialog);
    };

    // Runs on every close, whatever the cause (submit, Cancel, Esc), so the form always
    // starts empty next time it's opened. Leaves `engine` alone, matching
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
        if let Some(id) = pending_delete.get_untracked() {
            vm.delete(id);
        }
    };

    let engines = [DbEngine::Postgres, DbEngine::MySql]
        .map(|engine| {
            (
                engine.as_str().to_string(),
                SourcesViewModel::engine_label(engine).to_string(),
            )
        })
        .to_vec();

    view! {
        <Card attr:id="sources" class="shadow-xl">
            <div class="flex items-center justify-between">
                <CardTitle>"Registered data sources"</CardTitle>
                <button
                    class="btn btn-primary btn-sm"
                    type="button"
                    on:click=move |_| open_dialog(register_dialog)
                >
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
                                            <th>"Ingesting"</th>
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
                <CardActions>
                    <Pagination list=vm.list />
                </CardActions>
            </Show>
        </Card>

        <Modal
            node_ref=register_dialog
            title="Register a data source"
            on_close=on_register_closed
        >
            <form class="mt-4 flex flex-col gap-4" on:submit=on_submit_register>
                <Field legend="Name">
                    <TextInput value=vm.name required=true />
                </Field>
                <Field legend="Engine">
                    <Select
                        value=Signal::derive(move || vm.engine.get().as_str().to_string())
                        options=engines
                        on_change=move |value: String| {
                            vm.engine.set(if value == "mysql" { DbEngine::MySql } else { DbEngine::Postgres })
                        }
                    />
                </Field>
                <Field legend="Host">
                    <TextInput value=vm.host required=true />
                </Field>
                <Field legend="Port">
                    <TextInput
                        value=vm.port
                        required=true
                        placeholder=Signal::derive(move || vm.engine.get().default_port().to_string())
                    />
                </Field>
                <Field legend="Username">
                    <TextInput value=vm.username required=true />
                </Field>
                <Field legend="Password">
                    <TextInput value=vm.password kind="password" />
                </Field>
                <Field legend="Database">
                    <TextInput value=vm.database required=true />
                </Field>
                <ModalActions node_ref=register_dialog>
                    <button class="btn btn-primary" type="submit">
                        "Register"
                    </button>
                </ModalActions>
            </form>
        </Modal>

        <ConfirmDialog
            node_ref=delete_dialog
            title="Remove data source"
            message=move || {
                pending_delete
                    .get()
                    .and_then(|id| vm.name_of(&id))
                    .map(|name| {
                        format!(
                            "Are you sure you want to remove \"{name}\"? It stops being captured. This cannot be undone.",
                        )
                    })
                    .unwrap_or_default()
            }
            confirm_label="Remove"
            danger=true
            on_confirm=on_confirm_delete
        />
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
            <td>{SourcesViewModel::ingested_label(&source)}</td>
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
    // Taken out up front: the components' children closures take what they use by move.
    let name = source.name.clone();
    let engine = SourcesViewModel::engine_label(source.engine);
    let connection = SourcesViewModel::connection_label(&source);
    let ingesting = format!("Ingesting {}", SourcesViewModel::ingested_label(&source));
    let id = source.id;

    view! {
        <Card compact=true>
            <div class="flex items-start justify-between gap-2">
                <CardTitle level=3 class="break-all">
                    {name}
                </CardTitle>
                <Badge tone=Tone::Ghost small=true class="shrink-0">
                    {engine}
                </Badge>
            </div>
            <p class="font-mono text-sm break-all">{connection}</p>
            <p class="text-sm text-base-content/70">
                {move || {
                    vm.project_label(&project_id)
                        .map(|name| format!("Project: {name}"))
                        .unwrap_or_else(|| "No project".to_string())
                }}
            </p>
            <p class="text-sm text-base-content/70">{ingesting}</p>
            <CardActions class="items-center justify-between">
                <TestStatus vm=vm id=id.clone() />
                <SourceActions
                    vm=vm
                    delete_dialog=delete_dialog
                    pending_delete=pending_delete
                    id=id
                />
            </CardActions>
        </Card>
    }
}

/// The badge for a source's last connection test; nothing until it has been tested.
#[component]
fn TestStatus(vm: SourcesViewModel, id: String) -> impl IntoView {
    move || {
        vm.test_result(&id).map(|outcome| match outcome {
            ConnectionTestOutcome::Reachable => view! {
                <Badge tone=Tone::Success small=true>
                    "reachable"
                </Badge>
            }
            .into_any(),
            ConnectionTestOutcome::Unreachable { reason } => view! {
                <Badge tone=Tone::Error small=true attr:title=reason>
                    "unreachable"
                </Badge>
            }
            .into_any(),
        })
    }
}

/// "Explore", "Test connection" and remove buttons for one source.
#[component]
fn SourceActions(
    vm: SourcesViewModel,
    delete_dialog: NodeRef<html::Dialog>,
    pending_delete: RwSignal<Option<String>>,
    id: String,
) -> impl IntoView {
    let id_for_link = id.clone();
    let id_for_test = id.clone();
    let id_for_delete = id;

    let on_test = move |_| vm.test(id_for_test.clone());
    let on_delete = move |_| {
        pending_delete.set(Some(id_for_delete.clone()));
        open_dialog(delete_dialog);
    };

    view! {
        <div class="join">
            <a class="join-item btn btn-sm" href=format!("/sources/{id_for_link}")>
                "Explore"
            </a>
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
