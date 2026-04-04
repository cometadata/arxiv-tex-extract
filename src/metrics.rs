use std::fs::File;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

/// Per-tar processing metrics.
#[derive(Debug, Serialize)]
pub struct TarMetrics {
    pub tar_name: String,
    pub total_papers: u64,
    pub ok: u64,
    pub errors: u64,
    pub timeouts: u64,
    pub processing_time_ms: u64,
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
            ok: 95,
            errors: 3,
            timeouts: 2,
            processing_time_ms: 1500,
        };
        write_metrics(dir.path(), "test", &m).unwrap();

        let content = std::fs::read_to_string(dir.path().join("test_metrics.json")).unwrap();
        assert!(content.contains("\"total_papers\": 100"));
        assert!(content.contains("\"ok\": 95"));
    }
}
