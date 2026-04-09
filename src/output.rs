use std::fs::{self, File};
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

/// Writes extraction results to parquet shards with automatic splitting.
pub struct ParquetShardWriter {
    schema: Arc<Schema>,
    output_dir: PathBuf,
    base_name: String,
    shard_index: usize,
    max_rows: usize,
    max_bytes: usize,
    buffer: Vec<ExtractionResult>,
    current_bytes: usize,
    completed_shards: Vec<PathBuf>,
}

impl ParquetShardWriter {
    pub fn new(
        output_dir: &Path,
        base_name: &str,
        max_rows: usize,
        max_bytes: usize,
    ) -> Self {
        Self {
            schema: Arc::new(result_schema()),
            output_dir: output_dir.to_path_buf(),
            base_name: base_name.to_string(),
            shard_index: 0,
            max_rows,
            max_bytes,
            buffer: Vec::with_capacity(1024),
            current_bytes: 0,
            completed_shards: Vec::new(),
        }
    }

    /// Add a result to the buffer. Flushes to disk if limits are exceeded.
    pub fn write(&mut self, result: ExtractionResult) -> Result<()> {
        let byte_estimate = result.arxiv_id.len()
            + result.text.as_ref().map_or(0, |t| t.len())
            + result.error.as_ref().map_or(0, |e| e.len())
            + result.stage_timings_us.as_ref().map_or(0, |s| s.len())
            + 200;

        self.buffer.push(result);
        self.current_bytes += byte_estimate;

        if self.buffer.len() >= self.max_rows || self.current_bytes >= self.max_bytes {
            self.flush_shard()?;
        }

        Ok(())
    }

    /// Flush current buffer to a parquet shard file.
    fn flush_shard(&mut self) -> Result<()> {
        if self.buffer.is_empty() {
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

        let props = WriterProperties::builder()
            .set_compression(Compression::ZSTD(ZstdLevel::try_new(3)?))
            .build();

        let mut writer = ArrowWriter::try_new(file, self.schema.clone(), Some(props))?;
        let batch = self.build_record_batch(&shard_id)?;
        writer.write(&batch)?;
        writer.close()?;

        fs::rename(&temp_path, &final_path).with_context(|| {
            format!(
                "renaming {} to {}",
                temp_path.display(),
                final_path.display()
            )
        })?;

        self.completed_shards.push(final_path);
        self.buffer.clear();
        self.current_bytes = 0;
        self.shard_index += 1;

        Ok(())
    }

    /// Convert buffered results to an Arrow RecordBatch.
    fn build_record_batch(&self, shard_id: &str) -> Result<RecordBatch> {
        let len = self.buffer.len();

        let mut arxiv_ids = StringBuilder::with_capacity(len, len * 20);
        let mut statuses = StringBuilder::with_capacity(len, len * 8);
        let mut error_messages = StringBuilder::with_capacity(len, len * 32);
        let mut num_tex_files = UInt32Builder::with_capacity(len);
        let mut text_lengths = UInt32Builder::with_capacity(len);
        let mut texts = LargeStringBuilder::with_capacity(len, len * 30_000);
        let mut stage_timings = StringBuilder::with_capacity(len, len * 200);
        let mut total_times = UInt64Builder::with_capacity(len);
        let mut peak_memories = UInt64Builder::with_capacity(len);
        let mut outer_tars = StringBuilder::with_capacity(len, len * 20);
        let mut file_types = StringBuilder::with_capacity(len, len * 10);
        let mut entry_names = StringBuilder::with_capacity(len, len * 30);
        let mut shard_ids = StringBuilder::with_capacity(len, len * 20);

        for r in &self.buffer {
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

        RecordBatch::try_new(self.schema.clone(), columns).context("building record batch")
    }

    /// Flush remaining buffer and return all completed shard paths.
    pub fn finish(mut self) -> Result<Vec<PathBuf>> {
        self.flush_shard()?;
        Ok(self.completed_shards)
    }

    /// Number of rows written so far (across all shards + current buffer).
    pub fn rows_written(&self) -> usize {
        self.completed_shards.len() * self.max_rows + self.buffer.len()
    }
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
        let mut writer = ParquetShardWriter::new(dir.path(), "test", 10_000, 256_000_000);

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
        let mut writer = ParquetShardWriter::new(dir.path(), "split", 2, 256_000_000);

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
        let mut writer = ParquetShardWriter::new(dir.path(), "atomic", 10_000, 256_000_000);
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
        let writer = ParquetShardWriter::new(dir.path(), "empty", 10_000, 256_000_000);
        let shards = writer.finish().unwrap();
        assert!(shards.is_empty());
    }

    #[test]
    fn test_schema_field_count() {
        let schema = result_schema();
        assert_eq!(schema.fields().len(), 13);
    }
}
