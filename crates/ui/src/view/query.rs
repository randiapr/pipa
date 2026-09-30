//! View: the SQL query page. Queries only see the selected project's tables.

use leptos::prelude::*;

use crate::viewmodel::{QueryViewModel, SessionViewModel, cell_text};

#[component]
pub fn Query() -> impl IntoView {
    let session = expect_context::<SessionViewModel>();
    let vm = QueryViewModel::new(session);

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
                <textarea
                    class="textarea h-32 w-full font-mono"
                    required
                    placeholder="SELECT …"
                    prop:value=move || vm.sql.get()
                    on:input:target=move |ev| vm.sql.set(ev.target().value())
                ></textarea>
                <div>
                    <button
                        class="btn btn-primary"
                        type="submit"
                        disabled=move || vm.running.get()
                    >
                        {move || if vm.running.get() { "Running…" } else { "Run" }}
                    </button>
                </div>
            </form>

            {move || {
                vm.error
                    .get()
                    .map(|message| {
                        view! {
                            <div role="alert" class="alert alert-error">
                                <span>{message}</span>
                            </div>
                        }
                    })
            }}

            {move || {
                vm.rows
                    .get()
                    .map(|rows| {
                        let columns = vm.columns();
                        if rows.is_empty() {
                            return view! { <p class="text-base-content/70">"No rows."</p> }.into_any();
                        }
                        let count = rows.len();
                        view! {
                            <div class="flex flex-col gap-2">
                                <p class="text-sm text-base-content/70">{format!("{count} row(s)")}</p>
                                <div class="overflow-x-auto">
                                    <table class="table table-zebra table-sm">
                                        <thead>
                                            <tr>
                                                {columns
                                                    .iter()
                                                    .map(|column| view! { <th>{column.clone()}</th> })
                                                    .collect_view()}
                                            </tr>
                                        </thead>
                                        <tbody>
                                            {rows
                                                .iter()
                                                .map(|row| {
                                                    view! {
                                                        <tr>
                                                            {columns
                                                                .iter()
                                                                .map(|column| {
                                                                    view! { <td>{cell_text(row.get(column))}</td> }
                                                                })
                                                                .collect_view()}
                                                        </tr>
                                                    }
                                                })
                                                .collect_view()}
                                        </tbody>
                                    </table>
                                </div>
                            </div>
                        }
                            .into_any()
                    })
            }}
        </div>
    }
}
