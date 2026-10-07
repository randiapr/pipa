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
  no dependency on any other crate here — it reads registered sources directly from the shared
  object store, and is designed to run distributed (`INGESTION_SHARD_INDEX`/
  `INGESTION_SHARD_COUNT` split sources across instances).
- **`pipa-backend`** — the HTTP API: accounts and sign-in (admin/user roles), projects, OLTP
  data source management (registering/testing sources, over the shared RustFS/S3 object
  store), plus Apache Iceberg query access (`POST /query`: REST catalog client, DataFusion SQL,
  scoped to the caller's project). Every external caller (the dashboard, any future client)
  talks to this, never to `pipa-ingestion` directly.
- **`pipa-catalog-proxy`** — a small internal reverse proxy that SigV4-signs requests to
  RustFS's embedded Iceberg REST catalog, which rejects unsigned ones (`iceberg-catalog-rest`
  can't sign). The signing is implemented in-house on `hmac`/`sha2`; there is no AWS
  dependency. Not an external-facing service.
- **`pipa-api`** — the HTTP contract (request/response types, routes) shared by `pipa-backend`
  and `pipa-ui`; builds for both native and wasm32.
- **`pipa-ui`** — a Leptos dashboard (Tailwind CSS v4 + daisyUI) for signing in and managing
  projects, OLTP data sources, users and queries.

## Prerequisites

- Rust (stable) with the `wasm32-unknown-unknown` target: `rustup target add wasm32-unknown-unknown`
- [`just`](https://github.com/casey/just)
- [`trunk`](https://trunkrs.dev): `cargo install trunk`
- [`rustfs`](https://rustfs.com) and its `rc` CLI, for local S3-compatible object storage
- Node.js/npm — only used to resolve daisyUI as a Tailwind plugin (`crates/ui/package.json`); no JS build step otherwise

## Quickstart

```sh
just local::up      # rustfs + pipa-backend + pipa-ingestion + dashboard, all in the background
```

Open `http://localhost:3000` for the dashboard and `http://localhost:8080/healthz` for the
API. `just local::log` tails every service's output; `just local::down` stops everything.

`just local::up` does not start `pipa-catalog-proxy`, so locally the Iceberg side (ingestion
writes and `POST /query`) won't work against RustFS's signed-only catalog until it is running
and `ICEBERG_CATALOG_URI` points at it, e.g. `CATALOG_PROXY_LISTEN=127.0.0.1:8181 cargo run -p
pipa-catalog-proxy` plus `ICEBERG_CATALOG_URI=http://localhost:8181/iceberg` for the backend and
ingestion. The containerized stack below does this for you.

With `just local::up`, sign in as the first admin, `admin` / `admin-password` by default (local-only defaults;
override with `PIPA_ADMIN_USERNAME`/`PIPA_ADMIN_PASSWORD`, and `JWT_SECRET` for the token
secret). Admins create further accounts on the Users page and assign each `user`-role
account the projects it may see; a `user` never sees the Users menu.

Alternatively, containerized: RustFS, `pipa-catalog-proxy`, `pipa-backend`, two `pipa-ingestion`
shards, the dashboard, and an example Postgres source (`localhost:5432`, user/password `pipa`,
database `testdb`, `wal_level=logical`) to register as a data source:

```sh
just docker-up   # first run creates .env with a random JWT secret and admin password
just docker-down # stop it, keeping the data volumes (docker-down-clean also deletes them)
```

The dashboard is a static SPA built on the host with Trunk, which `just docker-up` does for you
(plain `docker compose up --build` needs `just build-ui-release` run first). It is served at
`http://localhost:3000` and the API at `http://localhost:8080`.

`just docker-up` does not use the local defaults above: it prints the generated admin login once (it stays in the gitignored `.env`, which
you can edit; the password needs at least 8 characters). Without `just`, `cp .env.example .env`, set `PIPA_JWT_SECRET` and
`PIPA_ADMIN_PASSWORD`, then `docker compose up --build`.

`POST /query` and `pipa-ingestion` talk to RustFS's own embedded Iceberg REST Catalog ("S3 Tables"
feature). That catalog requires SigV4-signed requests, which `iceberg-catalog-rest` can't send, so
compose runs `pipa-catalog-proxy` (`catalog`, `crates/catalog-proxy`) in front of it, which also enables
S3 Tables on the `pipa` bucket at startup — all automatic (see the note at the top of `docker-compose.yml`).

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
`pipa-ingestion` and `pipa-ui` each have their own `crates/*/.env.example`. `pipa-catalog-proxy`'s
are listed at the top of `crates/catalog-proxy/src/main.rs`.
