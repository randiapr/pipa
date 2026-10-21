// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! Domain layer: what maintenance does to a target table and when — the configuration, the
//! compaction planner, the [`TableMaintainer`] port and its report. No catalog or object-store
//! specifics; those are [`crate::maintenance::infrastructure`]'s.

use std::fmt;
use std::time::Duration;

use async_trait::async_trait;

use crate::datasource::DataSource;
use crate::write::domain::TargetTable;

const MIB: u64 = 1024 * 1024;

/// How maintenance runs, from `INGESTION_MAINTENANCE_*`/`INGESTION_COMPACTION_*`/
/// `INGESTION_SNAPSHOT_*`/`INGESTION_ORPHAN_*` environment variables.
#[derive(Debug, Clone, PartialEq)]
pub struct MaintenanceConfig {
    /// How often every owned table is maintained; `None` turns maintenance off.
    pub interval: Option<Duration>,
    pub compaction: CompactionPolicy,
    pub expiry: ExpiryPolicy,
    /// How old an unreferenced file must be before it is deleted as an orphan; `None` turns
    /// the sweep off. Must comfortably exceed the longest write-to-commit time, since a file a
    /// commit in flight has written is unreferenced until that commit lands.
    pub orphan_min_age: Option<Duration>,
}

/// When and how data files are rewritten into bigger ones.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionPolicy {
    /// The size compacted files are written up to.
    pub target_file_bytes: u64,
    /// Files under this size are candidates; bigger ones are left alone.
    pub small_file_bytes: u64,
    /// The fewest small files worth rewriting together (at least 2).
    pub min_input_files: usize,
    /// The most bytes one maintenance run of a table rewrites, bounding its IO.
    pub max_input_bytes: u64,
}

/// Which snapshots are expired.
#[derive(Debug, Clone, PartialEq)]
pub struct ExpiryPolicy {
    /// Snapshots older than this are expired…
    pub max_snapshot_age: Duration,
    /// …except the most recent this many, which are always kept (at least 1).
    pub min_snapshots_to_keep: usize,
}

impl Default for MaintenanceConfig {
    fn default() -> Self {
        let target_file_bytes = 128 * MIB;
        Self {
            interval: Some(Duration::from_secs(300)),
            compaction: CompactionPolicy {
                target_file_bytes,
                small_file_bytes: target_file_bytes / 4 * 3,
                min_input_files: 8,
                max_input_bytes: 1024 * MIB,
            },
            expiry: ExpiryPolicy {
                max_snapshot_age: Duration::from_secs(3600),
                min_snapshots_to_keep: 10,
            },
            orphan_min_age: Some(Duration::from_secs(3 * 24 * 3600)),
        }
    }
}

impl MaintenanceConfig {
    pub fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// [`MaintenanceConfig::from_env`] over any variable lookup, so it can be tested without
    /// touching the process environment. Unparsable values fall back to the default.
    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        fn parse<T: std::str::FromStr>(
            lookup: &impl Fn(&str) -> Option<String>,
            name: &str,
        ) -> Option<T> {
            let value = lookup(name)?;
            match value.trim().parse() {
                Ok(parsed) => Some(parsed),
                Err(_) => {
                    tracing::warn!(variable = name, %value, "ignoring an unparsable value");
                    None
                }
            }
        }
        let seconds = |name: &str, default: Option<Duration>| match parse::<u64>(&lookup, name) {
            Some(0) => None,
            Some(secs) => Some(Duration::from_secs(secs)),
            None => default,
        };

        let defaults = Self::default();
        let target_file_bytes = parse(&lookup, "INGESTION_COMPACTION_TARGET_FILE_BYTES")
            .filter(|bytes| *bytes > 0)
            .unwrap_or(defaults.compaction.target_file_bytes);
        let small_file_ratio = parse::<f64>(&lookup, "INGESTION_COMPACTION_SMALL_FILE_RATIO")
            .filter(|ratio| *ratio > 0.0 && *ratio <= 1.0)
            .unwrap_or(0.75);
        Self {
            interval: seconds("INGESTION_MAINTENANCE_INTERVAL_SECS", defaults.interval),
            compaction: CompactionPolicy {
                target_file_bytes,
                small_file_bytes: (target_file_bytes as f64 * small_file_ratio) as u64,
                min_input_files: parse(&lookup, "INGESTION_COMPACTION_MIN_INPUT_FILES")
                    .unwrap_or(defaults.compaction.min_input_files)
                    .max(2),
                max_input_bytes: parse(&lookup, "INGESTION_COMPACTION_MAX_INPUT_BYTES")
                    .filter(|bytes| *bytes > 0)
                    .unwrap_or(defaults.compaction.max_input_bytes),
            },
            expiry: ExpiryPolicy {
                max_snapshot_age: parse(&lookup, "INGESTION_SNAPSHOT_MAX_AGE_SECS")
                    .map(Duration::from_secs)
                    .unwrap_or(defaults.expiry.max_snapshot_age),
                min_snapshots_to_keep: parse(&lookup, "INGESTION_SNAPSHOT_MIN_TO_KEEP")
                    .unwrap_or(defaults.expiry.min_snapshots_to_keep)
                    .max(1),
            },
            orphan_min_age: seconds("INGESTION_ORPHAN_CLEANUP_AGE_SECS", defaults.orphan_min_age),
        }
    }
}

/// A live data file of a table, as the compaction planner sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateFile {
    pub path: String,
    pub size_bytes: u64,
    /// The file's data sequence number: the order commits added the files in.
    pub sequence_number: i64,
}

/// Groups `files` into the bins worth rewriting, each into files of up to the target size.
///
/// Bins are cut from runs of consecutive small files in commit order (a file at or above
/// [`CompactionPolicy::small_file_bytes`] ends a run), so every compacted file covers one
/// contiguous range of commits and its column bounds on `_position`/`_commit_timestamp_us`
/// stay tight — a size-only bin packing would mix old and new changes and blur them. A run is
/// cut greedily into bins of up to [`CompactionPolicy::target_file_bytes`]; a bin is kept when
/// it has at least [`CompactionPolicy::min_input_files`] files, or at least two that together
/// are no longer small. What is left over (typically the newest files) waits for more. Bins
/// are taken oldest first while they fit in [`CompactionPolicy::max_input_bytes`].
pub fn plan_compaction(
    files: &[CandidateFile],
    policy: &CompactionPolicy,
) -> Vec<Vec<CandidateFile>> {
    let mut sorted: Vec<&CandidateFile> = files.iter().collect();
    sorted.sort_by(|a, b| {
        a.sequence_number
            .cmp(&b.sequence_number)
            .then_with(|| a.path.cmp(&b.path))
    });

    let mut runs: Vec<Vec<&CandidateFile>> = vec![Vec::new()];
    for file in sorted {
        if file.size_bytes < policy.small_file_bytes {
            runs.last_mut().expect("never empty").push(file);
        } else if !runs.last().expect("never empty").is_empty() {
            runs.push(Vec::new());
        }
    }

    let worth_it = |bin: &[&CandidateFile]| {
        let bytes: u64 = bin.iter().map(|file| file.size_bytes).sum();
        bin.len() >= 2 && (bin.len() >= policy.min_input_files || bytes >= policy.small_file_bytes)
    };

    let mut bins = Vec::new();
    let mut planned_bytes = 0u64;
    for run in runs {
        let mut bin: Vec<&CandidateFile> = Vec::new();
        let mut bin_bytes = 0u64;
        let mut cut = Vec::new();
        for file in run {
            if !bin.is_empty() && bin_bytes + file.size_bytes > policy.target_file_bytes {
                cut.push(std::mem::take(&mut bin));
                bin_bytes = 0;
            }
            bin_bytes += file.size_bytes;
            bin.push(file);
        }
        cut.push(bin);

        for bin in cut.into_iter().filter(|bin| worth_it(bin)) {
            let bytes: u64 = bin.iter().map(|file| file.size_bytes).sum();
            if planned_bytes + bytes > policy.max_input_bytes {
                return bins;
            }
            planned_bytes += bytes;
            bins.push(bin.into_iter().cloned().collect());
        }
    }
    bins
}

/// What one maintenance run did to a table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MaintenanceReport {
    /// Data files replaced by compaction, and the bytes they held.
    pub files_rewritten: usize,
    pub bytes_rewritten: u64,
    /// Data files compaction wrote in their place.
    pub files_written: usize,
    /// Compaction commits that landed.
    pub compactions: usize,
    pub snapshots_expired: usize,
    /// Files deleted because no remaining snapshot referenced them any more.
    pub files_deleted: usize,
    /// Files deleted because nothing ever referenced them (left by failed commits).
    pub orphans_deleted: usize,
}

impl MaintenanceReport {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

impl fmt::Display for MaintenanceReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} compaction(s) rewrote {} file(s) ({} bytes) into {}; {} snapshot(s) expired; {} file(s) and {} orphan(s) deleted",
            self.compactions,
            self.files_rewritten,
            self.bytes_rewritten,
            self.files_written,
            self.snapshots_expired,
            self.files_deleted,
            self.orphans_deleted
        )
    }
}

/// Errors surfaced while maintaining a table.
#[derive(Debug, thiserror::Error)]
pub enum MaintenanceError {
    #[error("failed to access Iceberg catalog: {0}")]
    Catalog(String),
    #[error("failed to read or write table files: {0}")]
    Io(String),
    #[error("failed to commit: {0}")]
    Commit(String),
}

impl From<iceberg::Error> for MaintenanceError {
    fn from(err: iceberg::Error) -> Self {
        Self::Io(err.to_string())
    }
}

/// Port: keeps a source's target tables fast to read and bounded in size.
///
/// Implemented by [`crate::maintenance::infrastructure::IcebergTableMaintainer`]. Whatever it
/// does must leave every table's checkpoint (see
/// [`crate::write::domain::IcebergWriter::last_committed_position`]) and every row as they
/// were — only how they are stored may change.
#[async_trait]
pub trait TableMaintainer: Send + Sync {
    /// `source`'s target tables.
    async fn tables(&self, source: &DataSource) -> Result<Vec<TargetTable>, MaintenanceError>;

    /// Compacts `table`, expires its old snapshots and deletes the files nothing references
    /// any more.
    async fn maintain(&self, table: &TargetTable) -> Result<MaintenanceReport, MaintenanceError>;
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn policy() -> CompactionPolicy {
        CompactionPolicy {
            target_file_bytes: 100,
            small_file_bytes: 75,
            min_input_files: 3,
            max_input_bytes: 1_000,
        }
    }

    fn file(sequence_number: i64, size_bytes: u64) -> CandidateFile {
        CandidateFile {
            path: format!("f{sequence_number}"),
            size_bytes,
            sequence_number,
        }
    }

    fn paths(bins: &[Vec<CandidateFile>]) -> Vec<Vec<&str>> {
        bins.iter()
            .map(|bin| bin.iter().map(|file| file.path.as_str()).collect())
            .collect()
    }

    #[test]
    fn small_files_are_binned_in_commit_order_up_to_the_target_size() {
        let files: Vec<_> = (1..=7).rev().map(|seq| file(seq, 30)).collect();
        let bins = plan_compaction(&files, &policy());
        // 30+30+30 = 90 fits, a fourth would pass 100; the last bin's single file waits.
        assert_eq!(paths(&bins), [["f1", "f2", "f3"], ["f4", "f5", "f6"]]);
    }

    #[test]
    fn a_big_file_ends_a_run_so_bins_never_span_it() {
        let files = vec![
            file(1, 10),
            file(2, 10),
            file(3, 90),
            file(4, 10),
            file(5, 10),
            file(6, 10),
        ];
        let bins = plan_compaction(&files, &policy());
        // f1/f2 are only two files of 20 bytes: not worth it yet.
        assert_eq!(paths(&bins), [["f4", "f5", "f6"]]);
    }

    #[test]
    fn two_files_are_enough_when_together_they_are_no_longer_small() {
        let files = vec![file(1, 50), file(2, 40)];
        assert_eq!(paths(&plan_compaction(&files, &policy())), [["f1", "f2"]]);
    }

    #[test]
    fn too_few_small_files_are_left_alone() {
        let files = vec![file(1, 10), file(2, 10)];
        assert!(plan_compaction(&files, &policy()).is_empty());
        assert!(plan_compaction(&[file(1, 10)], &policy()).is_empty());
        assert!(plan_compaction(&[], &policy()).is_empty());
    }

    #[test]
    fn the_input_cap_stops_planning_at_a_whole_bin() {
        let files: Vec<_> = (1..=9).map(|seq| file(seq, 30)).collect();
        let capped = CompactionPolicy {
            max_input_bytes: 200,
            ..policy()
        };
        assert_eq!(
            paths(&plan_compaction(&files, &capped)),
            [["f1", "f2", "f3"], ["f4", "f5", "f6"]]
        );
        let tighter = CompactionPolicy {
            max_input_bytes: 100,
            ..policy()
        };
        assert_eq!(
            paths(&plan_compaction(&files, &tighter)),
            [["f1", "f2", "f3"]]
        );
    }

    #[test]
    fn config_reads_the_environment_and_zero_turns_things_off() {
        let vars = HashMap::from([
            ("INGESTION_MAINTENANCE_INTERVAL_SECS", "0"),
            ("INGESTION_COMPACTION_TARGET_FILE_BYTES", "1000"),
            ("INGESTION_COMPACTION_SMALL_FILE_RATIO", "0.5"),
            ("INGESTION_COMPACTION_MIN_INPUT_FILES", "1"),
            ("INGESTION_SNAPSHOT_MAX_AGE_SECS", "5"),
            ("INGESTION_SNAPSHOT_MIN_TO_KEEP", "0"),
            ("INGESTION_ORPHAN_CLEANUP_AGE_SECS", "0"),
        ]);
        let config = MaintenanceConfig::from_lookup(|name| vars.get(name).map(|v| v.to_string()));
        assert_eq!(config.interval, None);
        assert_eq!(config.compaction.target_file_bytes, 1000);
        assert_eq!(config.compaction.small_file_bytes, 500);
        assert_eq!(
            config.compaction.min_input_files, 2,
            "clamped to at least 2"
        );
        assert_eq!(config.expiry.max_snapshot_age, Duration::from_secs(5));
        assert_eq!(
            config.expiry.min_snapshots_to_keep, 1,
            "clamped to at least 1"
        );
        assert_eq!(config.orphan_min_age, None);
    }

    #[test]
    fn config_defaults_without_environment() {
        let config = MaintenanceConfig::from_lookup(|_| None);
        assert_eq!(config, MaintenanceConfig::default());
        assert_eq!(config.compaction.small_file_bytes, 96 * MIB);
    }
}
