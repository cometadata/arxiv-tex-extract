use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use arrow::array::{ArrayRef, LargeStringBuilder, StringBuilder, UInt32Builder, UInt64Builder};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;

use crate::result::ExtractionResult;

/// Build the Arrow schema for extraction results.
pub fn result_schema() -> Schema {
    Schema::new(vec![
        Field::new("arxiv_id", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("error_message", DataType::Utf8, true),
        Field::new("num_tex_files", DataType::UInt32, true),
        Field::new("text_length", DataType::UInt32, true),
        Field::new("text", DataType::LargeUtf8, true),
        Field::new("stage_timings_us", DataType::Utf8, true),
        Field::new("total_time_us", DataType::UInt64, true),
        Field::new("peak_memory_bytes", DataType::UInt64, true),
        Field::new("outer_tar", DataType::Utf8, true),
        Field::new("file_type", DataType::Utf8, true),
        Field::new("entry_name", DataType::Utf8, true),
        Field::new("shard_id", DataType::Utf8, false),
    ])
}

/// Rows per Arrow RecordBatch flushed to the Parquet writer within a shard.
/// Keeping this small bounds transient memory from Arrow builder pre-allocation
/// (`len * 4_000` bytes for the text column alone) so peak RSS during shard
/// write is ~1 MB per flush instead of ~300 MB on a 10 k-row shard.
const MICRO_BATCH_ROWS: usize = 256;

/// Writes extraction results to parquet shards with automatic splitting.
///
/// Rows are buffered only up to `MICRO_BATCH_ROWS` in memory; each flush
/// converts that micro-batch to an Arrow `RecordBatch` and hands it to the
/// live `ArrowWriter` for the current shard. A new shard is started once
/// `max_rows` or `max_bytes` is reached.
pub struct ParquetShardWriter {
    schema: Arc<Schema>,
    output_dir: PathBuf,
    base_name: String,
    shard_index: usize,
    max_rows: usize,
    max_bytes: usize,
    /// Rotate after this many papers, independent of max_rows / max_bytes.
    /// Set to `usize::MAX` to disable paper-count rotation.
    papers_per_shard: usize,
    /// Rows awaiting the next micro-batch flush.
    micro_batch: Vec<ExtractionResult>,
    /// Per-shard state; present while a shard is being written to.
    shard: Option<ShardWriter>,
    completed_shards: Vec<PathBuf>,
}

struct ShardWriter {
    final_path: PathBuf,
    temp_path: PathBuf,
    writer: ArrowWriter<File>,
    shard_id: String,
    rows_written: usize,
    bytes_estimate: usize,
}

impl ParquetShardWriter {
    pub fn new(
        output_dir: &Path,
        base_name: &str,
        max_rows: usize,
        max_bytes: usize,
        papers_per_shard: usize,
    ) -> Self {
        Self {
            schema: Arc::new(result_schema()),
            output_dir: output_dir.to_path_buf(),
            base_name: base_name.to_string(),
            shard_index: 0,
            max_rows,
            max_bytes,
            papers_per_shard,
            micro_batch: Vec::with_capacity(MICRO_BATCH_ROWS),
            shard: None,
            completed_shards: Vec::new(),
        }
    }

    /// Add a result. Triggers a micro-batch flush every `MICRO_BATCH_ROWS`
    /// and rotates to a new shard once `max_rows` / `max_bytes` is reached.
    pub fn write(&mut self, result: ExtractionResult) -> Result<()> {
        let byte_estimate = result.arxiv_id.len()
            + result.text.as_ref().map_or(0, |t| t.len())
            + result.error.as_ref().map_or(0, |e| e.len())
            + result.stage_timings_us.as_ref().map_or(0, |s| s.len())
            + 200;

        self.ensure_shard_open()?;
        self.micro_batch.push(result);
        {
            let sw = self.shard.as_mut().expect("shard open");
            sw.rows_written += 1;
            sw.bytes_estimate += byte_estimate;
        }

        if self.micro_batch.len() >= MICRO_BATCH_ROWS {
            self.flush_micro_batch()?;
        }

        let (rows, bytes) = {
            let sw = self.shard.as_ref().expect("shard open");
            (sw.rows_written, sw.bytes_estimate)
        };
        if rows >= self.papers_per_shard || rows >= self.max_rows || bytes >= self.max_bytes {
            self.close_shard()?;
        }

        Ok(())
    }

    fn ensure_shard_open(&mut self) -> Result<()> {
        if self.shard.is_some() {
            return Ok(());
        }
        let shard_name = if self.shard_index == 0 {
            format!("{}.parquet", self.base_name)
        } else {
            format!("{}_{:03}.parquet", self.base_name, self.shard_index)
        };
        let final_path = self.output_dir.join(&shard_name);
        let temp_path = self.output_dir.join(format!(".{}.tmp", shard_name));
        let shard_id = if self.shard_index == 0 {
            self.base_name.clone()
        } else {
            format!("{}_{:03}", self.base_name, self.shard_index)
        };

        let file = File::create(&temp_path)
            .with_context(|| format!("creating temp file {}", temp_path.display()))?;
        // Force row-group boundary to align with our micro-batch flushes.
        // ArrowWriter's default max_row_group_size is 1,048,576 rows, which
        // means every row we hand it sits in column builders until the shard
        // is closed — at scale, 4 concurrent writers × 500 MB of buffered
        // text/string columns each was dominating RSS on the 22 GB corpus.
        // Aligning the row-group size with MICRO_BATCH_ROWS turns each
        // write() into a completed row group that lands on disk.
        let props = WriterProperties::builder()
            .set_compression(Compression::ZSTD(ZstdLevel::try_new(3)?))
            .set_max_row_group_size(MICRO_BATCH_ROWS)
            .build();
        let writer = ArrowWriter::try_new(file, self.schema.clone(), Some(props))?;

        self.shard = Some(ShardWriter {
            final_path,
            temp_path,
            writer,
            shard_id,
            rows_written: 0,
            bytes_estimate: 0,
        });
        Ok(())
    }

    fn flush_micro_batch(&mut self) -> Result<()> {
        if self.micro_batch.is_empty() {
            return Ok(());
        }
        let sw = self.shard.as_mut().expect("shard open");
        let batch = build_record_batch(&self.schema, &self.micro_batch, &sw.shard_id)?;
        sw.writer.write(&batch)?;
        self.micro_batch.clear();
        Ok(())
    }

    fn close_shard(&mut self) -> Result<()> {
        self.flush_micro_batch()?;
        let sw = self.shard.take().expect("shard open");
        sw.writer.close()?;
        fs::rename(&sw.temp_path, &sw.final_path).with_context(|| {
            format!(
                "renaming {} to {}",
                sw.temp_path.display(),
                sw.final_path.display()
            )
        })?;
        self.completed_shards.push(sw.final_path);
        self.shard_index += 1;
        Ok(())
    }

    /// Flush the trailing micro-batch and close the final shard.
    pub fn finish(mut self) -> Result<Vec<PathBuf>> {
        if self.shard.is_some() {
            self.close_shard()?;
        }
        Ok(self.completed_shards)
    }

    /// Number of rows written across all shards so far. Useful for telemetry.
    pub fn rows_written(&self) -> usize {
        let current = self.shard.as_ref().map_or(0, |sw| sw.rows_written);
        self.completed_shards.len() * self.max_rows + current
    }

    /// Rows pending in the current micro-batch (not yet handed to Arrow).
    /// Exposed for test-assertion of streaming behaviour.
    pub fn micro_buffer_len(&self) -> usize {
        self.micro_batch.len()
    }
}

/// Build an Arrow `RecordBatch` from a slice of rows.
///
/// Capacity hints are sized for a micro-batch (≤ 256 rows): the text column
/// hint is `len * 4_000` bytes, ~1 MB upfront on a full micro-batch instead
/// of the previous `len * 30_000` which materialised ~300 MB on a 10 k-row
/// full-shard flush.
fn build_record_batch(
    schema: &Arc<Schema>,
    rows: &[ExtractionResult],
    shard_id: &str,
) -> Result<RecordBatch> {
    let len = rows.len();

    let mut arxiv_ids = StringBuilder::with_capacity(len, len * 20);
    let mut statuses = StringBuilder::with_capacity(len, len * 8);
    let mut error_messages = StringBuilder::with_capacity(len, len * 32);
    let mut num_tex_files = UInt32Builder::with_capacity(len);
    let mut text_lengths = UInt32Builder::with_capacity(len);
    let mut texts = LargeStringBuilder::with_capacity(len, len * 4_000);
    let mut stage_timings = StringBuilder::with_capacity(len, len * 200);
    let mut total_times = UInt64Builder::with_capacity(len);
    let mut peak_memories = UInt64Builder::with_capacity(len);
    let mut outer_tars = StringBuilder::with_capacity(len, len * 20);
    let mut file_types = StringBuilder::with_capacity(len, len * 10);
    let mut entry_names = StringBuilder::with_capacity(len, len * 30);
    let mut shard_ids = StringBuilder::with_capacity(len, len * 20);

    for r in rows {
        arxiv_ids.append_value(&r.arxiv_id);
        statuses.append_value(&r.status);
        append_option_str(&mut error_messages, r.error.as_deref());
        append_option_u32(&mut num_tex_files, r.num_tex_files.map(|n| n as u32));
        append_option_u32(&mut text_lengths, r.text_length.map(|n| n as u32));
        append_option_large_str(&mut texts, r.text.as_deref());
        append_option_str(&mut stage_timings, r.stage_timings_us.as_deref());
        append_option_u64(&mut total_times, r.total_time_us);
        append_option_u64(&mut peak_memories, r.peak_memory_bytes);
        append_option_str(&mut outer_tars, r.source_tar.as_deref());
        append_option_str(&mut file_types, r.file_type.map(|ft| ft.as_str()));
        append_option_str(&mut entry_names, r.entry_name.as_deref());
        shard_ids.append_value(shard_id);
    }

    let columns: Vec<ArrayRef> = vec![
        Arc::new(arxiv_ids.finish()),
        Arc::new(statuses.finish()),
        Arc::new(error_messages.finish()),
        Arc::new(num_tex_files.finish()),
        Arc::new(text_lengths.finish()),
        Arc::new(texts.finish()),
        Arc::new(stage_timings.finish()),
        Arc::new(total_times.finish()),
        Arc::new(peak_memories.finish()),
        Arc::new(outer_tars.finish()),
        Arc::new(file_types.finish()),
        Arc::new(entry_names.finish()),
        Arc::new(shard_ids.finish()),
    ];

    RecordBatch::try_new(schema.clone(), columns).context("building record batch")
}

fn append_option_str(builder: &mut StringBuilder, val: Option<&str>) {
    match val {
        Some(s) => builder.append_value(s),
        None => builder.append_null(),
    }
}

fn append_option_large_str(builder: &mut LargeStringBuilder, val: Option<&str>) {
    match val {
        Some(s) => builder.append_value(s),
        None => builder.append_null(),
    }
}

fn append_option_u32(builder: &mut UInt32Builder, val: Option<u32>) {
    match val {
        Some(n) => builder.append_value(n),
        None => builder.append_null(),
    }
}

fn append_option_u64(builder: &mut UInt64Builder, val: Option<u64>) {
    match val {
        Some(n) => builder.append_value(n),
        None => builder.append_null(),
    }
}

/// Writes extraction results to JSONL shards with automatic splitting.
///
/// Mirrors `ParquetShardWriter`'s shape for the JSONL output format. Rotates
/// to a new shard file once `papers_per_shard` is reached. Each shard is
/// written to a temp file (`.{name}.tmp`) and atomically renamed on close,
/// so the shard footer is either fully present or the file doesn't exist.
pub struct JsonlShardWriter {
    output_dir: PathBuf,
    base_name: String,
    shard_index: usize,
    /// Rotate after this many papers. Use `usize::MAX` to disable rotation.
    papers_per_shard: usize,
    shard: Option<JsonlShardState>,
    completed_shards: Vec<PathBuf>,
}

struct JsonlShardState {
    final_path: PathBuf,
    temp_path: PathBuf,
    writer: BufWriter<File>,
    rows_written: usize,
}

impl JsonlShardWriter {
    pub fn new(output_dir: &Path, base_name: &str, papers_per_shard: usize) -> Self {
        Self {
            output_dir: output_dir.to_path_buf(),
            base_name: base_name.to_string(),
            shard_index: 0,
            papers_per_shard,
            shard: None,
            completed_shards: Vec::new(),
        }
    }

    /// Serialize and append a single result, rotating the shard if needed.
    pub fn write(&mut self, result: &ExtractionResult) -> Result<()> {
        self.ensure_shard_open()?;
        {
            let sw = self.shard.as_mut().expect("shard open");
            serde_json::to_writer(&mut sw.writer, result)
                .context("serializing ExtractionResult to JSONL")?;
            sw.writer.write_all(b"\n")?;
            sw.rows_written += 1;
        }

        let rows = self.shard.as_ref().map_or(0, |s| s.rows_written);
        if rows >= self.papers_per_shard {
            self.close_shard()?;
        }
        Ok(())
    }

    fn ensure_shard_open(&mut self) -> Result<()> {
        if self.shard.is_some() {
            return Ok(());
        }
        let shard_name = if self.shard_index == 0 {
            format!("{}.jsonl", self.base_name)
        } else {
            format!("{}_{:03}.jsonl", self.base_name, self.shard_index)
        };
        let final_path = self.output_dir.join(&shard_name);
        let temp_path = self.output_dir.join(format!(".{}.tmp", shard_name));

        let file = File::create(&temp_path)
            .with_context(|| format!("creating temp file {}", temp_path.display()))?;
        let writer = BufWriter::new(file);

        self.shard = Some(JsonlShardState {
            final_path,
            temp_path,
            writer,
            rows_written: 0,
        });
        Ok(())
    }

    fn close_shard(&mut self) -> Result<()> {
        let mut sw = self.shard.take().expect("shard open");
        sw.writer.flush()?;
        drop(sw.writer);
        fs::rename(&sw.temp_path, &sw.final_path).with_context(|| {
            format!(
                "renaming {} to {}",
                sw.temp_path.display(),
                sw.final_path.display()
            )
        })?;
        self.completed_shards.push(sw.final_path);
        self.shard_index += 1;
        Ok(())
    }

    /// Flush and close the trailing shard.
    pub fn finish(mut self) -> Result<Vec<PathBuf>> {
        if self.shard.is_some() {
            self.close_shard()?;
        }
        Ok(self.completed_shards)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::FileType;
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

    fn make_result(id: &str, text: &str) -> ExtractionResult {
        ExtractionResult {
            arxiv_id: id.into(),
            source_tar: Some("test.tar".into()),
            status: "ok".into(),
            num_tex_files: Some(1),
            text_length: Some(text.len()),
            text: Some(text.into()),
            error: None,
            stage_timings_us: Some(r#"{"cleanup":42}"#.into()),
            total_time_us: Some(42),
            peak_memory_bytes: None,
            file_type: Some(FileType::Tex),
            entry_name: Some("2603/test.gz".into()),
        }
    }

    fn make_error_result(id: &str) -> ExtractionResult {
        ExtractionResult {
            arxiv_id: id.into(),
            source_tar: None,
            status: "error".into(),
            num_tex_files: None,
            text_length: None,
            text: None,
            error: Some("boom".into()),
            stage_timings_us: None,
            total_time_us: None,
            peak_memory_bytes: None,
            file_type: None,
            entry_name: None,
        }
    }

    #[test]
    fn test_write_and_read_parquet() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer =
            ParquetShardWriter::new(dir.path(), "test", 10_000, 256_000_000, usize::MAX);

        writer.write(make_result("2401.00001", "Hello world")).unwrap();
        writer.write(make_error_result("2401.00002")).unwrap();

        let shards = writer.finish().unwrap();
        assert_eq!(shards.len(), 1);
        assert!(shards[0].ends_with("test.parquet"));

        // Read back and verify
        let file = File::open(&shards[0]).unwrap();
        let reader = ParquetRecordBatchReaderBuilder::try_new(file)
            .unwrap()
            .build()
            .unwrap();

        let batches: Vec<_> = reader.collect::<Result<_, _>>().unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].num_rows(), 2);
        assert_eq!(batches[0].num_columns(), 13);
    }

    #[test]
    fn test_shard_splitting() {
        let dir = tempfile::tempdir().unwrap();
        // max_rows = 2 to force splitting
        let mut writer =
            ParquetShardWriter::new(dir.path(), "split", 2, 256_000_000, usize::MAX);

        writer.write(make_result("a", "text1")).unwrap();
        writer.write(make_result("b", "text2")).unwrap();
        // This should have triggered a flush (2 rows = max_rows)
        writer.write(make_result("c", "text3")).unwrap();

        let shards = writer.finish().unwrap();
        assert_eq!(shards.len(), 2, "expected 2 shards: {:?}", shards);
        assert!(shards[0].ends_with("split.parquet"));
        assert!(shards[1].ends_with("split_001.parquet"));
    }

    #[test]
    fn test_atomic_write_no_tmp_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer =
            ParquetShardWriter::new(dir.path(), "atomic", 10_000, 256_000_000, usize::MAX);
        writer.write(make_result("x", "data")).unwrap();
        let _shards = writer.finish().unwrap();

        for entry in fs::read_dir(dir.path()).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().to_string();
            assert!(!name.ends_with(".tmp"), "temp file left behind: {name}");
        }
    }

    #[test]
    fn test_empty_writer() {
        let dir = tempfile::tempdir().unwrap();
        let writer =
            ParquetShardWriter::new(dir.path(), "empty", 10_000, 256_000_000, usize::MAX);
        let shards = writer.finish().unwrap();
        assert!(shards.is_empty());
    }

    #[test]
    fn test_schema_field_count() {
        let schema = result_schema();
        assert_eq!(schema.fields().len(), 13);
    }

    #[test]
    fn writer_flushes_incrementally_within_shard() {
        // With a 256-row micro-batch and a 5000-row shard, writing 300 rows
        // must flush at least one micro-batch — so the in-memory buffer
        // is < MICRO_BATCH_ROWS.
        let dir = tempfile::tempdir().unwrap();
        let mut writer =
            ParquetShardWriter::new(dir.path(), "mb", 5000, usize::MAX, usize::MAX);
        for i in 0..300 {
            writer.write(make_result(&format!("id{i:03}"), "text")).unwrap();
        }
        assert!(
            writer.micro_buffer_len() < 256,
            "expected micro-buffer < 256, was {}",
            writer.micro_buffer_len()
        );
        let shards = writer.finish().unwrap();
        // Single shard produced — we didn't cross 5000 rows.
        assert_eq!(shards.len(), 1);

        // Read back and confirm all 300 rows landed.
        let file = File::open(&shards[0]).unwrap();
        let reader = ParquetRecordBatchReaderBuilder::try_new(file)
            .unwrap()
            .build()
            .unwrap();
        let batches: Vec<_> = reader.collect::<Result<_, _>>().unwrap();
        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total_rows, 300);
    }

    #[test]
    fn test_papers_per_shard_rotates_independent_of_max_rows() {
        // max_rows is generous; papers_per_shard=3 should drive rotation.
        let dir = tempfile::tempdir().unwrap();
        let mut writer = ParquetShardWriter::new(dir.path(), "pps", 1_000_000, usize::MAX, 3);

        for i in 0..7 {
            writer
                .write(make_result(&format!("id{i:03}"), "text"))
                .unwrap();
        }

        let shards = writer.finish().unwrap();
        assert_eq!(shards.len(), 3, "expected 3 shards: {:?}", shards);
        assert!(shards[0].ends_with("pps.parquet"));
        assert!(shards[1].ends_with("pps_001.parquet"));
        assert!(shards[2].ends_with("pps_002.parquet"));
    }

    #[test]
    fn test_jsonl_shard_writer_rotates() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = JsonlShardWriter::new(dir.path(), "jshard", 2);

        writer.write(&make_result("a", "one")).unwrap();
        writer.write(&make_result("b", "two")).unwrap();
        // 2 rows → rotate; third write goes into the next shard.
        writer.write(&make_result("c", "three")).unwrap();

        let shards = writer.finish().unwrap();
        assert_eq!(shards.len(), 2, "expected 2 shards: {:?}", shards);
        assert!(shards[0].ends_with("jshard.jsonl"));
        assert!(shards[1].ends_with("jshard_001.jsonl"));

        // No .tmp leftovers.
        for entry in fs::read_dir(dir.path()).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().to_string();
            assert!(!name.ends_with(".tmp"), "temp file left: {name}");
        }

        // First shard has 2 lines, second has 1.
        let first = std::fs::read_to_string(&shards[0]).unwrap();
        let second = std::fs::read_to_string(&shards[1]).unwrap();
        assert_eq!(first.lines().count(), 2);
        assert_eq!(second.lines().count(), 1);
    }

    #[test]
    fn test_jsonl_shard_writer_empty() {
        let dir = tempfile::tempdir().unwrap();
        let writer = JsonlShardWriter::new(dir.path(), "empty", 100);
        let shards = writer.finish().unwrap();
        assert!(shards.is_empty());
    }

    #[test]
    fn test_jsonl_shard_writer_roundtrip_content() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = JsonlShardWriter::new(dir.path(), "rt", usize::MAX);
        writer.write(&make_result("2401.00001", "hello")).unwrap();
        writer.write(&make_error_result("2401.00002")).unwrap();

        let shards = writer.finish().unwrap();
        assert_eq!(shards.len(), 1);

        let content = std::fs::read_to_string(&shards[0]).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"arxiv_id\":\"2401.00001\""));
        assert!(lines[1].contains("\"arxiv_id\":\"2401.00002\""));
    }
}
