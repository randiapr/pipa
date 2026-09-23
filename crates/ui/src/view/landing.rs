//! View: the landing page — a card gallery giving a quick overview of each project and how
//! many data sources are grouped under it, with a way into the full dashboard.

use leptos::prelude::*;

use crate::viewmodel::AppViewModel;

#[component]
pub fn Landing() -> impl IntoView {
    let vm = AppViewModel::new();
    Effect::new(move |_| vm.refresh_all());

    view! {
        <div class="flex flex-col gap-6">
            <div>
                <h1 class="text-2xl font-bold">"Projects"</h1>
                <p class="text-base-content/70">
                    "An overview of your projects and the data sources grouped under each."
                </p>
            </div>

            {move || {
                vm.status
                    .get()
                    .map(|msg| {
                        view! {
                            <div role="alert" class="alert">
                                <span>{msg}</span>
                            </div>
                        }
                    })
            }}

            <Show
                when=move || !vm.projects.projects.get().is_empty()
                fallback=|| {
                    view! {
                        <div class="card bg-base-100 shadow-sm">
                            <div class="card-body items-center text-center">
                                <h2 class="card-title">"No projects yet"</h2>
                                <p class="text-base-content/70">
                                    "Create a project on the dashboard to start grouping data sources."
                                </p>
                                <div class="card-actions">
                                    <a class="btn btn-primary" href="/dashboard#projects">
                                        "Go to dashboard"
                                    </a>
                                </div>
                            </div>
                        </div>
                    }
                }
            >
                <div class="grid grid-cols-1 gap-6 sm:grid-cols-2 lg:grid-cols-3">
                    <For
                        each=move || vm.projects.projects.get()
                        key=|project| project.id.clone()
                        children=move |project| view! { <ProjectCard vm=vm id=project.id name=project.name description=project.description /> }
                    />
                </div>
            </Show>
        </div>
    }
}

#[component]
fn ProjectCard(
    vm: AppViewModel,
    id: String,
    name: String,
    description: Option<String>,
) -> impl IntoView {
    let source_count = move || {
        vm.sources
            .sources
            .get()
            .iter()
            .filter(|source| source.project_id.as_deref() == Some(id.as_str()))
            .count()
    };

    view! {
        <div class="card bg-base-100 shadow-sm">
            <div class="card-body">
                <h2 class="card-title">{name}</h2>
                <p class="text-base-content/70">
                    {description.unwrap_or_else(|| "No description".to_string())}
                </p>
                <div class="badge badge-neutral">{move || format!("{} data source(s)", source_count())}</div>
                <div class="card-actions justify-end">
                    <a class="btn btn-sm btn-primary" href="/dashboard#sources">
                        "Manage"
                    </a>
                </div>
            </div>
        </div>
    }
}
