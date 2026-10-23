# pipa

Change Data Capture (CDC) tool that streams changes from OLTP databases into Apache
Iceberg. Written in Rust, built on Apache DataFusion (query engine) and RustFS
(S3-compatible object storage).

Supported source databases: PostgreSQL (via logical replication/WAL). MySQL (binlog) is
planned but not implemented yet.

## Workspace

A Cargo workspace of six crates, each versioned independently (see each crate's own
`CHANGELOG.md`):

- **`pipa-ingestion`** — the CDC engine. Streams row-level changes out of a registered
  Postgres source via logical replication and appends them to Iceberg tables as changelog
  rows (`_op`, `_position`, ... plus the source columns), with effectively-once delivery: the
  checkpoint is committed in the same Iceberg snapshot as the data. A standalone service with
  no dependency on `pipa-backend` (only on the embedded `pipa-catalog-proxy` signer) — it reads
  registered sources directly from the shared
  object store, and is designed to run distributed: with `REDIS_URL` set, instances share the sources through
  Redis and a stopped one's sources move to the others; without it, `INGESTION_SHARD_INDEX`/
  `INGESTION_SHARD_COUNT` split sources statically. It also maintains the tables it writes:
  small data files are compacted and old snapshots expired, so tables stay fast to read
  however long ingestion runs.
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
- **`pipa-integration-test`** — end-to-end tests: the real `pipa-backend` and `pipa-ingestion`
  binaries against RustFS, Postgres and Redis in containers ([testcontainers](https://github.com/testcontainers/testcontainers-rs)),
  driven over HTTP with positive and negative cases for every area. Needs Docker; run with
  `just test-integration`. Not part of `just test`.
- **`pipa-ui`** — a Leptos dashboard (Tailwind CSS v4 + daisyUI) for signing in and managing
  projects, OLTP data sources, users and queries.

## Prerequisites

- Rust (stable) with the `wasm32-unknown-unknown` target: `rustup target add wasm32-unknown-unknown`
- [`just`](https://github.com/casey/just)
- [`trunk`](https://trunkrs.dev): `cargo install trunk --locked` (0.22 or newer)
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

Alternatively, containerized: RustFS, Redis, `pipa-backend`, two `pipa-ingestion` instances and the
dashboard. There is no example source database in it: register your own (a Postgres on the host is
`host.docker.internal` to `pipa-backend`, with `wal_level=logical`; `just run-pg-native` starts one):

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

Each image is versioned by its own crate, like the crate itself: `ghcr.io/<owner>/pipa-backend` by
`crates/backend`, `pipa-ingestion` by `crates/ingestion` and `pipa-ui` by `crates/ui`, so the image
tag is that crate's `version` and its notes are that crate's `CHANGELOG.md`. On every merge to
`main`, `.github/workflows/publish-images.yaml` publishes (linux/amd64 and linux/arm64) each image
whose crate has a new version, tagged `X.Y.Z`, `X.Y` (and `X` from 1.0 on), `sha-<short>` and
`latest`, and creates a GitHub release `pipa-<image>-vX.Y.Z` with that changelog section as its
notes. A merge that doesn't bump a crate publishes nothing for it (the Changelog check requires the
bump). To stay on a specific version in Compose, set `PIPA_BACKEND_TAG`, `PIPA_INGESTION_TAG` and
`PIPA_UI_TAG` in `.env` (default `latest`); the three move independently, so check that the versions
you pin work together. The repo-wide `vX.Y.Z` releases drafted by `draft-release.yaml` no longer
publish images. The `pipa-ui` image has the API address `http://localhost:8080` compiled in (like the
local stack), so it only works as shipped next to a backend published on that address.

`POST /query` and `pipa-ingestion` talk to RustFS's own embedded Iceberg REST Catalog ("S3 Tables"
feature). That catalog requires SigV4-signed requests, which `iceberg-catalog-rest` can't send, so
both embed the `pipa-catalog-proxy` signer (`crates/catalog-proxy`) in their own process and enable
S3 Tables on the `pipa` bucket at startup — all automatic, locally and in compose, as long as
`ICEBERG_CATALOG_URI` is left unset (see the note at the top of `docker-compose.yml`).

## GitOps (Flux)

`gitops/` holds the Kubernetes manifests for the whole stack (the Kubernetes equivalent of
`docker-compose.yml`), and `just gitops::*` runs [Flux](https://fluxcd.io) on a local Kubernetes cluster
to keep the `pipa` namespace in step with this repo's GitHub branch. Flux and its web UI come from
the [Flux Operator](https://fluxoperator.dev). Nothing is applied by hand after the first `up`:
you change files under `gitops/`, push, and Flux reconciles.

```
gitops/
  clusters/local/   flux-instance.yaml (Flux itself, applied by `up`) + kustomization.yaml (what Flux syncs)
  apps/pipa/        rustfs, redis, create-bucket Job, backend, ingestion (StatefulSet, 2), ui
gitops.just         the `just gitops::*` recipes
```

How it fits together: `up` installs the Flux Operator with Helm, creates the secrets, and applies a
`FluxInstance`. The operator starts Flux, which watches `gitops/clusters/local` on the branch of
`https://github.com/randiapr/pipa.git` and applies it about once a minute (undoing manual drift).
`clusters/local/kustomization.yaml` pulls in `apps/pipa`, whose `kustomization.yaml` lists the
manifests and pins the image tags.

### GitOps prerequisites

- a local Kubernetes cluster (Docker Desktop, kind, minikube, …) as
  the current `kubectl` context, plus `kubectl` and [`helm`](https://helm.sh). `up` refuses a context whose API
  server isn't on loopback or a `192.168.*` address, unless `GITOPS_KUBE_CONTEXT` names it
- `openssl` (random secrets), and optionally [`gh`](https://cli.github.com) (the Git token, see below)
- optionally [`kubeconform`](https://github.com/yannh/kubeconform) for `just gitops::validate`

### Bring it up

```sh
# start a local Kubernetes cluster first, then:
just gitops::up         # Flux Operator + secrets + FluxInstance; waits until Flux is Ready
just gitops::status     # what Flux reconciled, and the pipa pods
```

`up` is safe to re-run: it upgrades the Helm release, re-applies the `FluxInstance` and keeps
secrets that already exist. On the first run it prints the generated admin login
(`admin / <password>`). The pods take a few minutes to become ready (image pulls, then rustfs →
bucket Job → backend/ingestion); until the bucket exists backend and ingestion restart, which is
expected.

**Flux reads from GitHub, not your working tree.** Commit and push `gitops/` to the branch Flux
follows before expecting the cluster to change.

### Follow a different branch

The branch defaults to `main`. To test a branch, push it and re-run `up` with the variable (or put
`GITOPS_BRANCH=feat/gitops` in `.env`, which `just` loads):

```sh
GITOPS_BRANCH=feat/gitops just gitops::up
```

Re-running rewrites the `FluxInstance`'s `ref`, and Flux switches over on its next reconcile.

### Git access (private repo, token)

`up` creates a `flux-system` basic-auth Secret from `GITHUB_TOKEN`, or from `gh auth token` when
that is unset. The token needs read access to the repo (a fine-grained token with "Contents:
read-only" is enough). With neither, `up` prints a warning and Flux reads the repo anonymously,
which only works for a public repo. To rotate the token, export a new `GITHUB_TOKEN` and re-run
`just gitops::up`.

### Open the Flux web UI

```sh
just gitops::ui         # http://localhost:9080, until Ctrl+C
```

The Flux Operator serves the UI itself (the `flux-operator` Service, port 9080), so there is nothing
extra to install. It shows the `FluxInstance`, the `GitRepository`, the `Kustomization` and every
resource they apply, with their reconcile state and events. It is reached through a port-forward
and has no Ingress; don't expose it without configuring authentication (see the Flux Operator docs).

### Use pipa on the cluster

```sh
just gitops::app        # dashboard http://localhost:3000, API http://localhost:8080, until Ctrl+C
```

Sign in with the admin login `up` printed. The dashboard has `http://localhost:8080` compiled in as
its API address and the backend allows the origin `http://localhost:3000` (`CORS_ALLOWED_ORIGINS` in
`gitops/apps/pipa/configmap.yaml`), so both ports must be forwarded; there is no Ingress.

To read the admin password again:

```sh
kubectl -n pipa get secret pipa-secrets -o jsonpath='{.data.admin-password}' | base64 -d; echo
```

The RustFS console (9001) is not forwarded by a recipe: `kubectl -n pipa port-forward svc/rustfs 9001:9001`
(the access key is `rustfsadmin`, the secret key is `rustfs-secret-key` in `pipa-secrets`).

### Register a Postgres source from the cluster

Sources are registered in the dashboard exactly as in the other setups (see
[Registering your own Postgres source](#registering-your-own-postgres-source)); the difference is
that `pipa-ingestion` runs in pods, so the database must be reachable from them and run with
`wal_level=logical`. A Postgres on your Mac is not `localhost` to a pod: use an address the cluster's
nodes can reach (the name for the host differs per tool, e.g. `host.docker.internal`; check from a pod, e.g.
`kubectl -n pipa run -it --rm probe --image=busybox -- nslookup host.docker.internal`), and allow it in
the database's `pg_hba.conf`. The two ingestion pods share the sources through Redis and
each captures its own through its own replication slot.

### Change the stack (the normal workflow)

1. Edit files under `gitops/apps/pipa/`.
2. Render them: `just gitops::validate` (offline; schema-checked when `kubeconform` is installed).
3. Commit and push to the followed branch.
4. Wait for the next reconcile (about a minute), or ask for one now:

   ```sh
   kubectl -n flux-system annotate gitrepository/flux-system \
     reconcile.fluxcd.io/requestedAt="$(date +%s)" --overwrite
   kubectl -n flux-system annotate kustomization/flux-system \
     reconcile.fluxcd.io/requestedAt="$(date +%s)" --overwrite
   ```

5. Check `just gitops::status` or the Flux UI.

Anything you change by hand in the `pipa` namespace (`kubectl edit`, `kubectl scale`) is reverted at
the next reconcile; to experiment, suspend the sync first (see below).

### Roll out a new pipa version

The images are pinned, not `latest`. In `gitops/apps/pipa/kustomization.yaml`, change the `newTag`
of the `images:` entries to the version you want, then push. The images are versioned by their own
crates (see [Published images](#published-images)), so the tags differ per image: the current ones
are the `version` in `crates/backend/Cargo.toml`, `crates/ingestion/Cargo.toml` and
`crates/ui/Cargo.toml`, and only versions that `publish-images.yaml` has published exist. The pods
are replaced by a normal rolling update. To roll back, push the old tag. Bump the three together
only when their changelogs say they must be: the dashboard and the backend share the HTTP contract
in `pipa-api`.

### Scale ingestion

Ingestion is a StatefulSet; `INGESTION_INSTANCE_ID` is the pod name (`ingestion-0`, `ingestion-1`, …),
unique and stable. Change `spec.replicas` in `gitops/apps/pipa/ingestion.yaml` and push. Instances
find each other through Redis, so about `1/N` of the sources move to a new pod, and a removed pod's
sources move to the others, with nothing lost or duplicated (see the coordination notes in
[Quickstart](#quickstart)).

### Change configuration

Non-secret settings live in `gitops/apps/pipa/configmap.yaml` (RustFS endpoint, region, Redis URL,
refresh and maintenance intervals, CORS origin, log level). Other runtime settings can be added
there too (see [Runtime settings](#runtime-settings)). A pod reads its environment at start, so a
changed ConfigMap does not restart anything by itself:

```sh
kubectl -n pipa rollout restart deployment/backend statefulset/ingestion
```

### Secrets

`pipa-secrets` holds `jwt-secret`, `admin-password`, `rustfs-access-key` and `rustfs-secret-key`.
It is **not in git**: `up` creates it once with random values and never overwrites it, so tokens and
the admin login survive re-runs. Everything in `gitops/` is public to anyone who can read the repo,
so never commit a plain Secret.

- **Rotate a value:** delete the Secret and re-run `up` (this also resets the admin login and
  invalidates tokens), or patch one key with `kubectl -n pipa create secret generic pipa-secrets
  --dry-run=client -o yaml --from-literal=… | kubectl apply -f -`, then
  `kubectl -n pipa rollout restart deployment/backend statefulset/ingestion statefulset/rustfs`.
  Changing the RustFS keys on an existing volume needs the RustFS data to be reset too.
- **Keep secrets in git, encrypted (optional, not wired in):** the Flux-native way is
  [SOPS](https://fluxcd.io/flux/guides/mozilla-sops/) with [age](https://github.com/FiloSottile/age).
  Outline: `age-keygen -o age.agekey`; store the private key in the cluster as a Secret `sops-age`
  in `flux-system`; add a `.sops.yaml` with the public key and `encrypted_regex: ^(data|stringData)$`;
  encrypt the Secret with `sops --encrypt --in-place`; and give the Flux `Kustomization` that
  applies `apps/pipa` `decryption: {provider: sops, secretRef: {name: sops-age}}`. Drop the
  secret creation from `up` at the same time, so the two don't fight. Back up the private key — a lost key means the
  encrypted values in git can't be recovered. This repo doesn't do it yet.
  [External Secrets](https://external-secrets.io) is the alternative for a real secret store.

### Private images

The `ghcr.io/randiapr/pipa-*` images are pulled anonymously. If the packages are private, create a
pull Secret and reference it from the pod specs (`imagePullSecrets`):

```sh
kubectl -n pipa create secret docker-registry ghcr \
  --docker-server=ghcr.io --docker-username=<user> --docker-password=<token with read:packages>
```

then add `imagePullSecrets: [{name: ghcr}]` to the three pipa pod templates under `gitops/apps/pipa/`
(`backend.yaml`, `ingestion.yaml`, `ui.yaml`; the rustfs, redis and rc images are public) and push.

### Pause Flux while you experiment

```sh
kubectl -n flux-system patch kustomization flux-system --type merge -p '{"spec":{"suspend":true}}'
# ... kubectl edit / scale / debug ...
kubectl -n flux-system patch kustomization flux-system --type merge -p '{"spec":{"suspend":false}}'
```

On resume Flux restores whatever differs from git.

### Stop and remove

```sh
just gitops::down       # deletes the FluxInstance, the pipa namespace (its data too), the operator
# then stop your local cluster the way you started it
```

`down` deletes the `pipa` namespace, so the RustFS volume — every captured table — goes with it.
Stopping the cluster alone keeps everything, and it resumes when the cluster is started again.

### Troubleshooting

| Symptom | Likely cause and fix |
| --- | --- |
| `up`: "no kubectl context" / "not a local cluster" | Start a local cluster and select it (`kubectl config use-context …`), or set `GITOPS_KUBE_CONTEXT` to the context's name to use a remote one on purpose. |
| `GitRepository` not Ready, authentication/not found | Private repo without a token, or a token without access: set `GITHUB_TOKEN` (or `gh auth login`) and re-run `up`. |
| `Kustomization` not Ready: path or `kustomization.yaml` not found | The branch Flux follows doesn't have `gitops/` yet. Push it, or re-run `up` with `GITOPS_BRANCH`. |
| `Kustomization` stuck on a build error | Run `just gitops::validate` and fix the manifest; `kubectl -n flux-system describe kustomization flux-system` shows Flux's message. |
| backend / ingestion pods restarting at first | Waiting for RustFS and the `pipa` bucket; check `kubectl -n pipa logs job/create-bucket`. Settles on its own. |
| `ImagePullBackOff` | The tag in `kustomization.yaml` doesn't exist, or the package is private (see Private images). |
| Dashboard loads but can't log in / network errors | Only `ui` is forwarded: run `just gitops::app` (it forwards the backend on 8080 too). |
| `create-bucket` Job unchanged after editing it | Jobs are immutable; Flux recreates it (the `force` annotation in `create-bucket.yaml`). Reconcile and check the Kustomization's events. |

Useful commands: `kubectl -n pipa get pods`, `kubectl -n pipa logs deploy/backend`,
`kubectl -n pipa logs sts/ingestion`, `kubectl -n flux-system get fluxinstance,gitrepository,kustomization`.

### Recipe reference

| Recipe | What it does |
| --- | --- |
| `just gitops::up` | Install the Flux Operator, create the secrets, apply the `FluxInstance`; wait until Ready. Re-runnable. |
| `just gitops::status` | Flux objects and the `pipa` pods. |
| `just gitops::ui` | Port-forward the Flux web UI to `localhost:9080`. |
| `just gitops::app` | Port-forward the dashboard (`3000`) and the API (`8080`). |
| `just gitops::validate` | Render `apps/pipa` and `clusters/local` offline; schema-check with `kubeconform` if installed. |
| `just gitops::down` | Remove the `FluxInstance`, the `pipa` namespace (data included) and the operator. |

Environment: `GITOPS_BRANCH` (branch Flux follows, default `main`), `GITHUB_TOKEN` (Git read
access, default `gh auth token`) and `GITOPS_KUBE_CONTEXT` (allow a non-local cluster). All can be set in `.env`. See also `gitops/README.md`.

## Known limitations

**Upstream (DataFusion / `iceberg-datafusion`).** `SELECT count(*)` over an Iceberg metadata table
(`…$snapshots`, `…$manifests`, which `POST /query` can read) fails with a 400 carrying DataFusion's
"Physical input schema should be the same as the one converted from logical input schema"
internal error. It appears to be the metadata table provider mishandling a query that needs no
columns, so it is not something `pipa-backend` can work around. Select a column instead, e.g.
`SELECT snapshot_id FROM "…$snapshots"`, and count the rows of the result. Data tables are not
affected, and the end-to-end test `query::iceberg_metadata_tables_can_be_queried` selects a column
for this reason. This has not been reported upstream yet; recheck after bumping `iceberg`
/ `datafusion-iceberg`.

## Commands

```sh
just check-all    # cargo check, native crates + pipa-ui (wasm32)
just test         # run tests for native crates
just test-integration  # end-to-end tests in containers (needs Docker); `just test-integration cdc::` runs some
just clippy       # lint native crates
just fmt          # format all crates
just clean        # remove build artifacts (target/, crates/ui/dist/)

just run-rustfs    # run local RustFS alone (S3 API :9000, console :9001)
just rustfs-init   # one-time: configure `rc` and ensure the "pipa" bucket exists
just run-pg-native # run the example Postgres source alone (brew's postgresql@18, :5432, wal_level=logical)
just run-backend   # run pipa-backend alone (0.0.0.0:8080)
just run-ingestion # run pipa-ingestion alone
just run-ui        # run the dashboard dev server (trunk serve, :3000)
just build-ui     # production build of the dashboard
```

Run `just --list` for the full list (`docker-up`/`docker-down`, `outdated`, `upgrade`, `crap`, `mutants`, `machete`, ...).

### Code quality

Three optional tools, installed once with `cargo install --locked` (each recipe checks for what it needs and says so if it is missing). None of them runs in CI.

| Recipe | Tool | What it tells you |
|---|---|---|
| `just crap` | [`cargo-crap`](https://github.com/minikin/cargo-crap) + [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov) | Functions ranked by CRAP score: cyclomatic complexity weighted by how little of the function the tests cover. |
| `just mutants` | [`cargo-mutants`](https://github.com/sourcefrog/cargo-mutants) | Whether the tests notice injected bugs; a surviving mutant is behaviour no test pins down. |
| `just machete` | [`cargo-machete`](https://github.com/bnjbvr/cargo-machete) | Dependencies a crate's `Cargo.toml` lists but its code never uses. |

```sh
cargo install cargo-llvm-cov cargo-crap cargo-mutants cargo-machete --locked

just crap                      # coverage of the native crates' unit tests, then the ranked table
just crap --fail-above         # exit non-zero when any function scores above the threshold (30; `--threshold N` changes it)
just crap-gate                 # exit non-zero only when a score got worse than in crap-baseline.json
just crap-baseline             # re-save crap-baseline.json (after paying scores down)
just mutants --list            # list the mutants (about a thousand) without running them
just mutants -p pipa-ingestion # run them for one crate; slow, so scope it (-f <file> works too)
just machete                   # no build needed; exit code 1 when something is unused
```

Extra arguments go straight to the tool. For `crap` and `mutants` only unit tests count, not the
`#[ignore]`d live tests or `pipa-integration-test`. The LCOV file is written to
`target/crap/lcov.info` and the mutation report to `mutants.out/` (gitignored). `machete` can report
a dependency that is only used through a macro or a rename; list those under
`[package.metadata.cargo-machete] ignored` in that crate's `Cargo.toml`.

#### Gating on CRAP

`--fail-above` is a plain flag, not `--fail-above 30`: the cutoff is a separate `--threshold N`
(default 30), so `just crap --threshold 30 --fail-above` is the long spelling of
`just crap --fail-above`. Either exits 1 while any function scores over the threshold, and some
already do: the I/O adapters and `main`s that only the end-to-end tests exercise have no unit-test
coverage, and a function whose complexity alone is over the threshold can't pass however well it is
tested. So a plain `--fail-above` can't be a day-to-day check yet.

`just crap-gate` is the one to run. It compares against the committed `crap-baseline.json` with
`--fail-regression`, so it fails only when a function's score rose, and what was already over the
threshold when the baseline was saved doesn't block anything.
After you lower scores, run `just crap-baseline` and commit the new file, so the gate ratchets
tighter instead of leaving the slack.

`.cargo-crap.toml` skips `**/tests.rs`. A test helper there, such as `CatalogCommitter` in
`maintenance/infrastructure/tests.rs`, has its `#[cfg(test)]` on the `mod` line in the parent file,
which `cargo-crap` can't see, so it would be scored as production code.

### Runtime settings

Environment variables: the compose secrets are in the root `.env.example`, and `pipa-backend`,
`pipa-ingestion` and `pipa-ui` each have their own `crates/*/.env.example`. The standalone
`pipa-catalog-proxy` binary's are listed at the top of `crates/catalog-proxy/src/main.rs`.

Optional runtime settings (all set in compose and by `just local::up` where it matters):

| Variable | Used by | Effect |
|---|---|---|
| `BACKEND_LISTEN` | backend | Address the API listens on (default `0.0.0.0:8080`). Leave it for compose, which publishes 8080; set it to run several backends on one host. |
| `REDIS_URL` | backend, ingestion | Backend: results cache in Redis, shared by all instances (unset: in-process memory). Ingestion: instances discover each other and share sources (unset: static `INGESTION_SHARD_INDEX`/`INGESTION_SHARD_COUNT`). No TLS (`rediss://`) yet, and Redis holds query results, so keep it private. |
| `CACHE_QUERY_TTL_SECS`, `CACHE_TABLES_TTL_SECS` | backend | Age a cached `POST /query` / `GET /tables` answer may reach (30 each; 0 turns that cache off). Staleness is bounded by this, not invalidated. |
| `CACHE_MAX_ENTRIES`, `CACHE_MAX_ENTRY_BYTES` | backend | In-memory cache size (256) and the largest result stored (1 MiB). |
| `INGESTION_INSTANCE_ID` | ingestion | Names an instance among its peers; must be unique. Unset: generated per process. |
| `INGESTION_COORDINATION_INTERVAL_SECS` | ingestion | How often an instance renews its claims and rebalances (5). |
| `INGESTION_SOURCE_REFRESH_INTERVAL_SECS` | ingestion | How often the registered sources are re-read (30). |
| `INGESTION_MAINTENANCE_INTERVAL_SECS` | ingestion | How often the tables of the sources an instance captures are compacted and their snapshots expired (300; 0 turns maintenance off). |
| `INGESTION_COMPACTION_TARGET_FILE_BYTES`, `INGESTION_COMPACTION_SMALL_FILE_RATIO`, `INGESTION_COMPACTION_MIN_INPUT_FILES`, `INGESTION_COMPACTION_MAX_INPUT_BYTES` | ingestion | Compaction: files under target × ratio (128 MiB × 0.75) are rewritten into files of up to the target once 8 have built up, at most 1 GiB per table per run. |
| `INGESTION_SNAPSHOT_MAX_AGE_SECS`, `INGESTION_SNAPSHOT_MIN_TO_KEEP` | ingestion | Snapshots older than this (3600) are expired, keeping at least this many (10). No data goes with them — every change is still a changelog row — only time travel to older table states. |
| `INGESTION_ORPHAN_CLEANUP_AGE_SECS` | ingestion | Files under a table's location that nothing references (left by failed commits) are deleted once this old (259200, 3 days; 0 turns the sweep off). |
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
