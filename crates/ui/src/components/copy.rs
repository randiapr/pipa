// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! [`CopyButton`]: a small icon button that copies a text to the clipboard.

use std::time::Duration;

use leptos::prelude::*;
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::wasm_bindgen;

use super::icons::{CheckIcon, CopyIcon};

/// How long the check mark replaces the copy icon after a copy.
const COPIED_FOR: Duration = Duration::from_millis(1500);

#[wasm_bindgen]
extern "C" {
    /// `navigator.clipboard.writeText`, declared here because `web-sys`' `Clipboard` is behind
    /// the `web_sys_unstable_apis` cfg. The returned promise is dropped: a refused write
    /// (insecure context, permission) just leaves nothing on the clipboard.
    #[wasm_bindgen(js_namespace = ["navigator", "clipboard"], js_name = writeText)]
    fn write_text(text: &str) -> JsValue;
}

/// Copies `text` on click, and shows a check mark for a moment afterwards.
#[component]
pub fn CopyButton(
    /// What lands on the clipboard.
    #[prop(into)]
    text: String,
    /// The tooltip and accessible name.
    #[prop(default = "Copy")]
    label: &'static str,
) -> impl IntoView {
    let copied = RwSignal::new(false);

    let on_click = move |_| {
        write_text(&text);
        copied.set(true);
        set_timeout(move || copied.set(false), COPIED_FOR);
    };

    view! {
        <button
            class="btn btn-ghost btn-xs btn-square"
            type="button"
            title=label
            aria-label=label
            on:click=on_click
        >
            {move || {
                if copied.get() {
                    view! { <CheckIcon /> }.into_any()
                } else {
                    view! { <CopyIcon /> }.into_any()
                }
            }}
        </button>
    }
}
