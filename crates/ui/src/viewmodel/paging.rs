//! ViewModel: page sizes and client-side pagination, shared by every paginated list (Projects,
//! Sources, Users, Query results) and, for its page size only, the server-paged Tables browser.
//!
//! How many rows a page holds depends on the layout: the desktop table has a rows-per-page
//! picker ([`PAGE_SIZE_OPTIONS`], at most [`MAX_PAGE_SIZE`]), the mobile cards a fixed
//! [`MOBILE_PAGE_SIZE`]. Both layouts render the same [`PagedList::paged`] slice.

use leptos::prelude::*;

use crate::viewmodel::{LayoutViewModel, layout::LayoutMode};

/// The choices of the desktop rows-per-page picker.
pub const PAGE_SIZE_OPTIONS: [usize; 4] = [10, 25, 50, 100];
/// The most rows a desktop page may hold, whatever is asked for.
pub const MAX_PAGE_SIZE: usize = 100;
/// The desktop page size before the user picks one.
pub const DEFAULT_PAGE_SIZE: usize = 10;
/// Cards are much taller than table rows, so a mobile page holds only a few.
pub const MOBILE_PAGE_SIZE: usize = 5;

/// The page size of one list: the user's pick on desktop, [`MOBILE_PAGE_SIZE`] on mobile.
/// Follows [`LayoutViewModel`], so it changes when a resize swaps the layout. When the layout
/// is unknown (no `matchMedia`, CSS picks), the desktop size applies.
#[derive(Copy, Clone)]
pub struct PageSize {
    desktop: RwSignal<usize>,
    layout: LayoutViewModel,
}

impl PageSize {
    /// Must be created below `App`, which provides the [`LayoutViewModel`].
    pub fn new() -> Self {
        Self {
            desktop: RwSignal::new(DEFAULT_PAGE_SIZE),
            layout: expect_context::<LayoutViewModel>(),
        }
    }

    /// The rows per page in the current layout.
    pub fn get(&self) -> usize {
        Self::effective(self.layout.mode.get(), self.desktop.get())
    }

    pub fn get_untracked(&self) -> usize {
        Self::effective(
            self.layout.mode.get_untracked(),
            self.desktop.get_untracked(),
        )
    }

    /// The desktop picker's current choice.
    pub fn desktop(&self) -> usize {
        self.desktop.get()
    }

    /// Sets the desktop page size, capped at [`MAX_PAGE_SIZE`].
    pub fn set_desktop(&self, size: usize) {
        self.desktop.set(size.clamp(1, MAX_PAGE_SIZE));
    }

    fn effective(mode: Option<LayoutMode>, desktop: usize) -> usize {
        match mode {
            Some(LayoutMode::Mobile) => MOBILE_PAGE_SIZE,
            Some(LayoutMode::Desktop) | None => desktop,
        }
    }
}

impl Default for PageSize {
    fn default() -> Self {
        Self::new()
    }
}

/// A list plus the page of it being shown. The position is kept as the index of the first row
/// shown rather than a page number, so changing the page size (picking another one, or a resize
/// swapping layouts) stays on the page holding that row instead of jumping elsewhere.
///
/// All fields are signal handles, so this is cheap to `Copy` whatever `T` is (hence the
/// hand-written `Clone`/`Copy` — a derive would demand `T: Copy`).
pub struct PagedList<T: Send + Sync + 'static> {
    pub items: RwSignal<Vec<T>>,
    pub size: PageSize,
    start: RwSignal<usize>,
}

impl<T: Send + Sync + 'static> Clone for PagedList<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Send + Sync + 'static> Copy for PagedList<T> {}

impl<T: Clone + Send + Sync + 'static> PagedList<T> {
    /// Must be created below `App`; see [`PageSize::new`].
    pub fn new() -> Self {
        Self::over(RwSignal::new(Vec::new()))
    }

    /// Paginates a list owned elsewhere (e.g. the session's project list).
    pub fn over(items: RwSignal<Vec<T>>) -> Self {
        Self {
            items,
            size: PageSize::new(),
            start: RwSignal::new(0),
        }
    }

    /// Replaces the list and goes back to its first page.
    pub fn reset(&self, items: Vec<T>) {
        self.items.set(items);
        self.start.set(0);
    }

    pub fn is_empty(&self) -> bool {
        self.items.with(Vec::is_empty)
    }

    pub fn len(&self) -> usize {
        self.items.with(Vec::len)
    }

    pub fn total_pages(&self) -> usize {
        self.len().div_ceil(self.size.get()).max(1)
    }

    /// The page being shown, clamped to the last valid one (e.g. after a delete shrinks the
    /// list past the page the user was on).
    pub fn current_page(&self) -> usize {
        (self.start.get() / self.size.get()).min(self.total_pages() - 1)
    }

    /// The current page's slice.
    pub fn paged(&self) -> Vec<T> {
        let size = self.size.get();
        let first = self.current_page() * size;
        self.items
            .with(|all| all.iter().skip(first).take(size).cloned().collect())
    }

    pub fn has_prev(&self) -> bool {
        self.current_page() > 0
    }

    pub fn has_next(&self) -> bool {
        self.current_page() + 1 < self.total_pages()
    }

    pub fn prev(&self) {
        self.go_to(self.current_page().saturating_sub(1));
    }

    pub fn next(&self) {
        if self.has_next() {
            self.go_to(self.current_page() + 1);
        }
    }

    fn go_to(&self, page: usize) {
        self.start.set(page * self.size.get());
    }
}

impl<T: Clone + Send + Sync + 'static> Default for PagedList<T> {
    fn default() -> Self {
        Self::new()
    }
}
