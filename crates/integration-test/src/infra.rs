//! The containers every test shares: RustFS, Postgres and Redis.
//!
//! Started once, by whichever test needs them first, and removed when the test process exits;
//! tests only see their [`Endpoints`]. A test that needs something of its own gets
//! it from these (a bucket, a database), never a container of its own.

use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, bail};
use chrono::Utc;
use pipa_catalog_proxy::sigv4::{Signer, amz_date, sha256_hex};
use sqlx::{Connection, PgConnection};
use testcontainers::core::wait::HttpWaitStrategy;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, CopyTargetOptions, GenericImage, ImageExt};

/// Same images (and credentials) as `docker-compose.yml`.
const RUSTFS_IMAGE: (&str, &str) = ("rustfs/rustfs", "1.0.1");
const POSTGRES_IMAGE: (&str, &str) = ("postgres", "18.6-alpine");
const REDIS_IMAGE: (&str, &str) = ("redis", "8.2-alpine");

pub const S3_ACCESS_KEY: &str = "rustfsadmin";
pub const S3_SECRET_KEY: &str = "rustfsadmin";
pub const S3_REGION: &str = "ap-southeast-3";
pub const PG_USER: &str = "pipa";
pub const PG_PASSWORD: &str = "pipa";

/// Where the shared services can be reached from the test process (and the binaries it starts).
#[derive(Debug, Clone)]
pub struct Endpoints {
    /// `http://host:port` of RustFS' S3 API (and Iceberg catalog).
    pub s3: String,
    pub postgres_host: String,
    pub postgres_port: u16,
    pub redis_url: String,
}

/// The shared services, started on first use.
///
/// The built-in test harness can't run code after the last test, and a static is never dropped,
/// so the containers are held by a thread of their own (with the runtime they were started on)
/// that parks for the rest of the process, and removed by an `atexit` hook. A first test that
/// finds them not started yet starts them, and every other test waits for that.
pub fn endpoints() -> &'static Endpoints {
    static ENDPOINTS: OnceLock<Result<Endpoints, String>> = OnceLock::new();
    ENDPOINTS
        .get_or_init(|| {
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .name("containers".to_string())
                .spawn(move || {
                    let runtime = tokio::runtime::Builder::new_multi_thread()
                        .enable_all()
                        .build()
                        .expect("tokio runtime");
                    match runtime.block_on(start()) {
                        Ok((endpoints, containers)) => {
                            crate::cleanup::remove_containers_at_exit(
                                containers
                                    .iter()
                                    .map(|container| container.id().to_string()),
                            );
                            let _ = sender.send(Ok(endpoints));
                            // Keeps the containers (and the runtime they run on) alive.
                            loop {
                                std::thread::park();
                            }
                        }
                        Err(err) => {
                            let _ = sender.send(Err(format!("{err:#}")));
                        }
                    }
                })
                .expect("spawning the containers thread");
            receiver.recv().expect("the containers thread died")
        })
        .as_ref()
        .unwrap_or_else(|err| {
            panic!("starting the test containers failed (is Docker running?): {err}")
        })
}

/// Starts the three containers in parallel and waits until each answers.
async fn start() -> anyhow::Result<(Endpoints, Vec<ContainerAsync<GenericImage>>)> {
    let (rustfs, postgres, redis) =
        tokio::try_join!(start_rustfs(), start_postgres(), start_redis())?;

    let endpoints = Endpoints {
        s3: format!(
            "http://{}:{}",
            host(&rustfs).await?,
            rustfs.get_host_port_ipv4(9000.tcp()).await?
        ),
        postgres_host: host(&postgres).await?,
        postgres_port: postgres.get_host_port_ipv4(5432.tcp()).await?,
        redis_url: format!(
            "redis://{}:{}",
            host(&redis).await?,
            redis.get_host_port_ipv4(6379.tcp()).await?
        ),
    };
    wait_for_postgres(&endpoints).await?;
    Ok((endpoints, vec![rustfs, postgres, redis]))
}

/// The host the containers' published ports are on, as an address rather than `localhost`: that
/// can resolve to `::1` first, where the published ports may not listen.
async fn host(container: &ContainerAsync<GenericImage>) -> anyhow::Result<String> {
    let host = container.get_host().await?.to_string();
    Ok(if host == "localhost" {
        "127.0.0.1".to_string()
    } else {
        host
    })
}

async fn start_rustfs() -> anyhow::Result<ContainerAsync<GenericImage>> {
    GenericImage::new(RUSTFS_IMAGE.0, RUSTFS_IMAGE.1)
        .with_exposed_port(9000.tcp())
        .with_wait_for(WaitFor::http(
            HttpWaitStrategy::new("/health/ready")
                .with_port(9000.tcp())
                .with_expected_status_code(200u16),
        ))
        .with_env_var("RUSTFS_ACCESS_KEY", S3_ACCESS_KEY)
        .with_env_var("RUSTFS_SECRET_KEY", S3_SECRET_KEY)
        .with_cmd(["/data"])
        .with_startup_timeout(Duration::from_secs(120))
        .start()
        .await
        .context("starting RustFS (is Docker running?)")
}

/// `wal_level=logical` for the replication slots `pipa-ingestion` creates, plenty of slots for
/// every test's source, and a `pg_hba.conf` line that lets the role open a `replication`
/// connection from outside the container (the image only allows `all` databases, which doesn't
/// cover `replication`).
async fn start_postgres() -> anyhow::Result<ContainerAsync<GenericImage>> {
    GenericImage::new(POSTGRES_IMAGE.0, POSTGRES_IMAGE.1)
        .with_exposed_port(5432.tcp())
        .with_env_var("POSTGRES_USER", PG_USER)
        .with_env_var("POSTGRES_PASSWORD", PG_PASSWORD)
        .with_env_var("POSTGRES_DB", "postgres")
        .with_cmd([
            "postgres",
            "-c",
            "wal_level=logical",
            "-c",
            "max_replication_slots=512",
            "-c",
            "max_wal_senders=512",
            "-c",
            "max_connections=1024",
        ])
        .with_copy_to(
            CopyTargetOptions::new("/docker-entrypoint-initdb.d/10-replication.sh")
                .with_mode(0o755),
            b"#!/bin/sh\necho 'host replication all all scram-sha-256' >> \"$PGDATA/pg_hba.conf\"\n"
                .to_vec(),
        )
        .with_startup_timeout(Duration::from_secs(120))
        .start()
        .await
        .context("starting Postgres")
}

async fn start_redis() -> anyhow::Result<ContainerAsync<GenericImage>> {
    GenericImage::new(REDIS_IMAGE.0, REDIS_IMAGE.1)
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .with_cmd(["redis-server", "--save", "", "--appendonly", "no"])
        .with_startup_timeout(Duration::from_secs(120))
        .start()
        .await
        .context("starting Redis")
}

/// Postgres restarts once during its first start (the init scripts run on a throwaway server),
/// so a container that is "running" can still refuse connections for a moment.
async fn wait_for_postgres(endpoints: &Endpoints) -> anyhow::Result<()> {
    let url = admin_url(endpoints);
    let mut last = None;
    for _ in 0..120 {
        match PgConnection::connect(&url).await {
            Ok(connection) => {
                connection.close().await.ok();
                return Ok(());
            }
            Err(err) => last = Some(err),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    bail!("Postgres never accepted a connection: {}", last.unwrap())
}

/// Connection URL of the maintenance database, to create per-test databases from.
pub fn admin_url(endpoints: &Endpoints) -> String {
    format!(
        "postgres://{PG_USER}:{PG_PASSWORD}@{}:{}/postgres",
        endpoints.postgres_host, endpoints.postgres_port
    )
}

/// Sends a SigV4-signed request with an empty body to RustFS' S3 API.
pub async fn s3_request(method: reqwest::Method, path: &str) -> anyhow::Result<reqwest::Response> {
    let endpoints = endpoints();
    let host = endpoints.s3.trim_start_matches("http://");
    let now = Utc::now();
    let date = amz_date(now);
    let payload_hash = sha256_hex(b"");
    let authorization = Signer::new(
        S3_ACCESS_KEY.to_string(),
        S3_SECRET_KEY.to_string(),
        S3_REGION.to_string(),
        "s3".to_string(),
    )
    .authorization(
        method.as_str(),
        path,
        "",
        &[
            ("host", host),
            ("x-amz-content-sha256", &payload_hash),
            ("x-amz-date", &date),
        ],
        &payload_hash,
        now,
    );
    Ok(reqwest::Client::new()
        .request(method, format!("{}{path}", endpoints.s3))
        .header("x-amz-content-sha256", payload_hash)
        .header("x-amz-date", date)
        .header("authorization", authorization)
        .send()
        .await?)
}

/// Creates the S3 bucket `name` (the stack's `create-bucket` step). `pipa-backend` and
/// `pipa-ingestion` enable the Iceberg catalog on it themselves at startup.
pub async fn create_bucket(name: &str) -> anyhow::Result<()> {
    let response = s3_request(reqwest::Method::PUT, &format!("/{name}"))
        .await
        .with_context(|| format!("creating bucket {name}"))?;
    if !response.status().is_success() {
        bail!(
            "creating bucket {name} failed with {}: {}",
            response.status(),
            response.text().await.unwrap_or_default()
        );
    }
    Ok(())
}
