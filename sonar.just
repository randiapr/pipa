# SonarQube static analysis (docker-compose.sonar.yml), invoked as `just sonar::up`,
# `just sonar::scan`, ... Runs in Docker beside, not inside, the pipa stack.

compose_file := "docker-compose.sonar.yml"
sonar_url := "http://localhost:9100"

# native crates, as in the root justfile (module recipes can't see its variables)
native := "-p pipa-api -p pipa-backend -p pipa-catalog-proxy -p pipa-ingestion"

compose := `docker compose version >/dev/null 2>&1 && echo "docker compose" || echo "docker-compose"`

# start SonarQube (UI on http://localhost:9100) in the background and wait until it is ready; first start takes a minute or two
up:
    #!/usr/bin/env bash
    set -euo pipefail
    {{ compose }} -f {{ compose_file }} up -d sonarqube
    printf "waiting for SonarQube at {{ sonar_url }}"
    for _ in $(seq 1 180); do
        if curl -fs {{ sonar_url }}/api/system/status 2>/dev/null | grep -q '"status":"UP"'; then
            echo " — up"
            exit 0
        fi
        printf .
        sleep 2
    done
    echo
    echo "SonarQube did not come up within 6 minutes; see \`just sonar::logs\`" >&2
    exit 1

# analyze the workspace: Clippy (and coverage, with cargo-llvm-cov installed) on the host, then the scanner in Docker; results at http://localhost:9100/dashboard?id=pipa
scan: up _token
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/sonar

    # Clippy's JSON messages, which SonarQube imports as issues (sonar-project.properties). Unlike
    # `just clippy` there is no -D warnings: the warnings are what the report is for.
    cargo clippy {{ native }} -p pipa-integration-test --all-targets --message-format=json > target/sonar/clippy.json
    reports=target/sonar/clippy.json
    if rustup target list --installed | grep -qx wasm32-unknown-unknown; then
        cargo clippy -p pipa-ui --target wasm32-unknown-unknown --message-format=json > target/sonar/clippy-ui.json
        reports="$reports,target/sonar/clippy-ui.json"
    fi

    # Unit-test coverage of the native crates. The #[ignore]d live tests and pipa-integration-test
    # need running services, so they don't count towards it.
    if cargo llvm-cov --version >/dev/null 2>&1; then
        cargo llvm-cov {{ native }} --lcov --output-path target/sonar/lcov.info
    else
        rm -f target/sonar/lcov.info
        echo "cargo-llvm-cov not installed — analyzing without coverage (cargo install cargo-llvm-cov --locked)"
    fi

    token=$(grep -E '^SONAR_TOKEN=' .env | tail -n1 | cut -d= -f2-)
    PIPA_ROOT="{{ justfile_directory() }}" SONAR_TOKEN="$token" \
        {{ compose }} -f {{ compose_file }} run --rm scanner \
        -Dsonar.rust.clippyReport.reportPaths="$reports"
    echo "results: {{ sonar_url }}/dashboard?id=pipa"

# follow SonarQube's logs; Ctrl+C stops following, not SonarQube
logs:
    {{ compose }} -f {{ compose_file }} logs --follow --tail 100 sonarqube

# stop SonarQube, keeping its volumes (analysis history, admin password, token)
down:
    {{ compose }} -f {{ compose_file }} down

# stop SonarQube AND delete its volumes — irreversible; the next scan sets it up from scratch
down-clean:
    {{ compose }} -f {{ compose_file }} down --volumes

# make sure `.env` holds a working analysis token (SONAR_TOKEN). On a fresh SonarQube this replaces
# the default admin/admin password with SONAR_ADMIN_PASSWORD from `.env` (generated if blank) and
# creates the token as that admin; both stay in the gitignored `.env`.
_token:
    #!/usr/bin/env bash
    set -euo pipefail
    command -v openssl >/dev/null || { echo "openssl is required to generate the SonarQube admin password" >&2; exit 1; }
    [ -f .env ] || { cp .env.example .env; chmod 600 .env; }

    env_value() { { grep -E "^$1=" .env || true; } | tail -n1 | cut -d= -f2-; }
    set_env() {
        if grep -qE "^$1=" .env; then
            awk -v k="$1" -v v="$2" 'BEGIN { FS = OFS = "=" } $1 == k { print k "=" v; next } { print }' .env > .env.tmp
            mv .env.tmp .env
            chmod 600 .env
        else
            [ -z "$(tail -c1 .env)" ] || echo >> .env
            printf '%s=%s\n' "$1" "$2" >> .env
        fi
    }
    valid() { curl -fs -u "$1" {{ sonar_url }}/api/authentication/validate | grep -q '"valid":true'; }

    token=$(env_value SONAR_TOKEN)
    if [ -n "$token" ] && valid "$token:"; then exit 0; fi

    # SonarQube's password rules: 12+ characters with upper and lower case, a digit and a symbol.
    password=$(env_value SONAR_ADMIN_PASSWORD)
    if [ -z "$password" ]; then
        password="$(openssl rand -hex 12)Aa1!"
        set_env SONAR_ADMIN_PASSWORD "$password"
        echo "generated SONAR_ADMIN_PASSWORD in .env (SonarQube login: admin / $password)"
    fi
    if valid admin:admin; then
        curl -fs -u admin:admin -X POST {{ sonar_url }}/api/users/change_password \
            --data-urlencode login=admin --data-urlencode previousPassword=admin \
            --data-urlencode "password=$password"
    elif ! valid "admin:$password"; then
        echo "can't sign in to SonarQube as admin: set SONAR_ADMIN_PASSWORD in .env to its current password" >&2
        echo "(or start over with \`just sonar::down-clean\`)" >&2
        exit 1
    fi

    token=$(curl -fs -u "admin:$password" -X POST {{ sonar_url }}/api/user_tokens/generate \
        --data-urlencode "name=pipa-scan-$(date +%s)" | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')
    [ -n "$token" ] || { echo "SonarQube did not return a token" >&2; exit 1; }
    set_env SONAR_TOKEN "$token"
    echo "generated SONAR_TOKEN in .env"
