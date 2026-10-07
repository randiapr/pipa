# export `.env` (the compose secrets, see `.env.example`) to every recipe, so `just docker-up` and
# friends pick up what's in it without sourcing it by hand; a missing `.env` is fine
set dotenv-load

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
    limit_kb=$(({{ target_limit_gb }} * 1024 * 1024))
    size_kb() { du -sk target | cut -f1; }
    gib() { awk -v kb="$1" 'BEGIN { printf "%.1f", kb / 1024 / 1024 }'; }
    used_kb=$(size_kb)
    [ "$used_kb" -le "$limit_kb" ] && exit 0
    echo "target/ is $(gib "$used_kb")GiB (limit {{ target_limit_gb }}GiB) — removing incremental caches"
    rm -rf target/*/incremental
    used_kb=$(size_kb)
    [ "$used_kb" -le "$limit_kb" ] && exit 0
    echo "still $(gib "$used_kb")GiB — running cargo clean"
    cargo clean

# show how big target/ is against the limit
target-size:
    @du -sh target 2>/dev/null || echo "target/ does not exist"
    @echo "limit: {{ target_limit_gb }}GiB"

# check native crates (backend, ingestion)
check: _target-guard
    cargo check {{ native }}

# check the ui crate against wasm32
check-ui: _target-guard
    cargo check -p pipa-ui --target wasm32-unknown-unknown

# check everything
check-all: check check-ui

# build native crates
build: _target-guard
    cargo build {{ native }}

# run a local RustFS server (S3 API on :9000, console on :9001, data under ./rustfs-data)
rustfs:
    # Credentials match the RUSTFS_ACCESS_KEY_ID/RUSTFS_SECRET_ACCESS_KEY defaults
    # `pipa-backend`'s `ObjectStoreConfig::from_env()` (and `pipa-ingestion`'s own duplicate of it)
    # use, so `just ingestion`/`just backend` connect with no extra setup. Still need the "pipa"
    # bucket created once — see `rustfs-init`, or just use `just local::up`, which does both
    # automatically.
    mkdir -p {{ rustfs_data }}
    rustfs server --console-enable --access-key rustfsadmin --secret-key rustfsadmin {{ rustfs_data }}

# one-time (idempotent) setup: point `rc` at the local RustFS and ensure the "pipa" bucket exists
rustfs-init:
    rc alias set pipa-local http://localhost:9000 rustfsadmin rustfsadmin
    rc bucket create pipa-local/pipa --ignore-existing

# run the CDC engine
ingestion: _target-guard
    cargo run -p pipa-ingestion

# run the backend API (serves on 0.0.0.0:8080, GET /healthz). Login needs a JWT secret and a
# first admin; these local-only defaults apply unless already set in the environment.
backend: _target-guard
    JWT_SECRET="${JWT_SECRET:-pipa-local-dev-secret-do-not-use-in-production}" \
    PIPA_ADMIN_USERNAME="${PIPA_ADMIN_USERNAME:-admin}" \
    PIPA_ADMIN_PASSWORD="${PIPA_ADMIN_PASSWORD:-admin-password}" \
    cargo run -p pipa-backend

# install crates/ui's prerequisites if missing (the wasm32 Rust target, daisyui's npm deps); a no-op otherwise
ui-deps:
    rustup target list --installed | grep -qx wasm32-unknown-unknown || rustup target add wasm32-unknown-unknown
    cd crates/ui && [ -d node_modules ] || npm install

# run the dashboard dev server
ui: _target-guard ui-deps
    cd crates/ui && trunk serve

# production build of the dashboard
build-ui: _target-guard ui-deps
    cd crates/ui && trunk build

# optimized build of the dashboard with the host's native Trunk, for the `ui` Docker image to package
build-ui-release: _target-guard ui-deps
    cd crates/ui && trunk build --release

# run tests for native crates
test: _target-guard
    cargo test {{ native }}

# format all crates
fmt:
    cargo fmt --all

# lint native crates
clippy: _target-guard
    cargo clippy {{ native }} -- -D warnings

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

# make sure the mandatory compose secrets exist (PIPA_JWT_SECRET, PIPA_ADMIN_PASSWORD; see
# docker-compose.yml): creates `.env` from `.env.example` if missing, then fills any of them that is
# neither exported in the shell nor already set in `.env` with a random value. Values you set
# yourself are never touched, so tokens survive restarts and the admin password stays stable.
_docker-env:
    #!/usr/bin/env bash
    set -euo pipefail

    command -v openssl >/dev/null || { echo "openssl is required to generate the secrets in .env" >&2; exit 1; }
    [ -f .env ] || { cp .env.example .env; chmod 600 .env; }

    # KEY's value in .env, without spaces or quotes ("" when missing or blank)
    env_value() { { grep -E "^$1=" .env || true; } | tail -n1 | cut -d= -f2- | tr -d " \"'"; }

    # set KEY=VALUE in .env, replacing an existing (e.g. blank) line in place, else appending
    set_env() {
        if grep -qE "^$1=" .env; then
            awk -v k="$1" -v v="$2" 'BEGIN { FS = OFS = "=" } $1 == k { print k "=" v; next } { print }' .env > .env.tmp
            mv .env.tmp .env
            chmod 600 .env
        else
            [ -z "$(tail -c1 .env)" ] || echo >> .env  # don't glue onto an unterminated last line
            printf '%s=%s\n' "$1" "$2" >> .env
        fi
    }

    # KEY's value in .env as compose reads it (one pair of surrounding quotes removed)
    raw_value() { { grep -E "^$1=" .env || true; } | tail -n1 | cut -d= -f2- | sed -e 's/^"\(.*\)"$/\1/' -e "s/^'\\(.*\\)'\$/\\1/"; }

    # A secret you set yourself is kept as-is, but pipa-backend exits at startup on a JWT secret
    # under 32 bytes while compose keeps the rest of the stack (and the login page) running —
    # so refuse it here rather than leave a dashboard nobody can sign in to.
    secret=${PIPA_JWT_SECRET:-$(raw_value PIPA_JWT_SECRET)}
    if [ -n "$secret" ] && [ "$(printf '%s' "$secret" | wc -c | tr -d ' ')" -lt 32 ]; then
        echo "PIPA_JWT_SECRET is only $(printf '%s' "$secret" | wc -c | tr -d ' ') bytes; pipa-backend needs at least 32." >&2
        echo "Set a longer one in .env (openssl rand -base64 48), or blank the value to have it generated." >&2
        exit 1
    fi

    # Likewise for the first admin's password: pipa-backend rejects one under 8 characters and exits.
    admin_password=${PIPA_ADMIN_PASSWORD:-$(raw_value PIPA_ADMIN_PASSWORD)}
    if [ -n "$admin_password" ] && [ "$(printf '%s' "$admin_password" | wc -c | tr -d ' ')" -lt 8 ]; then
        echo "PIPA_ADMIN_PASSWORD is only $(printf '%s' "$admin_password" | wc -c | tr -d ' ') characters; pipa-backend needs at least 8." >&2
        echo "Set a longer one in .env, or blank the value to have it generated." >&2
        exit 1
    fi

    password=""
    for key in PIPA_JWT_SECRET PIPA_ADMIN_PASSWORD; do
        [ -n "${!key:-}" ] && continue
        [ -n "$(env_value "$key")" ] && continue
        case "$key" in
            PIPA_JWT_SECRET) set_env "$key" "$(openssl rand -base64 48 | tr -d '\n')" ;;
            PIPA_ADMIN_PASSWORD) password=$(openssl rand -hex 16); set_env "$key" "$password" ;;
        esac
        echo "generated $key in .env"
    done

    if [ -n "$password" ]; then
        user=${PIPA_ADMIN_USERNAME:-$(env_value PIPA_ADMIN_USERNAME)}
        echo "first admin — username: ${user:-admin}  password: $password  (kept in .env)"
        echo "sign in at http://localhost:3000 once the stack is up"
    fi

# bring up the containerized stack (docker-compose.yml), building the dashboard on the host and images first; creates .env with random secrets on first run
docker-up: _docker-env build-ui-release
    {{ compose }} up --build

# `down` still interpolates docker-compose.yml, so the two required secrets get throwaway values
# in the recipes below (they never reach a running container) instead of demanding a real `.env`.

# tear down the containerized stack, leaving its volumes (rustfs-data, postgres-data) intact
docker-down:
    {{ compose }} down

# tear down the containerized stack AND delete its volumes — irreversible, wipes rustfs-data/postgres-data
docker-down-clean:
    {{ compose }} down --volumes
