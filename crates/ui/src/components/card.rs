//! daisyUI `card`: [`Card`] (a bordered card with its body), [`CardTitle`] and
//! [`CardActions`].

use leptos::prelude::*;

#[component]
pub fn Card(
    /// Extra classes on the card, e.g. `shadow-xl` or sizing.
    #[prop(optional, into)]
    class: String,
    /// Extra classes on the card body, e.g. its gap or alignment.
    #[prop(optional, into)]
    body_class: String,
    /// The small variant (`card-sm`), for list items.
    #[prop(optional)]
    compact: bool,
    /// Draws the border in the primary color, e.g. for the selected item.
    #[prop(optional, into)]
    highlighted: MaybeProp<bool>,
    children: Children,
) -> impl IntoView {
    let size = if compact { "card-sm " } else { "" };

    view! {
        <div
            class=format!("card card-border {size}bg-base-100 {class}")
            class=("border-primary", move || highlighted.get().unwrap_or(false))
        >
            <div class=format!("card-body {body_class}")>{children()}</div>
        </div>
    }
}

/// A card's title: an `h2`, or an `h3` with `level=3` (a card inside a page section).
#[component]
pub fn CardTitle(
    #[prop(default = 2)] level: u8,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let class = format!("card-title {class}");
    if level <= 2 {
        view! { <h2 class=class>{children()}</h2> }.into_any()
    } else {
        view! { <h3 class=class>{children()}</h3> }.into_any()
    }
}

/// A card's action row, `justify-end` unless `class` says otherwise.
#[component]
pub fn CardActions(
    #[prop(into, default = "justify-end".to_string())] class: String,
    children: Children,
) -> impl IntoView {
    view! { <div class=format!("card-actions {class}")>{children()}</div> }
}
