//! Integration tests for batch processing with parquet output and checkpoint-resume.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::Result;
use arrow::array::StringArray;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

/// Helper: create a minimal tar containing a single .gz paper.
fn create_test_tar(dir: &Path, name: &str, arxiv_id: &str, tex_content: &str) -> std::path::PathBuf {
    use flate2::write::GzEncoder;
    use flate2::Compression;

    let tar_path = dir.join(name);
    let tar_file = fs::File::create(&tar_path).unwrap();
    let mut tar_builder = tar::Builder::new(tar_file);

    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(tex_content.as_bytes()).unwrap();
    let gz_bytes = gz.finish().unwrap();

    let gz_name = format!("{}.gz", arxiv_id);
    let mut header = tar::Header::new_gnu();
    header.set_size(gz_bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();

    tar_builder.append_data(&mut header, &gz_name, &gz_bytes[..]).unwrap();
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
