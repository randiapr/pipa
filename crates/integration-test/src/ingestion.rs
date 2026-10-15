//! A `pipa-ingestion` process.

use std::time::Duration;

use crate::binaries::{self, INGESTION};
use crate::infra::{self, S3_ACCESS_KEY, S3_REGION, S3_SECRET_KEY};
use crate::process::Process;

#[derive(Debug, Clone)]
pub struct IngestionConfig {
    pub bucket: String,
    /// Instances find each other (and share the data sources) through Redis.
    pub redis_url: Option<String>,
    /// Names the instance among its peers; must be unique.
    pub instance_id: Option<String>,
    /// Without Redis: this instance's `(index, count)` of the static partitioning.
    pub shard: Option<(u32, u32)>,
}

impl IngestionConfig {
    pub fn new(bucket: &str) -> Self {
        Self {
            bucket: bucket.to_string(),
            redis_url: None,
            instance_id: None,
            shard: None,
        }
    }
}

pub struct Ingestion {
    process: Process,
}

impl Ingestion {
    /// Starts an ingestion instance. It has no HTTP surface, so there is nothing to wait for
    /// beyond it still running a moment later; the tests wait for its effects.
    pub async fn start(config: IngestionConfig) -> anyhow::Result<Self> {
        let endpoints = infra::endpoints();
        let mut envs = vec![
            ("RUSTFS_ENDPOINT", endpoints.s3.clone()),
            ("RUSTFS_BUCKET", config.bucket.clone()),
            ("RUSTFS_REGION", S3_REGION.to_string()),
            ("RUSTFS_ACCESS_KEY_ID", S3_ACCESS_KEY.to_string()),
            ("RUSTFS_SECRET_ACCESS_KEY", S3_SECRET_KEY.to_string()),
            ("RUSTFS_ALLOW_HTTP", "true".to_string()),
            ("ICEBERG_CATALOG_WAREHOUSE", config.bucket.clone()),
            // Short intervals, so a test doesn't wait on the production defaults.
            ("INGESTION_SOURCE_REFRESH_INTERVAL_SECS", "1".to_string()),
            ("INGESTION_COORDINATION_INTERVAL_SECS", "1".to_string()),
            ("INGESTION_BATCH_FLUSH_INTERVAL_MS", "200".to_string()),
            ("RUST_LOG", "info".to_string()),
        ];
        if let Some(url) = &config.redis_url {
            envs.push(("REDIS_URL", url.clone()));
        }
        if let Some(id) = &config.instance_id {
            envs.push(("INGESTION_INSTANCE_ID", id.clone()));
        }
        if let Some((index, count)) = config.shard {
            envs.push(("INGESTION_SHARD_INDEX", index.to_string()));
            envs.push(("INGESTION_SHARD_COUNT", count.to_string()));
        }
        let envs: Vec<(String, String)> = envs
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect();

        let mut process = Process::spawn("ingestion", &binaries::binary(INGESTION), &envs)?;
        tokio::time::sleep(Duration::from_secs(1)).await;
        anyhow::ensure!(
            process.is_running(),
            "pipa-ingestion exited during startup:\n{}",
            process.log_tail(40)
        );
        Ok(Self { process })
    }

    pub fn is_running(&mut self) -> bool {
        self.process.is_running()
    }

    pub fn log(&self) -> String {
        self.process.log()
    }

    pub fn log_tail(&self, lines: usize) -> String {
        self.process.log_tail(lines)
    }

    /// Stops it the way an orchestrator does (SIGTERM): it hands its sources back before exiting.
    pub async fn stop(&mut self) {
        self.process.terminate().await;
    }

    /// Kills it with no warning, like a crash or an out-of-memory kill.
    pub async fn crash(&mut self) {
        self.process.kill().await;
    }
}
