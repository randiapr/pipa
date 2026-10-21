// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! Committing a snapshot built by hand to the catalog.
//!
//! iceberg 0.10 has no rewrite (replace) action, and the pieces that would let one be added from
//! outside the crate — `TransactionAction`, `TableCommit::builder().build()` — are crate-private.
//! Every piece of the commit itself is public though (`TableUpdate`, `TableRequirement`, and the
//! REST protocol's `CommitTableRequest`), so [`RestSnapshotCommitter`] sends that request to
//! the table's REST endpoint itself, exactly as `RestCatalog::update_table` would.

use async_trait::async_trait;
use iceberg::{TableIdent, TableRequirement, TableUpdate};
use iceberg_catalog_rest::CommitTableRequest;
use reqwest::StatusCode;
use tokio::sync::OnceCell;

use crate::write::IcebergCatalogConfig;

/// Why a commit didn't land.
#[derive(Debug, thiserror::Error)]
pub enum CommitError {
    /// A requirement failed (someone else committed first): nothing was applied, and the
    /// commit may be rebuilt on the new state and retried.
    #[error("commit conflict: {0}")]
    Conflict(String),
    /// Rejected outright: nothing was applied.
    #[error("commit rejected: {0}")]
    Rejected(String),
    /// It is not known whether the commit was applied (a timeout, a server error). Files it
    /// references must be left alone: they may now be part of the table.
    #[error("commit state unknown: {0}")]
    Unknown(String),
}

/// Port: atomically applies `updates` to a table if `requirements` still hold.
#[async_trait]
pub trait SnapshotCommitter: Send + Sync {
    async fn commit(
        &self,
        table: &TableIdent,
        requirements: Vec<TableRequirement>,
        updates: Vec<TableUpdate>,
    ) -> Result<(), CommitError>;
}

/// [`SnapshotCommitter`] for the Iceberg REST catalog `IcebergCatalogConfig` points at: the
/// same one the writer's `RestCatalog` uses (through the same in-process signing proxy).
pub struct RestSnapshotCommitter {
    client: reqwest::Client,
    uri: String,
    warehouse: String,
    /// `{uri}/v1[/{prefix}]`, resolved from the catalog's `/v1/config` on first use.
    base: OnceCell<String>,
}

impl RestSnapshotCommitter {
    pub fn new(config: &IcebergCatalogConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            uri: config.uri.trim_end_matches('/').to_string(),
            warehouse: config.warehouse.clone(),
            base: OnceCell::new(),
        }
    }

    /// Resolves the catalog's URI and path prefix the way `RestCatalog` does: the server's
    /// `overrides` win over the client's settings, which win over its `defaults`.
    async fn base(&self) -> Result<&str, CommitError> {
        self.base
            .get_or_try_init(|| async {
                let url = reqwest::Url::parse_with_params(
                    &format!("{}/v1/config", self.uri),
                    [("warehouse", self.warehouse.as_str())],
                )
                .map_err(|err| CommitError::Rejected(err.to_string()))?;
                let response = self
                    .client
                    .get(url)
                    .send()
                    .await
                    .map_err(|err| CommitError::Rejected(format!("catalog config: {err}")))?;
                if !response.status().is_success() {
                    return Err(CommitError::Rejected(format!(
                        "catalog config: HTTP {}",
                        response.status()
                    )));
                }
                let body = response
                    .bytes()
                    .await
                    .map_err(|err| CommitError::Rejected(format!("catalog config: {err}")))?;
                let config: CatalogConfig = serde_json::from_slice(&body)
                    .map_err(|err| CommitError::Rejected(format!("catalog config: {err}")))?;
                Ok(config.base(&self.uri))
            })
            .await
            .map(String::as_str)
    }
}

/// The parts of the REST catalog's `GET /v1/config` response that shape its URLs.
#[derive(Debug, Default, serde::Deserialize)]
struct CatalogConfig {
    #[serde(default)]
    defaults: std::collections::HashMap<String, String>,
    #[serde(default)]
    overrides: std::collections::HashMap<String, String>,
}

impl CatalogConfig {
    fn base(&self, uri: &str) -> String {
        let uri = self
            .overrides
            .get("uri")
            .map(|uri| uri.trim_end_matches('/'))
            .unwrap_or(uri);
        match self
            .overrides
            .get("prefix")
            .or_else(|| self.defaults.get("prefix"))
        {
            Some(prefix) if !prefix.is_empty() => format!("{uri}/v1/{prefix}"),
            _ => format!("{uri}/v1"),
        }
    }
}

#[async_trait]
impl SnapshotCommitter for RestSnapshotCommitter {
    async fn commit(
        &self,
        table: &TableIdent,
        requirements: Vec<TableRequirement>,
        updates: Vec<TableUpdate>,
    ) -> Result<(), CommitError> {
        let url = format!(
            "{}/namespaces/{}/tables/{}",
            self.base().await?,
            table.namespace().to_url_string(),
            table.name()
        );
        let body = serde_json::to_vec(&CommitTableRequest {
            identifier: Some(table.clone()),
            requirements,
            updates,
        })
        .map_err(|err| CommitError::Rejected(err.to_string()))?;

        let response = self
            .client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            // The request may have reached the catalog before the connection failed.
            .map_err(|err| CommitError::Unknown(err.to_string()))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let detail = response.text().await.unwrap_or_default();
        let message = format!("HTTP {status}: {detail}");
        Err(match status {
            StatusCode::CONFLICT => CommitError::Conflict(message),
            status if status.is_server_error() => CommitError::Unknown(message),
            _ => CommitError::Rejected(message),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn the_server_prefix_and_uri_overrides_shape_the_base_url() {
        let none = CatalogConfig::default();
        assert_eq!(none.base("http://c"), "http://c/v1");

        let defaults = CatalogConfig {
            defaults: HashMap::from([("prefix".to_string(), "wh".to_string())]),
            ..Default::default()
        };
        assert_eq!(defaults.base("http://c"), "http://c/v1/wh");

        let overrides = CatalogConfig {
            defaults: HashMap::from([("prefix".to_string(), "wh".to_string())]),
            overrides: HashMap::from([
                ("prefix".to_string(), "bucket".to_string()),
                ("uri".to_string(), "http://other/".to_string()),
            ]),
        };
        assert_eq!(overrides.base("http://c"), "http://other/v1/bucket");
    }
}
