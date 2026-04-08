use crate::input_resolve::TexFile;
use anyhow::{Context, Result};
use std::io::Read;

/// A paper extracted from an arXiv archive.
pub struct PaperArchive {
    pub arxiv_id: String,
    pub tex_files: Vec<TexFile>,
}

/// Maximum decompressed size per archive (100MB).
const MAX_DECOMPRESSED_SIZE: u64 = 100_000_000;

/// Process papers from an outer tar file one at a time via callback.
///
/// Each entry in the outer tar is either:
/// - A `.tar.gz` containing multiple files (multi-file submission)
/// - A `.gz` containing a single file
/// - A `.tex` file directly
///
/// Unlike `iter_papers()`, this keeps only one `PaperArchive` in memory
/// at a time. Each paper is fully processed (via the callback `f`) before
/// the next entry is read from the tar archive.
pub fn for_each_paper(reader: impl Read, mut f: impl FnMut(Result<PaperArchive>)) {
    let mut archive = tar::Archive::new(reader);
    let entries = match archive.entries() {
        Ok(e) => e,
        Err(e) => {
            f(Err(anyhow::anyhow!("failed to read tar entries: {}", e)));
            return;
        }
    };

    for entry_result in entries {
        let entry = match entry_result {
            Ok(e) => e,
            Err(e) => {
                f(Err(anyhow::anyhow!("tar entry error: {}", e)));
                continue;
            }
        };

        let path = match entry.path() {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(_) => continue,
        };

        let arxiv_id = derive_arxiv_id(&path);
        f(process_entry(entry, &arxiv_id, &path));
    }
}

/// Extract all papers from an outer tar file into a Vec.
///
/// Convenience wrapper around `for_each_paper` for cases where collecting
/// all results is acceptable (testing, small archives).
pub fn iter_papers(reader: impl Read) -> Vec<Result<PaperArchive>> {
    let mut results = Vec::new();
    for_each_paper(reader, |result| results.push(result));
    results
}

/// Process a single entry from the outer tar.
fn process_entry<R: Read>(mut entry: tar::Entry<R>, arxiv_id: &str, path: &str) -> Result<PaperArchive> {
    // Guard: skip entries whose raw size already exceeds the decompression
    // limit. Prevents reading huge compressed blobs into memory.
    let raw_size = entry.header().size().unwrap_or(0);
    if raw_size > MAX_DECOMPRESSED_SIZE {
        anyhow::bail!(
            "entry {} raw size ({} bytes) exceeds {}MB limit",
            path,
            raw_size,
            MAX_DECOMPRESSED_SIZE / 1_000_000
        );
    }

    let mut raw_bytes = Vec::new();
    entry
        .read_to_end(&mut raw_bytes)
        .with_context(|| format!("reading entry {}", path))?;

    let tex_files = if path.ends_with(".tar.gz") || path.ends_with(".tgz") || path.ends_with(".gz") {
        // Try as gzipped tar first. If that yields no .tex files, also try
        // as a single gzipped .tex — some arXiv .tar.gz entries are just
        // gzipped tex files that the tar crate silently returns zero entries for.
        // Old arXiv .gz entries are often gzipped tar archives (multi-file
        // submissions) despite the plain .gz extension.
        match extract_inner_tar_gz(&raw_bytes, arxiv_id) {
            Ok(files) if !files.is_empty() => files,
            _ => extract_single_gz(&raw_bytes, arxiv_id).unwrap_or_default(),
        }
    } else if path.ends_with(".tex") {
        extract_single_tex(&raw_bytes, path)?
    } else {
        Vec::new()
    };

    Ok(PaperArchive {
        arxiv_id: arxiv_id.to_string(),
        tex_files,
    })
}

/// Extract .tex files from an inner .tar.gz archive.
fn extract_inner_tar_gz(raw: &[u8], arxiv_id: &str) -> Result<Vec<TexFile>> {
    let gz = flate2::read::GzDecoder::new(raw);
    let limited = gz.take(MAX_DECOMPRESSED_SIZE);
    let mut inner_archive = tar::Archive::new(limited);

    let mut tex_files = Vec::new();

    for entry_result in inner_archive
        .entries()
        .with_context(|| format!("reading inner tar for {}", arxiv_id))?
    {
        let mut entry = match entry_result {
            Ok(e) => e,
            Err(_) => continue,
        };

        let entry_path = match entry.path() {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(_) => continue,
        };

        let lower = entry_path.to_ascii_lowercase();
        if !lower.ends_with(".tex")
            && !lower.ends_with(".bbl")
            && !lower.ends_with(".ltx")
            && !lower.ends_with(".latex")
        {
            continue;
        }

        let mut content_bytes = Vec::new();
        if entry.read_to_end(&mut content_bytes).is_err() {
            continue;
        }

        if let Some(content) = decode_bytes(&content_bytes) {
            tex_files.push(TexFile {
                name: entry_path,
                content,
            });
        }
    }

    Ok(tex_files)
}

/// Extract a single .tex file from a .gz archive.
fn extract_single_gz(raw: &[u8], arxiv_id: &str) -> Result<Vec<TexFile>> {
    let gz = flate2::read::GzDecoder::new(raw);
    let mut limited = gz.take(MAX_DECOMPRESSED_SIZE);

    let mut content_bytes = Vec::new();
    limited
        .read_to_end(&mut content_bytes)
        .with_context(|| format!("decompressing gz for {}", arxiv_id))?;

    if let Some(content) = decode_bytes(&content_bytes) {
        Ok(vec![TexFile {
            name: format!("{}.tex", arxiv_id),
            content,
        }])
    } else {
        Ok(Vec::new())
    }
}

/// Extract a single .tex file from raw bytes.
fn extract_single_tex(raw: &[u8], path: &str) -> Result<Vec<TexFile>> {
    if let Some(content) = decode_bytes(raw) {
        Ok(vec![TexFile {
            name: path.to_string(),
            content,
        }])
    } else {
        Ok(Vec::new())
    }
}

/// Decode bytes to string: try UTF-8, then fall back to Latin-1.
fn decode_bytes(bytes: &[u8]) -> Option<String> {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return Some(s.strip_prefix('\u{FEFF}').unwrap_or(s).to_string());
    }

    let (decoded, _, had_errors) = encoding_rs::WINDOWS_1252.decode(bytes);
    if !had_errors {
        Some(decoded.to_string())
    } else {
        Some(String::from_utf8_lossy(bytes).to_string())
    }
}

/// Derive an arXiv ID from a tar entry path.
/// "2401.00001.tar.gz" → "2401.00001"
/// "hep-ph/0001001.gz" → "hep-ph/0001001"
fn derive_arxiv_id(path: &str) -> String {
    let name = path
        .rsplit('/')
        .next()
        .unwrap_or(path);
    name.trim_end_matches(".tar.gz")
        .trim_end_matches(".tgz")
        .trim_end_matches(".gz")
        .trim_end_matches(".tex")
        .to_string()
}

/// Process a single per-paper archive file (for testing / single-file mode).
pub fn load_paper_archive(file_path: &std::path::Path) -> Result<PaperArchive> {
    let arxiv_id = derive_arxiv_id(
        file_path
            .file_name()
            .unwrap_or_default()
            .to_str()
            .unwrap_or("unknown"),
    );

    let raw = std::fs::read(file_path)
        .with_context(|| format!("reading {}", file_path.display()))?;

    let path_str = file_path.to_string_lossy().to_string();

    let tex_files = if path_str.ends_with(".tar.gz") || path_str.ends_with(".tgz") || path_str.ends_with(".gz") {
        match extract_from_tar(&raw, &arxiv_id) {
            Ok(files) if !files.is_empty() => files,
            _ => {
                match extract_inner_tar_gz(&raw, &arxiv_id) {
                    Ok(files) if !files.is_empty() => files,
                    _ => extract_single_gz(&raw, &arxiv_id)?,
                }
            }
        }
    } else if path_str.ends_with(".tex") {
        extract_single_tex(&raw, &path_str)?
    } else {
        Vec::new()
    };

    Ok(PaperArchive { arxiv_id, tex_files })
}

/// Try to extract .tex files from raw bytes treated as a tar archive.
fn extract_from_tar(raw: &[u8], _arxiv_id: &str) -> Result<Vec<TexFile>> {
    let mut archive = tar::Archive::new(raw);
    let mut tex_files = Vec::new();

    for entry_result in archive.entries()? {
        let mut entry = entry_result?;
        let path = entry.path()?.to_string_lossy().to_string();

        let lower = path.to_ascii_lowercase();
        if !lower.ends_with(".tex")
            && !lower.ends_with(".bbl")
            && !lower.ends_with(".ltx")
            && !lower.ends_with(".latex")
        {
            continue;
        }

        let mut content_bytes = Vec::new();
        entry.read_to_end(&mut content_bytes)?;

        if let Some(content) = decode_bytes(&content_bytes) {
            tex_files.push(TexFile { name: path, content });
        }
    }

    Ok(tex_files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_arxiv_id() {
        assert_eq!(derive_arxiv_id("2401.00001.tar.gz"), "2401.00001");
        assert_eq!(derive_arxiv_id("2401.00001.gz"), "2401.00001");
        assert_eq!(derive_arxiv_id("path/to/2401.00001.tar.gz"), "2401.00001");
    }

    #[test]
    fn test_decode_utf8() {
        let bytes = "hello world".as_bytes();
        assert_eq!(decode_bytes(bytes).unwrap(), "hello world");
    }

    #[test]
    fn test_decode_latin1() {
        // Latin-1 encoded "café" (é = 0xe9 in Latin-1)
        let bytes = vec![0x63, 0x61, 0x66, 0xe9];
        let result = decode_bytes(&bytes).unwrap();
        assert!(result.contains("caf"));
    }

    #[test]
    fn test_decode_utf8_bom() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF]; // BOM
        bytes.extend_from_slice("hello".as_bytes());
        assert_eq!(decode_bytes(&bytes).unwrap(), "hello");
    }
}
