// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! One backend and one ingestion instance for the many tests that don't need a backend of their
//! own.
//!
//! Starting a backend costs seconds, and most tests need no more than an admin to call the API as
//! and a project, accounts and data sources of their own, which they create under names nobody
//! else uses. Those tests share [`stack`]: one bucket, one backend, one ingestion instance
//! capturing every source the tests register. Tests that only call the API share [`env`], a
//! backend of its own without ingestion, so the sources they register (many of them unreachable
//! on purpose) are not ingestion's business. A test that needs a pristine store (the last
//! admin, restarts, startup checks), its own configuration (cache, Redis), or to stop and crash
//! ingestion starts its own, see [`crate::env::TestEnv::new`] and [`crate::pipeline::Pipeline`].
//!
//! The shared stack lives on a runtime of its own that is never shut down, so tests running on
//! their own runtimes can use it; its processes are killed when the test process exits.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::Runtime;

use crate::env::TestEnv;
use crate::ingestion::Ingestion;

/// The runtime the shared things were started on and run on.
fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

/// Runs `init` on the shared runtime, once, and keeps its result for the rest of the process.
/// Blocks the calling test (and every other that asks meanwhile) until it is done; a failure is
/// kept too, so each test fails at once with the same message.
pub fn get_or_init<T, Fut>(cell: &'static OnceLock<Result<T, String>>, init: Fut) -> &'static T
where
    T: Send + Sync + 'static,
    Fut: Future<Output = T> + Send + 'static,
{
    cell.get_or_init(|| {
        let (sender, receiver) = std::sync::mpsc::channel();
        runtime().spawn(async move {
            let outcome = tokio::spawn(init).await.map_err(|err| {
                format!("starting the shared stack failed: {err}; see the output above")
            });
            let _ = sender.send(outcome);
        });
        receiver.recv().expect("the shared runtime died")
    })
    .as_ref()
    .unwrap_or_else(|err| panic!("{err}"))
}

pub struct Stack {
    /// A backend on a bucket of its own, with the first admin signed in.
    pub env: TestEnv,
    /// Captures the sources of every test that uses [`stack`].
    pub ingestion: Ingestion,
}

/// A shared backend with no ingestion, for tests that only call the API. Registering a data
/// source there starts nothing.
pub fn env() -> &'static TestEnv {
    static API: OnceLock<Result<TestEnv, String>> = OnceLock::new();
    get_or_init(&API, TestEnv::new())
}

/// The shared backend and ingestion instance, started on first use.
pub fn stack() -> &'static Stack {
    static STACK: OnceLock<Result<Stack, String>> = OnceLock::new();
    get_or_init(&STACK, async {
        let env = TestEnv::new().await;
        let ingestion = env.ingestion().await;
        Stack { env, ingestion }
    })
}

/// [`stack`]'s backend.
pub fn env_with_ingestion() -> &'static TestEnv {
    &stack().env
}
