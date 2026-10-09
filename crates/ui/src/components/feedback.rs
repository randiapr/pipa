//! daisyUI feedback components: [`Alert`], [`Toast`], [`Badge`] and [`Loading`].

use leptos::prelude::*;

/// A component's color: daisyUI's full set, whether or not a page uses each one yet.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tone {
    /// The component's own default color.
    #[default]
    Plain,
    Neutral,
    Primary,
    Secondary,
    Ghost,
    Info,
    Success,
    Warning,
    Error,
}

impl Tone {
    fn alert_class(self) -> &'static str {
        match self {
            Tone::Info => "alert-info",
            Tone::Success => "alert-success",
            Tone::Warning => "alert-warning",
            Tone::Error => "alert-error",
            Tone::Plain | Tone::Neutral | Tone::Primary | Tone::Secondary | Tone::Ghost => "",
        }
    }

    fn badge_class(self) -> &'static str {
        match self {
            Tone::Plain => "",
            Tone::Neutral => "badge-neutral",
            Tone::Primary => "badge-primary",
            Tone::Secondary => "badge-secondary",
            Tone::Ghost => "badge-ghost",
            Tone::Info => "badge-info",
            Tone::Success => "badge-success",
            Tone::Warning => "badge-warning",
            Tone::Error => "badge-error",
        }
    }
}

/// An alert (`role="alert"`). `soft` gives the lighter variant, for notes rather than errors.
#[component]
pub fn Alert(
    #[prop(optional)] tone: Tone,
    #[prop(optional)] soft: bool,
    children: Children,
) -> impl IntoView {
    let soft = if soft { " alert-soft" } else { "" };

    view! {
        <div role="alert" class=format!("alert {}{soft}", tone.alert_class())>
            <span>{children()}</span>
        </div>
    }
}

/// Pins its content (typically an [`Alert`]) to the top-right corner, above everything else.
#[component]
pub fn Toast(children: Children) -> impl IntoView {
    view! { <div class="toast toast-top toast-end">{children()}</div> }
}

/// How a [`Badge`] is filled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BadgeStyle {
    #[default]
    Solid,
    Soft,
    Outline,
}

#[component]
pub fn Badge(
    #[prop(optional)] tone: Tone,
    #[prop(optional)] style: BadgeStyle,
    /// `badge-sm`.
    #[prop(optional)]
    small: bool,
    /// Extra classes, e.g. `shrink-0` or a margin.
    #[prop(optional, into)]
    class: String,
    children: Children,
) -> impl IntoView {
    let style = match style {
        BadgeStyle::Solid => "",
        BadgeStyle::Soft => " badge-soft",
        BadgeStyle::Outline => " badge-outline",
    };
    let size = if small { " badge-sm" } else { "" };

    view! {
        <span class=format!("badge {}{style}{size} {class}", tone.badge_class())>{children()}</span>
    }
}

/// A spinner, for content still loading.
#[component]
pub fn Loading() -> impl IntoView {
    view! { <span class="loading loading-spinner" aria-label="Loading"></span> }
}
