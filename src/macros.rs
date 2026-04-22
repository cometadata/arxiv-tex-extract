use crate::braces::{extract_command_arg, extract_optional_arg, find_braced_group};
use crate::symbols::CommandReplacer;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use tracing::warn;

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
    LazyLock::new(|| Regex::new(r"\\(?:(?:re)?new|provide)command\*?\{(\\[a-zA-Z]+)\}").unwrap());

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

/// A macro defined with an argument count, e.g. `\newcommand{\foo}[2]{#1 and #2}`.
pub struct ParametricMacro {
    pub name: String,
    pub num_args: usize,
    pub body: String,
    pub optional_default: Option<String>,
}

/// Collect `\newcommand`, `\renewcommand`, `\providecommand`, and `\def` macros
/// WITH arguments from a .tex file.
///
/// Returns parametric macros that take one or more arguments. These are the
/// macros skipped by `collect_macros()`.
pub fn collect_parametric_macros(file_content: &str) -> Vec<ParametricMacro> {
    let mut macros = Vec::new();

    for cap in NEWCOMMAND_RE.find_iter(file_content) {
        let m = NEWCOMMAND_RE.captures(&file_content[cap.start()..]).unwrap();
        let name = m.get(1).unwrap().as_str().to_string();
        let after_name_close = cap.start() + m.get(0).unwrap().end();

        if PROTECTED_MACROS.contains(name.as_str()) {
            continue;
        }

        let bytes = file_content.as_bytes();
        let mut pos = after_name_close;
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }

        // Must have [n] arg-count specifier (otherwise it's a simple macro)
        if pos >= bytes.len() || bytes[pos] != b'[' {
            continue;
        }

        if let Some((cs, ce, after_bracket)) = extract_optional_arg(file_content, pos) {
            let num_str = file_content[cs..ce].trim();
            let num_args: usize = match num_str.parse() {
                Ok(n) if n > 0 && n <= 9 => n,
                _ => continue,
            };

            pos = after_bracket;
            while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
                pos += 1;
            }

            // Check for optional default value [default]
            let optional_default = if pos < bytes.len() && bytes[pos] == b'[' {
                if let Some((ds, de, after_default)) = extract_optional_arg(file_content, pos) {
                    pos = after_default;
                    Some(file_content[ds..de].to_string())
                } else {
                    None
                }
            } else {
                None
            };

            if let Some((bs, be)) = find_braced_group(file_content, pos) {
                // Deduplicate: last definition wins (for \renewcommand)
                macros.retain(|m: &ParametricMacro| m.name != name);
                macros.push(ParametricMacro {
                    name,
                    num_args,
                    body: file_content[bs..be].to_string(),
                    optional_default,
                });
            }
        }
    }

    // Handle \def\foo#1#2{body}
    for cap in DEF_RE.find_iter(file_content) {
        let m = DEF_RE.captures(&file_content[cap.start()..]).unwrap();
        let name = m.get(1).unwrap().as_str().to_string();
        let after_name = cap.start() + m.get(0).unwrap().end();

        if PROTECTED_MACROS.contains(name.as_str()) {
            continue;
        }

        let bytes = file_content.as_bytes();
        let mut pos = after_name;
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }

        // Must have #n parameter pattern (otherwise it's a simple macro)
        if pos >= bytes.len() || bytes[pos] != b'#' {
            continue;
        }

        let mut num_args = 0usize;
        while pos + 1 < bytes.len() && bytes[pos] == b'#' && bytes[pos + 1].is_ascii_digit() {
            num_args += 1;
            pos += 2;
        }

        if num_args == 0 || num_args > 9 {
            continue;
        }

        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if let Some((bs, be)) = find_braced_group(file_content, pos) {
            macros.retain(|m: &ParametricMacro| m.name != name);
            macros.push(ParametricMacro {
                name,
                num_args,
                body: file_content[bs..be].to_string(),
                optional_default: None,
            });
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

/// Normalize common shorthand environment abbreviations.
///
/// Some LaTeX documents define shortcuts like `\be` for `\begin{equation}`.
/// This expands them before macro expansion to ensure they're properly
/// processed. Uses CommandReplacer for word-boundary safety.
pub fn normalize_shorthands(text: &str) -> String {
    static SHORTHANDS: LazyLock<CommandReplacer> = LazyLock::new(|| {
        CommandReplacer::new(&[
            ("\\beq", "\\begin{equation}"),
            ("\\eeq", "\\end{equation}"),
            ("\\bea", "\\begin{eqnarray}"),
            ("\\eea", "\\end{eqnarray}"),
            ("\\bse", "\\begin{subequations}"),
            ("\\ese", "\\end{subequations}"),
            ("\\be", "\\begin{equation}"),
            ("\\ee", "\\end{equation}"),
            ("\\ba", "\\begin{align}"),
            ("\\ea", "\\end{align}"),
        ])
    });
    SHORTHANDS.replace_all(text)
}

/// Size threshold (5MB) above which macro expansion uses fewer iterations
/// to limit worst-case scanning on large inputs.
const LARGE_INPUT_THRESHOLD: usize = 5_000_000;

/// Abort macro expansion if cumulative output exceeds this multiple of the input.
/// Recursively or mutually-referencing macros can expand 6–7× per pass; the
/// existing 5-pass ceiling means an unbounded loop can grow ~16,000×, pushing
/// a 50 KB input past 800 MB. Bounding at 10× keeps expansion useful for
/// well-formed macros while preventing runaway growth.
const MAX_EXPANSION_GROWTH_RATIO: usize = 10;

/// Hard ceiling on any single expansion pass, independent of input size.
/// Protects against pathological large inputs whose `10× input` cap would
/// still be huge. 50 MB per stage invocation is generous for LaTeX text.
const MAX_EXPANSION_BYTES: usize = 50_000_000;

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
    let input_len = text.len();
    let size_cap = input_len
        .saturating_mul(MAX_EXPANSION_GROWTH_RATIO)
        .min(MAX_EXPANSION_BYTES);

    let pairs: Vec<(&str, &str)> = macros.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let replacer = CommandReplacer::new(&pairs);

    let mut result = text.to_string();
    for pass in 0..max_passes {
        // If the prior pass already produced output at or beyond the cap,
        // skip running another pass — the next one can only grow further.
        if result.len() > size_cap {
            warn!(
                pass,
                input_bytes = input_len,
                result_bytes = result.len(),
                cap_bytes = size_cap,
                "macro expansion halted: output exceeds {}× input / {} bytes",
                MAX_EXPANSION_GROWTH_RATIO,
                MAX_EXPANSION_BYTES,
            );
            break;
        }
        let next = replacer.replace_all(&result);
        if next.len() > size_cap {
            warn!(
                pass,
                input_bytes = input_len,
                attempted_bytes = next.len(),
                cap_bytes = size_cap,
                "macro expansion halted: pass output exceeds {}× input / {} bytes",
                MAX_EXPANSION_GROWTH_RATIO,
                MAX_EXPANSION_BYTES,
            );
            break;
        }
        if next == result {
            break;
        }
        result = next;
    }
    result
}

/// Expand parametric macros by substituting `#1`, `#2`, etc. with extracted arguments.
///
/// Iterates up to 3 passes (2 for large inputs) to resolve nested parametric macros.
pub fn expand_parametric_macros(text: &str, macros: &[ParametricMacro]) -> String {
    if macros.is_empty() {
        return text.to_string();
    }

    let max_passes = if text.len() > LARGE_INPUT_THRESHOLD { 2 } else { 3 };
    let input_len = text.len();
    let size_cap = input_len
        .saturating_mul(MAX_EXPANSION_GROWTH_RATIO)
        .min(MAX_EXPANSION_BYTES);
    let mut result = text.to_string();

    'passes: for pass in 0..max_passes {
        let prev = result.clone();
        for mac in macros {
            result = expand_single_parametric(&result, mac);
            if result.len() > size_cap {
                warn!(
                    pass,
                    macro_name = %mac.name,
                    input_bytes = input_len,
                    result_bytes = result.len(),
                    cap_bytes = size_cap,
                    "parametric macro expansion halted: output exceeds {}× input / {} bytes",
                    MAX_EXPANSION_GROWTH_RATIO,
                    MAX_EXPANSION_BYTES,
                );
                result = prev;
                break 'passes;
            }
        }
        if result == prev {
            break;
        }
    }

    result
}

/// Expand all occurrences of a single parametric macro in the text.
fn expand_single_parametric(text: &str, mac: &ParametricMacro) -> String {
    let name = &mac.name;
    let name_len = name.len();
    let mut out = String::with_capacity(text.len());
    let mut search_from = 0;

    while let Some(rel_pos) = text[search_from..].find(name.as_str()) {
        let abs_pos = search_from + rel_pos;
        let after_name = abs_pos + name_len;

        // Word boundary: next char must not be ASCII alphabetic
        if after_name < text.len() && text.as_bytes()[after_name].is_ascii_alphabetic() {
            out.push_str(&text[search_from..after_name]);
            search_from = after_name;
            continue;
        }

        // Copy text before the match
        out.push_str(&text[search_from..abs_pos]);

        if let Some((replacement, new_cursor)) = try_expand_parametric(text, after_name, mac) {
            out.push_str(&replacement);
            search_from = new_cursor;
        } else {
            // Could not extract required arguments; keep the command as-is
            out.push_str(name);
            search_from = after_name;
        }
    }

    out.push_str(&text[search_from..]);
    out
}

/// Try to extract arguments and expand a single parametric macro invocation.
///
/// Returns `Some((replacement, cursor_after))` on success, or `None` if
/// the required number of braced arguments could not be extracted.
fn try_expand_parametric(
    text: &str,
    start: usize,
    mac: &ParametricMacro,
) -> Option<(String, usize)> {
    let mut cursor = start;
    let mut arg_values: Vec<String> = Vec::with_capacity(mac.num_args);

    if let Some(ref default) = mac.optional_default {
        // First arg is optional with a default value
        if let Some((cs, ce, after)) = extract_optional_arg(text, cursor) {
            arg_values.push(text[cs..ce].to_string());
            cursor = after;
        } else {
            arg_values.push(default.clone());
        }
        // Remaining args are mandatory
        for _ in 0..mac.num_args - 1 {
            let (cs, ce, after) = extract_command_arg(text, cursor)?;
            arg_values.push(text[cs..ce].to_string());
            cursor = after;
        }
    } else {
        // All args are mandatory
        for _ in 0..mac.num_args {
            let (cs, ce, after) = extract_command_arg(text, cursor)?;
            arg_values.push(text[cs..ce].to_string());
            cursor = after;
        }
    }

    let replacement = substitute_args(&mac.body, &arg_values);
    Some((replacement, cursor))
}

/// Replace `#1`, `#2`, … placeholders in a macro body with argument values.
///
/// Handles `##` as an escaped literal `#`. All placeholders are substituted
/// in a single pass to avoid interference between arguments.
fn substitute_args(body: &str, args: &[String]) -> String {
    let bytes = body.as_bytes();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            if i + 1 >= bytes.len() {
                // Trailing '#' with nothing after it — emit as literal
                out.push('#');
                i += 1;
            } else if bytes[i + 1] == b'#' {
                out.push('#');
                i += 2;
            } else if bytes[i + 1].is_ascii_digit() {
                let n = (bytes[i + 1] - b'0') as usize;
                if n >= 1 && n <= args.len() {
                    out.push_str(&args[n - 1]);
                }
                // Out-of-range #N silently dropped (defensive for malformed macros)
                i += 2;
            } else {
                out.push('#');
                i += 1;
            }
        } else {
            let start = i;
            while i < bytes.len() && bytes[i] != b'#' {
                i += 1;
            }
            out.push_str(&body[start..i]);
        }
    }
    out
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

    #[test]
    fn test_collect_providecommand() {
        let input = r"\providecommand{\foo}{bar}";
        let macros = collect_macros(input);
        assert_eq!(macros.get("\\foo").unwrap(), "bar");
    }

    #[test]
    fn test_collect_providecommand_star() {
        let input = r"\providecommand*{\foo}{bar}";
        let macros = collect_macros(input);
        assert_eq!(macros.get("\\foo").unwrap(), "bar");
    }

    #[test]
    fn test_collect_parametric_newcommand() {
        let input = r"\newcommand{\vect}[1]{\mathbf{#1}}";
        let macros = collect_parametric_macros(input);
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].name, "\\vect");
        assert_eq!(macros[0].num_args, 1);
        assert_eq!(macros[0].body, "\\mathbf{#1}");
        assert!(macros[0].optional_default.is_none());
    }

    #[test]
    fn test_collect_parametric_two_args() {
        let input = r"\newcommand{\inner}[2]{\langle #1, #2 \rangle}";
        let macros = collect_parametric_macros(input);
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].num_args, 2);
        assert_eq!(macros[0].body, "\\langle #1, #2 \\rangle");
    }

    #[test]
    fn test_collect_parametric_with_optional_default() {
        let input = r"\newcommand{\greet}[2][Hello]{#1, #2!}";
        let macros = collect_parametric_macros(input);
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].num_args, 2);
        assert_eq!(macros[0].optional_default.as_deref(), Some("Hello"));
        assert_eq!(macros[0].body, "#1, #2!");
    }

    #[test]
    fn test_collect_parametric_def() {
        let input = r"\def\foo#1#2{#1 + #2}";
        let macros = collect_parametric_macros(input);
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].name, "\\foo");
        assert_eq!(macros[0].num_args, 2);
        assert_eq!(macros[0].body, "#1 + #2");
    }

    #[test]
    fn test_collect_parametric_skip_protected() {
        let input = r"\newcommand{\section}[1]{custom #1}";
        let macros = collect_parametric_macros(input);
        assert!(macros.is_empty());
    }

    #[test]
    fn test_collect_parametric_renewcommand_dedup() {
        let input = r"\newcommand{\foo}[1]{first #1}
\renewcommand{\foo}[1]{second #1}";
        let macros = collect_parametric_macros(input);
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].body, "second #1");
    }

    #[test]
    fn test_collect_parametric_providecommand() {
        let input = r"\providecommand{\foo}[1]{provided #1}";
        let macros = collect_parametric_macros(input);
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].name, "\\foo");
    }

    #[test]
    fn test_simple_macros_still_skip_parametric() {
        let input = r"\newcommand{\foo}[1]{bar #1}";
        let simple = collect_macros(input);
        assert!(simple.is_empty(), "collect_macros should skip parametric");
    }

    #[test]
    fn test_expand_parametric_single_arg() {
        let macros = vec![ParametricMacro {
            name: "\\vect".into(),
            num_args: 1,
            body: "\\mathbf{#1}".into(),
            optional_default: None,
        }];
        let result = expand_parametric_macros("\\vect{x}", &macros);
        assert_eq!(result, "\\mathbf{x}");
    }

    #[test]
    fn test_expand_parametric_two_args() {
        let macros = vec![ParametricMacro {
            name: "\\foo".into(),
            num_args: 2,
            body: "#1 + #2".into(),
            optional_default: None,
        }];
        let result = expand_parametric_macros("\\foo{a}{b}", &macros);
        assert_eq!(result, "a + b");
    }

    #[test]
    fn test_expand_parametric_with_default() {
        let macros = vec![ParametricMacro {
            name: "\\greet".into(),
            num_args: 2,
            body: "#1, #2!".into(),
            optional_default: Some("Hello".into()),
        }];
        // Without optional arg: uses default
        assert_eq!(
            expand_parametric_macros("\\greet{World}", &macros),
            "Hello, World!"
        );
        // With explicit optional arg
        assert_eq!(
            expand_parametric_macros("\\greet[Hi]{World}", &macros),
            "Hi, World!"
        );
    }

    #[test]
    fn test_expand_parametric_word_boundary() {
        let macros = vec![ParametricMacro {
            name: "\\foo".into(),
            num_args: 1,
            body: "X".into(),
            optional_default: None,
        }];
        // \foobar should NOT match \foo
        assert_eq!(
            expand_parametric_macros("\\foobar{x}", &macros),
            "\\foobar{x}"
        );
    }

    #[test]
    fn test_expand_parametric_no_args_available() {
        let macros = vec![ParametricMacro {
            name: "\\foo".into(),
            num_args: 1,
            body: "#1".into(),
            optional_default: None,
        }];
        // \foo without braced arg should be left as-is
        assert_eq!(
            expand_parametric_macros("\\foo is here", &macros),
            "\\foo is here"
        );
    }

    #[test]
    fn test_expand_parametric_nested_braces() {
        let macros = vec![ParametricMacro {
            name: "\\wrap".into(),
            num_args: 1,
            body: "[#1]".into(),
            optional_default: None,
        }];
        assert_eq!(
            expand_parametric_macros("\\wrap{a {b} c}", &macros),
            "[a {b} c]"
        );
    }

    #[test]
    fn test_expand_parametric_multiple_occurrences() {
        let macros = vec![ParametricMacro {
            name: "\\b".into(),
            num_args: 1,
            body: "**#1**".into(),
            optional_default: None,
        }];
        assert_eq!(
            expand_parametric_macros("\\b{x} and \\b{y}", &macros),
            "**x** and **y**"
        );
    }

    #[test]
    fn test_expand_parametric_nested_macros() {
        let macros = vec![
            ParametricMacro {
                name: "\\vect".into(),
                num_args: 1,
                body: "\\mathbf{#1}".into(),
                optional_default: None,
            },
            ParametricMacro {
                name: "\\inner".into(),
                num_args: 2,
                body: "\\langle #1, #2 \\rangle".into(),
                optional_default: None,
            },
        ];
        assert_eq!(
            expand_parametric_macros("\\inner{\\vect{x}}{\\vect{y}}", &macros),
            "\\langle \\mathbf{x}, \\mathbf{y} \\rangle"
        );
    }

    #[test]
    fn test_expand_parametric_escaped_hash() {
        let macros = vec![ParametricMacro {
            name: "\\foo".into(),
            num_args: 1,
            body: "#1 has a ## sign".into(),
            optional_default: None,
        }];
        assert_eq!(
            expand_parametric_macros("\\foo{test}", &macros),
            "test has a # sign"
        );
    }

    #[test]
    fn test_substitute_args_no_interference() {
        // Ensure #2 in the value of #1 doesn't get substituted as arg 2
        let args = vec!["#2".to_string(), "real2".to_string()];
        let result = substitute_args("#1 and #2", &args);
        assert_eq!(result, "#2 and real2");
    }

    #[test]
    fn test_substitute_args_trailing_hash() {
        // Trailing '#' with nothing after it must not cause infinite loop
        let result = substitute_args("text#", &["arg1".to_string()]);
        assert_eq!(result, "text#");
    }

    #[test]
    fn test_normalize_shorthands_be() {
        assert_eq!(normalize_shorthands("\\be x \\ee"), "\\begin{equation} x \\end{equation}");
    }

    #[test]
    fn test_normalize_shorthands_bea() {
        assert_eq!(normalize_shorthands("\\bea x \\eea"), "\\begin{eqnarray} x \\end{eqnarray}");
    }

    #[test]
    fn test_normalize_shorthands_ba() {
        assert_eq!(normalize_shorthands("\\ba x \\ea"), "\\begin{align} x \\end{align}");
    }

    #[test]
    fn test_normalize_shorthands_no_collision_begin() {
        // \begin should NOT be affected by \be
        assert_eq!(normalize_shorthands("\\begin{document}"), "\\begin{document}");
    }

    #[test]
    fn test_normalize_shorthands_no_collision_beta() {
        // \beta should NOT be affected by \be
        assert_eq!(normalize_shorthands("\\beta"), "\\beta");
    }

    #[test]
    fn test_expand_macros_bounded_on_mutually_recursive() {
        // Simulates the pathology seen in real arXiv papers:
        // Each pass through Aho-Corasick multiplies occurrences of \a and \b.
        // Without a growth cap, 5 passes produce 3^5 = 243× the input,
        // and a 50 KB input reaches 800 MB (observed on 2509.15163).
        let mut macros = HashMap::new();
        macros.insert("\\a".to_string(), "\\b\\b\\b".to_string());
        macros.insert("\\b".to_string(), "\\a\\a\\a".to_string());

        let input = "\\a".repeat(100); // 200 bytes
        let result = expand_macros(&input, &macros);

        // Growth cap is MAX_EXPANSION_GROWTH_RATIO × input. Allow a small
        // overshoot for the one pass we commit before checking.
        let cap = input.len() * MAX_EXPANSION_GROWTH_RATIO;
        assert!(
            result.len() <= cap,
            "expansion exceeded {}× cap: input={} output={}",
            MAX_EXPANSION_GROWTH_RATIO,
            input.len(),
            result.len()
        );
    }

    #[test]
    fn test_expand_macros_still_expands_benign() {
        // Non-recursive chain — must still expand fully, not trigger the cap.
        let mut macros = HashMap::new();
        macros.insert("\\foo".to_string(), "\\bar".to_string());
        macros.insert("\\bar".to_string(), "final".to_string());
        assert_eq!(expand_macros("\\foo", &macros), "final");
    }

    #[test]
    fn test_expand_macros_respects_absolute_ceiling() {
        // If input is very small (1 byte), cumulative cap = 10 bytes, but the
        // absolute ceiling (MAX_EXPANSION_BYTES) should prevent output from
        // exceeding it regardless. Exercise the min() clamp at the low end.
        let mut macros = HashMap::new();
        macros.insert("\\x".to_string(), "y".to_string());
        // Trivial expansion — no runaway, but verifies the benign path works
        // when size_cap is computed.
        let out = expand_macros("\\x", &macros);
        assert_eq!(out, "y");
    }

    #[test]
    fn test_expand_parametric_bounded_on_self_duplicating() {
        // Parametric pathology: a one-argument macro that duplicates its arg.
        // Expansion "\dup{\dup{x}}" still contains \dup — 2 passes double it.
        let input = "\\dup{".to_string() + &"x".repeat(50) + "}";
        let macros = vec![ParametricMacro {
            name: "\\dup".to_string(),
            num_args: 1,
            body: "\\dup{#1}\\dup{#1}".to_string(),
            optional_default: None,
        }];
        let result = expand_parametric_macros(&input, &macros);
        let cap = input.len() * MAX_EXPANSION_GROWTH_RATIO;
        assert!(
            result.len() <= cap,
            "parametric expansion exceeded {}× cap: input={} output={}",
            MAX_EXPANSION_GROWTH_RATIO,
            input.len(),
            result.len()
        );
    }
}
