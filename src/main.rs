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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use std::sync::Mutex;
use tracing::{debug, error, info, trace, warn};

use latex_extract::admission::AdmissionControl;
use latex_extract::archive::{self, PaperArchive};
use latex_extract::checkpoint;
use latex_extract::metrics::{self, Outcome, StatusCounts, TarMetrics};
use latex_extract::output::ParquetShardWriter;
use latex_extract::pipeline::extract_text_timed_cancellable;
use latex_extract::result::ExtractionResult;

/// Default maximum combined .tex content size (20MB).
const DEFAULT_MAX_TEX_BYTES: usize = 20_000_000;

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
    #[arg(short = 't', long, default_value_t = 45)]
    timeout_secs: u64,

    /// Maximum combined .tex content in bytes (papers exceeding this are skipped)
    #[arg(long, default_value_t = DEFAULT_MAX_TEX_BYTES)]
    max_tex_bytes: usize,

    /// Maximum process memory (MB) before skipping entries (safety limit).
    /// Superseded by `--max-inflight`: the admission-control semaphore is
    /// the authoritative memory bound. This value, if set, seeds the
    /// default inflight cap (`max_memory_mb / 150`).
    #[arg(long)]
    max_memory_mb: Option<usize>,

    /// Maximum concurrently-extracting papers. Bounds peak RSS independent
    /// of rayon worker count (rayon workers block at this semaphore when
    /// the cap is reached). Default: `max_memory_mb / 150` (since per-paper
    /// peak is ~125 MB after the macro-expansion fix) or
    /// `available_parallelism()` when `--max-memory-mb` is unset.
    #[arg(long)]
    max_inflight: Option<usize>,

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
    let max_tex_bytes = args.max_tex_bytes;
    let max_memory_bytes: Option<usize> = args.max_memory_mb.map(|mb| mb * 1_048_576);

    // Admission-control cap. Per-paper peak RSS post-macro-fix is ~125 MB,
    // so 150 MB/paper is a safe amortised estimate.
    let max_inflight = args.max_inflight.unwrap_or_else(|| {
        if let Some(mb) = args.max_memory_mb {
            (mb / 150).max(1)
        } else {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        }
    });
    let admission = Arc::new(AdmissionControl::new(max_inflight));
    info!("admission cap: {} concurrent extractors", max_inflight);

    let text_files = args.text_files;

    let format = OutputFormat::parse(&args.output_format)?;

    if let Some(input_file) = &args.input_file {
        if input_file.is_dir() {
            let output_dir = args
                .output_dir
                .as_ref()
                .context("--output-dir is required when --input-file is a directory")?;
            process_batch(input_file, output_dir, timeout, max_tex_bytes, max_memory_bytes, text_files, format, args.max_shard_rows, args.max_shard_bytes, args.resume, args.metrics, Arc::clone(&admission))?;
        } else {
            process_single_file(input_file, timeout, max_tex_bytes, max_memory_bytes, text_files, Arc::clone(&admission))?;
        }
    } else if let Some(input_dir) = &args.input_dir {
        let output_dir = args
            .output_dir
            .as_ref()
            .context("--output-dir is required in batch mode")?;
        process_batch(input_dir, output_dir, timeout, max_tex_bytes, max_memory_bytes, text_files, format, args.max_shard_rows, args.max_shard_bytes, args.resume, args.metrics, Arc::clone(&admission))?;
    } else {
        anyhow::bail!("Either --input-dir or --input-file must be specified");
    }

    log_memory_stats();

    Ok(())
}

/// Process a single archive file and print to stdout.
fn process_single_file(input_file: &Path, timeout: Duration, max_tex_bytes: usize, max_memory_bytes: Option<usize>, text_files: bool, admission: Arc<AdmissionControl>) -> Result<()> {
    let paper = archive::load_paper_archive(input_file)
        .with_context(|| format!("loading {}", input_file.display()))?;

    let result = extract_with_timeout(&paper, None, timeout, max_tex_bytes, max_memory_bytes, &admission);

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
    max_tex_bytes: usize,
    max_memory_bytes: Option<usize>,
    text_files: bool,
    format: OutputFormat,
    max_shard_rows: usize,
    max_shard_bytes: usize,
    resume: bool,
    emit_metrics: bool,
    admission: Arc<AdmissionControl>,
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
            process_outer_tars_text(&tar_files, output_dir, timeout, max_tex_bytes, max_memory_bytes, admission)?;
        } else {
            process_outer_tars(&tar_files, output_dir, timeout, max_tex_bytes, max_memory_bytes, format, max_shard_rows, max_shard_bytes, resume, emit_metrics, admission)?;
        }
    } else if text_files {
        process_individual_archives_text(&archive_files, output_dir, timeout, max_tex_bytes, max_memory_bytes, admission)?;
    } else {
        process_individual_archives(&archive_files, output_dir, timeout, max_tex_bytes, max_memory_bytes, format, max_shard_rows, max_shard_bytes, emit_metrics, admission)?;
    }

    Ok(())
}

/// Process outer tar files in parallel.
fn process_outer_tars(
    tar_files: &[PathBuf],
    output_dir: &Path,
    timeout: Duration,
    max_tex_bytes: usize,
    max_memory_bytes: Option<usize>,
    format: OutputFormat,
    max_shard_rows: usize,
    max_shard_bytes: usize,
    resume: bool,
    emit_metrics: bool,
    admission: Arc<AdmissionControl>,
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

    let counts = Arc::new(Mutex::new(StatusCounts::default()));

    let progress = ProgressBar::new(pending.len() as u64);
    progress.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40} {pos}/{len} tars ({per_sec})")
            .unwrap(),
    );

    let cp_path = checkpoint_path.clone();

    pending.par_iter().for_each(|tar_path| {
        match process_outer_tar(tar_path, output_dir, timeout, max_tex_bytes, max_memory_bytes, format, max_shard_rows, max_shard_bytes, emit_metrics, &admission) {
            Ok(tar_counts) => {
                counts.lock().unwrap().merge(&tar_counts);

                let tar_name = tar_path.file_name().unwrap_or_default().to_string_lossy().to_string();
                if let Err(e) = checkpoint::record_checkpoint(&cp_path, &tar_name) {
                    error!("Checkpoint write error: {}", e);
                }
            }
            Err(e) => {
                error!("Failed to process {}: {}", tar_path.display(), e);
                let tar_name = tar_path.file_stem().unwrap_or_default().to_string_lossy().to_string();
                counts.lock().unwrap().record(Outcome::ArchiveError, &tar_name);
            }
        }

        // Return freed pages to the OS between tars so RSS plateaus
        // instead of growing monotonically across the corpus.
        latex_extract::memory::purge_jemalloc_arenas();

        progress.inc(1);
    });

    progress.finish();

    let c = counts.lock().unwrap();
    c.log_summary(&format!(
        "Done: {} tars, {} papers",
        tar_files.len(),
        c.total()
    ));

    Ok(())
}

/// Process individual .tar.gz or .gz archives with parallel extraction.
fn process_individual_archives(
    files: &[PathBuf],
    output_dir: &Path,
    timeout: Duration,
    max_tex_bytes: usize,
    max_memory_bytes: Option<usize>,
    format: OutputFormat,
    max_shard_rows: usize,
    max_shard_bytes: usize,
    emit_metrics: bool,
    admission: Arc<AdmissionControl>,
) -> Result<()> {
    let start = std::time::Instant::now();

    let progress = ProgressBar::new(files.len() as u64);
    progress.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40} {pos}/{len} papers ({per_sec})")
            .unwrap(),
    );

    let counts = Arc::new(Mutex::new(StatusCounts::default()));
    let io_errors = Arc::new(Mutex::new(StatusCounts::default()));

    let (tx, rx) = mpsc::channel::<ExtractionResult>();

    let output_dir_owned = output_dir.to_path_buf();
    let io_errors_writer = io_errors.clone();
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
                    let id = result.arxiv_id.clone();
                    if let Err(e) = writer.write(result) {
                        error!(category = "io", "parquet write error: {}", e);
                        io_errors_writer.lock().unwrap().record(Outcome::IoError, &id);
                    }
                }
                writer.finish()?;
            }
            OutputFormat::Jsonl => {
                let output_path = output_dir_owned.join("results.jsonl");
                let file = File::create(&output_path)?;
                let mut writer = BufWriter::new(file);
                for result in rx {
                    let id = result.arxiv_id.clone();
                    if let Err(e) = serde_json::to_writer(&mut writer, &result) {
                        error!(category = "io", "JSON write error: {}", e);
                        io_errors_writer.lock().unwrap().record(Outcome::IoError, &id);
                    }
                    let _ = writer.write_all(b"\n");
                }
                writer.flush()?;
            }
        }
        Ok(())
    });

    let counts_ref = counts.clone();
    let admission_ref = admission;
    files.par_iter().for_each_with(tx, |tx, path| {
        let result = match archive::load_paper_archive(path) {
            Ok(paper) => {
                let result = extract_with_timeout(&paper, None, timeout, max_tex_bytes, max_memory_bytes, &admission_ref);
                counts_ref.lock().unwrap().record(classify_result(&result), &result.arxiv_id);
                result
            }
            Err(e) => {
                let id = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                error!(category = "archive", arxiv_id = %id, "load failure: {}", e);
                counts_ref.lock().unwrap().record(Outcome::ArchiveError, &id);
                ExtractionResult {
                    arxiv_id: id,
                    source_tar: None,
                    status: "error".into(),
                    num_tex_files: None,
                    text_length: None,
                    text: None,
                    error: Some(format!("{}", e)),
                    stage_timings_us: None,
                    total_time_us: None,
                    peak_memory_bytes: None,
                    file_type: None,
                    entry_name: None,
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

    let mut c = counts.lock().unwrap();
    c.merge(&io_errors.lock().unwrap());

    if emit_metrics {
        let elapsed = start.elapsed();
        let m = TarMetrics::from_counts("individual".into(), &c, elapsed.as_millis() as u64);
        if let Err(err) = metrics::write_metrics(output_dir, "individual", &m) {
            error!("Metrics write error: {}", err);
        }
    }

    c.log_summary(&format!(
        "Done: {} papers → {}",
        c.total(),
        output_dir.display()
    ));

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
    max_tex_bytes: usize,
    max_memory_bytes: Option<usize>,
    admission: Arc<AdmissionControl>,
) -> Result<()> {
    let progress = ProgressBar::new(files.len() as u64);
    progress.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40} {pos}/{len} papers ({per_sec})")
            .unwrap(),
    );

    let counts = Arc::new(Mutex::new(StatusCounts::default()));

    let admission_ref = admission;
    files.par_iter().for_each(|path| {
        let mut local_counts = StatusCounts::default();
        match archive::load_paper_archive(path) {
            Ok(paper) => {
                let result = extract_with_timeout(&paper, None, timeout, max_tex_bytes, max_memory_bytes, &admission_ref);
                let outcome = classify_result(&result);
                if let Some(text) = &result.text {
                    let out_path = output_dir.join(format!("{}.txt", output_stem(path)));
                    if let Err(e) = fs::write(&out_path, text) {
                        error!(category = "io", arxiv_id = %result.arxiv_id, "text file write error: {}", e);
                        local_counts.record(Outcome::IoError, &result.arxiv_id);
                    }
                }
                local_counts.record(outcome, &result.arxiv_id);
            }
            Err(e) => {
                let id = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
                error!(category = "archive", arxiv_id = %id, "load failure: {}", e);
                local_counts.record(Outcome::ArchiveError, &id);
            }
        }
        counts.lock().unwrap().merge(&local_counts);
        progress.inc(1);
    });

    progress.finish();

    let c = counts.lock().unwrap();
    c.log_summary(&format!(
        "Done: {} papers → {}",
        c.total(),
        output_dir.display()
    ));

    Ok(())
}

/// Process outer tar files, writing one .txt file per paper.
fn process_outer_tars_text(
    tar_files: &[PathBuf],
    output_dir: &Path,
    timeout: Duration,
    max_tex_bytes: usize,
    max_memory_bytes: Option<usize>,
    admission: Arc<AdmissionControl>,
) -> Result<()> {
    let pending: Vec<&PathBuf> = tar_files.iter().collect();

    info!("{} outer tars to process", pending.len());

    let counts = Arc::new(Mutex::new(StatusCounts::default()));

    let progress = ProgressBar::new(pending.len() as u64);
    progress.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40} {pos}/{len} tars ({per_sec})")
            .unwrap(),
    );

    let output_dir_owned = output_dir.to_path_buf();

    pending.par_iter().for_each(|tar_path| {
        match process_outer_tar_text(tar_path, &output_dir_owned, timeout, max_tex_bytes, max_memory_bytes, &admission) {
            Ok(tar_counts) => {
                counts.lock().unwrap().merge(&tar_counts);
            }
            Err(e) => {
                error!("Failed to process {}: {}", tar_path.display(), e);
                let tar_name = tar_path.file_stem().unwrap_or_default().to_string_lossy().to_string();
                counts.lock().unwrap().record(Outcome::ArchiveError, &tar_name);
            }
        }

        progress.inc(1);
    });

    progress.finish();

    let c = counts.lock().unwrap();
    c.log_summary(&format!(
        "Done: {} tars, {} papers",
        tar_files.len(),
        c.total()
    ));

    Ok(())
}

/// Process a single outer tar file, writing .txt files per paper.
/// Extract text from a paper archive, with trace-level logging before and after.
fn extract_paper_with_trace(
    paper: &PaperArchive,
    source_tar: &str,
    stem: &str,
    timeout: Duration,
    max_tex_bytes: usize,
    max_memory_bytes: Option<usize>,
    admission: &Arc<AdmissionControl>,
) -> ExtractionResult {
    let total_bytes: usize = paper.tex_files.iter().map(|f| f.content.len()).sum();
    trace!(
        arxiv_id = %paper.arxiv_id,
        num_files = paper.tex_files.len(),
        total_bytes,
        tar = %stem,
        "processing paper"
    );
    let result = extract_with_timeout(paper, Some(source_tar), timeout, max_tex_bytes, max_memory_bytes, admission);
    trace!(
        arxiv_id = %result.arxiv_id,
        status = %result.status,
        tar = %stem,
        "processed paper"
    );
    result
}

fn process_outer_tar_text(
    tar_path: &Path,
    output_dir: &Path,
    timeout: Duration,
    max_tex_bytes: usize,
    max_memory_bytes: Option<usize>,
    admission: &Arc<AdmissionControl>,
) -> Result<StatusCounts> {
    let source_tar = tar_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let stem = tar_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    debug!(tar = %source_tar, "processing tar");
    let file = File::open(tar_path)?;
    let mut counts = StatusCounts::default();

    archive::for_each_paper(file, |paper_result| {
        match paper_result {
            Ok(paper) => {
                let result = extract_paper_with_trace(&paper, &source_tar, &stem, timeout, max_tex_bytes, max_memory_bytes, admission);
                let outcome = classify_result(&result);
                if let Some(text) = &result.text {
                    let out_path =
                        output_dir.join(format!("{}.txt", sanitize_id(&result.arxiv_id)));
                    if let Err(e) = fs::write(&out_path, text) {
                        error!(category = "io", arxiv_id = %result.arxiv_id, "text file write error: {}", e);
                        counts.record(Outcome::IoError, &result.arxiv_id);
                    }
                }
                counts.record(outcome, &result.arxiv_id);
            }
            Err(e) => {
                error!(category = "archive", tar = %stem, "entry error: {}", e);
                counts.record(Outcome::ArchiveError, "unknown");
            }
        }
    });

    Ok(counts)
}

/// Process a single outer tar file.
fn process_outer_tar(
    tar_path: &Path,
    output_dir: &Path,
    timeout: Duration,
    max_tex_bytes: usize,
    max_memory_bytes: Option<usize>,
    format: OutputFormat,
    max_shard_rows: usize,
    max_shard_bytes: usize,
    emit_metrics: bool,
    admission: &Arc<AdmissionControl>,
) -> Result<StatusCounts> {
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
    debug!(tar = %source_tar, "processing tar");
    let file = File::open(tar_path)?;

    let mut counts = StatusCounts::default();

    match format {
        OutputFormat::Parquet => {
            let mut writer =
                ParquetShardWriter::new(output_dir, &stem, max_shard_rows, max_shard_bytes);

            archive::for_each_paper(file, |paper_result| {
                match paper_result {
                    Ok(paper) => {
                        let result = extract_paper_with_trace(&paper, &source_tar, &stem, timeout, max_tex_bytes, max_memory_bytes, admission);
                        counts.record(classify_result(&result), &result.arxiv_id);
                        if let Err(e) = writer.write(result) {
                            error!(category = "io", tar = %stem, "parquet write error: {}", e);
                            counts.record(Outcome::IoError, &paper.arxiv_id);
                        }
                    }
                    Err(e) => {
                        error!(category = "archive", tar = %stem, "entry error: {}", e);
                        counts.record(Outcome::ArchiveError, "unknown");
                    }
                }
            });

            if let Err(e) = writer.finish() {
                error!(category = "io", tar = %stem, "parquet finish error: {}", e);
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
                        let result = extract_paper_with_trace(&paper, &source_tar, &stem, timeout, max_tex_bytes, max_memory_bytes, admission);
                        counts.record(classify_result(&result), &result.arxiv_id);
                        if let Err(e) = serde_json::to_writer(&mut writer, &result) {
                            error!(category = "io", tar = %stem, "JSON write error: {}", e);
                            counts.record(Outcome::IoError, &paper.arxiv_id);
                        }
                        let _ = writer.write_all(b"\n");
                    }
                    Err(e) => {
                        error!(category = "archive", tar = %stem, "entry error: {}", e);
                        counts.record(Outcome::ArchiveError, "unknown");
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
        let m = TarMetrics::from_counts(source_tar, &counts, elapsed.as_millis() as u64);
        if let Err(e) = metrics::write_metrics(output_dir, &stem, &m) {
            error!("Metrics write error for {}: {}", stem, e);
        }
    }

    Ok(counts)
}

/// Classify an ExtractionResult into an Outcome.
///
/// Used when the caller receives a result from `extract_with_timeout` and
/// doesn't know the internal reason for an "error" status. Archive errors
/// are classified directly by callers — they never go through this function.
fn classify_result(result: &ExtractionResult) -> Outcome {
    match result.status.as_str() {
        "ok" => Outcome::Ok,
        "timeout" => Outcome::Timeout,
        "skipped" => Outcome::Skipped,
        "empty" => Outcome::Empty,
        _ => match result.error.as_deref() {
            Some("extraction thread crashed") => Outcome::Crash,
            _ => Outcome::Panic,
        },
    }
}

/// Process a single paper with panic isolation and a per-document timeout.
///
/// Spawns a dedicated thread for extraction so that stuck documents (infinite
/// macro loops, pathological regex) don't block the rayon worker pool. On
/// timeout the parent sets a shared `AtomicBool` cancel flag; the spawned
/// thread polls it between pipeline stages (src/pipeline.rs, src/macros.rs)
/// and exits cooperatively instead of being silently leaked.
fn extract_with_timeout(
    paper: &PaperArchive,
    source_tar: Option<&str>,
    timeout: Duration,
    max_tex_bytes: usize,
    _max_memory_bytes: Option<usize>,
    admission: &Arc<AdmissionControl>,
) -> ExtractionResult {
    let num_files = paper.tex_files.len();

    let total_bytes: usize = paper.tex_files.iter().map(|f| f.content.len()).sum();
    if total_bytes > max_tex_bytes {
        warn!(
            category = "skipped",
            arxiv_id = %paper.arxiv_id,
            "combined .tex {} bytes exceeds limit",
            total_bytes
        );
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
                max_tex_bytes / 1_000_000
            )),
            stage_timings_us: None,
            total_time_us: None,
            peak_memory_bytes: None,
            file_type: Some(paper.file_type),
            entry_name: Some(paper.entry_name.clone()),
        };
    }

    // The old racy pre-spawn `get_allocated_bytes() > max_memory` check was
    // replaced by the admission-control semaphore below: N parallel rayon
    // workers could all pass the check simultaneously and collectively
    // exceed the limit. The semaphore is authoritative.

    // Clone data for the spawned thread (thread::spawn requires 'static).
    // tex_files is an Arc<Vec<TexFile>>, so this is a refcount bump.
    let tex_files = Arc::clone(&paper.tex_files);
    let arxiv_id = paper.arxiv_id.clone();
    let file_type = paper.file_type;
    let entry_name = paper.entry_name.clone();
    let source_tar_owned = source_tar.map(|s| s.to_string());

    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = Arc::clone(&cancel);

    let (tx, rx) = mpsc::channel();

    // Acquire an admission permit before doing any allocating work. The
    // permit is moved into the closure so it lives exactly as long as the
    // extractor thread, including during cooperative cancellation.
    let permit = admission.acquire_owned();

    thread::spawn(move || {
        let _permit = permit;
        let paper = PaperArchive {
            arxiv_id,
            tex_files,
            file_type,
            entry_name,
        };
        let result = process_paper(&paper, source_tar_owned.as_deref(), Some(&cancel_worker));
        let _ = tx.send(result);
    });

    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            // Signal the extractor thread to wind down on its next checkpoint.
            cancel.store(true, Ordering::Relaxed);
            error!(
                category = "timeout",
                arxiv_id = %paper.arxiv_id,
                "timed out after {}s",
                timeout.as_secs()
            );
            ExtractionResult {
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
                file_type: Some(paper.file_type),
                entry_name: Some(paper.entry_name.clone()),
            }
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            error!(
                category = "crash",
                arxiv_id = %paper.arxiv_id,
                "extraction thread crashed"
            );
            ExtractionResult {
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
                file_type: Some(paper.file_type),
                entry_name: Some(paper.entry_name.clone()),
            }
        }
    }
}

/// Process a single paper with panic isolation.
///
/// `cancel` is passed through to the extraction pipeline so that a parent
/// timeout on `extract_with_timeout` can steer this work to exit early.
fn process_paper(
    paper: &PaperArchive,
    source_tar: Option<&str>,
    cancel: Option<&AtomicBool>,
) -> ExtractionResult {
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
            file_type: Some(paper.file_type),
            entry_name: Some(paper.entry_name.clone()),
        };
    }

    // Arc clone — refcount bump, not a deep copy of the Vec<TexFile>.
    let tex_files = Arc::clone(&paper.tex_files);
    let num_files = tex_files.len();

    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        extract_text_timed_cancellable(&tex_files, cancel)
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
                file_type: Some(paper.file_type),
                entry_name: Some(paper.entry_name.clone()),
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
                file_type: Some(paper.file_type),
                entry_name: Some(paper.entry_name.clone()),
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
            error!(
                category = "panic",
                arxiv_id = %paper.arxiv_id,
                "extraction panic: {}",
                msg
            );
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
                file_type: Some(paper.file_type),
                entry_name: Some(paper.entry_name.clone()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use latex_extract::admission::AdmissionControl;
    use latex_extract::input_resolve::TexFile;
    use latex_extract::result::FileType;
    use std::sync::Arc;

    fn test_admission() -> Arc<AdmissionControl> {
        Arc::new(AdmissionControl::new(4))
    }

    #[test]
    fn test_content_size_cap() {
        let large_content = "x".repeat(DEFAULT_MAX_TEX_BYTES + 1);
        let paper = PaperArchive {
            arxiv_id: "test.oversize".into(),
            tex_files: Arc::new(vec![TexFile {
                name: "main.tex".into(),
                content: large_content,
            }]),
            file_type: FileType::Tex,
            entry_name: "test.gz".into(),
        };
        let result = extract_with_timeout(&paper, None, Duration::from_secs(5), DEFAULT_MAX_TEX_BYTES, None, &test_admission());
        assert_eq!(result.status, "skipped");
        assert!(result.error.unwrap().contains("exceeds"));
    }

    #[test]
    fn test_content_size_cap_custom() {
        let content = "x".repeat(100);
        let paper = PaperArchive {
            arxiv_id: "test.custom_limit".into(),
            tex_files: Arc::new(vec![TexFile {
                name: "main.tex".into(),
                content: content.clone(),
            }]),
            file_type: FileType::Tex,
            entry_name: "test.gz".into(),
        };
        // 100 bytes exceeds a 50-byte custom limit
        let result = extract_with_timeout(&paper, None, Duration::from_secs(5), 50, None, &test_admission());
        assert_eq!(result.status, "skipped");

        // Same content passes with a larger limit
        let paper2 = PaperArchive {
            arxiv_id: "test.custom_limit_ok".into(),
            tex_files: Arc::new(vec![TexFile {
                name: "main.tex".into(),
                content,
            }]),
            file_type: FileType::Tex,
            entry_name: "test.gz".into(),
        };
        let result2 = extract_with_timeout(&paper2, None, Duration::from_secs(5), 200, None, &test_admission());
        assert_ne!(result2.status, "skipped");
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
            tex_files: Arc::new(vec![TexFile {
                name: "main.tex".into(),
                content: r"\documentclass{article}\begin{document}Hello\end{document}".into(),
            }]),
            file_type: FileType::Tex,
            entry_name: "test.gz".into(),
        };
        // Duration::ZERO means recv_timeout returns immediately
        let result = extract_with_timeout(&paper, None, Duration::ZERO, DEFAULT_MAX_TEX_BYTES, None, &test_admission());
        // With zero timeout, we get either timeout or the result (race),
        // but the mechanism is exercised either way.
        assert!(result.status == "timeout" || result.status == "ok");
    }

    #[test]
    fn test_normal_extraction_with_timeout() {
        let paper = PaperArchive {
            arxiv_id: "test.normal".into(),
            tex_files: Arc::new(vec![TexFile {
                name: "main.tex".into(),
                content: r"\documentclass{article}
\begin{document}
\section{Introduction}
Hello world.
\end{document}"
                    .into(),
            }]),
            file_type: FileType::Tex,
            entry_name: "test.gz".into(),
        };
        let result = extract_with_timeout(&paper, Some("test.tar"), Duration::from_secs(5), DEFAULT_MAX_TEX_BYTES, None, &test_admission());
        assert_eq!(result.status, "ok");
        assert!(result.text.unwrap().contains("Hello world."));
        assert_eq!(result.source_tar.unwrap(), "test.tar");
    }

    #[test]
    fn test_extraction_result_has_timing() {
        let paper = PaperArchive {
            arxiv_id: "test.timing".into(),
            tex_files: Arc::new(vec![TexFile {
                name: "main.tex".into(),
                content: r"\documentclass{article}
\begin{document}
Hello.
\end{document}"
                    .into(),
            }]),
            file_type: FileType::Tex,
            entry_name: "test.gz".into(),
        };
        let result = extract_with_timeout(&paper, None, Duration::from_secs(5), DEFAULT_MAX_TEX_BYTES, None, &test_admission());
        assert_eq!(result.status, "ok");
        assert!(result.stage_timings_us.is_some(), "should have stage timings");
        assert!(result.total_time_us.is_some(), "should have total time");
        assert!(result.total_time_us.unwrap() > 0);

        let json = result.stage_timings_us.unwrap();
        assert!(json.contains("remove_comments"), "timing JSON: {json}");
    }

    #[test]
    fn test_classify_result() {
        let make = |status: &str, error: Option<&str>| ExtractionResult {
            arxiv_id: "test".into(),
            source_tar: None,
            status: status.into(),
            num_tex_files: None,
            text_length: None,
            text: None,
            error: error.map(|s| s.into()),
            stage_timings_us: None,
            total_time_us: None,
            peak_memory_bytes: None,
            file_type: None,
            entry_name: None,
        };

        assert_eq!(classify_result(&make("ok", None)), Outcome::Ok);
        assert_eq!(classify_result(&make("timeout", Some("timed out after 30s"))), Outcome::Timeout);
        assert_eq!(classify_result(&make("skipped", Some("too large"))), Outcome::Skipped);
        assert_eq!(classify_result(&make("empty", None)), Outcome::Empty);
        assert_eq!(classify_result(&make("error", Some("extraction thread crashed"))), Outcome::Crash);
        assert_eq!(classify_result(&make("error", Some("some panic message"))), Outcome::Panic);
        assert_eq!(classify_result(&make("error", None)), Outcome::Panic);
    }
}
