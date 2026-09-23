//! Dashboard for setting up and connecting OLTP databases to pipa CDC pipelines.
//!
//! Organized as MVVM:
//! - [`model`] — wire/domain types shared between the API client and the views.
//! - [`api`] — the HTTP client talking to `pipa-rest`; the Model's I/O.
//! - [`viewmodel`] — reactive state (`RwSignal`s) plus the commands that mutate it; the only
//!   layer that calls `api`.
//! - [`view`] — Leptos components that render from a ViewModel's signals and call its
//!   methods in response to user input; they hold no business logic and never call `api`
//!   directly.

mod api;
mod model;
mod view;
mod viewmodel;

use view::App;

fn main() {
    console_error_panic_hook::set_once();
    _ = console_log::init_with_level(log::Level::Debug);
    leptos::mount::mount_to_body(App);
}
