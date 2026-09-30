# pipa

Change Data Capture (CDC) tool that streams changes from OLTP databases into Apache
Iceberg. Written in Rust, built on Apache DataFusion (query engine) and RustFS
(S3-compatible object storage).

Supported source databases: PostgreSQL (via logical replication/WAL). MySQL (binlog) is
planned but not implemented yet.

## Workspace

A Cargo workspace of three crates, each versioned independently (see each crate's own
`CHANGELOG.md`):

- **`pipa-ingestion`** — the CDC engine. Streams row-level changes out of a registered
  Postgres source via logical replication and, eventually, into Iceberg tables. A
  standalone service with no dependency on any other crate here — it reads registered
  sources directly from the shared object store, and is designed to run distributed
  (`INGESTION_SHARD_INDEX`/`INGESTION_SHARD_COUNT` split sources across instances).
- **`pipa-backend`** — the HTTP API: OLTP data source/project management (registering/
  testing sources, over the shared RustFS/S3 object store), plus its own Apache Iceberg
  catalog and query integration (REST catalog client, DataFusion SQL execution). Every
  external caller (the dashboard, any future client) talks to this, never to
  `pipa-ingestion` directly.
- **`pipa-ui`** — a Leptos dashboard (Tailwind CSS v4 + daisyUI) for registering and
  managing OLTP data sources.

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

With `just local::up`, sign in as the first admin, `admin` / `admin-password` by default (local-only defaults;
override with `PIPA_ADMIN_USERNAME`/`PIPA_ADMIN_PASSWORD`, and `JWT_SECRET` for the token
secret). Admins create further accounts on the Users page and assign each `user`-role
account the projects it may see; a `user` never sees the Users menu.

Alternatively, containerized (`pipa-ingestion` + `pipa-backend` + RustFS, no local Rust
toolchain needed — `pipa-ui` isn't included, it's a static SPA, not a Rust service):

```sh
just docker-up   # first run creates .env with a random JWT secret and admin password
```

`just docker-up` does not use those defaults: it prints the generated admin login once (it stays in the gitignored `.env`, which
you can edit). Without `just`, `cp .env.example .env`, set `PIPA_JWT_SECRET` and
`PIPA_ADMIN_PASSWORD`, then `docker compose up --build`.

`POST /query` talks to RustFS's own embedded Iceberg REST Catalog ("S3 Tables" feature) by
default now, but that feature is opt-in per bucket — until the `pipa` bucket has S3 Tables
enabled on it manually, `POST /query` will fail (see the note at the top of
`docker-compose.yml`).

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

Run `just --list` for the full list, including per-crate variants under `-p`.

Environment variables are documented per crate in `crates/*/.env.example`.
