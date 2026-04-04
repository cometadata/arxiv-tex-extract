use serde::Serialize;

/// Result of extracting text from a single paper.
///
/// Used for both JSONL serialization and parquet output.
#[derive(Debug, Clone, Serialize)]
pub struct ExtractionResult {
    pub arxiv_id: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_tar: Option<String>,

    pub status: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_tex_files: Option<usize>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_length: Option<usize>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// JSON map of pipeline stage name → microseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage_timings_us: Option<String>,

    /// Total pipeline time in microseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_time_us: Option<u64>,

    /// Best-effort peak memory estimate in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_memory_bytes: Option<u64>,
}

impl ExtractionResult {
    /// Convenience constructor for error/skip results.
    pub fn error(arxiv_id: String, source_tar: Option<String>, status: &str, error: String) -> Self {
        Self {
            arxiv_id,
            source_tar,
            status: status.into(),
            num_tex_files: None,
            text_length: None,
            text: None,
            error: Some(error),
            stage_timings_us: None,
            total_time_us: None,
            peak_memory_bytes: None,
        }
    }
}
