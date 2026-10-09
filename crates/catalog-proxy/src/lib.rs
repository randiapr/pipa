//! `pipa-catalog-proxy`: SigV4 signing in front of RustFS's embedded Iceberg REST catalog
//! ("S3 Tables"). That catalog rejects unsigned requests, and `iceberg-catalog-rest` can't sign
//! them, so its clients talk to a [`CatalogProxy`] instead. Each request is buffered, signed with
//! RustFS's credentials and forwarded unchanged otherwise; table data/metadata file IO never goes
//! through it.
//!
//! Used two ways:
//! - **Embedded** ([`CatalogProxy::serve_embedded`]): `pipa-backend` and `pipa-ingestion` run it
//!   inside their own process on a loopback port, so catalog calls don't cross the network to a
//!   separate service. `iceberg-catalog-rest` 0.10 only accepts a plain `reqwest::Client` (no
//!   request hook to sign with), which is why it is still an HTTP listener rather than a function
//!   call. Upstream iceberg-rust has since merged a pluggable `AuthManager`/`AuthSession` API
//!   (`RestCatalogBuilder::with_auth_manager`, apache/iceberg-rust#2838) but not released it; once
//!   0.11 ships, signing moves into an `AuthManager` built on [`sigv4`] and the loopback listener
//!   goes away.
//! - **Standalone** (the `pipa-catalog-proxy` binary, `main.rs`): the same proxy as its own
//!   service, for catalog clients outside this workspace.
//!
//! It depends on no other workspace crate, and on no AWS SDK or crate: the signing in
//! [`sigv4`] is implemented directly on `hmac`/`sha2`. It exposes no API of its own — it is
//! internal plumbing, not an external-facing service (that stays `pipa-backend`).
//!
//! RustFS's "S3 Tables" feature is opt-in per bucket; [`CatalogProxy::enable_tables`] turns it on
//! with an idempotent signed `PUT /iceberg/v1/buckets/{bucket}`, so the catalog works without a
//! manual step.

pub mod sigv4;

use std::{sync::Arc, time::Duration};

use anyhow::Context;
use axum::{
    Router,
    body::Bytes,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;

use sigv4::{Signer, amz_date, sha256_hex};

/// The SigV4 signing name RustFS's catalog expects.
pub const DEFAULT_SERVICE: &str = "s3";

/// Catalog requests are small JSON documents; anything bigger is refused rather than buffered.
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Headers that must not be forwarded as-is: hop-by-hop ones, ones `reqwest` recomputes, and the
/// ones this proxy sets itself as part of signing.
const DROPPED_HEADERS: [&str; 10] = [
    "host",
    "connection",
    "keep-alive",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "content-length",
    "authorization",
];

/// Signs and forwards Iceberg REST catalog requests to RustFS.
pub struct CatalogProxy {
    client: reqwest::Client,
    /// Upstream base URL without a trailing slash.
    upstream: String,
    /// `Host` header value `reqwest` will send for `upstream`, which is what gets signed.
    host: String,
    signer: Signer,
}

impl CatalogProxy {
    /// A proxy to the RustFS at `endpoint` (its S3 API base URL, e.g. `http://localhost:9000`),
    /// signing with `signer`.
    pub fn new(endpoint: &str, signer: Signer) -> anyhow::Result<Self> {
        let upstream = endpoint.trim_end_matches('/').to_string();
        let url = reqwest::Url::parse(&upstream)
            .with_context(|| format!("RustFS endpoint {upstream:?} is not a valid URL"))?;
        let host = url
            .host_str()
            .with_context(|| format!("RustFS endpoint {upstream:?} has no host"))?
            .to_string();
        let host = match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host,
        };

        Ok(Self {
            client: reqwest::Client::new(),
            upstream,
            host,
            signer,
        })
    }

    /// Upstream base URL, without a trailing slash.
    pub fn upstream(&self) -> &str {
        &self.upstream
    }

    /// Signs and sends one request to the upstream. `path_and_query` is forwarded verbatim; any
    /// `x-amz-*` or `authorization` header in `headers` is replaced by the signed ones.
    async fn send_signed(
        &self,
        method: Method,
        path_and_query: &str,
        mut headers: HeaderMap,
        body: Bytes,
    ) -> reqwest::Result<reqwest::Response> {
        let (path, query) = path_and_query
            .split_once('?')
            .unwrap_or((path_and_query, ""));
        let now = Utc::now();
        let date = amz_date(now);
        let payload_hash = sha256_hex(&body);
        let authorization = self.signer.authorization(
            method.as_str(),
            path,
            query,
            &[
                ("host", &self.host),
                ("x-amz-content-sha256", &payload_hash),
                ("x-amz-date", &date),
            ],
            &payload_hash,
            now,
        );

        for (name, value) in [
            ("x-amz-content-sha256", payload_hash.as_str()),
            ("x-amz-date", date.as_str()),
            (header::AUTHORIZATION.as_str(), authorization.as_str()),
        ] {
            headers.insert(
                HeaderName::from_static(name),
                HeaderValue::from_str(value).expect("signing output is valid header text"),
            );
        }

        self.client
            .request(method, format!("{}{path_and_query}", self.upstream))
            .headers(headers)
            .body(body)
            .send()
            .await
    }

    /// Enables RustFS's "S3 Tables" feature (its embedded Iceberg REST catalog) on `bucket`.
    /// Idempotent. https://docs.rustfs.com/en/administration/data/s3-tables
    pub async fn enable_tables(&self, bucket: &str) -> anyhow::Result<()> {
        let response = self
            .send_signed(
                Method::PUT,
                &format!("/iceberg/v1/buckets/{bucket}"),
                HeaderMap::new(),
                Bytes::new(),
            )
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("enabling S3 Tables on bucket {bucket:?} failed with {status}: {body}");
        }
        Ok(())
    }

    /// [`Self::enable_tables`], retried for about a minute while RustFS is still starting or the
    /// bucket doesn't exist yet.
    pub async fn enable_tables_with_retry(&self, bucket: &str) -> anyhow::Result<()> {
        const ATTEMPTS: u32 = 30;
        for attempt in 1..=ATTEMPTS {
            match self.enable_tables(bucket).await {
                Ok(()) => {
                    tracing::info!(%bucket, "S3 Tables enabled");
                    return Ok(());
                }
                Err(err) if attempt == ATTEMPTS => return Err(err),
                Err(err) => {
                    tracing::warn!(attempt, error = %err, "could not enable S3 Tables yet, retrying");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
        unreachable!("the last attempt returns")
    }

    /// An axum router proxying every request to the upstream.
    pub fn router(self: Arc<Self>) -> Router {
        Router::new().fallback(forward).with_state(self)
    }

    /// Enables S3 Tables on `bucket` (retrying like [`Self::enable_tables_with_retry`]), then
    /// serves the proxy on an ephemeral loopback port inside this process, in a background task
    /// that lives as long as the runtime. Returns the Iceberg REST catalog URI to give
    /// `iceberg-catalog-rest` (`http://127.0.0.1:{port}/iceberg`).
    pub async fn serve_embedded(self, bucket: &str) -> anyhow::Result<String> {
        self.enable_tables_with_retry(bucket).await?;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("failed to bind the embedded catalog proxy")?;
        let addr = listener.local_addr()?;
        let upstream = self.upstream.clone();
        let app = Arc::new(self).router();
        tokio::spawn(async move {
            if let Err(err) = axum::serve(listener, app).await {
                tracing::error!(error = %err, "embedded catalog proxy stopped");
            }
        });
        tracing::info!(%addr, %upstream, "embedded catalog proxy listening");
        Ok(format!("http://{addr}/iceberg"))
    }
}

async fn forward(State(proxy): State<Arc<CatalogProxy>>, request: Request) -> Response {
    match try_forward(&proxy, request).await {
        Ok(response) => response,
        Err((status, message)) => {
            tracing::warn!(%status, %message, "request not proxied");
            (status, message).into_response()
        }
    }
}

async fn try_forward(
    proxy: &CatalogProxy,
    request: Request,
) -> Result<Response, (StatusCode, String)> {
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, MAX_BODY_BYTES)
        .await
        .map_err(|err| (StatusCode::PAYLOAD_TOO_LARGE, err.to_string()))?;

    let mut headers = HeaderMap::new();
    for (name, value) in &parts.headers {
        if !DROPPED_HEADERS.contains(&name.as_str()) && !name.as_str().starts_with("x-amz-") {
            headers.append(name.clone(), value.clone());
        }
    }

    let path_and_query = parts
        .uri
        .path_and_query()
        .map_or("/", |path_and_query| path_and_query.as_str());
    let upstream = proxy
        .send_signed(parts.method, path_and_query, headers, body)
        .await
        .map_err(|err| {
            (
                StatusCode::BAD_GATEWAY,
                format!("upstream request failed: {err}"),
            )
        })?;

    let mut response = Response::builder().status(upstream.status());
    for (name, value) in upstream.headers() {
        if !DROPPED_HEADERS.contains(&name.as_str()) {
            response = response.header(name, value);
        }
    }
    let body = upstream.bytes().await.map_err(|err| {
        (
            StatusCode::BAD_GATEWAY,
            format!("upstream body failed: {err}"),
        )
    })?;
    response
        .body(Body::from(body))
        .map_err(|err| (StatusCode::BAD_GATEWAY, err.to_string()))
}

#[cfg(test)]
mod tests {
    use axum::http::Request as HttpRequest;
    use tower::ServiceExt;

    use super::*;

    /// Starts an upstream that answers with the request it saw, as JSON.
    async fn spawn_echo_upstream() -> String {
        let app = Router::new().fallback(|request: Request| async move {
            let (parts, body) = request.into_parts();
            let body = to_bytes(body, MAX_BODY_BYTES).await.unwrap();
            let headers: serde_json::Map<_, _> = parts
                .headers
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_str().unwrap().into()))
                .collect();
            axum::Json(serde_json::json!({
                "method": parts.method.as_str(),
                "uri": parts.uri.to_string(),
                "headers": headers,
                "body": String::from_utf8(body.to_vec()).unwrap(),
            }))
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn proxy_for(upstream: &str) -> CatalogProxy {
        CatalogProxy::new(
            upstream,
            Signer::new(
                "AKIDEXAMPLE".into(),
                "secret".into(),
                "ap-southeast-3".into(),
                DEFAULT_SERVICE.into(),
            ),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn signs_and_forwards_the_request() {
        let upstream = spawn_echo_upstream().await;
        let app = Arc::new(proxy_for(&upstream)).router();

        let response = app
            .oneshot(
                HttpRequest::post("/iceberg/v1/pipa/namespaces?warehouse=pipa")
                    .header("authorization", "Bearer client-token")
                    .header("x-amz-date", "stale")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"namespace":["cdc"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), MAX_BODY_BYTES)
            .await
            .unwrap();
        let seen: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(seen["method"], "POST");
        assert_eq!(seen["body"], r#"{"namespace":["cdc"]}"#);
        assert!(
            seen["uri"]
                .as_str()
                .unwrap()
                .ends_with("/iceberg/v1/pipa/namespaces?warehouse=pipa")
        );
        assert_eq!(seen["headers"]["content-type"], "application/json");
        assert_eq!(
            seen["headers"]["x-amz-content-sha256"],
            sha256_hex(br#"{"namespace":["cdc"]}"#)
        );
        assert_ne!(seen["headers"]["x-amz-date"], "stale");

        let authorization = seen["headers"]["authorization"].as_str().unwrap();
        assert!(authorization.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/"));
        assert!(authorization.contains("/ap-southeast-3/s3/aws4_request"));
        assert!(authorization.contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date,"));
    }

    /// Starts an upstream that only accepts a signed `PUT /iceberg/v1/buckets/pipa`.
    async fn spawn_tables_upstream() -> String {
        let app = Router::new().route(
            "/iceberg/v1/buckets/pipa",
            axum::routing::put(|headers: HeaderMap| async move {
                if headers.contains_key("authorization") {
                    StatusCode::OK
                } else {
                    StatusCode::FORBIDDEN
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        upstream
    }

    #[tokio::test]
    async fn enables_tables_with_a_signed_put_on_the_bucket() {
        let proxy = proxy_for(&spawn_tables_upstream().await);

        proxy.enable_tables("pipa").await.unwrap();
        let err = proxy.enable_tables("missing").await.unwrap_err();
        assert!(err.to_string().contains("404"), "{err}");
    }

    #[tokio::test]
    async fn serves_embedded_on_a_loopback_port() {
        let upstream = spawn_tables_upstream().await;
        let uri = proxy_for(&upstream).serve_embedded("pipa").await.unwrap();
        assert!(uri.starts_with("http://127.0.0.1:"), "{uri}");
        assert!(uri.ends_with("/iceberg"), "{uri}");

        // Requests through it reach the upstream signed.
        let response = reqwest::Client::new()
            .put(format!("{uri}/v1/buckets/pipa"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn reports_an_unreachable_upstream_as_bad_gateway() {
        let app = Arc::new(proxy_for("http://127.0.0.1:1")).router();
        let response = app
            .oneshot(
                HttpRequest::get("/iceberg/v1/config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn rejects_an_endpoint_without_a_host() {
        let signer = Signer::new("a".into(), "b".into(), "c".into(), "s3".into());
        assert!(CatalogProxy::new("not a url", signer).is_err());
    }
}
