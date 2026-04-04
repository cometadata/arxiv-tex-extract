use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use anyhow::{Context, Result};

/// Load the set of completed tar names from a checkpoint file.
///
/// Returns an empty set if the file doesn't exist.
pub fn load_checkpoint(path: &Path) -> Result<HashSet<String>> {
    if !path.exists() {
        return Ok(HashSet::new());
    }

    let file =
        File::open(path).with_context(|| format!("opening checkpoint {}", path.display()))?;
    let reader = BufReader::new(file);

    let mut completed = HashSet::new();
    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim().to_string();
        if !trimmed.is_empty() {
            completed.insert(trimmed);
        }
    }

    Ok(completed)
}

/// Append a completed tar name to the checkpoint file and fsync.
///
/// Creates the file if it doesn't exist.
pub fn record_checkpoint(path: &Path, tar_name: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening checkpoint {} for append", path.display()))?;

    writeln!(file, "{}", tar_name)?;
    file.sync_data().context("fsync checkpoint")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.log");
        let set = load_checkpoint(&path).unwrap();
        assert!(set.is_empty());
    }

    #[test]
    fn test_record_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("checkpoint.log");

        record_checkpoint(&path, "arXiv_src_0001.tar").unwrap();
        record_checkpoint(&path, "arXiv_src_0002.tar").unwrap();

        let set = load_checkpoint(&path).unwrap();
        assert_eq!(set.len(), 2);
        assert!(set.contains("arXiv_src_0001.tar"));
        assert!(set.contains("arXiv_src_0002.tar"));
    }

    #[test]
    fn test_resume_skips_completed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("checkpoint.log");

        record_checkpoint(&path, "done.tar").unwrap();
        let completed = load_checkpoint(&path).unwrap();

        let all_tars = vec!["done.tar", "pending.tar"];
        let pending: Vec<_> = all_tars
            .into_iter()
            .filter(|t| !completed.contains(*t))
            .collect();

        assert_eq!(pending, vec!["pending.tar"]);
    }

    #[test]
    fn test_idempotent_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("checkpoint.log");

        record_checkpoint(&path, "same.tar").unwrap();
        record_checkpoint(&path, "same.tar").unwrap();

        let set = load_checkpoint(&path).unwrap();
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn test_blank_lines_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("checkpoint.log");

        std::fs::write(&path, "a.tar\n\n  \nb.tar\n").unwrap();

        let set = load_checkpoint(&path).unwrap();
        assert_eq!(set.len(), 2);
        assert!(set.contains("a.tar"));
        assert!(set.contains("b.tar"));
    }
}
