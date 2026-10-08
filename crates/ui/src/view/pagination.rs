//! View: a daisyUI `join`-based prev/next pager plus the desktop rows-per-page picker, shared
//! by the paginated lists in both their desktop (table) and mobile (cards) layouts.

use leptos::prelude::*;

use crate::viewmodel::{LayoutViewModel, PAGE_SIZE_OPTIONS, PageSize, PagedList};

#[component]
pub fn Pagination<T: Clone + Send + Sync + 'static>(list: PagedList<T>) -> impl IntoView {
    view! {
        <div class="flex flex-wrap items-center justify-end gap-3">
            <PageSizePicker size=list.size />
            <div class="join">
                <button
                    class="join-item btn btn-sm"
                    type="button"
                    disabled=move || !list.has_prev()
                    on:click=move |_| list.prev()
                >
                    "\u{ab}"
                </button>
                <button class="join-item btn btn-sm no-animation" type="button">
                    {move || format!("Page {} / {}", list.current_page() + 1, list.total_pages())}
                </button>
                <button
                    class="join-item btn btn-sm"
                    type="button"
                    disabled=move || !list.has_next()
                    on:click=move |_| list.next()
                >
                    "\u{bb}"
                </button>
            </div>
        </div>
    }
}

/// The rows-per-page `<select>` (desktop only — mobile pages have a fixed size). Mounted and
/// hidden the same two ways as `ResponsiveList`'s table layout.
#[component]
pub fn PageSizePicker(size: PageSize) -> impl IntoView {
    let layout = expect_context::<LayoutViewModel>();

    view! {
        <Show when=move || layout.renders_desktop()>
            <label class="hidden items-center gap-2 text-sm text-base-content/70 lg:flex">
                "Rows per page"
                <select
                    class="select select-sm w-20"
                    on:change:target=move |ev| {
                        if let Ok(rows) = ev.target().value().parse() {
                            size.set_desktop(rows);
                        }
                    }
                >
                    {PAGE_SIZE_OPTIONS
                        .iter()
                        .map(|&rows| {
                            view! {
                                <option value=rows.to_string() selected=move || size.desktop() == rows>
                                    {rows.to_string()}
                                </option>
                            }
                        })
                        .collect_view()}
                </select>
            </label>
        </Show>
    }
}
