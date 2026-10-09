//! View: the sign-in page.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::components::{Alert, Card, Field, TextInput, Tone};
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
        <Card class="mx-auto w-fit shadow-xl" body_class="gap-4">
                <div>
                    <h1 class="text-2xl font-bold">"Sign in"</h1>
                    <p class="text-base-content/70">"Use your pipa account to continue."</p>
                </div>

                {move || {
                    session
                        .login_error
                        .get()
                        .map(|message| view! { <Alert tone=Tone::Error>{message}</Alert> })
                }}

                <form class="flex flex-col gap-4" on:submit=move |ev| session.login(ev)>
                    <Field legend="Username">
                        <TextInput value=session.username required=true autocomplete="username" />
                    </Field>
                    <Field legend="Password">
                        <TextInput
                            value=session.password
                            kind="password"
                            required=true
                            autocomplete="current-password"
                        />
                    </Field>
                    <button
                        class="btn btn-primary"
                        type="submit"
                        disabled=move || session.logging_in.get()
                    >
                        {move || if session.logging_in.get() { "Signing in…" } else { "Sign in" }}
                    </button>
                </form>
        </Card>
    }
}
