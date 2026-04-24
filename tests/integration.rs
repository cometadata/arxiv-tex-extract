//! Integration tests for batch processing with parquet output and checkpoint-resume.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::Result;
use arrow::array::{Array, StringArray};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

/// Helper: create a minimal tar containing a single .gz paper.
fn create_test_tar(dir: &Path, name: &str, arxiv_id: &str, tex_content: &str) -> std::path::PathBuf {
    create_test_tar_with_papers(dir, name, &[(arxiv_id, tex_content)])
}

/// Helper: create a tar containing multiple .gz papers.
fn create_test_tar_with_papers(
    dir: &Path,
    name: &str,
    papers: &[(&str, &str)],
) -> std::path::PathBuf {
    use flate2::write::GzEncoder;
    use flate2::Compression;

    let tar_path = dir.join(name);
    let tar_file = fs::File::create(&tar_path).unwrap();
    let mut tar_builder = tar::Builder::new(tar_file);

    for (arxiv_id, tex_content) in papers {
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(tex_content.as_bytes()).unwrap();
        let gz_bytes = gz.finish().unwrap();

        let gz_name = format!("{}.gz", arxiv_id);
        let mut header = tar::Header::new_gnu();
        header.set_size(gz_bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();

        tar_builder
            .append_data(&mut header, &gz_name, &gz_bytes[..])
            .unwrap();
    }
    tar_builder.finish().unwrap();

    tar_path
}

/// Helper: create a tar containing valid .gz papers plus one malformed entry
/// whose tar header claims a size larger than the `MAX_DECOMPRESSED_SIZE`
/// limit in `src/archive.rs`. The bad entry trips the size guard inside
/// `process_entry`, so `for_each_paper` yields `Err` for that entry.
/// The bad entry's filename is `{bad_arxiv_id}.{bad_extension}`, letting
/// tests exercise extension-based file_type classification.
fn create_test_tar_with_oversized_entry(
    dir: &Path,
    name: &str,
    good_papers: &[(&str, &str)],
    bad_arxiv_id: &str,
    bad_extension: &str,
) -> std::path::PathBuf {
    use flate2::write::GzEncoder;
    use flate2::Compression;

    let tar_path = dir.join(name);
    let tar_file = fs::File::create(&tar_path).unwrap();
    let mut tar_builder = tar::Builder::new(tar_file);

    for (arxiv_id, tex_content) in good_papers {
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(tex_content.as_bytes()).unwrap();
        let gz_bytes = gz.finish().unwrap();

        let gz_name = format!("{}.gz", arxiv_id);
        let mut header = tar::Header::new_gnu();
        header.set_size(gz_bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar_builder
            .append_data(&mut header, &gz_name, &gz_bytes[..])
            .unwrap();
    }

    // Bad entry: claim 200 MB in the header, attach an empty body. The
    // `tar` crate writes the 512-byte header verbatim and zero-pads the
    // body; on read, `process_entry` reads `header.size()` and bails
    // before touching any of the body bytes.
    let mut bad_header = tar::Header::new_gnu();
    bad_header.set_size(200_000_000);
    bad_header.set_mode(0o644);
    bad_header.set_cksum();
    let empty: &[u8] = &[];
    tar_builder
        .append_data(
            &mut bad_header,
            format!("{}.{}", bad_arxiv_id, bad_extension),
            empty,
        )
        .unwrap();

    tar_builder.finish().unwrap();
    tar_path
}

#[test]
fn test_parquet_output_end_to_end() {
    let input_dir = tempfile::tempdir().unwrap();
    let output_dir = tempfile::tempdir().unwrap();

    let tex = r"\documentclass{article}
\begin{document}
\section{Introduction}
This is a test paper about funding from NSF grant 12345.
\end{document}";

    create_test_tar(input_dir.path(), "test_batch.tar", "2401.00001", tex);

    let status = std::process::Command::new(env!("CARGO_BIN_EXE_latex-extract"))
        .args([
            "-d", input_dir.path().to_str().unwrap(),
            "-o", output_dir.path().to_str().unwrap(),
            "--output-format", "parquet",
            "--resume",
        ])
        .status()
        .unwrap();

    assert!(status.success(), "binary exited with: {}", status);

    let parquet_path = output_dir.path().join("test_batch.parquet");
    assert!(parquet_path.exists(), "parquet file should exist");

    let file = fs::File::open(&parquet_path).unwrap();
    let reader = ParquetRecordBatchReaderBuilder::try_new(file).unwrap().build().unwrap();
    let batches: Vec<_> = reader.collect::<Result<_, _>>().unwrap();

    assert_eq!(batches.len(), 1);
    let batch = &batches[0];
    assert_eq!(batch.num_rows(), 1);

    let ids = batch.column(0).as_any().downcast_ref::<StringArray>().unwrap();
    assert_eq!(ids.value(0), "2401.00001");

    let statuses = batch.column(1).as_any().downcast_ref::<StringArray>().unwrap();
    assert_eq!(statuses.value(0), "ok");

    let checkpoint = output_dir.path().join("checkpoint.log");
    assert!(checkpoint.exists(), "checkpoint should exist");
    let content = fs::read_to_string(&checkpoint).unwrap();
    assert!(content.contains("test_batch.tar"));
}

#[test]
fn test_checkpoint_resume_skips_completed() {
    let input_dir = tempfile::tempdir().unwrap();
    let output_dir = tempfile::tempdir().unwrap();

    let tex = r"\documentclass{article}\begin{document}Hello.\end{document}";
    create_test_tar(input_dir.path(), "batch_a.tar", "0001.00001", tex);
    create_test_tar(input_dir.path(), "batch_b.tar", "0002.00001", tex);

    // Pre-populate checkpoint with batch_a already done
    fs::write(output_dir.path().join("checkpoint.log"), "batch_a.tar\n").unwrap();
    // Also create the parquet file for batch_a (so the output exists)
    fs::write(output_dir.path().join("batch_a.parquet"), "dummy").unwrap();

    let status = std::process::Command::new(env!("CARGO_BIN_EXE_latex-extract"))
        .args([
            "-d", input_dir.path().to_str().unwrap(),
            "-o", output_dir.path().to_str().unwrap(),
            "--output-format", "parquet",
            "--resume",
        ])
        .status()
        .unwrap();

    assert!(status.success());

    assert!(output_dir.path().join("batch_b.parquet").exists());

    let checkpoint = fs::read_to_string(output_dir.path().join("checkpoint.log")).unwrap();
    assert!(checkpoint.contains("batch_a.tar"));
    assert!(checkpoint.contains("batch_b.tar"));
}

#[test]
fn test_jsonl_backward_compat() {
    let input_dir = tempfile::tempdir().unwrap();
    let output_dir = tempfile::tempdir().unwrap();

    let tex = r"\documentclass{article}\begin{document}Test.\end{document}";
    create_test_tar(input_dir.path(), "compat.tar", "9901.00001", tex);

    let status = std::process::Command::new(env!("CARGO_BIN_EXE_latex-extract"))
        .args([
            "-d", input_dir.path().to_str().unwrap(),
            "-o", output_dir.path().to_str().unwrap(),
            "--output-format", "jsonl",
            "--resume",
        ])
        .status()
        .unwrap();

    assert!(status.success());

    let jsonl_path = output_dir.path().join("compat.jsonl");
    assert!(jsonl_path.exists(), "JSONL file should exist");

    let content = fs::read_to_string(&jsonl_path).unwrap();
    assert!(content.contains("9901.00001"));
    assert!(content.contains("\"status\":\"ok\""));
}

/// Helper: create a standalone .gz file containing TeX content.
fn create_test_gz(dir: &Path, name: &str, tex_content: &str) -> std::path::PathBuf {
    use flate2::write::GzEncoder;
    use flate2::Compression;

    let gz_path = dir.join(name);
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(tex_content.as_bytes()).unwrap();
    let gz_bytes = gz.finish().unwrap();
    fs::write(&gz_path, &gz_bytes).unwrap();
    gz_path
}

/// Helper: create a .tar.gz file containing a single .tex file.
fn create_test_tar_gz(dir: &Path, name: &str, tex_name: &str, tex_content: &str) -> std::path::PathBuf {
    use flate2::write::GzEncoder;
    use flate2::Compression;

    let tar_gz_path = dir.join(name);
    let file = fs::File::create(&tar_gz_path).unwrap();
    let gz = GzEncoder::new(file, Compression::default());
    let mut tar_builder = tar::Builder::new(gz);

    let content_bytes = tex_content.as_bytes();
    let mut header = tar::Header::new_gnu();
    header.set_size(content_bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();

    tar_builder.append_data(&mut header, tex_name, content_bytes).unwrap();
    tar_builder.finish().unwrap();

    tar_gz_path
}

#[test]
fn test_files_mode_text() {
    let input_dir = tempfile::tempdir().unwrap();
    let output_dir = tempfile::tempdir().unwrap();

    let tex_a = r"\documentclass{article}\begin{document}Hello from paper A.\end{document}";
    create_test_gz(input_dir.path(), "paper_a.gz", tex_a);

    let tex_b = r"\documentclass{article}\begin{document}Hello from paper B.\end{document}";
    create_test_gz(input_dir.path(), "paper_b.gz", tex_b);

    let status = std::process::Command::new(env!("CARGO_BIN_EXE_latex-extract"))
        .args([
            "-d", input_dir.path().to_str().unwrap(),
            "-o", output_dir.path().to_str().unwrap(),
            "--text-files",
        ])
        .status()
        .unwrap();

    assert!(status.success(), "binary exited with: {}", status);

    // Verify output files named by input stems (not arxiv IDs)
    let a_path = output_dir.path().join("paper_a.txt");
    let b_path = output_dir.path().join("paper_b.txt");
    assert!(a_path.exists(), "paper_a.txt should exist");
    assert!(b_path.exists(), "paper_b.txt should exist");

    let a_text = fs::read_to_string(&a_path).unwrap();
    assert!(a_text.contains("Hello from paper A"), "got: {a_text}");

    let b_text = fs::read_to_string(&b_path).unwrap();
    assert!(b_text.contains("Hello from paper B"), "got: {b_text}");
}

#[test]
fn test_files_mode_parquet() {
    let input_dir = tempfile::tempdir().unwrap();
    let output_dir = tempfile::tempdir().unwrap();

    let tex = r"\documentclass{article}\begin{document}Parquet test content.\end{document}";
    create_test_gz(input_dir.path(), "sample.gz", tex);

    let status = std::process::Command::new(env!("CARGO_BIN_EXE_latex-extract"))
        .args([
            "-d", input_dir.path().to_str().unwrap(),
            "-o", output_dir.path().to_str().unwrap(),
            "--output-format", "parquet",
        ])
        .status()
        .unwrap();

    assert!(status.success(), "binary exited with: {}", status);

    // Find the parquet file (shard prefix is "individual")
    let parquet_files: Vec<_> = fs::read_dir(output_dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "parquet"))
        .collect();

    assert!(!parquet_files.is_empty(), "should have at least one parquet file");

    let file = fs::File::open(parquet_files[0].path()).unwrap();
    let reader = ParquetRecordBatchReaderBuilder::try_new(file).unwrap().build().unwrap();
    let batches: Vec<_> = reader.collect::<Result<_, _>>().unwrap();

    assert!(!batches.is_empty());
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total_rows, 1);

    let batch = &batches[0];
    let statuses = batch.column(1).as_any().downcast_ref::<StringArray>().unwrap();
    assert_eq!(statuses.value(0), "ok");
}

#[test]
fn test_files_mode_mixed_formats() {
    let input_dir = tempfile::tempdir().unwrap();
    let output_dir = tempfile::tempdir().unwrap();

    let tex1 = r"\documentclass{article}\begin{document}From standalone gz.\end{document}";
    create_test_gz(input_dir.path(), "standalone.gz", tex1);

    let tex2 = r"\documentclass{article}\begin{document}From tar gz bundle.\end{document}";
    create_test_tar_gz(input_dir.path(), "bundled.tar.gz", "main.tex", tex2);

    let status = std::process::Command::new(env!("CARGO_BIN_EXE_latex-extract"))
        .args([
            "-d", input_dir.path().to_str().unwrap(),
            "-o", output_dir.path().to_str().unwrap(),
            "--text-files",
        ])
        .status()
        .unwrap();

    assert!(status.success(), "binary exited with: {}", status);

    let standalone_path = output_dir.path().join("standalone.txt");
    let bundled_path = output_dir.path().join("bundled.txt");

    assert!(standalone_path.exists(), "standalone.txt should exist");
    assert!(bundled_path.exists(), "bundled.txt should exist");

    let text1 = fs::read_to_string(&standalone_path).unwrap();
    assert!(text1.contains("From standalone gz"), "got: {text1}");

    let text2 = fs::read_to_string(&bundled_path).unwrap();
    assert!(text2.contains("From tar gz bundle"), "got: {text2}");
}

// ---------------------------------------------------------------------------
// SIGKILL + resume round-trip tests.
//
// Gated on the `test-hooks` Cargo feature: these require the binary's
// `ARXIV_TEX_EXTRACT_KILL_AFTER_PAPERS` hook to be compiled in. Run with:
//   cargo test --features test-hooks
// ---------------------------------------------------------------------------

#[cfg(feature = "test-hooks")]
fn collect_all_arxiv_ids(dir: &Path) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().map_or(false, |e| e == "parquet") {
            let file = fs::File::open(&path).unwrap();
            let reader = ParquetRecordBatchReaderBuilder::try_new(file)
                .unwrap()
                .build()
                .unwrap();
            for batch_result in reader {
                let batch = batch_result.unwrap();
                let col = batch
                    .column(0)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap();
                for i in 0..col.len() {
                    ids.push(col.value(i).to_string());
                }
            }
        }
    }
    ids.sort();
    ids
}

#[cfg(feature = "test-hooks")]
fn six_papers() -> Vec<(String, String)> {
    (1..=6)
        .map(|i| {
            let id = format!("2401.{:05}", i);
            let tex = format!(
                r"\documentclass{{article}}\begin{{document}}Paper {}.\end{{document}}",
                i
            );
            (id, tex)
        })
        .collect()
}

#[cfg(feature = "test-hooks")]
#[test]
fn test_kill_resume_round_trip_tars_mode() {
    let input_dir = tempfile::tempdir().unwrap();
    let output_dir = tempfile::tempdir().unwrap();

    let papers = six_papers();
    let papers_refs: Vec<(&str, &str)> =
        papers.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    create_test_tar_with_papers(input_dir.path(), "killtest.tar", &papers_refs);

    // Run 1: kill after 3 successful writes. --papers-per-shard 2 means
    // the first rotation fsyncs the checkpoint for papers 1+2; paper 3's
    // write ticks the counter to 0 and exits before that shard rotates.
    let status1 = std::process::Command::new(env!("CARGO_BIN_EXE_latex-extract"))
        .args([
            "-d", input_dir.path().to_str().unwrap(),
            "-o", output_dir.path().to_str().unwrap(),
            "--output-format", "parquet",
            "--papers-per-shard", "2",
            "-j", "1",
            "--resume",
        ])
        .env("ARXIV_TEX_EXTRACT_KILL_AFTER_PAPERS", "3")
        .status()
        .unwrap();

    assert!(
        !status1.success(),
        "run 1 should have been killed, got: {}",
        status1
    );
    assert_eq!(status1.code(), Some(137), "expected exit 137");

    // Expect at least one rotated parquet shard and a checkpoint with
    // exactly 2 per-paper entries (not 3 — paper 3's shard never rotated).
    let checkpoint_content =
        fs::read_to_string(output_dir.path().join("checkpoint.log")).unwrap();
    let paper_lines: Vec<&str> = checkpoint_content
        .lines()
        .filter(|l| l.contains('\t'))
        .collect();
    assert_eq!(
        paper_lines.len(),
        2,
        "expected 2 per-paper checkpoint entries, got: {}",
        checkpoint_content
    );

    let rotated_shards: Vec<_> = fs::read_dir(output_dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "parquet"))
        .collect();
    assert!(
        !rotated_shards.is_empty(),
        "expected at least one rotated parquet shard"
    );

    // Run 2: resume without the kill hook. Should skip the 2 durable
    // papers, re-extract the rest, and produce shards covering all 6 IDs.
    let status2 = std::process::Command::new(env!("CARGO_BIN_EXE_latex-extract"))
        .args([
            "-d", input_dir.path().to_str().unwrap(),
            "-o", output_dir.path().to_str().unwrap(),
            "--output-format", "parquet",
            "--papers-per-shard", "2",
            "-j", "1",
            "--resume",
        ])
        .status()
        .unwrap();
    assert!(status2.success(), "run 2 should succeed: {}", status2);

    let final_ids = collect_all_arxiv_ids(output_dir.path());
    // Every paper should appear exactly once, no duplicates.
    let mut expected: Vec<String> = papers.iter().map(|(id, _)| id.clone()).collect();
    expected.sort();
    assert_eq!(
        final_ids, expected,
        "final shard set should equal the original 6 papers exactly once"
    );
    let deduped: std::collections::HashSet<_> = final_ids.iter().collect();
    assert_eq!(
        deduped.len(),
        final_ids.len(),
        "no paper should be duplicated across shards"
    );

    // No `.tmp` debris left behind.
    for entry in fs::read_dir(output_dir.path()).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().to_string();
        assert!(!name.ends_with(".tmp"), "stale tmp left: {name}");
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn test_kill_resume_round_trip_files_mode() {
    let input_dir = tempfile::tempdir().unwrap();
    let output_dir = tempfile::tempdir().unwrap();

    let papers = six_papers();
    for (id, tex) in &papers {
        create_test_gz(input_dir.path(), &format!("{}.gz", id), tex);
    }

    // Run 1: kill after 3 writes.
    let status1 = std::process::Command::new(env!("CARGO_BIN_EXE_latex-extract"))
        .args([
            "-d", input_dir.path().to_str().unwrap(),
            "-o", output_dir.path().to_str().unwrap(),
            "--output-format", "parquet",
            "--papers-per-shard", "2",
            "-j", "1",
            "--resume",
        ])
        .env("ARXIV_TEX_EXTRACT_KILL_AFTER_PAPERS", "3")
        .status()
        .unwrap();
    assert!(!status1.success(), "run 1 should have been killed");
    assert_eq!(status1.code(), Some(137));

    let cp_path = output_dir.path().join("checkpoint.log");
    let checkpoint_content = fs::read_to_string(&cp_path).unwrap_or_default();
    let paper_lines: Vec<&str> = checkpoint_content
        .lines()
        .filter(|l| l.contains('\t'))
        .collect();
    // With -j 1 and papers_per_shard=2, at least one rotation fired and
    // the checkpoint carries at least 2 per-paper entries.
    assert!(
        paper_lines.len() >= 2,
        "expected ≥2 paper entries, got {} in: {}",
        paper_lines.len(),
        checkpoint_content
    );

    // Run 2: resume.
    let status2 = std::process::Command::new(env!("CARGO_BIN_EXE_latex-extract"))
        .args([
            "-d", input_dir.path().to_str().unwrap(),
            "-o", output_dir.path().to_str().unwrap(),
            "--output-format", "parquet",
            "--papers-per-shard", "2",
            "-j", "1",
            "--resume",
        ])
        .status()
        .unwrap();
    assert!(status2.success(), "run 2 should succeed: {}", status2);

    let final_ids = collect_all_arxiv_ids(output_dir.path());
    let mut expected: Vec<String> = papers.iter().map(|(id, _)| id.clone()).collect();
    expected.sort();
    assert_eq!(
        final_ids, expected,
        "final shard set should contain all 6 papers exactly once"
    );
}

/// (arxiv_id, status, error, file_type, entry_name). Column indices come
/// from `result_schema()` in src/output.rs — 0, 1, 2, 10, 11.
type ParquetRow = (String, String, Option<String>, Option<String>, Option<String>);

fn read_parquet_rows(path: &Path) -> Vec<ParquetRow> {
    let file = fs::File::open(path).unwrap();
    let reader = ParquetRecordBatchReaderBuilder::try_new(file).unwrap().build().unwrap();
    let batches: Vec<_> = reader.collect::<Result<_, _>>().unwrap();

    let mut rows = Vec::new();
    for batch in &batches {
        let ids = batch.column(0).as_any().downcast_ref::<StringArray>().unwrap();
        let statuses = batch.column(1).as_any().downcast_ref::<StringArray>().unwrap();
        let errors = batch.column(2).as_any().downcast_ref::<StringArray>().unwrap();
        let file_types = batch.column(10).as_any().downcast_ref::<StringArray>().unwrap();
        let entry_names = batch.column(11).as_any().downcast_ref::<StringArray>().unwrap();
        for i in 0..batch.num_rows() {
            let opt = |arr: &StringArray, i: usize| {
                if arr.is_null(i) { None } else { Some(arr.value(i).to_string()) }
            };
            rows.push((
                ids.value(i).to_string(),
                statuses.value(i).to_string(),
                opt(errors, i),
                opt(file_types, i),
                opt(entry_names, i),
            ));
        }
    }
    rows
}

#[test]
fn test_parquet_archive_error_row_written() {
    let input_dir = tempfile::tempdir().unwrap();
    let output_dir = tempfile::tempdir().unwrap();

    let tex = r"\documentclass{article}\begin{document}Good paper.\end{document}";
    create_test_tar_with_oversized_entry(
        input_dir.path(),
        "mixed_batch.tar",
        &[("2401.00001", tex)],
        "2401.99999",
        "gz",
    );

    let status = std::process::Command::new(env!("CARGO_BIN_EXE_latex-extract"))
        .args([
            "-d", input_dir.path().to_str().unwrap(),
            "-o", output_dir.path().to_str().unwrap(),
            "--output-format", "parquet",
            "--resume",
        ])
        .status()
        .unwrap();
    assert!(status.success(), "binary exited with: {}", status);

    let parquet_path = output_dir.path().join("mixed_batch.parquet");
    assert!(parquet_path.exists(), "parquet file should exist");

    let rows = read_parquet_rows(&parquet_path);
    assert_eq!(rows.len(), 2, "expected exactly one good + one entry-error row");

    let ok_row = rows.iter().find(|r| r.0 == "2401.00001").expect("good paper row missing");
    assert_eq!(ok_row.1, "ok", "good paper should have status=ok");

    let err_row = rows.iter().find(|r| r.0 == "2401.99999").expect("bad entry row missing");
    assert_eq!(err_row.1, "archive_error");
    assert!(err_row.2.as_deref().unwrap_or("").contains("100MB limit"));
    assert_eq!(err_row.3.as_deref(), Some("unknown"), ".gz extension has no content inspection → Unknown");
    assert_eq!(err_row.4.as_deref(), Some("2401.99999.gz"));

    // The per-paper checkpoint is only emitted when --papers-per-shard is
    // set, so here we only assert the tar-level entry.
    let checkpoint = fs::read_to_string(output_dir.path().join("checkpoint.log")).unwrap();
    assert!(checkpoint.contains("mixed_batch.tar"));
}

/// Run the "oversized {ext} entry yields an archive_error row with
/// file_type={ext}" check end-to-end.
fn run_extension_file_type_case(ext: &str) {
    let input_dir = tempfile::tempdir().unwrap();
    let output_dir = tempfile::tempdir().unwrap();
    let tar_stem = format!("{}_batch", ext);

    let tex = r"\documentclass{article}\begin{document}Good paper.\end{document}";
    create_test_tar_with_oversized_entry(
        input_dir.path(),
        &format!("{}.tar", tar_stem),
        &[("2401.00001", tex)],
        "2401.99999",
        ext,
    );

    let status = std::process::Command::new(env!("CARGO_BIN_EXE_latex-extract"))
        .args([
            "-d", input_dir.path().to_str().unwrap(),
            "-o", output_dir.path().to_str().unwrap(),
            "--output-format", "parquet",
            "--resume",
        ])
        .status()
        .unwrap();
    assert!(status.success(), "binary exited with: {}", status);

    let rows = read_parquet_rows(&output_dir.path().join(format!("{}.parquet", tar_stem)));
    let err_row = rows.iter().find(|r| r.0 == "2401.99999")
        .unwrap_or_else(|| panic!("bad .{} row missing", ext));
    assert_eq!(err_row.1, "archive_error");
    assert_eq!(err_row.3.as_deref(), Some(ext));
    assert_eq!(err_row.4.as_deref(), Some(format!("2401.99999.{}", ext).as_str()));
}

#[test]
fn test_parquet_archive_error_pdf_entry_has_file_type() {
    run_extension_file_type_case("pdf");
}

#[test]
fn test_parquet_archive_error_tex_entry_has_file_type() {
    run_extension_file_type_case("tex");
}

#[test]
fn test_jsonl_archive_error_row_written() {
    let input_dir = tempfile::tempdir().unwrap();
    let output_dir = tempfile::tempdir().unwrap();

    let tex = r"\documentclass{article}\begin{document}Good paper.\end{document}";
    create_test_tar_with_oversized_entry(
        input_dir.path(),
        "mixed_batch.tar",
        &[("2401.00001", tex)],
        "2401.99999",
        "gz",
    );

    let status = std::process::Command::new(env!("CARGO_BIN_EXE_latex-extract"))
        .args([
            "-d", input_dir.path().to_str().unwrap(),
            "-o", output_dir.path().to_str().unwrap(),
            "--output-format", "jsonl",
            "--resume",
        ])
        .status()
        .unwrap();
    assert!(status.success(), "binary exited with: {}", status);

    let jsonl_path = output_dir.path().join("mixed_batch.jsonl");
    assert!(jsonl_path.exists(), "jsonl file should exist");

    let content = fs::read_to_string(&jsonl_path).unwrap();
    let rows: Vec<serde_json::Value> = content
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).expect("valid json line"))
        .collect();
    assert_eq!(rows.len(), 2, "expected exactly one good + one entry-error row");

    let ok_row = rows.iter().find(|r| r["arxiv_id"] == "2401.00001").expect("good paper row missing");
    assert_eq!(ok_row["status"], "ok");

    let err_row = rows.iter().find(|r| r["arxiv_id"] == "2401.99999").expect("bad entry row missing");
    assert_eq!(err_row["status"], "archive_error");
    assert!(err_row["error"].as_str().unwrap_or("").contains("100MB limit"));
    assert_eq!(err_row["file_type"], "unknown");
    assert_eq!(err_row["entry_name"], "2401.99999.gz");
}
