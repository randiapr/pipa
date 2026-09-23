//! View layer: Leptos components that render from a ViewModel.
//!
//! Components here read `RwSignal`s straight off a `crate::viewmodel` struct and call its
//! methods from event handlers. They hold no business logic of their own and never call
//! `crate::api` directly — that boundary belongs to the ViewModel.

mod landing;
mod pagination;
mod projects_card;
mod sources_card;

use leptos::prelude::*;
use leptos_router::{
    components::{Route, Router, Routes},
    path,
};

use landing::Landing;
pub use pagination::Pagination;
use projects_card::ProjectsCard;
use sources_card::SourcesCard;

use crate::viewmodel::AppViewModel;

/// Nav destinations shared between the desktop navbar menu and the mobile sidebar drawer.
/// Entries pointing into the dashboard carry the route (`/dashboard`) rather than a bare
/// `#anchor`, since the landing page (`/`) is now a separate route.
const NAV_LINKS: &[(&str, &str)] = &[
    ("/", "Home"),
    ("/dashboard#projects", "Projects"),
    ("/dashboard#sources", "Sources"),
];

#[component]
pub fn App() -> impl IntoView {
    // daisyUI's drawer only closes when the checkbox is unchecked — clicking the hamburger
    // or the overlay does that natively via their `<label for="app-drawer">`, but tapping a
    // sidebar nav link wouldn't, since it's a separate element. Track the checkbox state
    // explicitly so nav-link clicks can close the drawer too.
    let drawer_open = RwSignal::new(false);

    view! {
        <Router>
            <div class="drawer">
                <input
                    id="app-drawer"
                    type="checkbox"
                    class="drawer-toggle lg:hidden"
                    prop:checked=move || drawer_open.get()
                    on:change:target=move |ev| drawer_open.set(ev.target().checked())
                />
                <div class="drawer-content flex flex-col">
                    <div class="navbar bg-base-300 w-full">
                        <div class="flex-none lg:hidden">
                            <label
                                for="app-drawer"
                                aria-label="open sidebar"
                                class="btn btn-square btn-ghost drawer-button"
                            >
                                <svg
                                    xmlns="http://www.w3.org/2000/svg"
                                    fill="none"
                                    viewBox="0 0 24 24"
                                    class="inline-block h-6 w-6 stroke-current"
                                >
                                    <path
                                        stroke-linecap="round"
                                        stroke-linejoin="round"
                                        stroke-width="2"
                                        d="M4 6h16M4 12h16M4 18h16"
                                    ></path>
                                </svg>
                            </label>
                        </div>
                        <div class="mx-2 flex-1 px-2 text-xl font-bold">"pipa"</div>
                        <div class="hidden flex-none lg:block">
                            <ul class="menu menu-horizontal">
                                {NAV_LINKS
                                    .iter()
                                    .map(|(href, label)| view! { <li><a href=*href>{*label}</a></li> })
                                    .collect_view()}
                            </ul>
                        </div>
                    </div>
                    <main class="container mx-auto p-6">
                        <Routes fallback=|| "not found">
                            <Route path=path!("/") view=Landing />
                            <Route path=path!("/dashboard") view=Dashboard />
                        </Routes>
                    </main>
                </div>
                <div class="drawer-side">
                    <label for="app-drawer" aria-label="close sidebar" class="drawer-overlay"></label>
                    <ul class="menu bg-base-200 min-h-full w-80 p-4">
                        {NAV_LINKS
                            .iter()
                            .map(|(href, label)| {
                                view! {
                                    <li>
                                        <a href=*href on:click=move |_| drawer_open.set(false)>
                                            {*label}
                                        </a>
                                    </li>
                                }
                            })
                            .collect_view()}
                    </ul>
                </div>
            </div>
        </Router>
    }
}

#[component]
fn Dashboard() -> impl IntoView {
    let vm = AppViewModel::new();
    Effect::new(move |_| vm.refresh_all());

    view! {
        <div class="flex flex-col gap-6">
            <p class="text-base-content/70">
                "Connect and manage OLTP database sources for CDC capture."
            </p>

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

            <ProjectsCard vm=vm.projects />
            <SourcesCard vm=vm.sources />
        </div>
    }
}
