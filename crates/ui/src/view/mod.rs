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
/// `#anchor`, since the landing page (`/`) is now a separate route. The "Home" entry that used
/// to lead this list has been replaced by [`ThemeToggle`] (the brand link covers going home).
const NAV_LINKS: &[(&str, &str)] = &[
    ("/dashboard#projects", "Projects"),
    ("/dashboard#sources", "Sources"),
];

/// The two daisyUI themes registered in `tailwind.css` (`themes: light --default, dark
/// --prefersdark`).
#[derive(Copy, Clone, PartialEq, Eq)]
enum Theme {
    Light,
    Dark,
}

impl Theme {
    fn as_str(self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }
}

/// Light/dark theme switch, rendered where the "Home" nav entry used to sit. Drives the
/// `data-theme` attribute on `<html>` from a signal rather than daisyUI's CSS-only
/// `theme-controller`, since that mechanism only sets `data-theme` when checked — unchecked
/// falls back to the `prefers-color-scheme` media query rather than forcing "light", so a
/// user on a dark system could never explicitly pick light. An explicit signal always wins.
#[component]
fn ThemeToggle(theme: RwSignal<Theme>) -> impl IntoView {
    view! {
        <label class="swap swap-rotate btn btn-ghost btn-circle" aria-label="Toggle theme">
            <input
                type="checkbox"
                prop:checked=move || theme.get() == Theme::Dark
                on:change:target=move |ev| {
                    theme.set(if ev.target().checked() { Theme::Dark } else { Theme::Light })
                }
            />
            <svg
                class="swap-off h-5 w-5 fill-current"
                xmlns="http://www.w3.org/2000/svg"
                viewBox="0 0 24 24"
            >
                <path d="M5.64,17l-.71.71a1,1,0,0,0,0,1.41,1,1,0,0,0,1.41,0l.71-.71A1,1,0,0,0,5.64,17ZM5,12a1,1,0,0,0-1-1H3a1,1,0,0,0,0,2H4A1,1,0,0,0,5,12Zm7-7a1,1,0,0,0,1-1V3a1,1,0,0,0-2,0V4A1,1,0,0,0,12,5ZM5.64,7.05a1,1,0,0,0,.7.29,1,1,0,0,0,.71-.29,1,1,0,0,0,0-1.41l-.71-.71A1,1,0,0,0,4.93,6.34Zm12,.29a1,1,0,0,0,.7-.29l.71-.71a1,1,0,1,0-1.41-1.41L17,5.64a1,1,0,0,0,0,1.41A1,1,0,0,0,17.66,7.34ZM21,11H20a1,1,0,0,0,0,2h1a1,1,0,0,0,0-2Zm-9,8a1,1,0,0,0-1,1v1a1,1,0,0,0,2,0V20A1,1,0,0,0,12,19ZM18.36,17A1,1,0,0,0,17,18.36l.71.71a1,1,0,0,0,1.41,0,1,1,0,0,0,0-1.41ZM12,6.5A5.5,5.5,0,1,0,17.5,12,5.51,5.51,0,0,0,12,6.5Z"></path>
            </svg>
            <svg
                class="swap-on h-5 w-5 fill-current"
                xmlns="http://www.w3.org/2000/svg"
                viewBox="0 0 24 24"
            >
                <path d="M21.64,13a1,1,0,0,0-1.05-.14,8.05,8.05,0,0,1-3.37.73A8.15,8.15,0,0,1,9.08,5.49a8.59,8.59,0,0,1,.25-2A1,1,0,0,0,8,2.36,10.14,10.14,0,1,0,22,14.05,1,1,0,0,0,21.64,13Zm-9.5,6.69A8.14,8.14,0,0,1,7.08,5.22v.27A10.15,10.15,0,0,0,17.22,15.63a9.79,9.79,0,0,0,2.1-.22A8.11,8.11,0,0,1,12.14,19.73Z"></path>
            </svg>
        </label>
    }
}

#[component]
pub fn App() -> impl IntoView {
    // daisyUI's drawer only closes when the checkbox is unchecked — clicking the hamburger
    // or the overlay does that natively via their `<label for="app-drawer">`, but tapping a
    // sidebar nav link wouldn't, since it's a separate element. Track the checkbox state
    // explicitly so nav-link clicks can close the drawer too.
    let drawer_open = RwSignal::new(false);

    let theme = RwSignal::new(Theme::Light);
    Effect::new(move |_| {
        if let Some(root) = document().document_element() {
            _ = root.set_attribute("data-theme", theme.get().as_str());
        }
    });

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
                        <div class="mx-2 flex-1 px-2 text-xl font-bold">
                            <a href="/">"pipa"</a>
                        </div>
                        <div class="hidden flex-none items-center gap-2 lg:flex">
                            <ul class="menu menu-horizontal">
                                {NAV_LINKS
                                    .iter()
                                    .map(|(href, label)| view! { <li><a href=*href>{*label}</a></li> })
                                    .collect_view()}
                            </ul>
                            <ThemeToggle theme=theme />
                        </div>
                    </div>
                    <main class="p-6">
                        <div class="card card-border bg-base-100 shadow-xl w-full">
                            <div class="card-body">
                                <Routes fallback=|| "not found">
                                    <Route path=path!("/") view=Landing />
                                    <Route path=path!("/dashboard") view=Dashboard />
                                </Routes>
                            </div>
                        </div>
                    </main>
                </div>
                <div class="drawer-side">
                    <label for="app-drawer" aria-label="close sidebar" class="drawer-overlay"></label>
                    <ul class="menu bg-base-200 min-h-full w-80 p-4">
                        <li>
                            <ThemeToggle theme=theme />
                        </li>
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

/// How long a status toast stays on screen before it auto-dismisses.
pub(crate) const STATUS_TOAST_DURATION: std::time::Duration = std::time::Duration::from_secs(4);

#[component]
fn Dashboard() -> impl IntoView {
    let vm = AppViewModel::new();
    Effect::new(move |_| vm.refresh_all());

    // Auto-dismiss the status toast so it doesn't linger on screen forever.
    Effect::new(move |_| {
        if vm.status.get().is_some() {
            set_timeout(move || vm.status.set(None), STATUS_TOAST_DURATION);
        }
    });

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
                            <div class="toast toast-top toast-end">
                                <div role="alert" class=msg.alert_class()>
                                    <span>{msg.text().to_string()}</span>
                                </div>
                            </div>
                        }
                    })
            }}

            <ProjectsCard vm=vm.projects />
            <SourcesCard vm=vm.sources />
        </div>
    }
}
