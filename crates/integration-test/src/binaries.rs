// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! The `pipa-backend` and `pipa-ingestion` executables under test.
//!
//! Cargo only provides a test with the binaries of its own package, so they are built on first
//! use with the same toolchain and profile the tests were built with (a no-op when up to date) and
//! found in the workspace's target directory. Set `PIPA_BACKEND_BIN` / `PIPA_INGESTION_BIN` to
//! test prebuilt ones instead; nothing is built then.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Once;

pub const BACKEND: &str = "pipa-backend";
pub const INGESTION: &str = "pipa-ingestion";

/// Path of the executable of workspace package `package`, building it first if needed.
pub fn binary(package: &str) -> PathBuf {
    let override_var = format!(
        "PIPA_{}_BIN",
        package
            .trim_start_matches("pipa-")
            .to_ascii_uppercase()
            .replace('-', "_")
    );
    if let Ok(path) = std::env::var(&override_var) {
        return PathBuf::from(path);
    }

    static BUILD: Once = Once::new();
    BUILD.call_once(|| {
        let mut build = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        build
            .current_dir(workspace_root())
            .args(["build", "-p", BACKEND, "-p", INGESTION]);
        if !cfg!(debug_assertions) {
            build.arg("--release");
        }
        let status = build.status().expect("running cargo build");
        assert!(
            status.success(),
            "building {BACKEND} and {INGESTION} failed"
        );
    });

    let path = target_dir()
        .join(if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        })
        .join(package);
    assert!(path.exists(), "{} was not built", path.display());
    path
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

fn target_dir() -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => workspace_root().join("target"),
    }
}
