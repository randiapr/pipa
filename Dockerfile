# syntax=docker/dockerfile:1

# Builds pipa-ingestion, pipa-backend and pipa-catalog-proxy (native crates) in one builder stage so their
# shared workspace dependencies (datafusion, iceberg, sqlx, ...) are compiled once,
# then copies each release binary into its own minimal distroless runtime image. pipa-ui
# targets wasm32 instead and is built on the host with Trunk; the `ui` stage below just
# packages its crates/ui/dist/ output.
#
# Build a single service with: docker build --target ingestion -t pipa-ingestion .
#                               docker build --target backend -t pipa-backend .
#                               docker build --target catalog-proxy -t pipa-catalog-proxy .
#                               (just build-ui-release first)  docker build --target ui -t pipa-ui .
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
    cargo build --release -p pipa-ingestion -p pipa-backend -p pipa-catalog-proxy && \
    cp target/release/pipa-ingestion target/release/pipa-backend target/release/pipa-catalog-proxy /tmp/

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

FROM gcr.io/distroless/cc-debian13 AS catalog-proxy
COPY --from=builder /tmp/pipa-catalog-proxy /usr/local/bin/pipa-catalog-proxy
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/pipa-catalog-proxy"]

# pipa-ui is a Trunk-built wasm32 SPA (crates/ui/), not a `cargo build` binary, and it is
# deliberately NOT compiled inside Docker: the wasm build runs on the host with its native
# Trunk instead (`just build-ui-release`, which `just docker-up` runs for you), so the
# container VM's memory limit isn't a factor and the host's warm target/ and node_modules
# are reused. This image only packages the resulting crates/ui/dist/ — which is why
# .dockerignore must not exclude it.
#
# The build output is a static SPA that talks to pipa-backend from the browser
# (crates/ui/src/api.rs's API_BASE is a compile-time localhost:8080 constant, so it reaches
# pipa-backend via the host port mapping — no container-to-container wiring needed), so
# serving it needs nothing beyond a static file server.
#
# static-web-server's `2` image is `FROM scratch` (a single ~4MB binary, no shell or package
# manager, multi-arch amd64/arm64 like the other stages) and listens on port 80. The dashboard
# is a client-routed SPA (/login, /users, /query, ...), so SERVER_FALLBACK_PAGE serves
# index.html, with a 200, for any path that isn't a file — otherwise reloading or deep-linking
# to a client route would 404. Unlike SERVER_ROOT it must be an absolute path (it is not
# resolved against the root).
FROM joseluisq/static-web-server:2 AS ui
COPY crates/ui/dist /public
ENV SERVER_ROOT=/public \
    SERVER_FALLBACK_PAGE=/public/index.html
EXPOSE 80
