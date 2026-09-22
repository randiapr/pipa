# pipa

Change Data Capture (CDC) tool that streams changes from OLTP databases into Apache
Iceberg. Written in Rust, built on Apache DataFusion (query engine) and RustFS
(S3-compatible object storage).

Supported source databases: PostgreSQL (via logical replication/WAL). MySQL (binlog) is
planned but not implemented yet.

## Workspace

A Cargo workspace of four crates, each versioned independently (see each crate's own
`CHANGELOG.md`):

- **`pipa-core`** — the CDC engine. Streams row-level changes out of a registered
  Postgres source via logical replication and, eventually, into Iceberg tables.
- **`pipa-data`** — shared Iceberg + RustFS access layer, and the OLTP data source
  domain (registering/testing sources) used by both `pipa-core` and `pipa-rest`.
- **`pipa-rest`** — the HTTP facade in front of `pipa-data`. Every external caller
  (the dashboard, any future client) talks to this, never to `pipa-data`/`pipa-core`
  directly.
- **`pipa-ui`** — a Leptos dashboard (Tailwind CSS v4 + daisyUI) for registering and
  managing OLTP data sources.

See `CLAUDE.md` for the full architecture writeup (clean-architecture layering, why
things are split the way they are, extension points).

## Prerequisites

- Rust (stable) with the `wasm32-unknown-unknown` target: `rustup target add wasm32-unknown-unknown`
- [`just`](https://github.com/casey/just)
- [`trunk`](https://trunkrs.dev): `cargo install trunk`
- [`rustfs`](https://rustfs.com) and its `rc` CLI, for local S3-compatible object storage
- Node.js/npm — only used to resolve daisyUI as a Tailwind plugin (`crates/ui/package.json`); no JS build step otherwise

## Quickstart

```sh
just local::up      # rustfs + pipa-rest + pipa-core in the background, dashboard in the foreground
```

Open `http://localhost:3000` for the dashboard and `http://localhost:8080/healthz` for the
API. Ctrl+C stops everything; `just local::down` is a fallback if it didn't (closed
terminal, crash).

## Commands

```sh
just check-all    # cargo check, native crates + pipa-ui (wasm32)
just test         # run tests for native crates
just clippy       # lint native crates
just fmt          # format all crates
just clean        # remove build artifacts (target/, crates/ui/dist/)

just rustfs       # run local RustFS alone (S3 API :9000, console :9001)
just rustfs-init  # one-time: configure `rc` and ensure the "pipa" bucket exists
just rest         # run pipa-rest alone (0.0.0.0:8080)
just core         # run pipa-core alone
just ui           # run the dashboard dev server (trunk serve, :3000)
just build-ui     # production build of the dashboard
```

Run `just --list` for the full list, including per-crate variants under `-p`.

Environment variables are documented per crate in `crates/*/.env.example`.
