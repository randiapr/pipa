# local dev stack orchestration (`just local::up` brings up everything at once)
mod local

# native crates only (pipa-ui targets wasm32 and is excluded; pipa-api also builds for wasm32 via pipa-ui)
native := "-p pipa-api -p pipa-backend -p pipa-ingestion"

# soft cap (GiB) on target/ — build recipes trim it back under this before compiling (see `_target-guard`)
target_limit_gb := "10"

# local RustFS storage volume, rooted in the project so it's easy to find/wipe (gitignored)
rustfs_data := "rustfs-data"

# some machines only have the standalone `docker-compose` binary, not the `docker compose`
# CLI plugin (e.g. no ~/.docker/cli-plugins/docker-compose) — detect which works, once
compose := `docker compose version >/dev/null 2>&1 && echo "docker compose" || echo "docker-compose"`

# list available recipes
default:
    just --list

# keep target/ under `target_limit_gb`: first drop incremental caches (usually the biggest
# chunk, and cheap to regenerate), then fall back to a full `cargo clean` if still over.
# Runs as a dependency of every recipe that compiles, so target/ can't silently balloon.
_target-guard:
    #!/usr/bin/env bash
    set -euo pipefail
    [ -d target ] || exit 0
    limit_kb=$(({{target_limit_gb}} * 1024 * 1024))
    size_kb() { du -sk target | cut -f1; }
    gib() { awk -v kb="$1" 'BEGIN { printf "%.1f", kb / 1024 / 1024 }'; }
    used_kb=$(size_kb)
    [ "$used_kb" -le "$limit_kb" ] && exit 0
    echo "target/ is $(gib "$used_kb")GiB (limit {{target_limit_gb}}GiB) — removing incremental caches"
    rm -rf target/*/incremental
    used_kb=$(size_kb)
    [ "$used_kb" -le "$limit_kb" ] && exit 0
    echo "still $(gib "$used_kb")GiB — running cargo clean"
    cargo clean

# show how big target/ is against the limit
target-size:
    @du -sh target 2>/dev/null || echo "target/ does not exist"
    @echo "limit: {{target_limit_gb}}GiB"

# check native crates (backend, ingestion)
check: _target-guard
    cargo check {{native}}

# check the ui crate against wasm32
check-ui: _target-guard
    cargo check -p pipa-ui --target wasm32-unknown-unknown

# check everything
check-all: check check-ui

# build native crates
build: _target-guard
    cargo build {{native}}

# run a local RustFS server (S3 API on :9000, console on :9001, data under ./rustfs-data)
rustfs:
    # Credentials match the RUSTFS_ACCESS_KEY_ID/RUSTFS_SECRET_ACCESS_KEY defaults
    # `pipa-backend`'s `ObjectStoreConfig::from_env()` (and `pipa-ingestion`'s own duplicate of it)
    # use, so `just ingestion`/`just backend` connect with no extra setup. Still need the "pipa"
    # bucket created once — see `rustfs-init`, or just use `just local::up`, which does both
    # automatically.
    mkdir -p {{rustfs_data}}
    rustfs server --console-enable --access-key rustfsadmin --secret-key rustfsadmin {{rustfs_data}}

# one-time (idempotent) setup: point `rc` at the local RustFS and ensure the "pipa" bucket exists
rustfs-init:
    rc alias set pipa-local http://localhost:9000 rustfsadmin rustfsadmin
    rc bucket create pipa-local/pipa --ignore-existing

# run the CDC engine
ingestion: _target-guard
    cargo run -p pipa-ingestion

# run the backend API (serves on 0.0.0.0:8080, GET /healthz)
backend: _target-guard
    cargo run -p pipa-backend

# install crates/ui's npm deps (daisyui) if node_modules is missing; a no-op otherwise
ui-deps:
    cd crates/ui && [ -d node_modules ] || npm install

# run the dashboard dev server
ui: _target-guard ui-deps
    cd crates/ui && trunk serve

# production build of the dashboard
build-ui: _target-guard ui-deps
    cd crates/ui && trunk build

# run tests for native crates
test: _target-guard
    cargo test {{native}}

# format all crates
fmt:
    cargo fmt --all

# lint native crates
clippy: _target-guard
    cargo clippy {{native}} -- -D warnings

# check for outdated Rust dependencies across the whole workspace (requires cargo-outdated: cargo install cargo-outdated)
outdated:
    @DEPS=$(awk '/^\[/{s=$0} s~/\[workspace\.dependencies\]/ && /^[a-zA-Z]/{sub(/[[:space:]=].*/,""); print}' Cargo.toml | tr '\n' '|' | sed 's/|$//'); \
      cargo update --dry-run 2>&1 | grep -E " ($DEPS) " || echo "All direct dependencies are up to date"

# upgrade Rust dependencies to the latest semver-compatible versions (requires cargo-edit: cargo install cargo-edit)
upgrade:
    cargo upgrade

# check for outdated npm deps in crates/ui (daisyui)
outdated-ui: ui-deps
    cd crates/ui && npm outdated

# upgrade npm deps in crates/ui to the latest version allowed by package.json
upgrade-ui: ui-deps
    cd crates/ui && npm update

# remove build artifacts (target/, crates/ui/dist/) — leaves rustfs-data/ and node_modules/ alone
clean:
    cargo clean
    rm -rf crates/ui/dist

# bring up the containerized stack (docker-compose.yml), building images first
docker-up:
    {{compose}} up --build

# tear down the containerized stack, leaving its volumes (rustfs-data, postgres-data) intact
docker-down:
    {{compose}} down

# tear down the containerized stack AND delete its volumes — irreversible, wipes rustfs-data/postgres-data
docker-down-clean:
    {{compose}} down --volumes
