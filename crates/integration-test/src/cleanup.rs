//! Removing what the tests started when the test process exits.
//!
//! The built-in test harness runs nothing after the last test, and statics are never dropped, so
//! whatever outlives a single test (the containers, and the processes of the shared stack, see
//! [`crate::shared`]) is removed by an `atexit` hook.

use std::collections::BTreeSet;
use std::sync::{Mutex, Once};

static CONTAINER_IDS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static LIVE_PIDS: Mutex<BTreeSet<u32>> = Mutex::new(BTreeSet::new());
static HOOK: Once = Once::new();

unsafe extern "C" {
    fn atexit(callback: extern "C" fn()) -> i32;
}

extern "C" fn clean_up() {
    let pids: Vec<String> = LIVE_PIDS
        .lock()
        .map(|pids| pids.iter().map(u32::to_string).collect())
        .unwrap_or_default();
    if !pids.is_empty() {
        let _ = std::process::Command::new("kill")
            .arg("-KILL")
            .args(pids)
            .stderr(std::process::Stdio::null())
            .status();
    }
    let ids = CONTAINER_IDS
        .lock()
        .map(|ids| ids.clone())
        .unwrap_or_default();
    if !ids.is_empty() {
        let _ = std::process::Command::new("docker")
            .args(["rm", "--force", "--volumes"])
            .args(ids)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

fn ensure_hook() {
    HOOK.call_once(|| {
        // SAFETY: `clean_up` is a plain `extern "C"` function that only reads two static
        // mutexes and runs child processes.
        unsafe {
            atexit(clean_up);
        }
    });
}

pub fn remove_containers_at_exit(ids: impl Iterator<Item = String>) {
    ensure_hook();
    CONTAINER_IDS.lock().expect("container ids").extend(ids);
}

/// Kills `pid` at exit unless [`forget_process`] says it is gone by then.
pub fn kill_process_at_exit(pid: u32) {
    ensure_hook();
    LIVE_PIDS.lock().expect("pids").insert(pid);
}

pub fn forget_process(pid: u32) {
    LIVE_PIDS.lock().expect("pids").remove(&pid);
}
