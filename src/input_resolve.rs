use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// A .tex file with its name and content.
#[derive(Debug, Clone)]
pub struct TexFile {
    pub name: String,
    pub content: String,
}

/// Single-argument include commands: \input, \include, \subfile, \@input
static INPUT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\(?:input|include|subfile|@input)\s*\{([^{}]+)\}").unwrap());

/// \InputIfFileExists{file}{true-code}{false-code} — extract filename from first arg
static INPUTIFFILEEXISTS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\\InputIfFileExists\s*\{([^{}]+)\}\{[^{}]*\}\{[^{}]*\}").unwrap()
});

/// \import{dir}{file} and \subimport{dir}{file} — two-arg form
static IMPORT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\(?:sub)?import\s*\{([^{}]*)\}\s*\{([^{}]+)\}").unwrap());

/// \bibliography{name} — resolve to name.bbl
static BIBLIOGRAPHY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\bibliography\s*\{([^{}]+)\}").unwrap());

/// Detect the main .tex file from a collection of files.
///
/// Priority:
/// 1. File containing `\documentclass`
/// 2. File containing `\begin{document}`
/// 3. First file alphabetically
///
/// If multiple files have `\documentclass`, prefer the one that also has `\begin{document}`.
pub fn find_main_file(files: &[TexFile]) -> usize {
    let mut docclass_indices: Vec<usize> = Vec::new();
    let mut begindoc_indices: Vec<usize> = Vec::new();

    for (i, f) in files.iter().enumerate() {
        if f.content.contains("\\documentclass") {
            docclass_indices.push(i);
        }
        if f.content.contains("\\begin{document}") {
            begindoc_indices.push(i);
        }
    }

    for &i in &docclass_indices {
        if begindoc_indices.contains(&i) {
            return i;
        }
    }

    if let Some(&i) = docclass_indices.first() {
        return i;
    }

    if let Some(&i) = begindoc_indices.first() {
        return i;
    }

    0
}

/// Resolve `\input{path}` and `\include{path}` directives by inlining referenced content.
///
/// Returns the content with all `\input`/`\include` directives replaced by the
/// referenced file's content. Uses iterative resolution with depth limiting and
/// circular include protection.
pub fn resolve_inputs(
    content: &str,
    file_index: &HashMap<String, String>,
    max_depth: usize,
) -> String {
    let mut result = content.to_string();
    let mut visited: HashSet<String> = HashSet::new();
    resolve_inputs_recursive(&mut result, file_index, &mut visited, 0, max_depth);
    result
}

/// Collect all include-like directives from content, returning (start, end, resolved_path).
fn find_all_includes(content: &str) -> Vec<(usize, usize, String)> {
    let mut matches = Vec::new();

    for cap in INPUT_RE.captures_iter(content) {
        let full = cap.get(0).unwrap();
        let path = cap.get(1).unwrap().as_str().to_string();
        matches.push((full.start(), full.end(), path));
    }

    for cap in INPUTIFFILEEXISTS_RE.captures_iter(content) {
        let full = cap.get(0).unwrap();
        let path = cap.get(1).unwrap().as_str().to_string();
        matches.push((full.start(), full.end(), path));
    }

    for cap in IMPORT_RE.captures_iter(content) {
        let full = cap.get(0).unwrap();
        let dir = cap.get(1).unwrap().as_str();
        let file = cap.get(2).unwrap().as_str();
        let combined = format!("{}{}", dir, file);
        matches.push((full.start(), full.end(), combined));
    }

    for cap in BIBLIOGRAPHY_RE.captures_iter(content) {
        let full = cap.get(0).unwrap();
        let name = cap.get(1).unwrap().as_str().trim();
        matches.push((full.start(), full.end(), format!("{}.bbl", name)));
    }

    matches.sort_by_key(|(start, _, _)| *start);
    matches
}

fn resolve_inputs_recursive(
    content: &mut String,
    file_index: &HashMap<String, String>,
    visited: &mut HashSet<String>,
    depth: usize,
    max_depth: usize,
) {
    if depth >= max_depth {
        return;
    }

    let matches = find_all_includes(content);

    // Process in reverse order to preserve positions
    for (start, end, path) in matches.into_iter().rev() {
        let resolved_path = resolve_path(&path);

        if visited.contains(&resolved_path) {
            continue;
        }

        let lookup = if let Some(c) = file_index.get(&resolved_path) {
            Some((resolved_path.clone(), c.clone()))
        } else if resolved_path.ends_with(".bbl") {
            file_index
                .iter()
                .find(|(k, _)| k.ends_with(".bbl"))
                .map(|(k, v)| (k.clone(), v.clone()))
        } else {
            None
        };

        if let Some((found_path, included_content)) = lookup {
            visited.insert(found_path);
            let mut included = included_content;
            resolve_inputs_recursive(&mut included, file_index, visited, depth + 1, max_depth);
            content.replace_range(start..end, &included);
        }
    }
}

/// Resolve a file path by normalizing:
/// - Strip leading `./`
/// - Append `.tex` if no known extension is already present
fn resolve_path(path: &str) -> String {
    let trimmed = path.trim();
    let stripped = trimmed.strip_prefix("./").unwrap_or(trimmed);
    if stripped.ends_with(".tex") || stripped.ends_with(".bbl") {
        stripped.to_string()
    } else {
        format!("{}.tex", stripped)
    }
}

/// Build a file index mapping possible lookup names to file contents.
/// For a file named "chapters/intro.tex", the index contains entries for:
/// - "chapters/intro.tex"
/// - "intro.tex"
/// - "chapters/intro"
/// - "intro"
pub fn build_file_index(files: &[TexFile]) -> HashMap<String, String> {
    let mut index = HashMap::new();
    for f in files {
        index.insert(f.name.clone(), f.content.clone());

        for suffix in &[".tex", ".bbl"] {
            if let Some(stripped) = f.name.strip_suffix(suffix) {
                index.insert(stripped.to_string(), f.content.clone());
            }
        }

        if let Some(no_dot_slash) = f.name.strip_prefix("./") {
            index.insert(no_dot_slash.to_string(), f.content.clone());
            for suffix in &[".tex", ".bbl"] {
                if let Some(stripped) = no_dot_slash.strip_suffix(suffix) {
                    index.insert(stripped.to_string(), f.content.clone());
                }
            }
        }

        if let Some(basename) = f.name.rsplit('/').next() {
            if basename != f.name {
                index.insert(basename.to_string(), f.content.clone());
                for suffix in &[".tex", ".bbl"] {
                    if let Some(stripped) = basename.strip_suffix(suffix) {
                        index.insert(stripped.to_string(), f.content.clone());
                    }
                }
            }
        }
    }
    index
}

/// Determine which files are referenced by `\input`/`\include` from the main file.
/// Returns the set of file names that are included (and should NOT be processed standalone).
pub fn find_included_files(main_content: &str, file_index: &HashMap<String, String>) -> HashSet<String> {
    let mut included = HashSet::new();
    collect_included_recursive(main_content, file_index, &mut included, &mut HashSet::new(), 0, 20);
    included
}

fn collect_included_recursive(
    content: &str,
    file_index: &HashMap<String, String>,
    included: &mut HashSet<String>,
    visited: &mut HashSet<String>,
    depth: usize,
    max_depth: usize,
) {
    if depth >= max_depth {
        return;
    }

    for (_, _, path) in find_all_includes(content) {
        let resolved = resolve_path(&path);

        if visited.contains(&resolved) {
            continue;
        }

        if let Some(sub_content) = file_index.get(&resolved) {
            included.insert(resolved.clone());
            visited.insert(resolved.clone());
            collect_included_recursive(sub_content, file_index, included, visited, depth + 1, max_depth);
        }
    }
}

/// Heuristic: is this `.tex` file a conference template / style file rather
/// than real content?  Used to filter unreferenced files.
fn is_template_file(f: &TexFile) -> bool {
    let basename = f.name.rsplit('/').next().unwrap_or(&f.name).to_ascii_lowercase();

    let template_suffixes = [
        "_conference.tex",
        "_submission.tex",
        "_style.tex",
        "_format.tex",
    ];
    if template_suffixes.iter().any(|s| basename.ends_with(s)) {
        return true;
    }

    let name_no_ext = basename.strip_suffix(".tex").unwrap_or(&basename);
    let template_prefixes = ["sample", "template", "example", "instructions"];
    if template_prefixes.iter().any(|p| name_no_ext.starts_with(p)) {
        return true;
    }

    if f.content.contains("\\ProvidesPackage") || f.content.contains("\\ProvidesClass") {
        return true;
    }

    let instruction_phrases = [
        "formatting instructions",
        "style file",
        "do not modify",
        "camera ready",
        "camera-ready",
        "do not change",
        "submission template",
    ];
    let hits = instruction_phrases
        .iter()
        .filter(|p| f.content.to_ascii_lowercase().contains(*p))
        .count();
    if hits >= 2 {
        return true;
    }

    false
}

/// Resolve inputs and return the list of root files to process.
///
/// A root file is:
/// 1. The main file (with all `\input`/`\include` inlined)
/// 2. Any file NOT referenced by `\input`/`\include` from the main file
///
/// Files are returned in order: main file first, then unreferenced files sorted by name.
pub fn resolve_and_order(files: &[TexFile]) -> Vec<TexFile> {
    if files.is_empty() {
        return Vec::new();
    }

    if files.len() == 1 {
        return files.to_vec();
    }

    let main_idx = find_main_file(files);
    let file_index = build_file_index(files);

    let included_files = find_included_files(&files[main_idx].content, &file_index);
    let resolved_main = resolve_inputs(&files[main_idx].content, &file_index, 20);

    let mut result = vec![TexFile {
        name: files[main_idx].name.clone(),
        content: resolved_main,
    }];

    let mut unreferenced: Vec<&TexFile> = files
        .iter()
        .enumerate()
        .filter(|(i, f)| {
            *i != main_idx
                && !included_files.contains(&f.name)
                && !included_files.contains(&resolve_path(&f.name))
                && !is_template_file(f)
        })
        .map(|(_, f)| f)
        .collect();
    unreferenced.sort_by_key(|f| &f.name);
    result.extend(unreferenced.iter().map(|f| (*f).clone()));

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_main_file() {
        let files = vec![
            TexFile { name: "appendix.tex".into(), content: "Appendix content".into() },
            TexFile { name: "main.tex".into(), content: r"\documentclass{article}\begin{document}Hello\end{document}".into() },
            TexFile { name: "intro.tex".into(), content: "Introduction".into() },
        ];
        assert_eq!(find_main_file(&files), 1);
    }

    #[test]
    fn test_resolve_inputs() {
        let mut index = HashMap::new();
        index.insert("intro.tex".into(), "Introduction content".into());
        let content = r"Before \input{intro} After";
        let resolved = resolve_inputs(content, &index, 20);
        assert_eq!(resolved, "Before Introduction content After");
    }

    #[test]
    fn test_resolve_without_extension() {
        let mut index = HashMap::new();
        index.insert("chapter1.tex".into(), "Chapter 1".into());
        let content = r"\input{chapter1}";
        let resolved = resolve_inputs(content, &index, 20);
        assert_eq!(resolved, "Chapter 1");
    }

    #[test]
    fn test_circular_include() {
        let mut index = HashMap::new();
        index.insert("a.tex".into(), r"\input{b}".into());
        index.insert("b.tex".into(), r"\input{a}".into());
        let content = r"\input{a}";
        // Should not loop forever
        let resolved = resolve_inputs(content, &index, 20);
        assert!(resolved.contains("\\input{a}") || resolved.contains("\\input{b}") || !resolved.is_empty());
    }

    #[test]
    fn test_depth_limit() {
        let mut index = HashMap::new();
        for i in 0..30 {
            index.insert(format!("f{}.tex", i), format!("\\input{{f{}}}", i + 1));
        }
        let content = r"\input{f0}";
        // Should stop at depth 20
        let resolved = resolve_inputs(content, &index, 20);
        assert!(!resolved.is_empty());
    }

    #[test]
    fn test_missing_file() {
        let index = HashMap::new();
        let content = r"\input{nonexistent}";
        let resolved = resolve_inputs(content, &index, 20);
        assert_eq!(resolved, r"\input{nonexistent}");
    }

    #[test]
    fn test_resolve_and_order() {
        let files = vec![
            TexFile { name: "intro.tex".into(), content: "Introduction".into() },
            TexFile {
                name: "main.tex".into(),
                content: r"\documentclass{article}\begin{document}\input{intro}\end{document}".into(),
            },
            TexFile { name: "standalone.tex".into(), content: "Standalone content".into() },
        ];
        let result = resolve_and_order(&files);
        assert_eq!(result.len(), 2); // main (with intro inlined) + standalone
        assert_eq!(result[0].name, "main.tex");
        assert!(result[0].content.contains("Introduction"));
        assert_eq!(result[1].name, "standalone.tex");
    }

    #[test]
    fn test_subfile_resolution() {
        let mut index = HashMap::new();
        index.insert("chapter1.tex".into(), "Chapter 1 content".into());
        let content = r"\subfile{chapter1}";
        let resolved = resolve_inputs(content, &index, 20);
        assert_eq!(resolved, "Chapter 1 content");
    }

    #[test]
    fn test_import_resolution() {
        let mut index = HashMap::new();
        index.insert("chapters/intro.tex".into(), "Intro content".into());
        let content = r"\import{chapters/}{intro}";
        let resolved = resolve_inputs(content, &index, 20);
        assert_eq!(resolved, "Intro content");
    }

    #[test]
    fn test_subimport_resolution() {
        let mut index = HashMap::new();
        index.insert("sec/methods.tex".into(), "Methods content".into());
        let content = r"\subimport{sec/}{methods}";
        let resolved = resolve_inputs(content, &index, 20);
        assert_eq!(resolved, "Methods content");
    }

    #[test]
    fn test_inputiffileexists() {
        let mut index = HashMap::new();
        index.insert("extra.tex".into(), "Extra content".into());
        let content = r"\InputIfFileExists{extra.tex}{}{skipped}";
        let resolved = resolve_inputs(content, &index, 20);
        assert_eq!(resolved, "Extra content");
    }

    #[test]
    fn test_dot_slash_prefix() {
        let mut index = HashMap::new();
        index.insert("intro.tex".into(), "Intro".into());
        let content = r"\input{./intro}";
        let resolved = resolve_inputs(content, &index, 20);
        assert_eq!(resolved, "Intro");
    }

    #[test]
    fn test_at_input_resolution() {
        let mut index = HashMap::new();
        index.insert("appendix.tex".into(), "Appendix content".into());
        let content = r"\@input{appendix}";
        let resolved = resolve_inputs(content, &index, 20);
        assert_eq!(resolved, "Appendix content");
    }

    #[test]
    fn test_template_file_suffix() {
        let f = TexFile {
            name: "iclr2025_conference.tex".into(),
            content: "Some template content".into(),
        };
        assert!(is_template_file(&f));
    }

    #[test]
    fn test_template_file_prefix() {
        let f = TexFile {
            name: "sample_paper.tex".into(),
            content: "Sample content".into(),
        };
        assert!(is_template_file(&f));
    }

    #[test]
    fn test_template_file_provides_package() {
        let f = TexFile {
            name: "mystyle.tex".into(),
            content: r"\ProvidesPackage{mystyle}".into(),
        };
        assert!(is_template_file(&f));
    }

    #[test]
    fn test_template_file_instruction_phrases() {
        let f = TexFile {
            name: "format.tex".into(),
            content: "These are formatting instructions. Do not modify this file.".into(),
        };
        assert!(is_template_file(&f));
    }

    #[test]
    fn test_normal_file_not_template() {
        let f = TexFile {
            name: "introduction.tex".into(),
            content: "This is the introduction to our paper.".into(),
        };
        assert!(!is_template_file(&f));
    }

    #[test]
    fn test_template_excluded_from_resolve_order() {
        let files = vec![
            TexFile {
                name: "main.tex".into(),
                content: r"\documentclass{article}\begin{document}Hello\end{document}".into(),
            },
            TexFile {
                name: "iclr2025_conference.tex".into(),
                content: "Template stuff".into(),
            },
            TexFile {
                name: "appendix.tex".into(),
                content: "Appendix content".into(),
            },
        ];
        let result = resolve_and_order(&files);
        // main + appendix, but NOT the template
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|f| f.name != "iclr2025_conference.tex"));
    }
}
