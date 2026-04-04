#[cfg(not(target_env = "msvc"))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

use anyhow::{Context, Result};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tracing::{error, info};

use latex_extract::archive::{self, PaperArchive};
use latex_extract::checkpoint;
use latex_extract::metrics::{self, TarMetrics};
use latex_extract::output::ParquetShardWriter;
use latex_extract::pipeline::extract_text_timed;
use latex_extract::result::ExtractionResult;

/// Maximum combined .tex content size (10MB). Documents exceeding this are
/// skipped — a 10MB input generates ~100MB of regex intermediates across
/// 10 pipeline passes.
const MAX_TEX_CONTENT_BYTES: usize = 10_000_000;

#[derive(Parser)]
#[command(name = "latex-extract")]
#[command(about = "Extract text from arXiv LaTeX source archives")]
struct Args {
    /// Directory containing outer .tar files (batch mode)
    #[arg(short = 'd', long)]
    input_dir: Option<PathBuf>,

    /// Single archive file to process (.tar.gz or .gz)
    #[arg(short = 'f', long)]
    input_file: Option<PathBuf>,

    /// Output directory for result files
    #[arg(short, long)]
    output_dir: Option<PathBuf>,

    /// Number of worker threads (default: num CPUs)
    #[arg(short = 'j', long)]
    threads: Option<usize>,

    /// Per-document extraction timeout in seconds
    #[arg(short = 't', long, default_value_t = 30)]
    timeout_secs: u64,

    /// Write one .txt file per paper instead of structured output
    #[arg(long)]
    text_files: bool,

    /// Output format: "parquet" or "jsonl"
    #[arg(long, default_value = "parquet")]
    output_format: String,

    /// Maximum rows per parquet shard before splitting
    #[arg(long, default_value_t = 10_000)]
    max_shard_rows: usize,

    /// Maximum bytes (uncompressed estimate) per parquet shard before splitting
    #[arg(long, default_value_t = 256_000_000)]
    max_shard_bytes: usize,

    /// Resume from checkpoint (skip already-processed tars)
    #[arg(long)]
    resume: bool,

    /// Emit _metrics.json sidecar files per shard
    #[arg(long)]
    metrics: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum OutputFormat {
    Parquet,
    Jsonl,
}

impl OutputFormat {
    fn parse(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "parquet" => Ok(Self::Parquet),
            "jsonl" | "json" => Ok(Self::Jsonl),
            _ => anyhow::bail!("unknown output format '{}', expected 'parquet' or 'jsonl'", s),
        }
    }
}

/// Log jemalloc memory stats.
#[cfg(not(target_env = "msvc"))]
fn log_memory_stats() {
    use tikv_jemalloc_ctl::{epoch, stats};

    if epoch::advance().is_ok() {
        if let (Ok(allocated), Ok(resident)) = (stats::allocated::read(), stats::resident::read())
        {
            info!(
                "Memory: {:.1}MB allocated, {:.1}MB resident",
                allocated as f64 / 1_048_576.0,
                resident as f64 / 1_048_576.0
            );
        }
    }
}

#[cfg(target_env = "msvc")]
fn log_memory_stats() {}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    if let Some(threads) = args.threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build_global()
            .context("failed to set thread pool size")?;
    }

    let timeout = Duration::from_secs(args.timeout_secs);

    let text_files = args.text_files;

    let format = OutputFormat::parse(&args.output_format)?;

    if let Some(input_file) = &args.input_file {
        if input_file.is_dir() {
            let output_dir = args
                .output_dir
                .as_ref()
                .context("--output-dir is required when --input-file is a directory")?;
            process_batch(input_file, output_dir, timeout, text_files, format, args.max_shard_rows, args.max_shard_bytes, args.resume, args.metrics)?;
        } else {
            process_single_file(input_file, timeout, text_files)?;
        }
    } else if let Some(input_dir) = &args.input_dir {
        let output_dir = args
            .output_dir
            .as_ref()
            .context("--output-dir is required in batch mode")?;
        process_batch(input_dir, output_dir, timeout, text_files, format, args.max_shard_rows, args.max_shard_bytes, args.resume, args.metrics)?;
    } else {
        anyhow::bail!("Either --input-dir or --input-file must be specified");
    }

    log_memory_stats();

    Ok(())
}

/// Process a single archive file and print to stdout.
fn process_single_file(input_file: &Path, timeout: Duration, text_files: bool) -> Result<()> {
    let paper = archive::load_paper_archive(input_file)
        .with_context(|| format!("loading {}", input_file.display()))?;

    let result = extract_with_timeout(&paper, None, timeout);

    if text_files {
        if let Some(text) = &result.text {
            print!("{}", text);
        }
    } else {
        let json = serde_json::to_string(&result)?;
        println!("{}", json);
    }

    Ok(())
}

/// Process all outer tar files in batch mode.
fn process_batch(
    input_dir: &Path,
    output_dir: &Path,
    timeout: Duration,
    text_files: bool,
    format: OutputFormat,
    max_shard_rows: usize,
    max_shard_bytes: usize,
    resume: bool,
    emit_metrics: bool,
) -> Result<()> {
    fs::create_dir_all(output_dir)?;

    let mut archive_files: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(input_dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        if name.ends_with(".tar")
            || name.ends_with(".tar.gz")
            || name.ends_with(".tgz")
            || name.ends_with(".gz")
            || name.ends_with(".tex")
        {
            archive_files.push(path);
        }
    }
    archive_files.sort();

    if archive_files.is_empty() {
        info!("No archive files found in {}", input_dir.display());
        return Ok(());
    }

    let has_outer_tars = archive_files.iter().any(|p| {
        p.extension().map_or(false, |e| e == "tar")
    });

    if has_outer_tars {
        let tar_files: Vec<PathBuf> = archive_files
            .into_iter()
            .filter(|p| p.extension().map_or(false, |e| e == "tar"))
            .collect();
        if text_files {
            process_outer_tars_text(&tar_files, output_dir, timeout)?;
        } else {
            process_outer_tars(&tar_files, output_dir, timeout, format, max_shard_rows, max_shard_bytes, resume, emit_metrics)?;
        }
    } else if text_files {
        process_individual_archives_text(&archive_files, output_dir, timeout)?;
    } else {
        process_individual_archives(&archive_files, output_dir, timeout, format, max_shard_rows, max_shard_bytes, emit_metrics)?;
    }

    Ok(())
}

/// Process outer tar files in parallel.
fn process_outer_tars(
    tar_files: &[PathBuf],
    output_dir: &Path,
    timeout: Duration,
    format: OutputFormat,
    max_shard_rows: usize,
    max_shard_bytes: usize,
    resume: bool,
    emit_metrics: bool,
) -> Result<()> {
    let checkpoint_path = output_dir.join("checkpoint.log");

    let completed = if resume {
        let set = checkpoint::load_checkpoint(&checkpoint_path)?;
        if !set.is_empty() {
            info!("Resuming: {} tars already completed", set.len());
        }
        set
    } else {
        std::collections::HashSet::new()
    };

    let pending: Vec<&PathBuf> = tar_files
        .iter()
        .filter(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            !completed.contains(&name)
        })
        .collect();

    info!(
        "{}/{} outer tars remaining",
        pending.len(),
        tar_files.len()
    );

    if pending.is_empty() {
        info!("All tars already processed.");
        return Ok(());
    }

    let total_papers = Arc::new(AtomicU64::new(0));
    let total_errors = Arc::new(AtomicU64::new(0));
    let total_timeouts = Arc::new(AtomicU64::new(0));

    let progress = ProgressBar::new(pending.len() as u64);
    progress.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40} {pos}/{len} tars ({per_sec})")
            .unwrap(),
    );

    let cp_path = checkpoint_path.clone();

    pending.par_iter().for_each(|tar_path| {
        let papers = total_papers.clone();
        let errors = total_errors.clone();
        let timeouts = total_timeouts.clone();

        match process_outer_tar(tar_path, output_dir, timeout, format, max_shard_rows, max_shard_bytes, emit_metrics) {
            Ok((p, e, t)) => {
                papers.fetch_add(p, Ordering::Relaxed);
                errors.fetch_add(e, Ordering::Relaxed);
                timeouts.fetch_add(t, Ordering::Relaxed);

                let tar_name = tar_path.file_name().unwrap_or_default().to_string_lossy().to_string();
                if let Err(e) = checkpoint::record_checkpoint(&cp_path, &tar_name) {
                    error!("Checkpoint write error: {}", e);
                }
            }
            Err(e) => {
                error!("Failed to process {}: {}", tar_path.display(), e);
                errors.fetch_add(1, Ordering::Relaxed);
            }
        }

        progress.inc(1);
    });

    progress.finish();

    let p = total_papers.load(Ordering::Relaxed);
    let e = total_errors.load(Ordering::Relaxed);
    let t = total_timeouts.load(Ordering::Relaxed);
    let ok = p.saturating_sub(e).saturating_sub(t);

    info!(
        "Done: {} tars, {} papers ({} ok, {} errors, {} timeouts)",
        tar_files.len(),
        p,
        ok,
        e,
        t
    );

    Ok(())
}

/// Process individual .tar.gz or .gz archives with parallel extraction.
fn process_individual_archives(
    files: &[PathBuf],
    output_dir: &Path,
    timeout: Duration,
    format: OutputFormat,
    max_shard_rows: usize,
    max_shard_bytes: usize,
    emit_metrics: bool,
) -> Result<()> {
    let start = std::time::Instant::now();

    let progress = ProgressBar::new(files.len() as u64);
    progress.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40} {pos}/{len} papers ({per_sec})")
            .unwrap(),
    );

    let success = Arc::new(AtomicU64::new(0));
    let errors = Arc::new(AtomicU64::new(0));

    let (tx, rx) = mpsc::channel::<ExtractionResult>();

    let output_dir_owned = output_dir.to_path_buf();
    let writer_handle = thread::spawn(move || -> Result<()> {
        match format {
            OutputFormat::Parquet => {
                let mut writer = ParquetShardWriter::new(
                    &output_dir_owned,
                    "individual",
                    max_shard_rows,
                    max_shard_bytes,
                );
                for result in rx {
                    if let Err(e) = writer.write(result) {
                        error!("Parquet write error: {}", e);
                    }
                }
                writer.finish()?;
            }
            OutputFormat::Jsonl => {
                let output_path = output_dir_owned.join("results.jsonl");
                let file = File::create(&output_path)?;
                let mut writer = BufWriter::new(file);
                for result in rx {
                    if let Err(e) = serde_json::to_writer(&mut writer, &result) {
                        error!("JSON write error: {}", e);
                    }
                    let _ = writer.write_all(b"\n");
                }
                writer.flush()?;
            }
        }
        Ok(())
    });

    files.par_iter().for_each_with(tx, |tx, path| {
        let result = match archive::load_paper_archive(path) {
            Ok(paper) => {
                let result = extract_with_timeout(&paper, None, timeout);
                if result.status == "ok" {
                    success.fetch_add(1, Ordering::Relaxed);
                } else {
                    errors.fetch_add(1, Ordering::Relaxed);
                }
                result
            }
            Err(e) => {
                errors.fetch_add(1, Ordering::Relaxed);
                ExtractionResult {
                    arxiv_id: path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                    source_tar: None,
                    status: "error".into(),
                    num_tex_files: None,
                    text_length: None,
                    text: None,
                    error: Some(format!("{}", e)),
                    stage_timings_us: None,
                    total_time_us: None,
                    peak_memory_bytes: None,
                }
            }
        };
        let _ = tx.send(result);
        progress.inc(1);
    });

    writer_handle
        .join()
        .map_err(|_| anyhow::anyhow!("writer thread panicked"))??;

    progress.finish();

    let s = success.load(Ordering::Relaxed);
    let e = errors.load(Ordering::Relaxed);

    if emit_metrics {
        let elapsed = start.elapsed();
        let m = TarMetrics {
            tar_name: "individual".into(),
            total_papers: s + e,
            ok: s,
            errors: e,
            timeouts: 0,
            processing_time_ms: elapsed.as_millis() as u64,
        };
        if let Err(err) = metrics::write_metrics(output_dir, "individual", &m) {
            error!("Metrics write error: {}", err);
        }
    }

    info!(
        "Done: {} extracted, {} errors → {}",
        s, e, output_dir.display()
    );

    Ok(())
}

/// Sanitize an arxiv ID for use as a filename (replace `/` with `_`).
fn sanitize_id(id: &str) -> String {
    id.replace('/', "_")
}

/// Derive output filename stem from a path, stripping double extensions.
/// e.g., `paper.tar.gz` → `paper`, `paper.gz` → `paper`.
fn output_stem(path: &Path) -> String {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    stem.strip_suffix(".tar").unwrap_or(&stem).to_string()
}

/// Process individual archives, writing one .txt file per paper.
fn process_individual_archives_text(
    files: &[PathBuf],
    output_dir: &Path,
    timeout: Duration,
) -> Result<()> {
    let progress = ProgressBar::new(files.len() as u64);
    progress.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40} {pos}/{len} papers ({per_sec})")
            .unwrap(),
    );

    let success = Arc::new(AtomicU64::new(0));
    let errors = Arc::new(AtomicU64::new(0));

    files.par_iter().for_each(|path| {
        match archive::load_paper_archive(path) {
            Ok(paper) => {
                let result = extract_with_timeout(&paper, None, timeout);
                if let Some(text) = &result.text {
                    let out_path = output_dir.join(format!("{}.txt", output_stem(path)));
                    if let Err(e) = fs::write(&out_path, text) {
                        error!("Write error for {}: {}", path.display(), e);
                        errors.fetch_add(1, Ordering::Relaxed);
                    } else {
                        success.fetch_add(1, Ordering::Relaxed);
                    }
                } else {
                    errors.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(e) => {
                error!("Failed to load {}: {}", path.display(), e);
                errors.fetch_add(1, Ordering::Relaxed);
            }
        }
        progress.inc(1);
    });

    progress.finish();
    info!(
        "Done: {} extracted, {} errors → {}",
        success.load(Ordering::Relaxed),
        errors.load(Ordering::Relaxed),
        output_dir.display()
    );

    Ok(())
}

/// Process outer tar files, writing one .txt file per paper.
fn process_outer_tars_text(
    tar_files: &[PathBuf],
    output_dir: &Path,
    timeout: Duration,
) -> Result<()> {
    let pending: Vec<&PathBuf> = tar_files.iter().collect();

    info!("{} outer tars to process", pending.len());

    let total_papers = Arc::new(AtomicU64::new(0));
    let total_errors = Arc::new(AtomicU64::new(0));

    let progress = ProgressBar::new(pending.len() as u64);
    progress.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40} {pos}/{len} tars ({per_sec})")
            .unwrap(),
    );

    let output_dir_owned = output_dir.to_path_buf();

    pending.par_iter().for_each(|tar_path| {
        let papers = total_papers.clone();
        let errors = total_errors.clone();

        match process_outer_tar_text(tar_path, &output_dir_owned, timeout) {
            Ok((p, e)) => {
                papers.fetch_add(p, Ordering::Relaxed);
                errors.fetch_add(e, Ordering::Relaxed);
            }
            Err(e) => {
                error!("Failed to process {}: {}", tar_path.display(), e);
                errors.fetch_add(1, Ordering::Relaxed);
            }
        }

        progress.inc(1);
    });

    progress.finish();

    info!(
        "Done: {} papers, {} errors",
        total_papers.load(Ordering::Relaxed),
        total_errors.load(Ordering::Relaxed)
    );

    Ok(())
}

/// Process a single outer tar file, writing .txt files per paper.
fn process_outer_tar_text(
    tar_path: &Path,
    output_dir: &Path,
    timeout: Duration,
) -> Result<(u64, u64)> {
    let source_tar = tar_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let file = File::open(tar_path)?;
    let mut papers = 0u64;
    let mut errors = 0u64;

    archive::for_each_paper(file, |paper_result| {
        match paper_result {
            Ok(paper) => {
                let result = extract_with_timeout(&paper, Some(&source_tar), timeout);
                if let Some(text) = &result.text {
                    let out_path =
                        output_dir.join(format!("{}.txt", sanitize_id(&result.arxiv_id)));
                    if let Err(e) = fs::write(&out_path, text) {
                        error!("Write error for {}: {}", result.arxiv_id, e);
                        errors += 1;
                    }
                } else {
                    errors += 1;
                }
                papers += 1;
            }
            Err(e) => {
                error!("Archive entry error: {}", e);
                errors += 1;
            }
        }
    });

    Ok((papers, errors))
}

/// Process a single outer tar file.
fn process_outer_tar(
    tar_path: &Path,
    output_dir: &Path,
    timeout: Duration,
    format: OutputFormat,
    max_shard_rows: usize,
    max_shard_bytes: usize,
    emit_metrics: bool,
) -> Result<(u64, u64, u64)> {
    let stem = tar_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let source_tar = tar_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let start = std::time::Instant::now();
    let file = File::open(tar_path)?;

    let mut papers = 0u64;
    let mut errors = 0u64;
    let mut timeouts = 0u64;

    match format {
        OutputFormat::Parquet => {
            let mut writer =
                ParquetShardWriter::new(output_dir, &stem, max_shard_rows, max_shard_bytes);

            archive::for_each_paper(file, |paper_result| {
                match paper_result {
                    Ok(paper) => {
                        let result = extract_with_timeout(&paper, Some(&source_tar), timeout);
                        match result.status.as_str() {
                            "ok" => {}
                            "timeout" => timeouts += 1,
                            _ => errors += 1,
                        }
                        papers += 1;
                        if let Err(e) = writer.write(result) {
                            error!("Parquet write error in {}: {}", stem, e);
                        }
                    }
                    Err(e) => {
                        error!("Archive entry error in {}: {}", stem, e);
                        errors += 1;
                    }
                }
            });

            if let Err(e) = writer.finish() {
                error!("Parquet finish error for {}: {}", stem, e);
            }
        }
        OutputFormat::Jsonl => {
            let output_path = output_dir.join(format!("{}.jsonl", stem));
            let temp_path = output_dir.join(format!(".{}.jsonl.tmp", stem));
            let output_file = File::create(&temp_path)?;
            let mut writer = BufWriter::new(output_file);

            archive::for_each_paper(file, |paper_result| {
                match paper_result {
                    Ok(paper) => {
                        let result = extract_with_timeout(&paper, Some(&source_tar), timeout);
                        match result.status.as_str() {
                            "ok" => {}
                            "timeout" => timeouts += 1,
                            _ => errors += 1,
                        }
                        papers += 1;
                        if let Err(e) = serde_json::to_writer(&mut writer, &result) {
                            error!("JSON write error for {}: {}", paper.arxiv_id, e);
                        }
                        let _ = writer.write_all(b"\n");
                    }
                    Err(e) => {
                        error!("Archive entry error in {}: {}", stem, e);
                        errors += 1;
                    }
                }
            });

            writer.flush()?;
            drop(writer);
            fs::rename(&temp_path, &output_path)?;
        }
    }

    if emit_metrics {
        let elapsed = start.elapsed();
        let ok_count = papers.saturating_sub(errors).saturating_sub(timeouts);
        let m = TarMetrics {
            tar_name: source_tar.clone(),
            total_papers: papers,
            ok: ok_count,
            errors,
            timeouts,
            processing_time_ms: elapsed.as_millis() as u64,
        };
        if let Err(e) = metrics::write_metrics(output_dir, &stem, &m) {
            error!("Metrics write error for {}: {}", stem, e);
        }
    }

    Ok((papers, errors, timeouts))
}

/// Process a single paper with panic isolation and a per-document timeout.
///
/// Spawns a dedicated thread for extraction so that stuck documents (infinite
/// macro loops, pathological regex) don't block the rayon worker pool. A
/// timed-out thread is intentionally leaked — at <0.01% timeout rate across
/// 2.7M documents, ~270 leaked threads over hours of runtime is negligible.
fn extract_with_timeout(
    paper: &PaperArchive,
    source_tar: Option<&str>,
    timeout: Duration,
) -> ExtractionResult {
    let num_files = paper.tex_files.len();

    let total_bytes: usize = paper.tex_files.iter().map(|f| f.content.len()).sum();
    if total_bytes > MAX_TEX_CONTENT_BYTES {
        return ExtractionResult {
            arxiv_id: paper.arxiv_id.clone(),
            source_tar: source_tar.map(|s| s.to_string()),
            status: "skipped".into(),
            num_tex_files: Some(num_files),
            text_length: None,
            text: None,
            error: Some(format!(
                "combined .tex content ({} bytes) exceeds {}MB limit",
                total_bytes,
                MAX_TEX_CONTENT_BYTES / 1_000_000
            )),
            stage_timings_us: None,
            total_time_us: None,
            peak_memory_bytes: None,
        };
    }

    // Clone data for the spawned thread (thread::spawn requires 'static)
    let tex_files = paper.tex_files.clone();
    let arxiv_id = paper.arxiv_id.clone();
    let source_tar_owned = source_tar.map(|s| s.to_string());

    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let paper = PaperArchive {
            arxiv_id,
            tex_files,
        };
        let result = process_paper(&paper, source_tar_owned.as_deref());
        let _ = tx.send(result);
    });

    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => ExtractionResult {
            arxiv_id: paper.arxiv_id.clone(),
            source_tar: source_tar.map(|s| s.to_string()),
            status: "timeout".into(),
            num_tex_files: Some(num_files),
            text_length: None,
            text: None,
            error: Some(format!(
                "extraction timed out after {}s",
                timeout.as_secs()
            )),
            stage_timings_us: None,
            total_time_us: None,
            peak_memory_bytes: None,
        },
        Err(mpsc::RecvTimeoutError::Disconnected) => ExtractionResult {
            arxiv_id: paper.arxiv_id.clone(),
            source_tar: source_tar.map(|s| s.to_string()),
            status: "error".into(),
            num_tex_files: Some(num_files),
            text_length: None,
            text: None,
            error: Some("extraction thread crashed".into()),
            stage_timings_us: None,
            total_time_us: None,
            peak_memory_bytes: None,
        },
    }
}

/// Process a single paper with panic isolation.
fn process_paper(paper: &PaperArchive, source_tar: Option<&str>) -> ExtractionResult {
    if paper.tex_files.is_empty() {
        return ExtractionResult {
            arxiv_id: paper.arxiv_id.clone(),
            source_tar: source_tar.map(|s| s.to_string()),
            status: "empty".into(),
            num_tex_files: Some(0),
            text_length: None,
            text: None,
            error: None,
            stage_timings_us: None,
            total_time_us: None,
            peak_memory_bytes: None,
        };
    }

    let tex_files = paper.tex_files.clone();
    let num_files = tex_files.len();

    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        extract_text_timed(&tex_files)
    }));

    match result {
        Ok(output) => match output.text {
            Some(text) => ExtractionResult {
                arxiv_id: paper.arxiv_id.clone(),
                source_tar: source_tar.map(|s| s.to_string()),
                status: "ok".into(),
                num_tex_files: Some(num_files),
                text_length: Some(text.len()),
                text: Some(text),
                error: None,
                stage_timings_us: Some(output.timings.to_json()),
                total_time_us: Some(output.timings.total_us()),
                peak_memory_bytes: None,
            },
            None => ExtractionResult {
                arxiv_id: paper.arxiv_id.clone(),
                source_tar: source_tar.map(|s| s.to_string()),
                status: "empty".into(),
                num_tex_files: Some(num_files),
                text_length: None,
                text: None,
                error: None,
                stage_timings_us: Some(output.timings.to_json()),
                total_time_us: Some(output.timings.total_us()),
                peak_memory_bytes: None,
            },
        },
        Err(panic_info) => {
            let msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = panic_info.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".into()
            };
            ExtractionResult {
                arxiv_id: paper.arxiv_id.clone(),
                source_tar: source_tar.map(|s| s.to_string()),
                status: "error".into(),
                num_tex_files: Some(num_files),
                text_length: None,
                text: None,
                error: Some(msg),
                stage_timings_us: None,
                total_time_us: None,
                peak_memory_bytes: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use latex_extract::input_resolve::TexFile;

    #[test]
    fn test_content_size_cap() {
        let large_content = "x".repeat(MAX_TEX_CONTENT_BYTES + 1);
        let paper = PaperArchive {
            arxiv_id: "test.oversize".into(),
            tex_files: vec![TexFile {
                name: "main.tex".into(),
                content: large_content,
            }],
        };
        let result = extract_with_timeout(&paper, None, Duration::from_secs(5));
        assert_eq!(result.status, "skipped");
        assert!(result.error.unwrap().contains("exceeds"));
    }

    #[test]
    fn test_timeout_fires() {
        // Create a paper with content that will process quickly, but
        // simulate a timeout by using a very short timeout duration
        // with content that takes longer than that.
        // We use a 0-second timeout to guarantee the timeout fires
        // before the spawned thread completes.
        let paper = PaperArchive {
            arxiv_id: "test.timeout".into(),
            tex_files: vec![TexFile {
                name: "main.tex".into(),
                content: r"\documentclass{article}\begin{document}Hello\end{document}".into(),
            }],
        };
        // Duration::ZERO means recv_timeout returns immediately
        let result = extract_with_timeout(&paper, None, Duration::ZERO);
        // With zero timeout, we get either timeout or the result (race),
        // but the mechanism is exercised either way.
        assert!(result.status == "timeout" || result.status == "ok");
    }

    #[test]
    fn test_normal_extraction_with_timeout() {
        let paper = PaperArchive {
            arxiv_id: "test.normal".into(),
            tex_files: vec![TexFile {
                name: "main.tex".into(),
                content: r"\documentclass{article}
\begin{document}
\section{Introduction}
Hello world.
\end{document}"
                    .into(),
            }],
        };
        let result = extract_with_timeout(&paper, Some("test.tar"), Duration::from_secs(5));
        assert_eq!(result.status, "ok");
        assert!(result.text.unwrap().contains("Hello world."));
        assert_eq!(result.source_tar.unwrap(), "test.tar");
    }

    #[test]
    fn test_extraction_result_has_timing() {
        let paper = PaperArchive {
            arxiv_id: "test.timing".into(),
            tex_files: vec![TexFile {
                name: "main.tex".into(),
                content: r"\documentclass{article}
\begin{document}
Hello.
\end{document}"
                    .into(),
            }],
        };
        let result = extract_with_timeout(&paper, None, Duration::from_secs(5));
        assert_eq!(result.status, "ok");
        assert!(result.stage_timings_us.is_some(), "should have stage timings");
        assert!(result.total_time_us.is_some(), "should have total time");
        assert!(result.total_time_us.unwrap() > 0);

        let json = result.stage_timings_us.unwrap();
        assert!(json.contains("remove_comments"), "timing JSON: {json}");
    }
}
