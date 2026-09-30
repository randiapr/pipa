//! Thin wrapper over the browser's `localStorage`, for the few things that must survive a page
//! reload: the login token and the selected project. Every call quietly does nothing (or returns
//! `None`) when storage is unavailable, e.g. in a private window that blocks it.

/// Key under which the bearer token from login is kept.
pub const TOKEN_KEY: &str = "pipa.token";
/// Key under which the selected project's id is kept.
pub const PROJECT_KEY: &str = "pipa.project";

fn local() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

pub fn get(key: &str) -> Option<String> {
    local()?.get_item(key).ok()?
}

pub fn set(key: &str, value: &str) {
    if let Some(storage) = local() {
        let _ = storage.set_item(key, value);
    }
}

pub fn remove(key: &str) {
    if let Some(storage) = local() {
        let _ = storage.remove_item(key);
    }
}
