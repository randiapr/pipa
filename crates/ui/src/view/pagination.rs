//! View: a daisyUI `join`-based prev/next pager, shared by the paginated data tables.

use leptos::prelude::*;

use crate::viewmodel::PAGE_SIZE;

#[component]
pub fn Pagination(page: RwSignal<usize>, total: Signal<usize>) -> impl IntoView {
    let total_pages = move || total.get().div_ceil(PAGE_SIZE).max(1);
    let current_page = move || page.get().min(total_pages() - 1);

    view! {
        <div class="join">
            <button
                class="join-item btn btn-sm"
                type="button"
                disabled={move || current_page() == 0}
                on:click=move |_| page.update(|p| *p = p.saturating_sub(1))
            >
                "\u{ab}"
            </button>
            <button class="join-item btn btn-sm no-animation" type="button">
                {move || format!("Page {} / {}", current_page() + 1, total_pages())}
            </button>
            <button
                class="join-item btn btn-sm"
                type="button"
                disabled={move || current_page() + 1 >= total_pages()}
                on:click=move |_| page.update(|p| *p += 1)
            >
                "\u{bb}"
            </button>
        </div>
    }
}
