// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! `pipa-catalog-auth`: SigV4 signing for RustFS's embedded Iceberg REST catalog ("S3 Tables"),
//! which rejects unsigned requests.
//!
//! - [`SigV4AuthManager`] plugs into `iceberg-catalog-rest`'s `AuthManager` API
//!   (`RestCatalogBuilder::with_auth_manager`, new in iceberg-rust 0.11), so every catalog request
//!   is signed in-process just before it is sent. Table data/metadata file IO doesn't go through
//!   the catalog and isn't affected.
//! - [`sigv4::Signer::sign_request`] signs a plain `reqwest` request, for the catalog calls pipa
//!   makes outside `RestCatalog` (enabling S3 Tables below, the maintenance snapshot commit).
//! - [`enable_s3_tables_with_retry`] turns RustFS's "S3 Tables" feature, opt-in per bucket, on
//!   with an idempotent signed `PUT /iceberg/v1/buckets/{bucket}`, so the catalog works without a
//!   manual step.
//!
//! iceberg-rust 0.11 has the hook but no SigV4 signer of its own (apache/iceberg-rust#3082 is
//! unreleased), hence [`sigv4`], implemented directly on `hmac`/`sha2` — no AWS SDK or crate.
//! It depends on no other workspace crate.

pub mod sigv4;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use iceberg::{Error, ErrorKind};
use iceberg_catalog_rest::{AuthManager, AuthSession, HttpClient, HttpRequest};
use reqwest::Url;

use sigv4::Signer;

/// The SigV4 signing name RustFS's catalog expects.
pub const DEFAULT_SERVICE: &str = "s3";

/// The Iceberg REST catalog URI RustFS serves next to its S3 API at `endpoint`.
pub fn rustfs_catalog_uri(endpoint: &str) -> String {
    format!("{}/iceberg", endpoint.trim_end_matches('/'))
}

/// [`AuthManager`] whose sessions SigV4-sign every catalog request, the `/v1/config` handshake
/// included. Build one per catalog: `iceberg-catalog-rest` doesn't allow sharing a manager.
#[derive(Debug)]
pub struct SigV4AuthManager {
    signer: Signer,
}

impl SigV4AuthManager {
    pub fn new(signer: Signer) -> Self {
        Self { signer }
    }
}

#[async_trait]
impl AuthManager for SigV4AuthManager {
    async fn init_session(
        &self,
        _client: &HttpClient,
        _props: &HashMap<String, String>,
    ) -> iceberg::Result<Box<dyn AuthSession>> {
        Ok(Box::new(SigV4Session {
            signer: self.signer.clone(),
        }))
    }

    async fn catalog_session(
        &self,
        _client: &HttpClient,
        _props: &HashMap<String, String>,
    ) -> iceberg::Result<Arc<dyn AuthSession>> {
        Ok(Arc::new(SigV4Session {
            signer: self.signer.clone(),
        }))
    }
}

/// Signs one catalog request at a time; holds nothing but the credentials.
#[derive(Debug)]
struct SigV4Session {
    signer: Signer,
}

#[async_trait]
impl AuthSession for SigV4Session {
    async fn authenticate(&self, request: &mut HttpRequest) -> iceberg::Result<()> {
        let url = Url::parse(request.url_str()).map_err(|err| {
            Error::new(
                ErrorKind::DataInvalid,
                format!("catalog request URL can't be signed: {err}"),
            )
        })?;
        // Catalog requests are small JSON documents, always buffered; copied out so the
        // headers can be borrowed mutably below.
        let body = request
            .body()
            .as_bytes()
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::FeatureUnsupported,
                    sigv4::StreamingBody.to_string(),
                )
            })?
            .to_vec();
        let method = request.method().clone();
        self.signer
            .sign(&method, &url, request.headers_mut(), &body, Utc::now());
        Ok(())
    }
}

/// Enables RustFS's "S3 Tables" feature (its embedded Iceberg REST catalog) on `bucket`, at the
/// RustFS whose S3 API is at `endpoint`. Idempotent.
/// https://docs.rustfs.com/en/administration/data/s3-tables
pub async fn enable_s3_tables(
    client: &reqwest::Client,
    endpoint: &str,
    bucket: &str,
    signer: &Signer,
) -> anyhow::Result<()> {
    let url = format!("{}/v1/buckets/{bucket}", rustfs_catalog_uri(endpoint));
    let mut request = client.put(url).build()?;
    signer.sign_request(&mut request)?;
    let response = client.execute(request).await?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("enabling S3 Tables on bucket {bucket:?} failed with {status}: {body}");
    }
    Ok(())
}

/// [`enable_s3_tables`], retried for about a minute while RustFS is still starting or the
/// bucket doesn't exist yet.
pub async fn enable_s3_tables_with_retry(
    endpoint: &str,
    bucket: &str,
    signer: &Signer,
) -> anyhow::Result<()> {
    const ATTEMPTS: u32 = 30;
    let client = reqwest::Client::new();
    for attempt in 1..=ATTEMPTS {
        match enable_s3_tables(&client, endpoint, bucket, signer).await {
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

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use axum::Router;
    use axum::body::{Bytes, to_bytes};
    use axum::extract::{Request, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::{IntoResponse, Response};
    use iceberg::{Catalog, CatalogBuilder, NamespaceIdent};
    use iceberg_catalog_rest::{REST_CATALOG_PROP_URI, RestCatalogBuilder};

    use super::*;
    use sigv4::sha256_hex;

    fn signer() -> Signer {
        Signer::new(
            "rustfsadmin".to_string(),
            "rustfssecret".to_string(),
            "ap-southeast-3".to_string(),
            DEFAULT_SERVICE.to_string(),
        )
    }

    /// What the fake RustFS saw: each request's method and path, and whether it was signed
    /// correctly.
    type Seen = Arc<Mutex<Vec<(String, String, bool)>>>;

    /// Recomputes the signature the way RustFS checks it, from what arrived on the wire.
    fn signed_correctly(
        method: &str,
        uri: &axum::http::Uri,
        headers: &HeaderMap,
        body: &Bytes,
    ) -> bool {
        let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
        let (Some(host), Some(date), Some(hash), Some(authorization)) = (
            header("host"),
            header("x-amz-date"),
            header("x-amz-content-sha256"),
            header("authorization"),
        ) else {
            return false;
        };
        let Ok(now) = chrono::NaiveDateTime::parse_from_str(date, "%Y%m%dT%H%M%SZ") else {
            return false;
        };
        hash == sha256_hex(body)
            && authorization
                == signer().authorization(
                    method,
                    uri.path(),
                    uri.query().unwrap_or_default(),
                    &[
                        ("host", host),
                        ("x-amz-content-sha256", hash),
                        ("x-amz-date", date),
                    ],
                    hash,
                    now.and_utc(),
                )
    }

    /// A RustFS stand-in: answers the handful of catalog routes the tests use, and only when
    /// the request is signed.
    async fn spawn_fake_rustfs() -> (String, Seen) {
        async fn handle(State(seen): State<Seen>, request: Request) -> Response {
            let (parts, body) = request.into_parts();
            let body = to_bytes(body, usize::MAX).await.unwrap();
            let ok = signed_correctly(parts.method.as_str(), &parts.uri, &parts.headers, &body);
            seen.lock()
                .unwrap()
                .push((parts.method.to_string(), parts.uri.path().to_string(), ok));
            if !ok {
                return (StatusCode::FORBIDDEN, "SignatureDoesNotMatch").into_response();
            }
            let json = match (parts.method.as_str(), parts.uri.path()) {
                ("GET", "/iceberg/v1/config") => r#"{"defaults":{},"overrides":{}}"#.to_string(),
                ("GET", "/iceberg/v1/namespaces") => r#"{"namespaces":[["cdc"]]}"#.to_string(),
                ("POST", "/iceberg/v1/namespaces") => String::from_utf8(body.to_vec()).unwrap(),
                ("PUT", "/iceberg/v1/buckets/pipa") => "{}".to_string(),
                _ => return StatusCode::NOT_FOUND.into_response(),
            };
            ([("content-type", "application/json")], json).into_response()
        }

        let seen = Seen::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().fallback(handle).with_state(seen.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), seen)
    }

    #[tokio::test]
    async fn the_rest_catalog_signs_every_request_through_the_auth_manager() {
        let (endpoint, seen) = spawn_fake_rustfs().await;
        let catalog = RestCatalogBuilder::default()
            .with_auth_manager(SigV4AuthManager::new(signer()))
            .load(
                "pipa",
                HashMap::from([(
                    REST_CATALOG_PROP_URI.to_string(),
                    rustfs_catalog_uri(&endpoint),
                )]),
            )
            .await
            .unwrap();

        let namespaces = catalog.list_namespaces(None).await.unwrap();
        assert_eq!(namespaces, vec![NamespaceIdent::new("cdc".to_string())]);
        // A request with a JSON body: the payload hash must cover it.
        catalog
            .create_namespace(&NamespaceIdent::new("new".to_string()), HashMap::new())
            .await
            .unwrap();

        let seen = seen.lock().unwrap();
        assert!(seen.iter().all(|(_, _, ok)| *ok), "{seen:?}");
        let routes: Vec<_> = seen
            .iter()
            .map(|(method, path, _)| format!("{method} {path}"))
            .collect();
        assert!(
            routes.contains(&"GET /iceberg/v1/config".to_string()),
            "{routes:?}"
        );
        assert!(
            routes.contains(&"POST /iceberg/v1/namespaces".to_string()),
            "{routes:?}"
        );
    }

    #[tokio::test]
    async fn an_unsigned_catalog_is_refused_by_the_fake() {
        let (endpoint, seen) = spawn_fake_rustfs().await;
        // `load` may defer the `/v1/config` handshake to the first call, so make one.
        let result = async {
            RestCatalogBuilder::default()
                .load(
                    "pipa",
                    HashMap::from([(
                        REST_CATALOG_PROP_URI.to_string(),
                        rustfs_catalog_uri(&endpoint),
                    )]),
                )
                .await?
                .list_namespaces(None)
                .await
        }
        .await;
        assert!(result.is_err());
        assert!(seen.lock().unwrap().iter().all(|(_, _, ok)| !*ok));
    }

    #[tokio::test]
    async fn enable_s3_tables_sends_a_signed_put_for_the_bucket() {
        let (endpoint, seen) = spawn_fake_rustfs().await;
        enable_s3_tables(
            &reqwest::Client::new(),
            &format!("{endpoint}/"),
            "pipa",
            &signer(),
        )
        .await
        .unwrap();
        assert_eq!(
            *seen.lock().unwrap(),
            vec![(
                "PUT".to_string(),
                "/iceberg/v1/buckets/pipa".to_string(),
                true
            )]
        );

        let err = enable_s3_tables(&reqwest::Client::new(), &endpoint, "other", &signer())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("404"), "{err}");
    }

    #[tokio::test]
    async fn enable_s3_tables_with_retry_returns_on_the_first_success() {
        let (endpoint, seen) = spawn_fake_rustfs().await;
        enable_s3_tables_with_retry(&endpoint, "pipa", &signer())
            .await
            .unwrap();
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn the_catalog_uri_is_rustfs_iceberg_path() {
        assert_eq!(
            rustfs_catalog_uri("http://rustfs:9000/"),
            "http://rustfs:9000/iceberg"
        );
    }
}
