//! View: the landing page — a card gallery giving a quick overview of each project and how
//! many data sources are grouped under it, with a way into the projects and sources pages.

use leptos::prelude::*;

use crate::components::{Badge, Card, CardActions, CardTitle, Tone};
use crate::view::{StatusToast, auto_dismiss};
use crate::viewmodel::{AppViewModel, SessionViewModel};

#[component]
pub fn Landing() -> impl IntoView {
    let vm = AppViewModel::new();
    // Every project's card shows its own data-source count, so load all sources rather than
    // only those of the selected project.
    Effect::new(move |_| {
        vm.projects.refresh();
        vm.sources.refresh_unscoped();
    });
    auto_dismiss(vm.status);

    view! {
        <div class="flex flex-col gap-6">
            <div>
                <h1 class="text-2xl font-bold">"Projects"</h1>
                <p class="text-base-content/70">
                    "An overview of your projects and the data sources grouped under each."
                </p>
            </div>

            <StatusToast status=vm.status />

            <Show
                when=move || !vm.projects.list.items.get().is_empty()
                fallback=|| {
                    view! {
                        <Card class="shadow-xl" body_class="items-center text-center">
                            <CardTitle>"No projects yet"</CardTitle>
                            <p class="text-base-content/70">
                                "Create a project (or ask an admin to assign you one) to start grouping data sources."
                            </p>
                            <CardActions class="">
                                <a class="btn btn-primary" href="/projects">
                                    "Go to projects"
                                </a>
                            </CardActions>
                        </Card>
                    }
                }
            >
                <div class="grid grid-cols-1 gap-6 sm:grid-cols-2 lg:grid-cols-3">
                    <For
                        each=move || vm.projects.list.items.get()
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
    let session = expect_context::<SessionViewModel>();
    let id_for_selected = id.clone();
    let id_for_manage = id.clone();
    let is_selected =
        move || session.current_project_id.get().as_deref() == Some(id_for_selected.as_str());
    let source_count = move || {
        vm.sources
            .list
            .items
            .get()
            .iter()
            .filter(|source| source.project_id.as_deref() == Some(id.as_str()))
            .count()
    };
    let description = description.unwrap_or_else(|| "No description".to_string());

    view! {
        <Card class="shadow-xl" highlighted=Signal::derive(is_selected)>
            <CardTitle>{name}</CardTitle>
            <p class="text-base-content/70">{description}</p>
            <Badge tone=Tone::Neutral>{move || format!("{} data source(s)", source_count())}</Badge>
            <CardActions>
                <a
                    class="btn btn-sm btn-primary"
                    href="/sources"
                    on:click=move |_| session.select_project(Some(id_for_manage.clone()))
                >
                    "Manage"
                </a>
            </CardActions>
        </Card>
    }
}
