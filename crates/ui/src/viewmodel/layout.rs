//! ViewModel: which layout the lists render in — a paginated `table` on desktop, a stack of
//! `card`s on mobile.
//!
//! The mode comes from `window.matchMedia(DESKTOP_QUERY)` and follows it live (resizing,
//! rotating a tablet), so only the matching layout is ever in the DOM. The view additionally
//! wraps each layout in the same breakpoint's CSS (`hidden lg:block` / `lg:hidden`, see
//! `view::ResponsiveList`): when `matchMedia` is unavailable the mode is unknown, both layouts
//! are rendered and CSS alone picks one, so the page degrades to CSS-only instead of guessing.

use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::MediaQueryListEvent;

/// Tailwind's `lg` breakpoint (`64rem`), the one the navbar already switches its drawer on, so
/// the whole dashboard changes layout at a single width. Must stay in step with the `lg:`
/// classes in `view::ResponsiveList`.
const DESKTOP_QUERY: &str = "(min-width: 64rem)";

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LayoutMode {
    /// Lists render as a `table`.
    Desktop,
    /// Lists render as a stack of `card`s.
    Mobile,
}

impl LayoutMode {
    fn from_matches(desktop: bool) -> Self {
        if desktop { Self::Desktop } else { Self::Mobile }
    }
}

/// Shared through context (created once in `App`, like [`crate::viewmodel::SessionViewModel`]).
#[derive(Copy, Clone)]
pub struct LayoutViewModel {
    /// `None` when the browser can't evaluate the media query; the view then leaves the choice
    /// to CSS.
    pub mode: ReadSignal<Option<LayoutMode>>,
}

impl LayoutViewModel {
    pub fn new() -> Self {
        let query = window().match_media(DESKTOP_QUERY).ok().flatten();
        let (mode, set_mode) = signal(
            query
                .as_ref()
                .map(|query| LayoutMode::from_matches(query.matches())),
        );

        if let Some(query) = query {
            let on_change =
                Closure::<dyn FnMut(MediaQueryListEvent)>::new(move |ev: MediaQueryListEvent| {
                    set_mode.set(Some(LayoutMode::from_matches(ev.matches())))
                });
            if query
                .add_event_listener_with_callback("change", on_change.as_ref().unchecked_ref())
                .is_ok()
            {
                // Lives as long as the page: this ViewModel is created once, by `App`.
                on_change.forget();
            }
        }

        Self { mode }
    }

    /// Whether the desktop layout should be rendered: when the media query says so, and also
    /// when it can't be evaluated (both layouts are rendered then, and CSS hides one).
    pub fn renders_desktop(&self) -> bool {
        self.mode.get() != Some(LayoutMode::Mobile)
    }

    /// Whether the mobile layout should be rendered; see [`Self::renders_desktop`].
    pub fn renders_mobile(&self) -> bool {
        self.mode.get() != Some(LayoutMode::Desktop)
    }
}

impl Default for LayoutViewModel {
    fn default() -> Self {
        Self::new()
    }
}
