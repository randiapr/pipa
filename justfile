# local dev stack orchestration (`just local::up` brings up everything at once)
mod local

# native crates only (pipa-ui targets wasm32 and is excluded)
native := "-p pipa-core -p pipa-data -p pipa-rest"

# local RustFS storage volume, rooted in the project so it's easy to find/wipe (gitignored)
rustfs_data := "rustfs-data"

# list available recipes
default:
    just --list

# check native crates (core, data, rest)
check:
    cargo check {{native}}

# check the ui crate against wasm32
check-ui:
    cargo check -p pipa-ui --target wasm32-unknown-unknown

# check everything
check-all: check check-ui

# build native crates
build:
    cargo build {{native}}

# run a local RustFS server (S3 API on :9000, console on :9001, data under ./rustfs-data)
rustfs:
    # Credentials match the RUSTFS_ACCESS_KEY_ID/RUSTFS_SECRET_ACCESS_KEY defaults
    # `pipa-data::ObjectStoreConfig::from_env()` uses, so `just core`/`just rest` connect
    # with no extra setup. Still need the "pipa" bucket created once — see `rustfs-init`,
    # or just use `just local::up`, which does both automatically.
    mkdir -p {{rustfs_data}}
    rustfs server --console-enable --access-key rustfsadmin --secret-key rustfsadmin {{rustfs_data}}

# one-time (idempotent) setup: point `rc` at the local RustFS and ensure the "pipa" bucket exists
rustfs-init:
    rc alias set pipa-local http://localhost:9000 rustfsadmin rustfsadmin
    rc bucket create pipa-local/pipa --ignore-existing

# run the CDC engine
core:
    cargo run -p pipa-core

# run the REST API (serves on 0.0.0.0:8080, GET /healthz)
rest:
    cargo run -p pipa-rest

# install crates/ui's npm deps (daisyui) if node_modules is missing; a no-op otherwise
ui-deps:
    cd crates/ui && [ -d node_modules ] || npm install

# run the dashboard dev server
ui: ui-deps
    cd crates/ui && trunk serve

# production build of the dashboard
build-ui: ui-deps
    cd crates/ui && trunk build

# run tests for native crates
test:
    cargo test {{native}}

# format all crates
fmt:
    cargo fmt --all

# lint native crates
clippy:
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
