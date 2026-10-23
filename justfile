# export `.env` (the compose secrets, see `.env.example`) to every recipe, so `just docker-up` and
# friends pick up what's in it without sourcing it by hand; a missing `.env` is fine
set dotenv-load

# local dev stack orchestration (`just local::up` brings up everything at once)
mod local

# Flux GitOps on a local Kubernetes cluster (`just gitops::up`)
mod gitops

# native crates only (pipa-ui targets wasm32 and is excluded; pipa-api also builds for wasm32 via pipa-ui)
native := "-p pipa-api -p pipa-backend -p pipa-catalog-proxy -p pipa-ingestion"

# soft cap (GiB) on target/ — build recipes trim it back under this before compiling (see `_target-guard`)
target_limit_gb := "10"

# local RustFS storage volume, rooted in the project so it's easy to find/wipe (gitignored)
rustfs_data := "rustfs-data"

# local example Postgres cluster (brew's postgresql@18), rooted in the project like RustFS's volume (gitignored)
postgres_data := "postgres-data"

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

# check native crates (api, backend, catalog-proxy, ingestion)
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
run-rustfs:
    # Credentials match the RUSTFS_ACCESS_KEY_ID/RUSTFS_SECRET_ACCESS_KEY defaults
    # `pipa-backend`'s `ObjectStoreConfig::from_env()` (and `pipa-ingestion`'s own duplicate of it)
    # use, so `just run-ingestion`/`just run-backend` connect with no extra setup. Still need the "pipa"
    # bucket created once — see `rustfs-init`, or just use `just local::up`, which does both
    # automatically.
    mkdir -p {{ rustfs_data }}
    rustfs server --console-enable --access-key rustfsadmin --secret-key rustfsadmin {{ rustfs_data }}

# Same cluster as `just local::up` (./postgres-data; 127.0.0.1:5432, pipa/pipa, db testdb). wal_level=logical is what pipa-ingestion's replication slot needs.

# run an example Postgres source in the foreground, from brew's postgresql@18; Ctrl+C stops it
run-pg-native:
    #!/usr/bin/env bash
    set -euo pipefail
    pg_bin="$(brew --prefix postgresql@18)/bin"
    if "$pg_bin/pg_isready" -q -h 127.0.0.1 -p 5432; then
        echo "postgres is already listening on :5432 (is \`just local::up\` running?)" >&2
        exit 1
    fi
    pg_opts="-c wal_level=logical -c listen_addresses=127.0.0.1 -p 5432"
    if [ ! -f {{ postgres_data }}/PG_VERSION ]; then
        pwfile="$(mktemp)"; echo pipa > "$pwfile"
        "$pg_bin/initdb" -D {{ postgres_data }} -U pipa --pwfile="$pwfile" --auth=scram-sha-256 >/dev/null
        rm -f "$pwfile"
    fi
    # the server only runs in the foreground below, so testdb is created through a short-lived instance
    if [ ! -f {{ postgres_data }}/.testdb-created ]; then
        "$pg_bin/pg_ctl" -D {{ postgres_data }} -o "$pg_opts" -w -s start
        PGPASSWORD=pipa "$pg_bin/psql" -h 127.0.0.1 -U pipa -d postgres -tAc \
            "SELECT 1 FROM pg_database WHERE datname = 'testdb'" | grep -q 1 \
            || PGPASSWORD=pipa "$pg_bin/createdb" -h 127.0.0.1 -U pipa testdb
        "$pg_bin/pg_ctl" -D {{ postgres_data }} -m fast -w -s stop
        touch {{ postgres_data }}/.testdb-created
    fi
    exec "$pg_bin/postgres" -D {{ postgres_data }} $pg_opts

# one-time (idempotent) setup: point `rc` at the local RustFS and ensure the "pipa" bucket exists
rustfs-init:
    rc alias set pipa-local http://localhost:9000 rustfsadmin rustfsadmin
    rc bucket create pipa-local/pipa --ignore-existing

# run the CDC engine
run-ingestion: _target-guard
    cargo run -p pipa-ingestion

# run the backend API (serves on 0.0.0.0:8080, GET /healthz). Login needs a JWT secret and a
# first admin; these local-only defaults apply unless already set in the environment.
run-backend: _target-guard
    JWT_SECRET="${JWT_SECRET:-pipa-local-dev-secret-do-not-use-in-production}" \
    PIPA_ADMIN_USERNAME="${PIPA_ADMIN_USERNAME:-admin}" \
    PIPA_ADMIN_PASSWORD="${PIPA_ADMIN_PASSWORD:-admin-password}" \
    cargo run -p pipa-backend

# install crates/ui's prerequisites if missing (the wasm32 Rust target, daisyui's npm deps); a no-op otherwise
ui-deps:
    rustup target list --installed | grep -qx wasm32-unknown-unknown || rustup target add wasm32-unknown-unknown
    cd crates/ui && [ -d node_modules ] || npm install

# run the dashboard dev server
run-ui: _target-guard ui-deps
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

# end-to-end tests: the real pipa-backend and pipa-ingestion binaries against RustFS, Postgres and
# Redis in containers (testcontainers, so Docker must be running). Takes a few minutes; pass a
# filter to run some, e.g. `just test-integration cdc::` or `just test-integration login`
test-integration *filter: _target-guard
    #!/usr/bin/env bash
    set -euo pipefail
    # Colima, Rancher Desktop and rootless Docker keep their socket elsewhere than /var/run/docker.sock
    if [ -z "${DOCKER_HOST:-}" ] && [ ! -S /var/run/docker.sock ]; then
        host=$(docker context inspect --format '{{ "{{" }}.Endpoints.docker.Host}}' 2>/dev/null || true)
        [ -z "$host" ] || export DOCKER_HOST="$host"
    fi
    cargo test -p pipa-integration-test --test e2e -- {{ filter }}

# format all crates
fmt:
    cargo fmt --all

# lint native crates
clippy: _target-guard
    cargo clippy {{ native }} -p pipa-integration-test --all-targets -- -D warnings

# Only unit tests count towards coverage: the #[ignore]d live tests and pipa-integration-test need
# running services. Settings (excluded test-helper files) are in .cargo-crap.toml. Extra args go to cargo-crap,
# e.g. `just crap --threshold 20 --fail-above` (exit 1 when a function scores over the threshold, default 30).

# rank the native crates' functions by CRAP score, complexity x missing test coverage (requires cargo-llvm-cov and cargo-crap: cargo install cargo-llvm-cov cargo-crap --locked)
crap *args: _crap-lcov
    cargo crap {{ native }} --lcov target/crap/lcov.info {{ args }}

# fail if a function's CRAP score rose since crap-baseline.json; scores already over the threshold when it was saved don't fail, only getting worse does
crap-gate: _crap-lcov
    cargo crap {{ native }} --lcov target/crap/lcov.info --baseline crap-baseline.json --fail-regression

# save today's CRAP scores as crap-baseline.json (commit it); do this after paying scores down, so the gate ratchets
crap-baseline: _crap-lcov
    cargo crap {{ native }} --lcov target/crap/lcov.info --sort file --format json --output crap-baseline.json

# unit-test coverage of the native crates as LCOV, for the crap recipes
_crap-lcov: _target-guard
    #!/usr/bin/env bash
    set -euo pipefail
    # check before the (slow) coverage run, not after it
    for tool in llvm-cov crap; do
        cargo "$tool" --version >/dev/null 2>&1 || { echo "cargo-$tool is not installed (cargo install cargo-$tool --locked)" >&2; exit 1; }
    done
    mkdir -p target/crap
    cargo llvm-cov {{ native }} --lcov --output-path target/crap/lcov.info

# Slow: every mutant rebuilds and reruns the tests, so scope it, e.g. `just mutants -f crates/ingestion/src/capture/application.rs`,
# `just mutants -p pipa-ingestion`, or `just mutants --list` to only list the mutants. Extra args go to cargo-mutants; the
# report is in mutants.out/ (gitignored). Only unit tests run: the #[ignore]d live tests need running services.

# check that the tests catch injected bugs in the native crates; surviving mutants are untested behaviour (requires cargo-mutants: cargo install cargo-mutants --locked)
mutants *args: _target-guard
    cargo mutants {{ native }} {{ args }}

# find unused dependencies in every crate's Cargo.toml, no build needed; can false-positive on renamed or macro-only deps (requires cargo-machete: cargo install cargo-machete --locked)
machete *args:
    cargo machete {{ args }}

# check every workspace crate's direct dependencies for newer versions: `Compat` is the newest one the Cargo.toml requirement allows (minor/patch, what `cargo update` takes), `Latest` the newest published (including major bumps). Extra args go to cargo-outdated, e.g. `just outdated --exit-code 1` to fail when anything is outdated (requires cargo-outdated: cargo install cargo-outdated --locked; add --features vendored-openssl if OpenSSL isn't installed)
outdated *args:
    cargo outdated --workspace --root-deps-only {{ args }}

# audit Cargo.lock for known security advisories and upgrade the vulnerable dependencies it can (requires cargo-audit
# with the fix feature: cargo install cargo-audit --locked --features fix). Edits Cargo.toml for direct dependencies
# only; a transitive one needs `cargo update -p <crate>`. `just audit --dry-run` previews; plain `cargo audit` only reports.
audit *args:
    cargo audit fix {{ args }}

# check dependency licenses, bans and sources (permissive licenses only, no wildcard versions, crates.io only; see deny.toml; requires cargo-deny: cargo install cargo-deny --locked)
deny:
    cargo deny check licenses bans sources

# check that every Rust source file has the license header from .licenserc.yaml (installs license-eye with `go install` if missing, needs Go)
license: (_license-eye "header" "check")

# add the license header to files missing it (same tool as `just license`)
license-fix: (_license-eye "header" "fix")

# run license-eye (apache/skywalking-eyes), installing the latest with `go install` first if it isn't on PATH.
# CI pins v0.9.0 (.github/workflows/license.yaml); `@latest` here can be newer, so a disagreement
# between the two may just be a version difference.
_license-eye *args:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v license-eye >/dev/null 2>&1; then
        command -v go >/dev/null 2>&1 || { echo "license-eye is not installed and Go is needed to install it (https://go.dev/dl/)" >&2; exit 1; }
        echo "license-eye not found, installing it with go install"
        go install github.com/apache/skywalking-eyes/cmd/license-eye@latest
        # go install drops the binary in GOBIN, else GOPATH/bin, which may not be on PATH
        bin=$(go env GOBIN)
        [ -n "$bin" ] || bin=$(go env GOPATH)/bin
        export PATH="$bin:$PATH"
    fi
    license-eye {{ args }}

# upgrade Rust dependencies to the latest semver-compatible versions (requires cargo-edit: cargo install cargo-edit)
upgrade:
    cargo upgrade

# check for outdated npm deps in crates/ui (daisyui)
outdated-ui: ui-deps
    cd crates/ui && npm outdated

# upgrade npm deps in crates/ui to the latest version allowed by package.json
upgrade-ui: ui-deps
    cd crates/ui && npm update

# remove build artifacts (target/, crates/ui/dist/) and the local stack's logs (logs/) — leaves
# rustfs-data/ and node_modules/ alone. A running stack's logs go too (its services keep writing to
# the deleted files), except the pid file of a Redis `just local::up` started, which `just
# local::down` needs to stop it.
clean:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo clean
    rm -rf crates/ui/dist
    if [ -f logs/redis.pid ] && kill -0 "$(cat logs/redis.pid)" 2>/dev/null; then
        find logs -mindepth 1 ! -name redis.pid -delete
    else
        rm -rf logs
    fi

# resources for the Colima VM; match the existing `default` profile (Colima's own 2 CPU / 2 GiB is too small for this stack)
colima_cpus := "6"
colima_memory := "8"

# start Colima without Kubernetes for the containerized stack; runtime is docker (default) or containerd (no Docker socket, so docker-up and test-integration won't work), e.g. `just run-colima containerd`; extra args go to `colima start`. On a VM that is already running this does nothing: `just stop-colima` first to switch modes
run-colima runtime="docker" *args:
    colima start --runtime {{ runtime }} --cpu {{ colima_cpus }} --memory {{ colima_memory }} {{ args }}

# start Colima with its built-in k3s Kubernetes cluster; runtime is docker (default) or containerd, e.g. `just run-colima-k8s containerd`; extra args go to `colima start`, e.g. `--kubernetes-version v1.35.0+k3s1`. On a VM that is already running this does nothing: `just stop-colima` first to switch modes
run-colima-k8s runtime="docker" *args:
    colima start --kubernetes --runtime {{ runtime }} --cpu {{ colima_cpus }} --memory {{ colima_memory }} {{ args }}

# stop Colima (and Kubernetes, if it was started with it); the VM and its images are kept
stop-colima:
    colima stop

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

# bring up the containerized stack (docker-compose.yml) from the published ghcr.io images; creates .env with random secrets on first run
docker-up: _docker-env
    {{ compose }} up

# follow the containerized stack's logs (all services, or the ones named, e.g. `just docker-logs backend ingestion-0`); Ctrl+C stops following, not the stack
docker-logs *services:
    {{ compose }} logs --follow --tail 100 {{ services }}

# `down` still interpolates docker-compose.yml, so the two required secrets get throwaway values
# in the recipes below (they never reach a running container) instead of demanding a real `.env`.

# tear down the containerized stack, leaving its volumes (rustfs-data) intact
docker-down:
    {{ compose }} down

# tear down the containerized stack AND delete its volumes — irreversible, wipes rustfs-data
docker-down-clean:
    {{ compose }} down --volumes
