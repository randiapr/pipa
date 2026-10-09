//! daisyUI `modal`, on the native `<dialog>` element (`showModal()`/`close()`): [`Modal`], its
//! [`ModalActions`] row, and [`ConfirmDialog`], which every update and delete goes through.
//! A modal opened from inside another one (a confirmation over an edit form) stacks on top of
//! it; closing the top one leaves the other as it was.

use leptos::html;
use leptos::prelude::*;

/// Shows `dialog` as a modal.
pub fn open_dialog(dialog: NodeRef<html::Dialog>) {
    if let Some(dialog) = dialog.get_untracked() {
        let _ = dialog.show_modal();
    }
}

/// Closes `dialog`.
pub fn close_dialog(dialog: NodeRef<html::Dialog>) {
    if let Some(dialog) = dialog.get_untracked() {
        dialog.close();
    }
}

/// A modal box with a title and a ✕ in the corner. Open it with [`open_dialog`] on `node_ref`.
#[component]
pub fn Modal(
    node_ref: NodeRef<html::Dialog>,
    #[prop(into)] title: TextProp,
    /// Runs on every close, whatever closed it (a button, Esc, [`close_dialog`]) — the place to
    /// reset a form.
    #[prop(optional, into)]
    on_close: Option<Callback<()>>,
    children: Children,
) -> impl IntoView {
    view! {
        <dialog
            node_ref=node_ref
            class="modal"
            on:close=move |_| {
                if let Some(on_close) = on_close {
                    on_close.run(());
                }
            }
        >
            <div class="modal-box max-h-[85vh] overflow-y-auto">
                <button
                    class="btn btn-sm btn-circle btn-ghost absolute right-2 top-2"
                    type="button"
                    aria-label="Close"
                    on:click=move |_| close_dialog(node_ref)
                >
                    "✕"
                </button>
                <h3 class="text-lg font-bold">{move || title.get()}</h3>
                {children()}
            </div>
        </dialog>
    }
}

/// The button row at the bottom of a [`Modal`]: a Cancel button closing `node_ref`, then
/// `children` (the confirming button — `type="submit"` inside a form).
#[component]
pub fn ModalActions(node_ref: NodeRef<html::Dialog>, children: Children) -> impl IntoView {
    view! {
        <div class="modal-action">
            <button class="btn" type="button" on:click=move |_| close_dialog(node_ref)>
                "Cancel"
            </button>
            {children()}
        </div>
    }
}

/// Asks before a change is made: `message` says what is about to happen, the confirming
/// button runs `on_confirm` and closes the dialog, Cancel just closes it.
#[component]
pub fn ConfirmDialog(
    node_ref: NodeRef<html::Dialog>,
    #[prop(into)] title: TextProp,
    /// Read when the dialog renders, so it can name the target and the consequences.
    #[prop(into)]
    message: TextProp,
    /// The confirming button's label, e.g. "Delete" or "Save".
    #[prop(into)]
    confirm_label: TextProp,
    /// Styles the confirming button as destructive (deletes, discards).
    #[prop(optional)]
    danger: bool,
    #[prop(into)] on_confirm: Callback<()>,
) -> impl IntoView {
    let confirm_class = if danger {
        "btn btn-error"
    } else {
        "btn btn-primary"
    };

    view! {
        <Modal node_ref=node_ref title=title>
            <p class="py-4">{move || message.get()}</p>
            <ModalActions node_ref=node_ref>
                <button
                    class=confirm_class
                    type="button"
                    on:click=move |_| {
                        on_confirm.run(());
                        close_dialog(node_ref);
                    }
                >
                    {move || confirm_label.get()}
                </button>
            </ModalActions>
        </Modal>
    }
}
