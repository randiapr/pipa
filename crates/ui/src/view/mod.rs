//! View layer: Leptos components that render from a ViewModel.
//!
//! Components here read `RwSignal`s straight off a `crate::viewmodel` struct and call its
//! methods from event handlers. They hold no business logic of their own and never call
//! `crate::api` directly — that boundary belongs to the ViewModel.

mod landing;
mod login;
mod pagination;
mod projects_card;
mod query;
mod rows_table;
mod sources_card;
mod tables;
mod users;

use leptos::prelude::*;
use leptos_router::{
    components::{Redirect, Route, Router, Routes},
    hooks::{use_location, use_navigate},
    path,
};

use landing::Landing;
use login::Login;
pub use pagination::Pagination;
use projects_card::ProjectsCard;
use query::Query;
pub use rows_table::RowsTable;
use sources_card::SourcesCard;
use tables::Tables;
use users::Users;

use crate::viewmodel::{AppViewModel, SessionViewModel};

/// Nav destinations shared between the desktop navbar menu and the mobile sidebar drawer.
/// The "Home" entry that used to lead this list has been replaced by [`ThemeToggle`] (the
/// brand link covers going home).
/// Projects, Sources, Query and Tables are for developers and admins, Users for admins only. The
/// view-only `user` role gets no links at all: Tables is its only page and it lands there after
/// signing in (it picks its project in the nav bar's switcher). The routes are guarded too, and
/// the backend is what actually refuses anyone not allowed.
fn nav_links(can_develop: bool, is_admin: bool) -> Vec<(&'static str, &'static str)> {
    let mut links = Vec::new();
    if can_develop {
        links.push(("/projects", "Projects"));
        links.push(("/sources", "Sources"));
        links.push(("/query", "Query"));
        links.push(("/tables", "Tables"));
    }
    if is_admin {
        links.push(("/users", "Users"));
    }
    links
}

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

/// Renders its children everywhere except the sign-in page, which stands alone without the navbar.
#[component]
fn HideOnLogin(children: ChildrenFn) -> impl IntoView {
    let location = use_location();

    view! { <Show when=move || location.pathname.get() != "/login">{children()}</Show> }
}

/// The card every page renders inside — except the sign-in page, which draws its own card and
/// would otherwise sit as a card in a card. The wrapper elements stay mounted and only their
/// classes change, so navigating to/from `/login` doesn't remount the routes.
#[component]
fn PageFrame(children: Children) -> impl IntoView {
    let location = use_location();
    let on_login = move || location.pathname.get() == "/login";

    view! {
        <div class=move || {
            if on_login() { "flex w-full flex-1 items-center justify-center" } else { "card card-border bg-base-100 shadow-xl w-full" }
        }>
            <div class=move || if on_login() { "" } else { "card-body" }>{children()}</div>
        </div>
    }
}

#[component]
pub fn App() -> impl IntoView {
    let session = SessionViewModel::new();
    provide_context(session);
    session.init();

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
                <div class="drawer-content flex min-h-screen flex-col">
                    <HideOnLogin>
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
                            <Show when=move || session.is_authenticated()>
                                <ul class="menu menu-horizontal">
                                    {move || {
                                        nav_links(session.can_develop(), session.is_admin())
                                            .into_iter()
                                            .map(|(href, label)| view! { <li><a href=href>{label}</a></li> })
                                            .collect_view()
                                    }}
                                </ul>
                                <ProjectSwitcher />
                                <UserMenu />
                            </Show>
                            <ThemeToggle theme=theme />
                        </div>
                    </div>
                    </HideOnLogin>
                    <main class="flex flex-1 flex-col p-6">
                        <PageFrame>
                                <Routes fallback=|| "not found">
                                    <Route path=path!("/login") view=Login />
                                    <Route
                                        path=path!("/")
                                        view=|| view! { <RequireAuth><Home /></RequireAuth> }
                                    />
                                    <Route
                                        path=path!("/projects")
                                        view=|| {
                                            view! {
                                                <RequireAuth developer_only=true>
                                                    <ProjectsPage />
                                                </RequireAuth>
                                            }
                                        }
                                    />
                                    <Route
                                        path=path!("/sources")
                                        view=|| {
                                            view! {
                                                <RequireAuth developer_only=true>
                                                    <SourcesPage />
                                                </RequireAuth>
                                            }
                                        }
                                    />
                                    <Route
                                        path=path!("/query")
                                        view=|| {
                                            view! {
                                                <RequireAuth developer_only=true>
                                                    <Query />
                                                </RequireAuth>
                                            }
                                        }
                                    />
                                    <Route
                                        path=path!("/tables")
                                        view=|| view! { <RequireAuth><Tables /></RequireAuth> }
                                    />
                                    <Route
                                        path=path!("/users")
                                        view=|| {
                                            view! {
                                                <RequireAuth admin_only=true>
                                                    <Users />
                                                </RequireAuth>
                                            }
                                        }
                                    />
                                </Routes>
                        </PageFrame>
                    </main>
                </div>
                <div class="drawer-side">
                    <label for="app-drawer" aria-label="close sidebar" class="drawer-overlay"></label>
                    <ul class="menu bg-base-200 min-h-full w-80 p-4">
                        <li>
                            <ThemeToggle theme=theme />
                        </li>
                        <Show when=move || session.is_authenticated()>
                            <li class="mb-2">
                                <ProjectSwitcher />
                            </li>
                            {move || {
                                nav_links(session.can_develop(), session.is_admin())
                                    .into_iter()
                                    .map(|(href, label)| {
                                        view! {
                                            <li>
                                                <a href=href on:click=move |_| drawer_open.set(false)>
                                                    {label}
                                                </a>
                                            </li>
                                        }
                                    })
                                    .collect_view()
                            }}
                            <li class="mt-2">
                                <UserMenu />
                            </li>
                        </Show>
                    </ul>
                </div>
            </div>
        </Router>
    }
}

/// The project the whole app is scoped to. An admin may also pick "All projects"; anyone else
/// only sees the projects an admin assigned to them.
#[component]
fn ProjectSwitcher() -> impl IntoView {
    let session = expect_context::<SessionViewModel>();

    view! {
        <Show
            when=move || session.is_admin() || !session.projects.get().is_empty()
            fallback=|| view! { <span class="text-sm text-base-content/70">"No projects assigned"</span> }
        >
            <select
                class="select select-sm w-48"
                aria-label="Current project"
                on:change:target=move |ev| {
                    session.select_project(Some(ev.target().value()).filter(|id| !id.is_empty()))
                }
            >
                <Show when=move || session.is_admin()>
                    <option value="" prop:selected=move || session.current_project_id.get().is_none()>
                        "All projects"
                    </option>
                </Show>
                <For
                    each=move || session.projects.get()
                    key=|project| (project.id.clone(), project.name.clone())
                    children=move |project| {
                        let id = project.id.clone();
                        view! {
                            <option
                                value=project.id
                                prop:selected=move || {
                                    session.current_project_id.get().as_deref() == Some(id.as_str())
                                }
                            >
                                {project.name}
                            </option>
                        }
                    }
                />
            </select>
        </Show>
    }
}

/// Who is signed in, and the way out.
#[component]
fn UserMenu() -> impl IntoView {
    let session = expect_context::<SessionViewModel>();
    let navigate = use_navigate();

    view! {
        <div class="flex items-center gap-2">
            <span class="text-sm">
                {move || session.user.get().map(|user| user.username).unwrap_or_default()}
            </span>
            <span class="badge badge-sm badge-outline">
                {move || session.role_name()}
            </span>
            <button
                class="btn btn-sm btn-ghost"
                type="button"
                on:click=move |_| {
                    session.logout();
                    navigate("/login", Default::default());
                }
            >
                "Sign out"
            </button>
        </div>
    }
}

/// The landing page: the projects overview for developers and admins. A view-only `user` has no
/// Projects page, so it goes straight to the tables.
#[component]
fn Home() -> impl IntoView {
    let session = expect_context::<SessionViewModel>();

    move || {
        if session.can_develop() {
            view! { <Landing /> }.into_any()
        } else {
            view! { <Redirect path="/tables" /> }.into_any()
        }
    }
}

/// Route guard: renders `children` only for a signed-in user (an admin, with `admin_only`; an
/// admin or developer, with `developer_only`). Anyone else is redirected — to the login page when
/// signed out, to the landing page when merely not allowed. Waits for the stored token to be
/// checked first, so a reload doesn't flash the login page at someone who is still signed in.
#[component]
fn RequireAuth(
    #[prop(optional)] admin_only: bool,
    #[prop(optional)] developer_only: bool,
    children: ChildrenFn,
) -> impl IntoView {
    let session = expect_context::<SessionViewModel>();
    let navigate = use_navigate();

    Effect::new(move |_| {
        if !session.checked.get() {
            return;
        }
        if !session.is_authenticated() {
            navigate("/login", Default::default());
        } else if (admin_only && !session.is_admin()) || (developer_only && !session.can_develop())
        {
            navigate("/", Default::default());
        }
    });

    move || {
        let allowed = session.checked.get()
            && session.is_authenticated()
            && (!admin_only || session.is_admin())
            && (!developer_only || session.can_develop());
        allowed.then(|| children())
    }
}

/// How long a status toast stays on screen before it auto-dismisses.
pub(crate) const STATUS_TOAST_DURATION: std::time::Duration = std::time::Duration::from_secs(4);

/// Fetches on mount and whenever the selected project changes (`refresh_all` reads it), and
/// auto-dismisses the status toast so it doesn't linger on screen forever.
fn page_view_model() -> AppViewModel {
    let vm = AppViewModel::new();
    Effect::new(move |_| vm.refresh_all());
    Effect::new(move |_| {
        if vm.status.get().is_some() {
            set_timeout(move || vm.status.set(None), STATUS_TOAST_DURATION);
        }
    });
    vm
}

#[component]
fn StatusToast(vm: AppViewModel) -> impl IntoView {
    move || {
        vm.status.get().map(|msg| {
            view! {
                <div class="toast toast-top toast-end">
                    <div role="alert" class=msg.alert_class()>
                        <span>{msg.text().to_string()}</span>
                    </div>
                </div>
            }
        })
    }
}

#[component]
fn ProjectsPage() -> impl IntoView {
    let vm = page_view_model();

    view! {
        <div class="flex flex-col gap-6">
            <StatusToast vm=vm />
            <ProjectsCard vm=vm.projects />
        </div>
    }
}

#[component]
fn SourcesPage() -> impl IntoView {
    let vm = page_view_model();

    view! {
        <div class="flex flex-col gap-6">
            <p class="text-base-content/70">
                "Connect and manage OLTP database sources for CDC capture."
            </p>
            <ProjectScopeNote />
            <StatusToast vm=vm />
            <SourcesCard vm=vm.sources />
        </div>
    }
}

/// Says which project the sources below are limited to.
#[component]
fn ProjectScopeNote() -> impl IntoView {
    let session = expect_context::<SessionViewModel>();

    view! {
        <p class="text-sm text-base-content/70">
            {move || match session.current_project() {
                Some(project) => format!("Showing data sources of \"{}\".", project.name),
                None => "Showing data sources of all projects.".to_string(),
            }}
        </p>
    }
}
