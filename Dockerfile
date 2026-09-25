# syntax=docker/dockerfile:1

# Builds pipa-ingestion and pipa-backend (native crates only — pipa-ui targets wasm32 and is
# built/served separately via Trunk, see crates/ui/) in one builder stage so their
# shared workspace dependencies (datafusion, iceberg, sqlx, ...) are compiled once,
# then copies each release binary into its own minimal distroless runtime image.
#
# Build a single service with: docker build --target ingestion -t pipa-ingestion .
#                               docker build --target backend -t pipa-backend .
# (docker-compose.yml does this via each service's `build.target`.)

FROM rust:1.98.1-slim-trixie AS builder
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
