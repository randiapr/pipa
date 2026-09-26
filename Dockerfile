# syntax=docker/dockerfile:1

# Builds pipa-ingestion and pipa-backend (native crates) in one builder stage so their
# shared workspace dependencies (datafusion, iceberg, sqlx, ...) are compiled once,
# then copies each release binary into its own minimal distroless runtime image. pipa-ui
# targets wasm32 instead and gets its own `ui-builder` stage below (Trunk, not `cargo build`).
#
# Build a single service with: docker build --target ingestion -t pipa-ingestion .
#                               docker build --target backend -t pipa-backend .
#                               docker build --target ui -t pipa-ui .
# (docker-compose.yml does this via each service's `build.target`.)

FROM rust:slim-trixie AS builder
WORKDIR /build

# BuildKit cache mounts keep the registry index/crate cache and compiled dependency
# artifacts around across separate `docker build` invocations — not just within one —
# so editing application source doesn't re-download or re-compile the dependency graph.
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --release -p pipa-ingestion -p pipa-backend && \
    cp target/release/pipa-ingestion target/release/pipa-backend /tmp/

# gcr.io/distroless/cc-debian13 provides glibc, libgcc, libstdc++ and CA certificates
# (what a dynamically-linked Rust binary needs to run and make TLS connections) with no
# shell, package manager, or OpenSSL — pipa uses rustls throughout, so nothing here
# needs the OpenSSL that a larger base image would carry instead.

FROM gcr.io/distroless/cc-debian13 AS ingestion
COPY --from=builder /tmp/pipa-ingestion /usr/local/bin/pipa-ingestion
ENTRYPOINT ["/usr/local/bin/pipa-ingestion"]

FROM gcr.io/distroless/cc-debian13 AS backend
COPY --from=builder /tmp/pipa-backend /usr/local/bin/pipa-backend
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/pipa-backend"]

# Separate stage: pipa-ui is a Trunk-built wasm32 SPA (crates/ui/), not a `cargo build`
# binary, so it needs the wasm32 target, Trunk, and Node/npm (Tailwind v4's standalone CLI
# resolves `@plugin "daisyui"` via node_modules — see crates/ui/package.json) instead of
# anything the shared `builder` stage above already has.
FROM rust:slim-trixie AS ui-builder
WORKDIR /build
ARG TARGETARCH
RUN apt-get update && apt-get install -y --no-install-recommends nodejs npm curl \
    && rm -rf /var/lib/apt/lists/*
RUN rustup target add wasm32-unknown-unknown
# Prebuilt binary rather than `cargo install trunk`: trunk's own dependency tree is large
# enough that compiling it from source can OOM a memory-constrained build host.
RUN case "$TARGETARCH" in \
      amd64) trunk_arch=x86_64-unknown-linux-gnu ;; \
      arm64) trunk_arch=aarch64-unknown-linux-gnu ;; \
      *) echo "unsupported TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/trunk-rs/trunk/releases/download/v0.21.14/trunk-${trunk_arch}.tar.gz" \
      | tar -xz -C /usr/local/bin trunk
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cd crates/ui && npm install && trunk build --release

# The build output (crates/ui/dist/) is a static SPA that talks to pipa-backend from the
# browser (crates/ui/src/api.rs's API_BASE is a compile-time localhost:8080 constant, so it
# reaches pipa-backend via the host port mapping — no container-to-container wiring needed),
# so serving it needs nothing beyond a static file server.
FROM nginx:alpine AS ui
COPY --from=ui-builder /build/crates/ui/dist /usr/share/nginx/html
EXPOSE 80
