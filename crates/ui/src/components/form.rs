//! daisyUI form components: [`Field`] (a labelled `fieldset`), [`TextInput`], [`TextArea`],
//! [`Select`] and [`Checkbox`]. Inputs are bound two-way to an `RwSignal<String>`.

use leptos::prelude::*;

/// A form field: `legend` above the control(s) in `children`, `hint` below.
#[component]
pub fn Field(
    #[prop(into)] legend: TextProp,
    #[prop(optional, into)] hint: Option<String>,
    children: Children,
) -> impl IntoView {
    view! {
        <fieldset class="fieldset">
            <legend class="fieldset-legend">{move || legend.get()}</legend>
            {children()}
            {hint.map(|hint| view! { <p class="label">{hint}</p> })}
        </fieldset>
    }
}

/// A text-like `<input>` bound to `value`.
#[component]
pub fn TextInput(
    value: RwSignal<String>,
    /// The input `type`: `text`, `password`, `search`, …
    #[prop(default = "text")]
    kind: &'static str,
    #[prop(optional)] required: bool,
    #[prop(optional, into)] placeholder: MaybeProp<String>,
    #[prop(optional)] autocomplete: Option<&'static str>,
    #[prop(optional)] minlength: Option<u32>,
    /// `input-sm`.
    #[prop(optional)]
    small: bool,
    /// Sizing classes.
    #[prop(into, default = "w-full".to_string())]
    class: String,
) -> impl IntoView {
    let size = if small { "input-sm " } else { "" };

    view! {
        <input
            type=kind
            class=format!("input {size}{class}")
            required=required
            placeholder=move || placeholder.get()
            autocomplete=autocomplete
            minlength=minlength.map(|length| length.to_string())
            prop:value=move || value.get()
            on:input:target=move |ev| value.set(ev.target().value())
        />
    }
}

/// A `<textarea>` bound to `value`.
#[component]
pub fn TextArea(
    value: RwSignal<String>,
    #[prop(optional)] required: bool,
    #[prop(optional, into)] placeholder: MaybeProp<String>,
    #[prop(into, default = "w-full".to_string())] class: String,
) -> impl IntoView {
    view! {
        <textarea
            class=format!("textarea {class}")
            required=required
            placeholder=move || placeholder.get()
            prop:value=move || value.get()
            on:input:target=move |ev| value.set(ev.target().value())
        ></textarea>
    }
}

/// A `<select>` over `options` (`(value, label)` pairs, which may change), showing `value` and
/// reporting the picked value to `on_change`.
#[component]
pub fn Select(
    #[prop(into)] value: Signal<String>,
    #[prop(into)] options: Signal<Vec<(String, String)>>,
    #[prop(into)] on_change: Callback<String>,
    /// `select-sm`.
    #[prop(optional)]
    small: bool,
    #[prop(into, default = "w-full".to_string())] class: String,
    #[prop(optional, into)] aria_label: Option<String>,
) -> impl IntoView {
    let size = if small { "select-sm " } else { "" };

    view! {
        <select
            class=format!("select {size}{class}")
            aria-label=aria_label
            on:change:target=move |ev| on_change.run(ev.target().value())
        >
            <For
                each=move || options.get()
                key=|option| option.clone()
                children=move |(option, label)| {
                    let option_for_selected = option.clone();
                    view! {
                        <option
                            value=option
                            prop:selected=move || value.with(|value| *value == option_for_selected)
                        >
                            {label}
                        </option>
                    }
                }
            />
        </select>
    }
}

/// A checkbox. `indeterminate` shows the "some" state (a heading over several items).
#[component]
pub fn Checkbox(
    #[prop(into)] checked: Signal<bool>,
    #[prop(into)] on_change: Callback<bool>,
    /// Its accessible name, for a checkbox without a visible `<label>`.
    #[prop(optional, into)]
    label: Option<String>,
    #[prop(optional, into)] indeterminate: MaybeProp<bool>,
    /// `checkbox-primary`.
    #[prop(optional)]
    primary: bool,
    /// `checkbox-sm`.
    #[prop(optional)]
    small: bool,
    /// Keeps its clicks from reaching an enclosing clickable element, such as a [`Collapse`]
    /// title it sits in.
    ///
    /// [`Collapse`]: super::Collapse
    #[prop(optional)]
    stop_click: bool,
) -> impl IntoView {
    let tone = if primary { " checkbox-primary" } else { "" };
    let size = if small { " checkbox-sm" } else { "" };

    view! {
        <input
            type="checkbox"
            class=format!("checkbox{tone}{size}")
            aria-label=label
            prop:checked=move || checked.get()
            prop:indeterminate=move || indeterminate.get().unwrap_or(false)
            on:click=move |ev| {
                if stop_click {
                    ev.stop_propagation();
                }
            }
            on:change:target=move |ev| on_change.run(ev.target().checked())
        />
    }
}
