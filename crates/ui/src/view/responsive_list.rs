//! View: renders a list as a `table` on desktop or a stack of `card`s on mobile.
//!
//! Two layers decide which one shows, on the same breakpoint (Tailwind's `lg`, the
//! `DESKTOP_QUERY` of [`LayoutViewModel`]):
//! - `matchMedia`, through [`LayoutViewModel`], mounts only the matching layout, so a list's
//!   rows aren't built (or kept in the DOM) twice;
//! - CSS (`hidden lg:block` / `lg:hidden`) hides whichever doesn't match. It changes nothing
//!   while `matchMedia` works, but takes over when it doesn't: both layouts are mounted then
//!   and CSS alone picks one.

use leptos::prelude::*;

use crate::viewmodel::LayoutViewModel;

#[component]
pub fn ResponsiveList(
    /// The desktop layout, typically a `<table>` in an `overflow-x-auto` wrapper.
    #[prop(into)]
    table: ViewFn,
    /// The mobile layout, typically a column of daisyUI `card`s.
    #[prop(into)]
    cards: ViewFn,
) -> impl IntoView {
    let layout = expect_context::<LayoutViewModel>();

    view! {
        <Show when=move || layout.renders_desktop()>
            <div class="hidden lg:block">{table.run()}</div>
        </Show>
        <Show when=move || layout.renders_mobile()>
            <div class="flex flex-col gap-3 lg:hidden">{cards.run()}</div>
        </Show>
    }
}
