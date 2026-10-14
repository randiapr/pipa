# pipa

Change Data Capture (CDC) tool that streams changes from OLTP databases into Apache
Iceberg. Written in Rust, built on Apache DataFusion (query engine) and RustFS
(S3-compatible object storage).

Supported source databases: PostgreSQL (via logical replication/WAL). MySQL (binlog) is
planned but not implemented yet.

## Workspace

A Cargo workspace of five crates, each versioned independently (see each crate's own
`CHANGELOG.md`):

- **`pipa-ingestion`** — the CDC engine. Streams row-level changes out of a registered
  Postgres source via logical replication and appends them to Iceberg tables as changelog
  rows (`_op`, `_position`, ... plus the source columns), with effectively-once delivery: the
  checkpoint is committed in the same Iceberg snapshot as the data. A standalone service with
  no dependency on `pipa-backend` (only on the embedded `pipa-catalog-proxy` signer) — it reads
  registered sources directly from the shared
  object store, and is designed to run distributed: with `REDIS_URL` set, instances share the sources through
  Redis and a stopped one's sources move to the others; without it, `INGESTION_SHARD_INDEX`/
  `INGESTION_SHARD_COUNT` split sources statically.
- **`pipa-backend`** — the HTTP API: accounts and sign-in (admin/developer/user roles), projects, read-only table browsing, OLTP
  data source management (registering/testing sources, over the shared RustFS/S3 object
  store), plus Apache Iceberg query access (`POST /query`: REST catalog client, DataFusion SQL,
  scoped to the caller's project), whose results — and the table listing — are cached for a short
  TTL in memory, or in Redis with `REDIS_URL` set. Every external caller (the dashboard, any future client)
  talks to this, never to `pipa-ingestion` directly.
- **`pipa-catalog-proxy`** — SigV4 signing for RustFS's embedded Iceberg REST catalog, which
  rejects unsigned requests (`iceberg-catalog-rest` can't sign). A library that `pipa-backend`
  and `pipa-ingestion` embed in their own process, so catalog calls need no separate service or
  network hop, plus a binary that runs the same proxy standalone for clients outside this
  workspace. The signing is implemented in-house on `hmac`/`sha2`; there is no AWS dependency.
  Not an external-facing service.
- **`pipa-api`** — the HTTP contract (request/response types, routes) shared by `pipa-backend`
  and `pipa-ui`; builds for both native and wasm32.
- **`pipa-ui`** — a Leptos dashboard (Tailwind CSS v4 + daisyUI) for signing in and managing
  projects, OLTP data sources, users and queries.

## Prerequisites

- Rust (stable) with the `wasm32-unknown-unknown` target: `rustup target add wasm32-unknown-unknown`
- [`just`](https://github.com/casey/just)
- [`trunk`](https://trunkrs.dev): `cargo install trunk`
- [`rustfs`](https://rustfs.com) and its `rc` CLI, for local S3-compatible object storage
- [`PostgreSQL 18`](https://www.postgresql.org/download) — `just local::up` runs it as the example source database
- [`Redis`](https://redis.io/docs/latest/operate/oss_and_stack/install/) (`redis-server`/`redis-cli` on `PATH`) — `just local::up` starts it for ingestion's coordination and the backend's query cache
- Node.js/npm — only used to resolve daisyUI as a Tailwind plugin (`crates/ui/package.json`); no JS build step otherwise

## Quickstart

```sh
just local::up      # example Postgres + redis + rustfs + pipa-backend + pipa-ingestion + dashboard, all in the background
```

Open `http://localhost:3000` for the dashboard and `http://localhost:8080/healthz` for the
API. `just local::log` tails every service's output; `just local::down` stops everything.

`just local::up` also starts an example Postgres source to register as a data source:
`localhost:5432`, user/password `pipa`, database `testdb`, `wal_level=logical`. Its cluster lives in
the gitignored `postgres-data/`, initialized on the first run; if something already listens on
`:5432`, that server is used instead and must have `wal_level=logical` itself.

A newly registered source captures **no tables**: open it in the dashboard (its explorer page lists
the source's tables live) and choose the tables to capture. `pipa-ingestion` picks the selection up
on its next read of the registered sources (every 30 seconds by default), and the captured tables
then show up on the Tables page.

### Registering your own Postgres source

`pipa-ingestion` creates the replication objects itself on first use of a source: a `FOR ALL TABLES`
publication and a `pgoutput` logical replication slot, both named `pipa_cdc_{source_id}` (the id
without dashes) and created only if missing. You don't run `CREATE PUBLICATION` or
`pg_create_logical_replication_slot` by hand. What it cannot do for you:

- **`wal_level=logical`** on the server (needs a restart; managed services usually expose it as a
  parameter such as `rds.logical_replication`).
- **A role that may create and stream from them.** The registered user needs the `REPLICATION`
  attribute (or superuser) for the slot, and creating a `FOR ALL TABLES` publication normally needs
  superuser. On managed Postgres, grant the provider's replication role (e.g. `rds_replication`) and
  create the publication yourself under that name if the role cannot (creation then reports an
  existing object and is skipped). Without these, the source fails with a
  "failed to prepare replication objects" error in the `pipa-ingestion` log.
- **A `pg_hba.conf` entry** that lets that role open a `replication` connection from where
  `pipa-ingestion` runs.
- **Free slots and senders:** each source takes one slot and one WAL sender, so keep
  `max_replication_slots` and `max_wal_senders` above the number of sources.

The publication covers every table; the table selection is applied by `pipa-ingestion`, which
drops changes to unselected tables as they arrive.

With `just local::up`, sign in as the first admin, `admin` / `admin-password` by default (local-only defaults;
override with `PIPA_ADMIN_USERNAME`/`PIPA_ADMIN_PASSWORD`, and `JWT_SECRET` for the token
secret). Admins create further accounts on the Users page and assign each non-admin account the
projects it may access. Roles:

- **admin**: everything, including the Users page and creating projects.
- **developer**: everything inside its assigned projects (sources, free SQL queries) except user
  and project management; it never sees the Users menu.
- **user**: view-only. It can browse the Iceberg tables of its projects (paged, no SQL) but cannot run
  queries or see data sources.

Alternatively, containerized: RustFS, Redis, `pipa-backend`, two `pipa-ingestion`
instances, the dashboard, and an example Postgres source (`localhost:5432`, user/password `pipa`,
database `testdb`, `wal_level=logical`) to register as a data source:

```sh
just docker-up   # first run creates .env with a random JWT secret and admin password
just docker-logs # follow the stack's logs (or name services: just docker-logs backend ingestion-0)
just docker-down # stop it, keeping the data volumes (docker-down-clean also deletes them)
```

The dashboard is a static SPA built on the host with Trunk, which `just docker-up` does for you
(plain `docker compose up --build` needs `just build-ui-release` run first). It is served at
`http://localhost:3000` and the API at `http://localhost:8080`.

`just docker-up` does not use the local defaults above: it prints the generated admin login once (it stays in the gitignored `.env`, which
you can edit; the password needs at least 8 characters). Without `just`, `cp .env.example .env`, set `PIPA_JWT_SECRET` and
`PIPA_ADMIN_PASSWORD`, then `docker compose up --build`.

To scale ingestion out, copy the `ingestion-1` service in `docker-compose.yml` as `ingestion-2` with
its own unique `INGESTION_INSTANCE_ID` and `docker compose up -d ingestion-2`: within a few
seconds about a third of the sources move to it (`docker compose exec redis redis-cli --scan --pattern 'pipa:ingestion:*'`
shows the members and each source's lease). Every instance captures a source through its own
replication slot position, so a handoff loses and duplicates nothing.

### Published images

When a release is published on GitHub, `.github/workflows/publish-images.yaml` pushes
`ghcr.io/<owner>/pipa-backend`, `pipa-ingestion` and `pipa-ui` (linux/amd64), each tagged with the
release's semver (`X.Y.Z`, `X.Y`, and `X` from 1.0 on), `sha-<short>` and `latest`. Merges to `main`
don't publish. The `pipa-ui` image has the API address `http://localhost:8080` compiled in (like the
local stack), so it only works as shipped next to a backend published on that address.

`POST /query` and `pipa-ingestion` talk to RustFS's own embedded Iceberg REST Catalog ("S3 Tables"
feature). That catalog requires SigV4-signed requests, which `iceberg-catalog-rest` can't send, so
both embed the `pipa-catalog-proxy` signer (`crates/catalog-proxy`) in their own process and enable
S3 Tables on the `pipa` bucket at startup — all automatic, locally and in compose, as long as
`ICEBERG_CATALOG_URI` is left unset (see the note at the top of `docker-compose.yml`).

## Commands

```sh
just check-all    # cargo check, native crates + pipa-ui (wasm32)
just test         # run tests for native crates
just clippy       # lint native crates
just fmt          # format all crates
just clean        # remove build artifacts (target/, crates/ui/dist/)

just rustfs       # run local RustFS alone (S3 API :9000, console :9001)
just rustfs-init  # one-time: configure `rc` and ensure the "pipa" bucket exists
just backend      # run pipa-backend alone (0.0.0.0:8080)
just ingestion    # run pipa-ingestion alone
just ui           # run the dashboard dev server (trunk serve, :3000)
just build-ui     # production build of the dashboard
```

Run `just --list` for the full list (`docker-up`/`docker-down`, `outdated`, `upgrade`, ...).

Environment variables: the compose secrets are in the root `.env.example`, and `pipa-backend`,
`pipa-ingestion` and `pipa-ui` each have their own `crates/*/.env.example`. The standalone
`pipa-catalog-proxy` binary's are listed at the top of `crates/catalog-proxy/src/main.rs`.

Optional runtime settings (all set in compose and by `just local::up` where it matters):

| Variable | Used by | Effect |
|---|---|---|
| `REDIS_URL` | backend, ingestion | Backend: results cache in Redis, shared by all instances (unset: in-process memory). Ingestion: instances discover each other and share sources (unset: static `INGESTION_SHARD_INDEX`/`INGESTION_SHARD_COUNT`). No TLS (`rediss://`) yet, and Redis holds query results, so keep it private. |
| `CACHE_QUERY_TTL_SECS`, `CACHE_TABLES_TTL_SECS` | backend | Age a cached `POST /query` / `GET /tables` answer may reach (30 each; 0 turns that cache off). Staleness is bounded by this, not invalidated. |
| `CACHE_MAX_ENTRIES`, `CACHE_MAX_ENTRY_BYTES` | backend | In-memory cache size (256) and the largest result stored (1 MiB). |
| `INGESTION_INSTANCE_ID` | ingestion | Names an instance among its peers; must be unique. Unset: generated per process. |
| `INGESTION_COORDINATION_INTERVAL_SECS` | ingestion | How often an instance renews its claims and rebalances (5). |
| `INGESTION_SOURCE_REFRESH_INTERVAL_SECS` | ingestion | How often the registered sources are re-read (30). |
| `LOG_FORMAT=json` | all binaries | One JSON object per log line (`just local::up` sets it, for the files under `logs/`); default is text. |

## Contributing

Pull requests target `main`. Branch off it as `<type>/<short-name>` (e.g. `feat/developer-role-table-access`)
and keep each PR to one concern, with a description of what changed and why.

Run the same checks CI does before opening it:

```sh
just fmt && just clippy && just test
cargo clippy -p pipa-ui --target wasm32-unknown-unknown -- -D warnings   # if you touched pipa-ui
```

CI (`.github/workflows/ci.yaml`) runs `cargo fmt --all -- --check`, clippy with `-D warnings` and the
tests for `pipa-api`, `pipa-backend`, `pipa-catalog-proxy` and `pipa-ingestion`, and clippy for
`pipa-ui` on wasm32. All three must pass. Tests marked `#[ignore]` (the live Postgres WAL test) need a
real database and are skipped. The `Clippy & test (backend, ingestion)` job name is required by a
branch ruleset on `main`, so don't rename it.

Commit messages and PR titles follow [Conventional Commits](https://www.conventionalcommits.org/)
(`feat:`, `fix:`, `refactor:`, `chore:`, `docs:`, `ci:`, ..., optional scope). The type decides the next
release version: `!` or a `BREAKING CHANGE` footer bumps major, `feat` bumps minor, and
`fix`/`refactor`/`chore` bump patch. Other types (`docs`, `ci`, `style`, `test`) don't trigger a release.

When a change touches a crate, bump that crate's own version in its `Cargo.toml` and add a
matching `## [x.y.z]` entry to its `CHANGELOG.md`; crates are versioned independently. The
`Changelog` workflow (`.github/scripts/check-changelog.sh`) enforces this on every PR: any change
under `crates/<name>/` other than Markdown files needs a changed changelog, a bumped version and
a heading for that version. For changes that don't warrant a release, add the `skip-changelog`
label to the PR to skip the check. Changes to the
HTTP wire format start in `pipa-api`. `pipa-ingestion` and `pipa-backend` must not depend on each
other: they only share the data contract in the object store.
