//! Dashboard for setting up and connecting OLTP databases to pipa CDC pipelines.

mod api;

use api::{ConnectionConfig, ConnectionTestOutcome, DataSourceView, DbEngine, NewDataSource};
use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::{
    components::{Route, Router, Routes},
    path,
};

fn main() {
    console_error_panic_hook::set_once();
    _ = console_log::init_with_level(log::Level::Debug);
    leptos::mount::mount_to_body(App);
}

/// Nav destinations shared between the desktop navbar menu and the mobile sidebar drawer.
const NAV_LINKS: &[(&str, &str)] = &[("#register", "Register"), ("#sources", "Sources")];

#[component]
fn App() -> impl IntoView {
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
                            <Route path=path!("/") view=Dashboard />
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
    let sources = RwSignal::new(Vec::<DataSourceView>::new());
    let status = RwSignal::new(Option::<String>::None);

    let name = RwSignal::new(String::new());
    let engine = RwSignal::new(DbEngine::Postgres);
    let host = RwSignal::new(String::new());
    let port = RwSignal::new(String::new());
    let username = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let database = RwSignal::new(String::new());

    let refresh = move || {
        spawn_local(async move {
            match api::list_sources().await {
                Ok(list) => sources.set(list),
                Err(err) => status.set(Some(format!("Failed to load data sources: {err}"))),
            }
        });
    };

    Effect::new(move |_| refresh());

    let on_submit = move |ev: SubmitEvent| {
        ev.prevent_default();

        let port_value: u16 = match port.get().trim().parse() {
            Ok(parsed) => parsed,
            Err(_) => {
                status.set(Some("Port must be a valid number.".to_string()));
                return;
            }
        };

        let new_source = NewDataSource {
            name: name.get(),
            engine: engine.get(),
            connection: ConnectionConfig {
                host: host.get(),
                port: port_value,
                username: username.get(),
                password: password.get(),
                database: database.get(),
            },
        };

        spawn_local(async move {
            match api::register_source(&new_source).await {
                Ok(_) => {
                    status.set(Some("Data source registered.".to_string()));
                    name.set(String::new());
                    host.set(String::new());
                    port.set(String::new());
                    username.set(String::new());
                    password.set(String::new());
                    database.set(String::new());
                    refresh();
                }
                Err(err) => status.set(Some(format!("Failed to register data source: {err}"))),
            }
        });
    };

    view! {
        <p>"Connect and manage OLTP database sources for CDC capture."</p>

        {move || status.get().map(|msg| view! { <p class="status">{msg}</p> })}

        <section id="register">
            <h2>"Register a data source"</h2>
            <form on:submit=on_submit>
                <label>
                    "Name"
                    <input
                        type="text"
                        required
                        prop:value=move || name.get()
                        on:input:target=move |ev| name.set(ev.target().value())
                    />
                </label>
                <label>
                    "Engine"
                    <select
                        prop:value=move || engine.get().wire_value()
                        on:change:target=move |ev| {
                            let value = ev.target().value();
                            engine.set(if value == "mysql" { DbEngine::Mysql } else { DbEngine::Postgres });
                        }
                    >
                        <option value="postgres">"PostgreSQL"</option>
                        <option value="mysql">"MySQL"</option>
                    </select>
                </label>
                <label>
                    "Host"
                    <input
                        type="text"
                        required
                        prop:value=move || host.get()
                        on:input:target=move |ev| host.set(ev.target().value())
                    />
                </label>
                <label>
                    "Port"
                    <input
                        type="text"
                        required
                        placeholder=move || engine.get().default_port().to_string()
                        prop:value=move || port.get()
                        on:input:target=move |ev| port.set(ev.target().value())
                    />
                </label>
                <label>
                    "Username"
                    <input
                        type="text"
                        required
                        prop:value=move || username.get()
                        on:input:target=move |ev| username.set(ev.target().value())
                    />
                </label>
                <label>
                    "Password"
                    <input
                        type="password"
                        prop:value=move || password.get()
                        on:input:target=move |ev| password.set(ev.target().value())
                    />
                </label>
                <label>
                    "Database"
                    <input
                        type="text"
                        required
                        prop:value=move || database.get()
                        on:input:target=move |ev| database.set(ev.target().value())
                    />
                </label>
                <button type="submit">"Register"</button>
            </form>
        </section>

        <section id="sources">
            <h2>"Registered data sources"</h2>
            <Show
                when=move || !sources.get().is_empty()
                fallback=|| view! { <p>"No data sources registered yet."</p> }
            >
                <ul>
                    <For
                        each=move || sources.get()
                        key=|source| source.id.clone()
                        children=move |source| {
                            let id_for_test = source.id.clone();
                            let id_for_delete = source.id.clone();
                            let test_result = RwSignal::new(Option::<ConnectionTestOutcome>::None);

                            let on_test = move |_| {
                                let id = id_for_test.clone();
                                spawn_local(async move {
                                    match api::test_source(&id).await {
                                        Ok(outcome) => test_result.set(Some(outcome)),
                                        Err(err) => status.set(Some(format!("Connection test failed: {err}"))),
                                    }
                                });
                            };

                            let on_delete = move |_| {
                                let id = id_for_delete.clone();
                                spawn_local(async move {
                                    match api::delete_source(&id).await {
                                        Ok(()) => refresh(),
                                        Err(err) => status.set(Some(format!("Failed to remove data source: {err}"))),
                                    }
                                });
                            };

                            view! {
                                <li>
                                    <strong>{source.name.clone()}</strong>
                                    " (" {source.engine.label()} ") "
                                    {format!(
                                        "{}:{}/{}",
                                        source.connection.host,
                                        source.connection.port,
                                        source.connection.database,
                                    )}
                                    " "
                                    <button on:click=on_test type="button">"Test connection"</button>
                                    " "
                                    <button on:click=on_delete type="button">"Remove"</button>
                                    {move || {
                                        test_result.get().map(|outcome| match outcome {
                                            ConnectionTestOutcome::Reachable => {
                                                view! { <span class="ok">" reachable"</span> }.into_any()
                                            }
                                            ConnectionTestOutcome::Unreachable { reason } => {
                                                view! {
                                                    <span class="err">{format!(" unreachable: {reason}")}</span>
                                                }
                                                    .into_any()
                                            }
                                        })
                                    }}
                                </li>
                            }
                        }
                    />
                </ul>
            </Show>
        </section>
    }
}
