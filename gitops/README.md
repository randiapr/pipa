# gitops

Kubernetes manifests for pipa and the Flux setup that applies them. The Compose stack
(`docker-compose.yml`) is the reference; `apps/pipa/` is its Kubernetes equivalent.

- `clusters/local/` — what Flux syncs (`kustomization.yaml`) and the `FluxInstance` that makes
  Flux follow this repository (`flux-instance.yaml`, applied by `just gitops::up`, not by Flux).
- `apps/pipa/` — rustfs, redis, the bucket Job, backend, ingestion (a StatefulSet of 2) and ui.

## How it works

`just gitops::up` installs the Flux Operator (Helm), creates the secrets, and applies the
`FluxInstance`. Flux then watches `gitops/clusters/local` on `GITOPS_BRANCH` (default `main`) of
`https://github.com/randiapr/pipa.git` and applies it every minute, undoing manual drift. The
cluster only changes when something is pushed to that branch.

The Flux web UI is served by the operator itself: `just gitops::ui`, then http://localhost:9080.

## Secrets

`pipa-secrets` (jwt-secret, admin-password, rustfs keys) is created in the cluster by
`just gitops::up` with random values and is never in git. Re-running keeps it. For a shared
cluster, encrypt it with SOPS or sync it from an external store instead.

## Images

`apps/pipa/kustomization.yaml` pins the published `ghcr.io/randiapr/pipa-*` images, each to its own crate's version (the image tags follow `crates/<backend|ingestion|ui>/Cargo.toml`).
Change `newTag` and push to roll out a new version. If the packages are private, add an
`imagePullSecret`.

## Limits

- The dashboard has its API address compiled in (`http://localhost:8080`, `crates/ui/src/api.rs`),
  so there is no Ingress: reach the stack with `just gitops::app` (port-forwards).
- Postgres sources are registered in the dashboard; one outside the cluster must be reachable from
  the pods and run with `wal_level=logical`.
