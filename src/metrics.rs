use std::fs::File;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;
use tracing::info;

/// Maximum sample IDs to collect per error category.
const MAX_SAMPLES: usize = 5;

/// Type-safe classification of every document's extraction result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Successful extraction
    Ok,
    /// No tex files or extraction produced empty text
    Empty,
    /// Combined .tex content exceeds size limit
    Skipped,
    /// Per-document extraction timeout
    Timeout,
    /// Extraction pipeline panicked (catch_unwind)
    Panic,
    /// Extraction thread disconnected unexpectedly
    Crash,
    /// Tar entry / archive load / decompression failure
    ArchiveError,
    /// Failed to write output (parquet, jsonl, text file)
    IoError,
}

/// Tracks count and sample IDs for a single error category.
#[derive(Debug, Clone, Default)]
pub struct CategoryDetail {
    pub count: u64,
    pub samples: Vec<String>,
}

impl CategoryDetail {
    fn record(&mut self, id: &str) {
        self.count += 1;
        if self.samples.len() < MAX_SAMPLES {
            self.samples.push(id.to_string());
        }
    }

    fn merge(&mut self, other: &CategoryDetail) {
        self.count += other.count;
        for s in &other.samples {
            if self.samples.len() < MAX_SAMPLES {
                self.samples.push(s.clone());
            }
        }
    }
}

/// Aggregated extraction outcome counts with sample IDs for diagnostics.
#[derive(Debug, Clone, Default)]
pub struct StatusCounts {
    pub ok: u64,
    pub empty: CategoryDetail,
    pub skipped: CategoryDetail,
    pub timeouts: CategoryDetail,
    pub panics: CategoryDetail,
    pub crashes: CategoryDetail,
    pub archive_errors: CategoryDetail,
    pub io_errors: CategoryDetail,
}

impl StatusCounts {
    /// Record an outcome for a given document.
    pub fn record(&mut self, outcome: Outcome, id: &str) {
        match outcome {
            Outcome::Ok => self.ok += 1,
            Outcome::Empty => self.empty.record(id),
            Outcome::Skipped => self.skipped.record(id),
            Outcome::Timeout => self.timeouts.record(id),
            Outcome::Panic => self.panics.record(id),
            Outcome::Crash => self.crashes.record(id),
            Outcome::ArchiveError => self.archive_errors.record(id),
            Outcome::IoError => self.io_errors.record(id),
        }
    }

    /// Merge counts from another instance.
    pub fn merge(&mut self, other: &StatusCounts) {
        self.ok += other.ok;
        self.empty.merge(&other.empty);
        self.skipped.merge(&other.skipped);
        self.timeouts.merge(&other.timeouts);
        self.panics.merge(&other.panics);
        self.crashes.merge(&other.crashes);
        self.archive_errors.merge(&other.archive_errors);
        self.io_errors.merge(&other.io_errors);
    }

    /// Total extraction outcomes (excludes io_errors, which are orthogonal).
    pub fn total(&self) -> u64 {
        self.ok
            + self.empty.count
            + self.skipped.count
            + self.timeouts.count
            + self.panics.count
            + self.crashes.count
            + self.archive_errors.count
    }

    /// Emit structured info! lines with counts and sample IDs.
    pub fn log_summary(&self, prefix: &str) {
        info!("{} ({} ok)", prefix, self.ok);
        log_category("timeouts", &self.timeouts);
        log_category("skipped", &self.skipped);
        log_category("panics", &self.panics);
        log_category("archive errors", &self.archive_errors);
        log_category("empty", &self.empty);
        log_category("crashes", &self.crashes);
        if self.io_errors.count > 0 {
            info!("  {} I/O write errors", self.io_errors.count);
        }
    }
}

fn log_category(label: &str, detail: &CategoryDetail) {
    if detail.count > 0 {
        if detail.samples.is_empty() {
            info!("  {} {}", detail.count, label);
        } else {
            info!(
                "  {} {} (e.g. {})",
                detail.count,
                label,
                detail.samples.join(", ")
            );
        }
    }
}

/// Per-tar processing metrics.
#[derive(Debug, Serialize)]
pub struct TarMetrics {
    pub tar_name: String,
    pub total_papers: u64,
    pub ok: u64,
    pub empty: u64,
    pub skipped: u64,
    pub timeouts: u64,
    pub panics: u64,
    pub crashes: u64,
    pub archive_errors: u64,
    pub io_errors: u64,
    pub processing_time_ms: u64,
}

impl TarMetrics {
    /// Construct from a StatusCounts snapshot.
    pub fn from_counts(tar_name: String, counts: &StatusCounts, processing_time_ms: u64) -> Self {
        Self {
            tar_name,
            total_papers: counts.total(),
            ok: counts.ok,
            empty: counts.empty.count,
            skipped: counts.skipped.count,
            timeouts: counts.timeouts.count,
            panics: counts.panics.count,
            crashes: counts.crashes.count,
            archive_errors: counts.archive_errors.count,
            io_errors: counts.io_errors.count,
            processing_time_ms,
        }
    }
}

/// Write metrics to a JSON sidecar file.
///
/// For a shard at `output_dir/0712.parquet`, writes `output_dir/0712_metrics.json`.
pub fn write_metrics(output_dir: &Path, base_name: &str, metrics: &TarMetrics) -> Result<()> {
    let path = output_dir.join(format!("{}_metrics.json", base_name));
    let json = serde_json::to_string_pretty(metrics)?;
    let mut file =
        File::create(&path).with_context(|| format!("creating {}", path.display()))?;
    file.write_all(json.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_metrics() {
        let dir = tempfile::tempdir().unwrap();
        let m = TarMetrics {
            tar_name: "test.tar".into(),
            total_papers: 100,
            ok: 90,
            empty: 2,
            skipped: 1,
            timeouts: 2,
            panics: 3,
            crashes: 1,
            archive_errors: 1,
            io_errors: 0,
            processing_time_ms: 1500,
        };
        write_metrics(dir.path(), "test", &m).unwrap();

        let content = std::fs::read_to_string(dir.path().join("test_metrics.json")).unwrap();
        assert!(content.contains("\"total_papers\": 100"));
        assert!(content.contains("\"ok\": 90"));
        assert!(content.contains("\"panics\": 3"));
        assert!(content.contains("\"crashes\": 1"));
    }

    #[test]
    fn test_status_counts_record() {
        let mut counts = StatusCounts::default();
        counts.record(Outcome::Ok, "2401.00001");
        counts.record(Outcome::Timeout, "2401.00002");
        counts.record(Outcome::Panic, "2401.00003");
        counts.record(Outcome::Crash, "2401.00004");
        counts.record(Outcome::ArchiveError, "2401.00005");
        counts.record(Outcome::IoError, "2401.00006");
        counts.record(Outcome::Empty, "2401.00007");
        counts.record(Outcome::Skipped, "2401.00008");

        assert_eq!(counts.ok, 1);
        assert_eq!(counts.timeouts.count, 1);
        assert_eq!(counts.panics.count, 1);
        assert_eq!(counts.crashes.count, 1);
        assert_eq!(counts.archive_errors.count, 1);
        assert_eq!(counts.io_errors.count, 1);
        assert_eq!(counts.empty.count, 1);
        assert_eq!(counts.skipped.count, 1);

        assert_eq!(counts.timeouts.samples, vec!["2401.00002"]);
        assert_eq!(counts.panics.samples, vec!["2401.00003"]);
    }

    #[test]
    fn test_status_counts_record_sample_limit() {
        let mut counts = StatusCounts::default();
        for i in 0..10 {
            counts.record(Outcome::Timeout, &format!("2401.{:05}", i));
        }
        assert_eq!(counts.timeouts.count, 10);
        assert_eq!(counts.timeouts.samples.len(), MAX_SAMPLES);
    }

    #[test]
    fn test_status_counts_merge() {
        let mut a = StatusCounts::default();
        a.record(Outcome::Ok, "");
        a.record(Outcome::Timeout, "2401.00001");
        a.record(Outcome::Panic, "2401.00002");

        let mut b = StatusCounts::default();
        b.record(Outcome::Ok, "");
        b.record(Outcome::Ok, "");
        b.record(Outcome::Timeout, "2401.00003");
        b.record(Outcome::ArchiveError, "2401.00004");

        a.merge(&b);
        assert_eq!(a.ok, 3);
        assert_eq!(a.timeouts.count, 2);
        assert_eq!(a.timeouts.samples.len(), 2);
        assert_eq!(a.panics.count, 1);
        assert_eq!(a.archive_errors.count, 1);
    }

    #[test]
    fn test_status_counts_total() {
        let mut counts = StatusCounts::default();
        counts.record(Outcome::Ok, "");
        counts.record(Outcome::Ok, "");
        counts.record(Outcome::Timeout, "t1");
        counts.record(Outcome::Panic, "p1");
        counts.record(Outcome::IoError, "io1");

        // total excludes io_errors
        assert_eq!(counts.total(), 4);
    }

    #[test]
    fn test_tar_metrics_from_counts() {
        let mut counts = StatusCounts::default();
        counts.record(Outcome::Ok, "");
        counts.record(Outcome::Timeout, "t1");
        counts.record(Outcome::Panic, "p1");
        counts.record(Outcome::Empty, "e1");
        counts.record(Outcome::IoError, "io1");

        let m = TarMetrics::from_counts("test.tar".into(), &counts, 5000);
        assert_eq!(m.tar_name, "test.tar");
        assert_eq!(m.total_papers, 4); // io_errors excluded
        assert_eq!(m.ok, 1);
        assert_eq!(m.timeouts, 1);
        assert_eq!(m.panics, 1);
        assert_eq!(m.empty, 1);
        assert_eq!(m.io_errors, 1);
        assert_eq!(m.processing_time_ms, 5000);
    }
}
