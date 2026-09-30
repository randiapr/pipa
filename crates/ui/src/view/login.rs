//! View: the sign-in page.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::viewmodel::SessionViewModel;

#[component]
pub fn Login() -> impl IntoView {
    let session = expect_context::<SessionViewModel>();

    // Already signed in (or just now became so): go to the app.
    let navigate = use_navigate();
    Effect::new(move |_| {
        if session.is_authenticated() {
            navigate("/", Default::default());
        }
    });

    view! {
        <div class="mx-auto flex w-full max-w-sm flex-col gap-4">
            <div>
                <h1 class="text-2xl font-bold">"Sign in"</h1>
                <p class="text-base-content/70">"Use your pipa account to continue."</p>
            </div>

            {move || {
                session
                    .login_error
                    .get()
                    .map(|message| {
                        view! {
                            <div role="alert" class="alert alert-error">
                                <span>{message}</span>
                            </div>
                        }
                    })
            }}

            <form class="flex flex-col gap-4" on:submit=move |ev| session.login(ev)>
                <fieldset class="fieldset">
                    <legend class="fieldset-legend">"Username"</legend>
                    <input
                        type="text"
                        class="input w-full"
                        required
                        autocomplete="username"
                        prop:value=move || session.username.get()
                        on:input:target=move |ev| session.username.set(ev.target().value())
                    />
                </fieldset>
                <fieldset class="fieldset">
                    <legend class="fieldset-legend">"Password"</legend>
                    <input
                        type="password"
                        class="input w-full"
                        required
                        autocomplete="current-password"
                        prop:value=move || session.password.get()
                        on:input:target=move |ev| session.password.set(ev.target().value())
                    />
                </fieldset>
                <button
                    class="btn btn-primary"
                    type="submit"
                    disabled=move || session.logging_in.get()
                >
                    {move || if session.logging_in.get() { "Signing in…" } else { "Sign in" }}
                </button>
            </form>
        </div>
    }
}
