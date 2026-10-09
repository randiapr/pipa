# Local dev stack orchestration, invoked as `just local::up` (just's module
# separator is `::`, not `/` — there's no such thing as `just local/up`).

rustfs_data := "rustfs-data"
rustfs_alias := "pipa-local"
postgres_data := "postgres-data"
logs_dir := "logs"

# bring up an example Postgres source (brew), rustfs, pipa-backend, pipa-ingestion, and the dashboard dev server, all detached in
# the background (ensuring the "pipa" bucket exists via `rc` first) — `up` returns once
# everything is listening rather than blocking the terminal. Each service's
# stdout/stderr goes to its own file under `{{logs_dir}}/`; tail them with
# `just local::log`, stop everything with `just local::down`.
up:
    #!/usr/bin/env bash
    set -euo pipefail

    wait_for_port() {
        local port="$1" max_tries="${2:-60}" tries=0
        until (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; do
            tries=$((tries + 1))
            if [ "$tries" -ge "$max_tries" ]; then
                echo "timed out waiting for port $port" >&2
                exit 1
            fi
            sleep 0.5
        done
        exec 3<&- 3>&-
    }

    mkdir -p {{rustfs_data}} {{logs_dir}}

    # Example OLTP source for local CDC testing, from brew's postgresql@18 (same user/password/db
    # as docker-compose's example-source). Its cluster lives in {{postgres_data}}/, initialised on
    # first run; wal_level=logical is what PostgresWalSource's replication slot needs.
    pg_bin="$(brew --prefix postgresql@18)/bin"
    if [ ! -f {{postgres_data}}/PG_VERSION ]; then
        pwfile="$(mktemp)"; echo pipa > "$pwfile"
        "$pg_bin/initdb" -D {{postgres_data}} -U pipa --pwfile="$pwfile" --auth=scram-sha-256 >/dev/null
        rm -f "$pwfile"
    fi
    if ! "$pg_bin/pg_isready" -q -h 127.0.0.1 -p 5432; then
        "$pg_bin/pg_ctl" -D {{postgres_data}} -l {{logs_dir}}/postgres.log \
            -o "-c wal_level=logical -c listen_addresses=127.0.0.1 -p 5432" -w start
    fi
    PGPASSWORD=pipa "$pg_bin/psql" -h 127.0.0.1 -U pipa -d postgres -tAc \
        "SELECT 1 FROM pg_database WHERE datname = 'testdb'" | grep -q 1 \
        || PGPASSWORD=pipa "$pg_bin/createdb" -h 127.0.0.1 -U pipa testdb
    echo "postgres ready on :5432 (pipa/pipa, db testdb)"

    # `nohup ... &` + `disown` detaches each service from this script's shell, so it
    # keeps running (logging to its own file) after `up` itself exits — `nohup` stops it
    # getting SIGHUP when the shell exits, `disown` drops it from the job table so the
    # shell doesn't wait on it.
    nohup rustfs server --console-enable --access-key rustfsadmin --secret-key rustfsadmin {{rustfs_data}} \
        > {{logs_dir}}/rustfs.log 2>&1 &
    disown

    echo "waiting for rustfs on :9000..."
    wait_for_port 9000

    rc alias set {{rustfs_alias}} http://localhost:9000 rustfsadmin rustfsadmin >/dev/null
    rc bucket create {{rustfs_alias}}/pipa --ignore-existing >/dev/null
    echo "rustfs ready, \"pipa\" bucket ensured"

    # build first, together, so a cold compile (datafusion/sqlx/parquet etc. can take
    # minutes) shows normal cargo progress instead of silently eating the wait_for_port
    # budget below. Then exec the built binaries directly rather than `cargo run`ning
    # each -p separately: two concurrent single-package `cargo run` invocations resolve
    # workspace feature unification differently than this combined build, which
    # invalidates and recompiles shared deps all over again.
    cargo build -p pipa-backend -p pipa-ingestion

    # Local-only login defaults (like the rustfsadmin credentials above), overridable from the
    # environment: the backend refuses to start without a JWT secret and a first admin.
    export JWT_SECRET="${JWT_SECRET:-pipa-local-dev-secret-do-not-use-in-production}"
    export PIPA_ADMIN_USERNAME="${PIPA_ADMIN_USERNAME:-admin}"
    export PIPA_ADMIN_PASSWORD="${PIPA_ADMIN_PASSWORD:-admin-password}"

    # RustFS's Iceberg REST catalog rejects unsigned requests with 403 and `iceberg-catalog-rest`
    # can't sign, so the backend and ingestion each embed the pipa-catalog-proxy signer on a
    # loopback port of their own process (ICEBERG_CATALOG_URI unset selects it), which also
    # enables S3 Tables on the bucket at startup.
    nohup ./target/debug/pipa-backend > {{logs_dir}}/backend.log 2>&1 &
    disown
    nohup ./target/debug/pipa-ingestion > {{logs_dir}}/ingestion.log 2>&1 &
    disown

    echo "waiting for pipa-backend on :8080..."
    wait_for_port 8080

    # trunk's wasm build fails (yet it still opens :3000) without the wasm32 target
    rustup target list --installed | grep -qx wasm32-unknown-unknown || rustup target add wasm32-unknown-unknown

    ( cd crates/ui
      [ -d node_modules ] || npm install
      nohup trunk serve > "../../{{logs_dir}}/ui.log" 2>&1 &
      disown )

    # `trunk serve` only starts listening once its first wasm build finishes, and a cold
    # one (empty target/wasm32-unknown-unknown) takes minutes — allow up to 10 minutes.
    echo "waiting for dashboard on :3000 (first wasm build can take a few minutes)..."
    wait_for_port 3000 1200

    echo "local stack is up — sign in at http://localhost:3000 as $PIPA_ADMIN_USERNAME (first run only creates it)"
    echo "logs: just local::log, stop: just local::down"

# tail every service's log from the background stack `up` started (Ctrl+C just stops
# tailing, it doesn't stop the services — use `just local::down` for that)
log:
    tail -n 100 -f {{logs_dir}}/*.log

# stop any local stack processes left running
down:
    #!/usr/bin/env bash
    set -uo pipefail

    pg_bin="$(brew --prefix postgresql@18 2>/dev/null)/bin"
    if [ -f {{postgres_data}}/PG_VERSION ] && "$pg_bin/pg_ctl" -D {{postgres_data}} status >/dev/null 2>&1; then
        "$pg_bin/pg_ctl" -D {{postgres_data}} -m fast stop >/dev/null && echo "stopped: postgres"
        stopped_pg=1
    fi

    patterns=(
        "rustfs server"
        "target/debug/pipa-backend"
        "target/debug/pipa-ingestion"
        "trunk serve"
    )

    stopped=${stopped_pg:-0}
    for pattern in "${patterns[@]}"; do
        if pkill -f "$pattern" 2>/dev/null; then
            echo "stopped: $pattern"
            stopped=1
        fi
    done

    [ "$stopped" -eq 0 ] && echo "nothing running"
    exit 0
