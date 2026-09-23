//! View: the Sources card — a paginated table of registered data sources with per-row
//! connection-test and remove actions, plus a "register a data source" form presented as a
//! native `<dialog>` modal (`showModal()`/`close()`) rather than an inline form.

use leptos::ev::SubmitEvent;
use leptos::html;
use leptos::prelude::*;

use crate::model::{ConnectionTestOutcome, DataSourceView, DbEngine};
use crate::view::Pagination;
use crate::viewmodel::SourcesViewModel;

#[component]
pub fn SourcesCard(vm: SourcesViewModel) -> impl IntoView {
    let register_dialog = NodeRef::<html::Dialog>::new();

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

    // Fires on every close, whatever the cause (submit, Cancel, backdrop click, Esc), so the
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
        vm.selected_project_id.set(String::new());
    };

    view! {
        <section id="sources" class="card bg-base-100 shadow-sm">
            <div class="card-body">
                <div class="flex items-center justify-between">
                    <h2 class="card-title">"Registered data sources"</h2>
                    <button class="btn btn-primary btn-sm" type="button" on:click=open_register>
                        "New data source"
                    </button>
                </div>
                <Show
                    when=move || !vm.sources.get().is_empty()
                    fallback=|| {
                        view! { <p class="text-base-content/70">"No data sources registered yet."</p> }
                    }
                >
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
                                    each=move || vm.paged()
                                    key=|source| source.id.clone()
                                    children=move |source| view! { <SourceRow vm=vm source=source /> }
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

        <dialog node_ref=register_dialog class="modal" on:close=on_register_closed>
            <div class="modal-box max-h-[85vh] overflow-y-auto">
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
                        <legend class="fieldset-legend">"Project"</legend>
                        <select
                            class="select w-full"
                            prop:value=move || vm.selected_project_id.get()
                            on:change:target=move |ev| vm.selected_project_id.set(ev.target().value())
                        >
                            <option value="">"(none)"</option>
                            {move || {
                                vm.projects()
                                    .into_iter()
                                    .map(|project| {
                                        view! { <option value=project.id.clone()>{project.name.clone()}</option> }
                                    })
                                    .collect_view()
                            }}
                        </select>
                    </fieldset>
                    <fieldset class="fieldset">
                        <legend class="fieldset-legend">"Engine"</legend>
                        <select
                            class="select w-full"
                            prop:value=move || vm.engine.get().wire_value()
                            on:change:target=move |ev| {
                                let value = ev.target().value();
                                vm.engine.set(if value == "mysql" { DbEngine::Mysql } else { DbEngine::Postgres });
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
            <form method="dialog" class="modal-backdrop">
                <button>"close"</button>
            </form>
        </dialog>
    }
}

/// A single data source row. Unlike a project row, a source has no inline edit, so its own
/// fields (name/engine/connection) are safe to read once from `source` — only its project's
/// *name* can change out from under it, which is why that one field is still looked up
/// reactively via `vm.project_label`.
#[component]
fn SourceRow(vm: SourcesViewModel, source: DataSourceView) -> impl IntoView {
    let id_for_test = source.id.clone();
    let id_for_delete = source.id.clone();
    let project_id = source.project_id.clone();
    let test_result = RwSignal::new(Option::<ConnectionTestOutcome>::None);

    let on_test = move |_| vm.test(id_for_test.clone(), test_result);
    let on_delete = move |_| vm.delete(id_for_delete.clone());

    view! {
        <tr>
            <td>
                <strong>{source.name.clone()}</strong>
            </td>
            <td>{source.engine.label()}</td>
            <td>
                {format!(
                    "{}:{}/{}",
                    source.connection.host,
                    source.connection.port,
                    source.connection.database,
                )}
            </td>
            <td>{move || vm.project_label(&project_id).unwrap_or_else(|| "\u{2014}".to_string())}</td>
            <td>
                {move || {
                    test_result
                        .get()
                        .map(|outcome| match outcome {
                            ConnectionTestOutcome::Reachable => {
                                view! { <span class="badge badge-success badge-sm">"reachable"</span> }
                                    .into_any()
                            }
                            ConnectionTestOutcome::Unreachable { reason } => {
                                view! {
                                    <span class="badge badge-error badge-sm" title=reason>
                                        "unreachable"
                                    </span>
                                }
                                    .into_any()
                            }
                        })
                }}
            </td>
            <td class="text-right">
                <div class="join">
                    <button class="join-item btn btn-sm" on:click=on_test type="button">
                        "Test connection"
                    </button>
                    <button class="join-item btn btn-sm btn-error btn-soft" on:click=on_delete type="button">
                        "Remove"
                    </button>
                </div>
            </td>
        </tr>
    }
}
