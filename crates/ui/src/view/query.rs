//! View: the SQL query page. Queries only see the selected project's tables.

use leptos::prelude::*;

use crate::components::{Alert, TextArea, Tone};
use crate::view::{Pagination, RowsView};
use crate::viewmodel::{QueryViewModel, SessionViewModel};

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
                <TextArea
                    value=vm.sql
                    required=true
                    placeholder="SELECT …"
                    class="h-32 w-full font-mono"
                />
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
                    .map(|message| view! { <Alert tone=Tone::Error>{message}</Alert> })
            }}

            <Show when=move || vm.ran.get()>
                <div class="flex flex-col gap-2">
                    <p class="text-sm text-base-content/70">
                        {move || format!("{} row(s)", vm.results.len())}
                    </p>
                    {move || view! { <RowsView rows=vm.results.paged() /> }}
                    <Show when=move || !vm.results.is_empty()>
                        <Pagination list=vm.results />
                    </Show>
                </div>
            </Show>
        </div>
    }
}
