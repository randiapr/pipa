# Local dev stack orchestration, invoked as `just local::up` (just's module
# separator is `::`, not `/` — there's no such thing as `just local/up`).

rustfs_data := "rustfs-data"
rustfs_alias := "pipa-local"
logs_dir := "logs"

# bring up rustfs, pipa-backend, pipa-ingestion, and the dashboard dev server, all
# detached in the background (ensuring the "pipa" bucket exists via `rc` first) — `up`
# returns once everything is listening rather than blocking the terminal. Each service's
# stdout/stderr goes to its own file under `{{logs_dir}}/`; tail them with
# `just local::log`, stop everything with `just local::down`.
up:
    #!/usr/bin/env bash
    set -euo pipefail

    wait_for_port() {
        local port="$1" tries=0
        until (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; do
            tries=$((tries + 1))
            if [ "$tries" -ge 60 ]; then
                echo "timed out waiting for port $port" >&2
                exit 1
            fi
            sleep 0.5
        done
        exec 3<&- 3>&-
    }

    mkdir -p {{rustfs_data}} {{logs_dir}}

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

    nohup ./target/debug/pipa-backend > {{logs_dir}}/backend.log 2>&1 &
    disown
    nohup ./target/debug/pipa-ingestion > {{logs_dir}}/ingestion.log 2>&1 &
    disown

    echo "waiting for pipa-backend on :8080..."
    wait_for_port 8080

    ( cd crates/ui
      [ -d node_modules ] || npm install
      nohup trunk serve > "../../{{logs_dir}}/ui.log" 2>&1 &
      disown )

    echo "waiting for dashboard on :3000..."
    wait_for_port 3000

    echo "local stack is up — logs: just local::log, stop: just local::down"

# tail every service's log from the background stack `up` started (Ctrl+C just stops
# tailing, it doesn't stop the services — use `just local::down` for that)
log:
    tail -n 100 -f {{logs_dir}}/*.log

# stop any local stack processes left running
down:
    #!/usr/bin/env bash
    set -uo pipefail

    patterns=(
        "rustfs server"
        "target/debug/pipa-backend"
        "target/debug/pipa-ingestion"
        "trunk serve"
    )

    stopped=0
    for pattern in "${patterns[@]}"; do
        if pkill -f "$pattern" 2>/dev/null; then
            echo "stopped: $pattern"
            stopped=1
        fi
    done

    [ "$stopped" -eq 0 ] && echo "nothing running"
    exit 0
