# Local dev stack orchestration, invoked as `just local::up` (just's module
# separator is `::`, not `/` — there's no such thing as `just local/up`).

rustfs_data := "rustfs-data"
rustfs_alias := "pipa-local"

# bring up rustfs, pipa-rest, and pipa-core in the background (ensuring the "pipa"
# bucket exists via `rc` first), then run the dashboard dev server in the foreground.
# Ctrl+C stops everything. Logs from the background services interleave in this
# terminal since nothing is redirected — for isolated output use the individual
# `just rustfs` / `just rest` / `just core` / `just ui` recipes in separate terminals.
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

    cleaned_up=0
    cleanup() {
        [ "$cleaned_up" -eq 1 ] && return
        cleaned_up=1
        echo "stopping local stack..."
        kill "$rustfs_pid" "$rest_pid" "$core_pid" 2>/dev/null || true
        wait "$rustfs_pid" "$rest_pid" "$core_pid" 2>/dev/null || true
    }
    trap cleanup EXIT INT TERM

    mkdir -p {{rustfs_data}}
    rustfs server --console-enable --access-key rustfsadmin --secret-key rustfsadmin {{rustfs_data}} &
    rustfs_pid=$!

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
    cargo build -p pipa-rest -p pipa-core

    ./target/debug/pipa-rest &
    rest_pid=$!
    ./target/debug/pipa-core &
    core_pid=$!

    echo "waiting for pipa-rest on :8080..."
    wait_for_port 8080

    cd crates/ui
    [ -d node_modules ] || npm install
    trunk serve

# stop any local stack processes left running (fallback for when `up`'s Ctrl+C cleanup didn't run)
down:
    #!/usr/bin/env bash
    set -uo pipefail

    patterns=(
        "rustfs server"
        "target/debug/pipa-rest"
        "target/debug/pipa-core"
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
