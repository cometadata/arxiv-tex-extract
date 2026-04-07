use crate::braces::find_braced_group;
use crate::symbols::CommandReplacer;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

static PROTECTED_MACROS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "\\a", "\\b", "\\c", "\\d", "\\e", "\\f", "\\g", "\\h", "\\i", "\\j",
        "\\k", "\\l", "\\m", "\\n", "\\o", "\\p", "\\q", "\\r", "\\s", "\\t",
        "\\u", "\\v", "\\w", "\\x", "\\y", "\\z",
        "\\it", "\\bf", "\\rm", "\\tt", "\\sc", "\\sl", "\\sf",
        "\\em", "\\be", "\\bi", "\\le", "\\ge", "\\ne", "\\in", "\\to",
        "\\if", "\\or", "\\do", "\\at", "\\on", "\\of",
        "\\begin", "\\end", "\\item", "\\label", "\\ref", "\\cite",
        "\\section", "\\subsection", "\\subsubsection", "\\chapter", "\\part",
        "\\paragraph", "\\subparagraph", "\\title", "\\author", "\\date",
        "\\large", "\\Large", "\\LARGE", "\\huge", "\\Huge",
        "\\small", "\\footnotesize", "\\scriptsize", "\\tiny",
        "\\normalsize", "\\normalfont",
        "\\newcommand", "\\renewcommand", "\\def", "\\let",
        "\\input", "\\include", "\\usepackage", "\\documentclass",
    ]
    .into_iter()
    .collect()
});

static NEWCOMMAND_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\(?:re)?newcommand\*?\{(\\[a-zA-Z]+)\}").unwrap());

static DEF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\def\s*(\\[a-zA-Z]+)").unwrap());

/// Collect `\newcommand` and `\def` macros WITHOUT arguments from a .tex file.
///
/// Returns `{macro_name: macro_value}`. Skips macros that take arguments
/// (detected by `[n]` between the name and body).
pub fn collect_macros(file_content: &str) -> HashMap<String, String> {
    let mut macros = HashMap::new();

    for cap in NEWCOMMAND_RE.find_iter(file_content) {
        let m = NEWCOMMAND_RE.captures(&file_content[cap.start()..]).unwrap();
        let name = m.get(1).unwrap().as_str().to_string();
        let after_name_close = cap.start() + m.get(0).unwrap().end();

        let bytes = file_content.as_bytes();
        let mut pos = after_name_close;
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        // Skip macros that take arguments (detected by [n] arg-count specifier)
        if pos < bytes.len() && bytes[pos] == b'[' {
            continue;
        }

        if let Some((cs, ce)) = find_braced_group(file_content, pos) {
            let value = file_content[cs..ce].to_string();
            if !PROTECTED_MACROS.contains(name.as_str()) {
                macros.insert(name, value);
            }
        }
    }

    for cap in DEF_RE.find_iter(file_content) {
        let m = DEF_RE.captures(&file_content[cap.start()..]).unwrap();
        let name = m.get(1).unwrap().as_str().to_string();
        let after_name = cap.start() + m.get(0).unwrap().end();

        let bytes = file_content.as_bytes();
        let mut pos = after_name;
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }

        // Skip \def macros with parameter patterns (#1, #2, etc.)
        if pos < bytes.len() && bytes[pos] == b'#' {
            continue;
        }

        if let Some((cs, ce)) = find_braced_group(file_content, pos) {
            let value = file_content[cs..ce].to_string();
            if !PROTECTED_MACROS.contains(name.as_str()) {
                macros.insert(name, value);
            }
        }
    }

    macros
}

/// Collect `\newtheorem{env_name}{Display Name}` definitions from the preamble.
///
/// Handles `\newtheorem*`, optional `[counter]` before the display name,
/// and optional `[parent]` after. Returns `{env_name: Display Name}`.
pub fn collect_newtheorems(file_content: &str) -> HashMap<String, String> {
    let mut theorems = HashMap::new();
    let pattern = "\\newtheorem";

    let mut search_start = 0;
    while let Some(pos) = file_content[search_start..].find(pattern) {
        let abs_pos = search_start + pos;
        let mut cursor = abs_pos + pattern.len();
        let bytes = file_content.as_bytes();

        if cursor < bytes.len() && bytes[cursor] == b'*' {
            cursor += 1;
        }

        if cursor < bytes.len() && bytes[cursor].is_ascii_alphabetic() {
            search_start = cursor;
            continue;
        }

        let brace_pos = skip_ws_bytes(bytes, cursor);
        if let Some((cs, ce)) = find_braced_group(file_content, brace_pos) {
            let env_name = file_content[cs..ce].to_string();
            let after_name = ce + 1;

            let mut after_opt = after_name;
            let ws_pos = skip_ws_bytes(bytes, after_opt);
            if ws_pos < bytes.len() && bytes[ws_pos] == b'[' {
                let mut depth = 1u32;
                let mut i = ws_pos + 1;
                while i < bytes.len() && depth > 0 {
                    match bytes[i] {
                        b'[' => depth += 1,
                        b']' => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
                if depth == 0 {
                    after_opt = i;
                }
            }

            let disp_pos = skip_ws_bytes(bytes, after_opt);
            if let Some((ds, de)) = find_braced_group(file_content, disp_pos) {
                let display_name = file_content[ds..de].to_string();
                theorems.insert(env_name, display_name);
                search_start = de + 1;
                continue;
            }
        }

        search_start = cursor;
    }

    theorems
}

fn skip_ws_bytes(bytes: &[u8], pos: usize) -> usize {
    let mut i = pos;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\n') {
        i += 1;
    }
    i
}

/// Size threshold (5MB) above which macro expansion uses fewer iterations
/// to limit worst-case scanning on large inputs.
const LARGE_INPUT_THRESHOLD: usize = 5_000_000;

/// Safely expand macros using Aho-Corasick single-pass replacement.
///
/// Iterates up to `max_passes` times to resolve multi-level macro chains
/// (e.g. \foo -> \bar -> "final"). For inputs above 5MB, the cap is reduced
/// from 5 to 3 passes to limit worst-case scanning.
pub fn expand_macros(text: &str, macros: &HashMap<String, String>) -> String {
    if macros.is_empty() {
        return text.to_string();
    }

    let max_passes = if text.len() > LARGE_INPUT_THRESHOLD { 3 } else { 5 };

    let pairs: Vec<(&str, &str)> = macros.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let replacer = CommandReplacer::new(&pairs);

    let mut result = text.to_string();
    for _ in 0..max_passes {
        let next = replacer.replace_all(&result);
        if next == result {
            break;
        }
        result = next;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_newcommand() {
        let input = r"\newcommand{\foo}{bar baz}";
        let macros = collect_macros(input);
        assert_eq!(macros.get("\\foo").unwrap(), "bar baz");
    }

    #[test]
    fn test_collect_newcommand_star() {
        let input = r"\newcommand*{\foo}{bar}";
        let macros = collect_macros(input);
        assert_eq!(macros.get("\\foo").unwrap(), "bar");
    }

    #[test]
    fn test_skip_macro_with_args() {
        let input = r"\newcommand{\foo}[1]{bar #1}";
        let macros = collect_macros(input);
        assert!(macros.is_empty());
    }

    #[test]
    fn test_collect_def() {
        let input = r"\def\myname{John Doe}";
        let macros = collect_macros(input);
        assert_eq!(macros.get("\\myname").unwrap(), "John Doe");
    }

    #[test]
    fn test_skip_def_with_args() {
        let input = r"\def\foo#1{bar #1}";
        let macros = collect_macros(input);
        assert!(macros.is_empty());
    }

    #[test]
    fn test_skip_protected() {
        let input = r"\newcommand{\section}{custom}";
        let macros = collect_macros(input);
        assert!(macros.is_empty());
    }

    #[test]
    fn test_nested_body() {
        let input = r"\newcommand{\foo}{\textbf{nested {braces}}}";
        let macros = collect_macros(input);
        assert_eq!(
            macros.get("\\foo").unwrap(),
            r"\textbf{nested {braces}}"
        );
    }

    #[test]
    fn test_expand_macros() {
        let mut macros = HashMap::new();
        macros.insert("\\foo".to_string(), "bar".to_string());
        assert_eq!(expand_macros("\\foo is good", &macros), "bar is good");
    }

    #[test]
    fn test_expand_macros_boundary() {
        let mut macros = HashMap::new();
        macros.insert("\\foo".to_string(), "bar".to_string());
        // \foobar should NOT be expanded
        assert_eq!(expand_macros("\\foobar", &macros), "\\foobar");
    }

    #[test]
    fn test_expand_macros_at_eof() {
        let mut macros = HashMap::new();
        macros.insert("\\foo".to_string(), "bar".to_string());
        assert_eq!(expand_macros("\\foo", &macros), "bar");
    }

    #[test]
    fn test_expand_macros_chain() {
        let mut macros = HashMap::new();
        macros.insert("\\foo".to_string(), "\\bar".to_string());
        macros.insert("\\bar".to_string(), "final".to_string());
        assert_eq!(expand_macros("\\foo", &macros), "final");
    }

    #[test]
    fn test_expand_macros_three_level_chain() {
        let mut macros = HashMap::new();
        macros.insert("\\aaa".to_string(), "\\bbb".to_string());
        macros.insert("\\bbb".to_string(), "\\ccc".to_string());
        macros.insert("\\ccc".to_string(), "done".to_string());
        assert_eq!(expand_macros("\\aaa", &macros), "done");
    }

    #[test]
    fn test_expand_macros_large_input_cap() {
        let mut macros = HashMap::new();
        macros.insert("\\aaa".to_string(), "\\bbb".to_string());
        macros.insert("\\bbb".to_string(), "\\ccc".to_string());
        macros.insert("\\ccc".to_string(), "done".to_string());

        // Input above LARGE_INPUT_THRESHOLD still resolves a 3-level chain
        let mut large_input = "x".repeat(LARGE_INPUT_THRESHOLD + 1);
        large_input.push_str("\\aaa");

        let result = expand_macros(&large_input, &macros);
        assert!(
            result.ends_with("done"),
            "3-level chain should resolve with 3 passes on large input"
        );
    }
}
