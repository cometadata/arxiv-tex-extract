use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use anyhow::{Context, Result};

/// Checkpoint state parsed from the log file.
///
/// Two entry formats coexist for backward compatibility:
/// - Bare tar name (1 field): the whole tar completed.
/// - `tar_name\tarxiv_id` (2 fields): that specific paper's row is in a
///   readable shard on disk.
///
/// The per-paper form is written as each shard is closed (see
/// `ParquetShardWriter::close_shard` and `JsonlShardWriter::close_shard`),
/// preserving the invariant that every paper in `papers` is in a readable
/// shard footer, not a lingering `.tmp`.
#[derive(Debug, Default, Clone)]
pub struct Checkpoint {
    pub tars: HashSet<String>,
    pub papers: HashSet<(String, String)>,
}

/// Load the checkpoint from disk, parsing both legacy (tar-only) and
/// per-paper entries. Returns an empty `Checkpoint` if the file doesn't exist.
pub fn load(path: &Path) -> Result<Checkpoint> {
    let mut cp = Checkpoint::default();
    if !path.exists() {
        return Ok(cp);
    }

    let file =
        File::open(path).with_context(|| format!("opening checkpoint {}", path.display()))?;
    let reader = BufReader::new(file);

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match trimmed.split_once('\t') {
            Some((tar, id)) => {
                cp.papers.insert((tar.to_string(), id.to_string()));
            }
            None => {
                cp.tars.insert(trimmed.to_string());
            }
        }
    }

    Ok(cp)
}

/// Load only the set of completed tar names (legacy-compatible view).
///
/// Retained for callers that don't need per-paper information.
pub fn load_checkpoint(path: &Path) -> Result<HashSet<String>> {
    Ok(load(path)?.tars)
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

/// Append a batch of completed `(tar_name, arxiv_id)` entries and fsync.
///
/// Each line is written as `tar\tarxiv_id`. Called from the writer's
/// `close_shard` path — i.e. only after the shard's rows are on disk in a
/// readable (footer-complete) state. This single fsync per shard bounds
/// kill-loss to the in-flight shard's rows.
///
/// Safe for concurrent callers: the OS append is atomic for writes
/// under PIPE_BUF, which each `tar\tid\n` line comfortably is.
pub fn record_papers(path: &Path, tar_name: &str, arxiv_ids: &[String]) -> Result<()> {
    if arxiv_ids.is_empty() {
        return Ok(());
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening checkpoint {} for append", path.display()))?;

    for id in arxiv_ids {
        writeln!(file, "{}\t{}", tar_name, id)?;
    }
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

    #[test]
    fn test_record_papers_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("checkpoint.log");

        record_papers(
            &path,
            "arXiv_src_0001",
            &["2401.00001".into(), "2401.00002".into()],
        )
        .unwrap();
        record_papers(&path, "arXiv_src_0002", &["2401.00003".into()]).unwrap();

        let cp = load(&path).unwrap();
        assert!(cp.tars.is_empty());
        assert_eq!(cp.papers.len(), 3);
        assert!(cp
            .papers
            .contains(&("arXiv_src_0001".into(), "2401.00001".into())));
        assert!(cp
            .papers
            .contains(&("arXiv_src_0001".into(), "2401.00002".into())));
        assert!(cp
            .papers
            .contains(&("arXiv_src_0002".into(), "2401.00003".into())));
    }

    #[test]
    fn test_mixed_format_parsing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("checkpoint.log");

        // Legacy tar-only entries alongside new per-paper entries.
        record_checkpoint(&path, "legacy_completed.tar").unwrap();
        record_papers(&path, "fresh_tar", &["2401.00001".into()]).unwrap();
        record_checkpoint(&path, "another_legacy.tar").unwrap();

        let cp = load(&path).unwrap();
        assert_eq!(cp.tars.len(), 2);
        assert!(cp.tars.contains("legacy_completed.tar"));
        assert!(cp.tars.contains("another_legacy.tar"));
        assert_eq!(cp.papers.len(), 1);
        assert!(cp
            .papers
            .contains(&("fresh_tar".into(), "2401.00001".into())));
    }

    #[test]
    fn test_load_checkpoint_legacy_view_ignores_paper_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("checkpoint.log");

        record_checkpoint(&path, "done.tar").unwrap();
        record_papers(&path, "inflight.tar", &["2401.00001".into()]).unwrap();

        // Legacy loader returns only bare-tar entries.
        let tars = load_checkpoint(&path).unwrap();
        assert_eq!(tars.len(), 1);
        assert!(tars.contains("done.tar"));
    }

    #[test]
    fn test_record_papers_empty_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("checkpoint.log");

        record_papers(&path, "t", &[]).unwrap();
        assert!(!path.exists(), "empty batch should not create the file");
    }
}
