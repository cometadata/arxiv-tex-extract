use crate::input_resolve::TexFile;
use crate::result::FileType;
use anyhow::{Context, Result};
use std::io::Read;
use std::sync::Arc;

/// A paper extracted from an arXiv archive.
///
/// `tex_files` is wrapped in `Arc` so handing the files across thread
/// boundaries is a refcount bump instead of a deep Vec<TexFile> clone.
pub struct PaperArchive {
    pub arxiv_id: String,
    pub tex_files: Arc<Vec<TexFile>>,
    pub file_type: FileType,
    pub entry_name: String,
}

/// Maximum decompressed size per archive (100MB).
const MAX_DECOMPRESSED_SIZE: u64 = 100_000_000;

fn detect_file_type(bytes: &[u8]) -> FileType {
    if bytes.is_empty() {
        return FileType::Unknown;
    }
    if bytes.starts_with(b"%PDF") {
        FileType::Pdf
    } else if bytes.starts_with(b"%!PS") {
        FileType::Postscript
    } else {
        // 15 bytes accommodates "<!doctype html>" (longest prefix we check).
        let prefix = &bytes[..bytes.len().min(15)];
        if (prefix.len() >= 5 && prefix[..5].eq_ignore_ascii_case(b"<html"))
            || (prefix.len() >= 9 && prefix[..9].eq_ignore_ascii_case(b"<!doctype"))
        {
            FileType::Html
        } else {
            FileType::Tex
        }
    }
}

fn classify_gz(raw: &[u8], arxiv_id: &str) -> (Vec<TexFile>, FileType) {
    match decompress_gz(raw, arxiv_id) {
        Ok(decompressed) => {
            let ft = detect_file_type(&decompressed);
            if ft == FileType::Tex {
                let tex_files = if let Some(content) = decode_bytes(&decompressed) {
                    vec![TexFile {
                        name: format!("{}.tex", arxiv_id),
                        content,
                    }]
                } else {
                    Vec::new()
                };
                (tex_files, FileType::Tex)
            } else {
                (Vec::new(), ft)
            }
        }
        Err(_) => (Vec::new(), FileType::Unknown),
    }
}

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
pub fn for_each_paper(
    reader: impl Read,
    mut f: impl FnMut(String, String, Result<PaperArchive>),
) {
    let mut archive = tar::Archive::new(reader);
    let entries = match archive.entries() {
        Ok(e) => e,
        Err(e) => {
            f(
                "unknown".into(),
                "unknown".into(),
                Err(anyhow::anyhow!("failed to read tar entries: {}", e)),
            );
            return;
        }
    };

    for entry_result in entries {
        let entry = match entry_result {
            Ok(e) => e,
            Err(e) => {
                f(
                    "unknown".into(),
                    "unknown".into(),
                    Err(anyhow::anyhow!("tar entry error: {}", e)),
                );
                continue;
            }
        };

        let path = match entry.path() {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(_) => continue,
        };

        // Skip directory entries (tar metadata, not paper submissions).
        if path.ends_with('/') {
            continue;
        }

        let arxiv_id = derive_arxiv_id(&path);
        f(
            arxiv_id.clone(),
            path.clone(),
            process_entry(entry, &arxiv_id, &path),
        );
    }
}

/// Best-effort file type derived purely from a filename extension, for
/// cases where content wasn't inspected (e.g. archive-load failures).
/// Use `detect_file_type` on decompressed bytes when you have them.
pub fn classify_by_extension(path: &str) -> FileType {
    if path.ends_with(".pdf") {
        FileType::Pdf
    } else if path.ends_with(".tex") {
        FileType::Tex
    } else {
        FileType::Unknown
    }
}

/// Convenience wrapper around `for_each_paper` for cases where collecting
/// all results is acceptable (testing, small archives).
pub fn iter_papers(reader: impl Read) -> Vec<Result<PaperArchive>> {
    let mut results = Vec::new();
    for_each_paper(reader, |_id, _name, result| results.push(result));
    results
}

fn process_entry<R: Read>(mut entry: tar::Entry<R>, arxiv_id: &str, path: &str) -> Result<PaperArchive> {
    // Skip entries whose raw size already exceeds the decompression limit
    // to avoid reading huge compressed blobs into memory.
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

    let (tex_files, file_type) = if path.ends_with(".pdf") {
        (Vec::new(), FileType::Pdf)
    } else if path.ends_with(".tar.gz") || path.ends_with(".tgz") || path.ends_with(".gz") {
        // Old arXiv .gz entries are often gzipped tar archives (multi-file
        // submissions) despite the plain .gz extension, so try tar first.
        match extract_inner_tar_gz(&raw_bytes, arxiv_id) {
            Ok(files) if !files.is_empty() => (files, FileType::Tex),
            _ => classify_gz(&raw_bytes, arxiv_id),
        }
    } else if path.ends_with(".tex") {
        (extract_single_tex(&raw_bytes, path)?, FileType::Tex)
    } else {
        (Vec::new(), FileType::Unknown)
    };

    Ok(PaperArchive {
        arxiv_id: arxiv_id.to_string(),
        tex_files: Arc::new(tex_files),
        file_type,
        entry_name: path.to_string(),
    })
}

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

fn decompress_gz(raw: &[u8], arxiv_id: &str) -> Result<Vec<u8>> {
    let gz = flate2::read::GzDecoder::new(raw);
    let mut limited = gz.take(MAX_DECOMPRESSED_SIZE);

    let mut content_bytes = Vec::new();
    limited
        .read_to_end(&mut content_bytes)
        .with_context(|| format!("decompressing gz for {}", arxiv_id))?;

    Ok(content_bytes)
}

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
/// Strips a leading UTF-8 BOM if present.
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
        .trim_end_matches(".pdf")
        .to_string()
}

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

    let (tex_files, file_type) = if path_str.ends_with(".pdf") {
        (Vec::new(), FileType::Pdf)
    } else if path_str.ends_with(".tar.gz") || path_str.ends_with(".tgz") || path_str.ends_with(".gz") {
        match extract_from_tar(&raw, &arxiv_id) {
            Ok(files) if !files.is_empty() => (files, FileType::Tex),
            _ => match extract_inner_tar_gz(&raw, &arxiv_id) {
                Ok(files) if !files.is_empty() => (files, FileType::Tex),
                _ => classify_gz(&raw, &arxiv_id),
            },
        }
    } else if path_str.ends_with(".tex") {
        (extract_single_tex(&raw, &path_str)?, FileType::Tex)
    } else {
        (Vec::new(), FileType::Unknown)
    };

    Ok(PaperArchive { arxiv_id, tex_files: Arc::new(tex_files), file_type, entry_name: path_str })
}

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
        let bytes = vec![0x63, 0x61, 0x66, 0xe9];
        let result = decode_bytes(&bytes).unwrap();
        assert!(result.contains("caf"));
    }

    #[test]
    fn test_decode_utf8_bom() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("hello".as_bytes());
        assert_eq!(decode_bytes(&bytes).unwrap(), "hello");
    }

    #[test]
    fn test_detect_file_type_empty() {
        assert_eq!(detect_file_type(b""), FileType::Unknown);
    }

    #[test]
    fn test_detect_file_type_pdf() {
        assert_eq!(detect_file_type(b"%PDF-1.4 fake pdf"), FileType::Pdf);
    }

    #[test]
    fn test_detect_file_type_postscript() {
        assert_eq!(detect_file_type(b"%!PS-Adobe-3.0"), FileType::Postscript);
    }

    #[test]
    fn test_detect_file_type_html_doctype() {
        assert_eq!(detect_file_type(b"<!DOCTYPE html>"), FileType::Html);
        assert_eq!(detect_file_type(b"<!doctype html>"), FileType::Html);
    }

    #[test]
    fn test_detect_file_type_html_tag() {
        assert_eq!(detect_file_type(b"<html>"), FileType::Html);
        assert_eq!(detect_file_type(b"<HTML>"), FileType::Html);
    }

    #[test]
    fn test_detect_file_type_tex() {
        assert_eq!(
            detect_file_type(b"\\documentclass{article}"),
            FileType::Tex
        );
    }

    #[test]
    fn paper_archive_tex_files_is_arc() {
        use std::sync::Arc;
        let p = PaperArchive {
            arxiv_id: "x".into(),
            tex_files: Arc::new(vec![]),
            file_type: FileType::Unknown,
            entry_name: "x".into(),
        };
        let p2_files = Arc::clone(&p.tex_files);
        assert!(Arc::ptr_eq(&p.tex_files, &p2_files));
    }

    #[test]
    fn test_for_each_paper_passes_arxiv_id_on_error() {
        let buf: Vec<u8> = Vec::new();
        let mut tar_builder = tar::Builder::new(buf);
        let mut header = tar::Header::new_gnu();
        header.set_size(200_000_000);
        header.set_mode(0o644);
        header.set_cksum();
        let empty: &[u8] = &[];
        tar_builder
            .append_data(&mut header, "2401.99999.gz", empty)
            .unwrap();
        let tar_bytes = tar_builder.into_inner().unwrap();

        let mut observed: Vec<(String, String, Result<PaperArchive>)> = Vec::new();
        for_each_paper(&tar_bytes[..], |id, name, r| observed.push((id, name, r)));

        let (id, name, result) = observed
            .into_iter()
            .find(|(_, _, r)| r.is_err())
            .expect("expected an Err from the oversized entry");
        assert_eq!(id, "2401.99999");
        assert_eq!(name, "2401.99999.gz");
        let err = match result {
            Err(e) => e,
            Ok(_) => unreachable!(),
        };
        let err_msg = format!("{}", err);
        assert!(
            err_msg.contains("100MB limit"),
            "error should reference the 100MB limit, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_classify_by_extension() {
        assert_eq!(classify_by_extension("2401.00001.pdf"), FileType::Pdf);
        assert_eq!(classify_by_extension("2401.00001.tex"), FileType::Tex);
        assert_eq!(classify_by_extension("2401.00001.gz"), FileType::Unknown);
        assert_eq!(classify_by_extension("2401.00001.tar.gz"), FileType::Unknown);
        assert_eq!(classify_by_extension("no_extension"), FileType::Unknown);
    }
}
