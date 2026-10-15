//! `rstest` fixtures shared by the test modules.

use pipa_integration_test::env::TestEnv;
use pipa_integration_test::shared;
use rstest::fixture;

/// The shared backend (no ingestion): for tests that call the API with an admin and make their
/// own projects, accounts and sources under names of their own. Starting one per test would cost
/// seconds each.
#[fixture]
pub fn env() -> &'static TestEnv {
    shared::env()
}

/// A backend of the test's own on a fresh bucket, with the first admin signed in: for tests that
/// need a pristine store (the last admin), restart it or change its settings. Take it as
/// `#[future] own_env: TestEnv` and `.await` it.
#[fixture]
pub async fn own_env() -> TestEnv {
    TestEnv::new().await
}
