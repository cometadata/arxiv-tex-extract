use serde::Serialize;

/// Source file type detected from archive content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileType {
    Tex,
    Pdf,
    Postscript,
    Html,
    Unknown,
}

impl FileType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tex => "tex",
            Self::Pdf => "pdf",
            Self::Postscript => "postscript",
            Self::Html => "html",
            Self::Unknown => "unknown",
        }
    }
}

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

    /// Source file type detected from archive content (tex, pdf, postscript, html, unknown).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_type: Option<FileType>,

    /// Original tar entry path or file path for this paper.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_name: Option<String>,
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
            file_type: None,
            entry_name: None,
        }
    }
}
