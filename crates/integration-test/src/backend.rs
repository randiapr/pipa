// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! A `pipa-backend` process and an HTTP client for it.

use std::net::TcpListener;
use std::time::Duration;

use anyhow::{Context, bail};
use reqwest::Method;
use serde::Serialize;

use crate::binaries::{self, BACKEND};
use crate::infra::{self, S3_ACCESS_KEY, S3_REGION, S3_SECRET_KEY};
use crate::process::Process;
use crate::reply::Reply;
use crate::wait;

pub const ADMIN_USERNAME: &str = "admin";
pub const ADMIN_PASSWORD: &str = "admin-password";
pub const JWT_SECRET: &str = "integration-test-secret-0123456789abcdef";

/// How a backend is configured; `None` leaves the variable unset, which is how the startup
/// failures are provoked.
#[derive(Debug, Clone)]
pub struct BackendConfig {
    pub bucket: String,
    pub jwt_secret: Option<String>,
    pub admin_username: Option<String>,
    pub admin_password: Option<String>,
    /// Shared cache and (with ingestion) coordination in Redis; otherwise the in-memory cache.
    pub redis_url: Option<String>,
    /// Seconds a query result / table list stays cached. 0 (the default here) turns the cache
    /// off, so a test reads what Iceberg holds right now.
    pub query_ttl_secs: u64,
    pub tables_ttl_secs: u64,
    pub cors_allowed_origins: Option<String>,
    /// `BACKEND_LISTEN` to use instead of a free port.
    pub listen: Option<String>,
    /// Any other environment variable (e.g. `CACHE_MAX_ENTRY_BYTES`).
    pub extra_env: Vec<(String, String)>,
}

impl BackendConfig {
    pub fn new(bucket: &str) -> Self {
        Self {
            bucket: bucket.to_string(),
            jwt_secret: Some(JWT_SECRET.to_string()),
            admin_username: Some(ADMIN_USERNAME.to_string()),
            admin_password: Some(ADMIN_PASSWORD.to_string()),
            redis_url: None,
            query_ttl_secs: 0,
            tables_ttl_secs: 0,
            cors_allowed_origins: None,
            listen: None,
            extra_env: Vec::new(),
        }
    }

    fn envs(&self, listen: &str) -> Vec<(String, String)> {
        let endpoints = infra::endpoints();
        let mut envs = vec![
            ("BACKEND_LISTEN".to_string(), listen.to_string()),
            ("RUSTFS_ENDPOINT".to_string(), endpoints.s3.clone()),
            ("RUSTFS_BUCKET".to_string(), self.bucket.clone()),
            ("RUSTFS_REGION".to_string(), S3_REGION.to_string()),
            (
                "RUSTFS_ACCESS_KEY_ID".to_string(),
                S3_ACCESS_KEY.to_string(),
            ),
            (
                "RUSTFS_SECRET_ACCESS_KEY".to_string(),
                S3_SECRET_KEY.to_string(),
            ),
            ("RUSTFS_ALLOW_HTTP".to_string(), "true".to_string()),
            ("ICEBERG_CATALOG_WAREHOUSE".to_string(), self.bucket.clone()),
            (
                "CACHE_QUERY_TTL_SECS".to_string(),
                self.query_ttl_secs.to_string(),
            ),
            (
                "CACHE_TABLES_TTL_SECS".to_string(),
                self.tables_ttl_secs.to_string(),
            ),
            ("RUST_LOG".to_string(), "info".to_string()),
        ];
        let optional = [
            ("JWT_SECRET", &self.jwt_secret),
            ("PIPA_ADMIN_USERNAME", &self.admin_username),
            ("PIPA_ADMIN_PASSWORD", &self.admin_password),
            ("REDIS_URL", &self.redis_url),
            ("CORS_ALLOWED_ORIGINS", &self.cors_allowed_origins),
        ];
        envs.extend(
            optional
                .into_iter()
                .filter_map(|(key, value)| Some((key.to_string(), value.clone()?))),
        );
        envs.extend(self.extra_env.clone());
        envs
    }
}

pub struct Backend {
    process: Process,
    http: reqwest::Client,
    pub base_url: String,
    pub config: BackendConfig,
}

impl Backend {
    /// Starts a backend and waits until `/healthz` answers. Fails with the backend's log if it
    /// exits or doesn't come up.
    pub async fn start(config: BackendConfig) -> anyhow::Result<Self> {
        let (process, base_url) = spawn(&config)?;
        let mut backend = Self {
            process,
            // No idle connections are kept: a backend of the shared stack is used from the runtime
            // of one test after another, and a pooled connection belongs to the runtime that made it.
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .pool_max_idle_per_host(0)
                .build()?,
            base_url,
            config,
        };
        backend.wait_until_healthy().await?;
        Ok(backend)
    }

    async fn wait_until_healthy(&mut self) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
        loop {
            if !self.process.is_running() {
                bail!(
                    "pipa-backend exited during startup:\n{}",
                    self.process.log_tail(40)
                );
            }
            if self
                .http
                .get(format!("{}/healthz", self.base_url))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success())
            {
                return Ok(());
            }
            if tokio::time::Instant::now() > deadline {
                bail!(
                    "pipa-backend did not answer /healthz:\n{}",
                    self.process.log_tail(40)
                );
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// Stops the backend gracefully and starts a new one on the same bucket, as a restart does
    /// (on a new port: `base_url` changes). Tokens stay valid, as they only name the account.
    pub async fn restart(&mut self) -> anyhow::Result<()> {
        self.process.terminate().await;
        let (process, base_url) = spawn(&self.config)?;
        self.process = process;
        self.base_url = base_url;
        self.wait_until_healthy().await
    }

    /// Starts a backend that is expected to refuse to start: returns its log if it exited with a
    /// failure within the time allowed, and fails if it kept running or exited successfully.
    pub async fn start_expecting_failure(config: BackendConfig) -> anyhow::Result<String> {
        let (mut process, _) = spawn(&config)?;
        let status = process
            .exited_within(Duration::from_secs(60))
            .await
            .context("pipa-backend kept running although its configuration is invalid")?;
        if status.success() {
            bail!("pipa-backend exited successfully");
        }
        Ok(process.log())
    }

    /// Stops the backend gracefully.
    pub async fn stop(mut self) {
        self.process.terminate().await;
    }

    pub fn log(&self) -> String {
        self.process.log()
    }

    pub fn request(&self, method: Method, path: &str) -> Request {
        Request {
            builder: self
                .http
                .request(method, format!("{}{path}", self.base_url)),
        }
    }

    pub fn get(&self, path: &str) -> Request {
        self.request(Method::GET, path)
    }

    pub fn post(&self, path: &str) -> Request {
        self.request(Method::POST, path)
    }

    pub fn put(&self, path: &str) -> Request {
        self.request(Method::PUT, path)
    }

    pub fn delete(&self, path: &str) -> Request {
        self.request(Method::DELETE, path)
    }

    /// `POST /auth/login`.
    pub async fn login(&self, username: &str, password: &str) -> Reply {
        self.post(pipa_api::path::LOGIN)
            .json(&pipa_api::LoginRequest {
                username: username.to_string(),
                password: password.to_string(),
            })
            .send()
            .await
    }

    /// A bearer token for an account that must exist.
    pub async fn token(&self, username: &str, password: &str) -> String {
        let reply = self.login(username, password).await.ok();
        reply.parse::<pipa_api::LoginData>().token
    }
}

fn spawn(config: &BackendConfig) -> anyhow::Result<(Process, String)> {
    // Bind to port 0 to learn a free one. The port is free again by the time the backend binds
    // it; nothing else on a test machine is racing for ephemeral ports.
    let listen = match &config.listen {
        Some(listen) => listen.clone(),
        None => {
            let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
            format!("127.0.0.1:{port}")
        }
    };
    let process = Process::spawn("backend", &binaries::binary(BACKEND), &config.envs(&listen))?;
    Ok((process, format!("http://{listen}")))
}

/// A request being built; [`Request::send`] never fails on an HTTP error status, only the
/// assertions on the [`Reply`] do.
pub struct Request {
    builder: reqwest::RequestBuilder,
}

impl Request {
    pub fn bearer(mut self, token: &str) -> Self {
        self.builder = self.builder.bearer_auth(token);
        self
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.builder = self.builder.header(name, value);
        self
    }

    pub fn json(mut self, body: &impl Serialize) -> Self {
        self.builder = self.builder.json(body);
        self
    }

    /// A body sent as it is (to send malformed JSON), declared as JSON.
    pub fn raw_json(mut self, body: &str) -> Self {
        self.builder = self
            .builder
            .header("content-type", "application/json")
            .body(body.to_string());
        self
    }

    pub async fn send(self) -> Reply {
        let response = self.builder.send().await.expect("sending the request");
        Reply::from_response(response).await
    }

    /// The response's status, headers (lowercase names) and body, for what the envelope doesn't
    /// cover (CORS headers).
    pub async fn raw(self) -> RawReply {
        let response = self.builder.send().await.expect("sending the request");
        RawReply {
            status: response.status().as_u16(),
            headers: response
                .headers()
                .iter()
                .map(|(name, value)| {
                    (
                        name.as_str().to_string(),
                        value.to_str().unwrap_or_default().to_string(),
                    )
                })
                .collect(),
        }
    }
}

pub struct RawReply {
    pub status: u16,
    pub headers: std::collections::HashMap<String, String>,
}

/// Waits for `backend`'s catalog to answer, i.e. for S3 Tables to be enabled on its bucket: a
/// query that doesn't touch any table is enough to go through the catalog.
pub async fn wait_for_catalog(backend: &Backend, token: &str) {
    wait::eventually("the Iceberg catalog to answer", || async {
        let reply = backend
            .post(pipa_api::path::QUERY)
            .bearer(token)
            .json(&pipa_api::QueryRequest {
                sql: "SELECT 1 AS one".to_string(),
                project_id: None,
            })
            .send()
            .await;
        if reply.status == 200 {
            Ok(())
        } else {
            Err(reply.body.to_string())
        }
    })
    .await
}
