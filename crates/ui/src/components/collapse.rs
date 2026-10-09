//! daisyUI `collapse` ([`Collapse`]) and the expand/collapse-all button for a set of them
//! ([`ExpandToggle`]). A collapse has an arrow at the start and is opened and closed from the
//! outside: `open` says whether it is expanded (forced with `collapse-open`/`collapse-close`,
//! so the caller's state stays the single source of truth) and `on_toggle` asks to flip it.
//! Collapses nest: daisyUI's rules only match a collapse's direct children, so a closed one
//! stays closed inside an open one.

use leptos::ev::KeyboardEvent;
use leptos::prelude::*;

use super::{CollapseAllIcon, ExpandAllIcon};

#[component]
pub fn Collapse(
    #[prop(into)] open: Signal<bool>,
    #[prop(into)] on_toggle: Callback<()>,
    /// The title row, always shown. Clicking it (or Enter/Space on it) toggles; a control
    /// inside it must stop its clicks from reaching the title (see `Checkbox::stop_click`).
    #[prop(into)]
    title: ViewFn,
    /// The background, distinguishing nesting levels.
    #[prop(into, default = "bg-base-100".to_string())]
    background: String,
    /// The content, only rendered while open.
    children: ChildrenFn,
) -> impl IntoView {
    let children = StoredValue::new(children);

    view! {
        <div
            class=format!("collapse collapse-arrow border border-base-300 {background}")
            class:collapse-open=move || open.get()
            class:collapse-close=move || !open.get()
        >
            <div
                class="collapse-title cursor-pointer ps-12 pe-4 after:start-5 after:end-auto"
                role="button"
                tabindex="0"
                aria-expanded=move || open.get().to_string()
                on:click=move |_| on_toggle.run(())
                on:keydown=move |ev: KeyboardEvent| {
                    // Only the title itself: Space on a checkbox inside it ticks that instead.
                    if ev.target() == ev.current_target() && (ev.key() == "Enter" || ev.key() == " ") {
                        ev.prevent_default();
                        on_toggle.run(());
                    }
                }
            >
                {title.run()}
            </div>
            <div class="collapse-content">
                <Show when=move || open.get()>{children.read_value()()}</Show>
            </div>
        </div>
    }
}

/// An icon button expanding a set of collapses, or collapsing them once they are all open; its
/// icon shows which it will do. `label` names the set in its tooltip ("Expand {label}").
#[component]
pub fn ExpandToggle(
    #[prop(into)] all_open: Signal<bool>,
    #[prop(into)] on_toggle: Callback<()>,
    #[prop(into)] label: String,
    #[prop(optional, into)] disabled: MaybeProp<bool>,
) -> impl IntoView {
    let text = move || {
        let action = if all_open.get() { "Collapse" } else { "Expand" };
        format!("{action} {label}")
    };

    view! {
        <button
            class="btn btn-sm btn-square"
            type="button"
            title=text.clone()
            aria-label=text
            disabled=move || disabled.get().unwrap_or(false)
            on:click=move |_| on_toggle.run(())
        >
            {move || {
                if all_open.get() {
                    view! { <CollapseAllIcon /> }.into_any()
                } else {
                    view! { <ExpandAllIcon /> }.into_any()
                }
            }}
        </button>
    }
}
