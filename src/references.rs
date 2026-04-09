use crate::braces::{extract_command_arg, skip_optional_arg};
use crate::symbols::CommandReplacer;
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

static LABEL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\label\{[^{}]*\}").unwrap());

/// Common astronomy / physics journal abbreviation macros.
static JOURNAL_MACROS: LazyLock<CommandReplacer> = LazyLock::new(|| {
    CommandReplacer::new(&[
        ("\\aap", "A&A"),
        ("\\aapr", "A&A Rev."),
        ("\\aaps", "A&AS"),
        ("\\apj", "ApJ"),
        ("\\apjl", "ApJL"),
        ("\\apjs", "ApJS"),
        ("\\apss", "Ap&SS"),
        ("\\mnras", "MNRAS"),
        ("\\prl", "Phys. Rev. Lett."),
        ("\\prd", "Phys. Rev. D"),
        ("\\nat", "Nature"),
        ("\\sci", "Science"),
        ("\\araa", "ARA&A"),
        ("\\pasp", "PASP"),
        ("\\pasj", "PASJ"),
        ("\\jcap", "JCAP"),
        ("\\solphys", "Sol. Phys."),
        ("\\icarus", "Icarus"),
        ("\\planss", "Planet. Space Sci."),
        ("\\procspie", "Proc. SPIE"),
    ])
});

/// Scan `\bibitem{key}` entries in document order and build a key → display text map.
/// Detects author-year mode: if any `\bibitem[label]{key}` has a non-numeric optional
/// label (e.g., `Smith et al., 2020`), uses those labels as display text.
/// Otherwise falls back to sequential numeric indices.
fn build_cite_key_map(text: &str) -> HashMap<String, String> {
    let pattern = "\\bibitem";

    let mut entries: Vec<(String, Option<String>)> = Vec::new();
    let mut search_start = 0;
    while let Some(pos) = text[search_start..].find(pattern) {
        let abs_pos = search_start + pos;
        let after = abs_pos + pattern.len();
        let bytes = text.as_bytes();

        if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
            search_start = after;
            continue;
        }

        let after_opt = skip_optional_arg(text, after);
        let opt_label = if after_opt > after {
            let bytes = text.as_bytes();
            let mut i = after;
            while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\n') {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'[' {
                Some(text[i + 1..after_opt - 1].to_string())
            } else {
                None
            }
        } else {
            None
        };

        if let Some((cs, ce, after_close)) = extract_command_arg(text, after_opt) {
            let key = text[cs..ce].to_string();
            entries.push((key, opt_label));
            search_start = after_close;
        } else {
            search_start = after;
        }
    }

    let author_year = entries.iter().any(|(_, opt)| {
        opt.as_ref().map_or(false, |label| {
            !label.trim().is_empty() && !label.trim().chars().all(|c| c.is_ascii_digit())
        })
    });

    let mut map = HashMap::new();
    for (i, (key, opt_label)) in entries.into_iter().enumerate() {
        let display = if author_year {
            opt_label.unwrap_or_else(|| (i + 1).to_string())
        } else {
            (i + 1).to_string()
        };
        map.entry(key).or_insert(display);
    }

    map
}

/// Scan `\label{key}` entries in document order and build a key → display number map.
/// Keys are split on `:` for type prefix; per-prefix counters (fig, eq, sec, tab, thm, etc.).
fn build_label_map(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut prefix_counters: HashMap<String, usize> = HashMap::new();
    let mut generic_counter = 0usize;
    let pattern = "\\label";

    let mut search_start = 0;
    while let Some(pos) = text[search_start..].find(pattern) {
        let abs_pos = search_start + pos;
        let after = abs_pos + pattern.len();
        let bytes = text.as_bytes();

        if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
            search_start = after;
            continue;
        }

        if let Some((cs, ce, after_close)) = extract_command_arg(text, after) {
            let key = text[cs..ce].to_string();
            let display = if let Some(colon_pos) = key.find(':') {
                let prefix = &key[..colon_pos];
                let counter = prefix_counters.entry(prefix.to_string()).or_insert(0);
                *counter += 1;
                counter.to_string()
            } else {
                generic_counter += 1;
                generic_counter.to_string()
            };
            map.entry(key).or_insert(display);
            search_start = after_close;
        } else {
            search_start = after;
        }
    }

    map
}

/// Handle citation and reference commands.
/// `section_labels` provides section-number mappings from the structure pass.
pub fn convert_references(text: &str, section_labels: &HashMap<String, String>) -> String {
    let mut result = text.to_string();

    result = JOURNAL_MACROS.replace_all(&result);

    let cite_map = build_cite_key_map(&result);
    let mut label_map = build_label_map(&result);
    for (k, v) in section_labels {
        label_map.insert(k.clone(), v.clone());
    }

    result = replace_cite_commands(&result, &cite_map);

    for cmd in &["ref", "eqref", "pageref", "autoref", "cref", "Cref", "nameref"] {
        result = replace_ref_command(&result, cmd, &label_map);
    }

    result = LABEL_RE.replace_all(&result, "").to_string();

    result = replace_url_command(&result);

    result = replace_href_command(&result);

    result = replace_hyperref_command(&result);

    result = convert_bibitems(&result, &cite_map);

    result
}

/// Replace \cite, \citep, \citet, \citealp, \citealt, \citeauthor,
/// \citeyear, \citenum, \cite* variants with [N] or [key] if not in map.
fn replace_cite_commands(text: &str, cite_map: &HashMap<String, String>) -> String {
    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;

    let mut search_start = 0;
    while let Some(pos) = text[search_start..].find("\\cite") {
        let abs_pos = search_start + pos;
        let mut cmd_end = abs_pos + 5; // after "\cite"
        let bytes = text.as_bytes();

        let suffix_start = cmd_end;
        while cmd_end < bytes.len() && bytes[cmd_end].is_ascii_alphabetic() {
            cmd_end += 1;
        }
        let suffix = &text[suffix_start..cmd_end];
        let valid_suffixes = [
            "", "p", "t", "alp", "alt", "author", "year", "num",
        ];
        if !valid_suffixes.contains(&suffix) {
            search_start = cmd_end;
            continue;
        }

        if cmd_end < bytes.len() && bytes[cmd_end] == b'*' {
            cmd_end += 1;
        }

        // Skip optional arguments (natbib allows two: \citep[see][p.~5]{key})
        let after_opt = skip_optional_arg(text, cmd_end);
        let after_opt = skip_optional_arg(text, after_opt);

        if let Some((cs, ce, after_close)) = extract_command_arg(text, after_opt) {
            result.push_str(&text[last_end..abs_pos]);
            result.push('[');
            let keys_str = &text[cs..ce];
            let resolved: Vec<String> = keys_str
                .split(',')
                .map(|k| {
                    let k = k.trim();
                    match cite_map.get(k) {
                        Some(display) => display.clone(),
                        None => k.to_string(),
                    }
                })
                .collect();
            result.push_str(&resolved.join(","));
            result.push(']');
            last_end = after_close;
            search_start = after_close;
        } else {
            search_start = cmd_end;
        }
    }

    result.push_str(&text[last_end..]);
    result
}

/// Replace \ref{label} → resolved number (or raw label if not found).
fn replace_ref_command(text: &str, cmd_name: &str, label_map: &HashMap<String, String>) -> String {
    let pattern = format!("\\{}", cmd_name);
    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;

    let mut search_start = 0;
    while let Some(pos) = text[search_start..].find(&pattern) {
        let abs_pos = search_start + pos;
        let after = abs_pos + pattern.len();
        let bytes = text.as_bytes();

        if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
            search_start = after;
            continue;
        }

        if let Some((cs, ce, after_close)) = extract_command_arg(text, after) {
            result.push_str(&text[last_end..abs_pos]);
            let key = &text[cs..ce];
            match label_map.get(key) {
                Some(display) => result.push_str(display),
                None => result.push_str(key),
            }
            last_end = after_close;
            search_start = after_close;
        } else {
            search_start = after;
        }
    }

    result.push_str(&text[last_end..]);
    result
}

/// Replace \url{...} → URL text.
fn replace_url_command(text: &str) -> String {
    let pattern = "\\url";
    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;

    let mut search_start = 0;
    while let Some(pos) = text[search_start..].find(pattern) {
        let abs_pos = search_start + pos;
        let after = abs_pos + pattern.len();
        let bytes = text.as_bytes();

        if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
            search_start = after;
            continue;
        }

        if let Some((cs, ce, after_close)) = extract_command_arg(text, after) {
            result.push_str(&text[last_end..abs_pos]);
            result.push_str(&text[cs..ce]);
            last_end = after_close;
            search_start = after_close;
        } else {
            search_start = after;
        }
    }

    result.push_str(&text[last_end..]);
    result
}

/// Replace \bibitem[label]{key} → \n[N] (clean bibliography entry separator).
fn convert_bibitems(text: &str, cite_map: &HashMap<String, String>) -> String {
    let pattern = "\\bibitem";
    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;

    let mut search_start = 0;
    while let Some(pos) = text[search_start..].find(pattern) {
        let abs_pos = search_start + pos;
        let after = abs_pos + pattern.len();
        let bytes = text.as_bytes();

        // Boundary check: not followed by alpha (e.g., \bibitemize)
        if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
            search_start = after;
            continue;
        }

        result.push_str(&text[last_end..abs_pos]);
        result.push('\n');

        let after_opt = skip_optional_arg(text, after);

        if let Some((cs, ce, after_close)) = extract_command_arg(text, after_opt) {
            let key = &text[cs..ce];
            result.push('[');
            match cite_map.get(key) {
                Some(display) => result.push_str(display),
                None => result.push_str(key),
            }
            result.push_str("] ");
            last_end = after_close;
            search_start = after_close;
        } else {
            last_end = after_opt;
            search_start = after_opt;
        }
    }

    result.push_str(&text[last_end..]);
    result
}

/// Replace \hyperref[label]{text} → text (extract braced argument, skip optional label).
fn replace_hyperref_command(text: &str) -> String {
    let pattern = "\\hyperref";
    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;

    let mut search_start = 0;
    while let Some(pos) = text[search_start..].find(pattern) {
        let abs_pos = search_start + pos;
        let after = abs_pos + pattern.len();
        let bytes = text.as_bytes();

        if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
            search_start = after;
            continue;
        }

        let after_opt = skip_optional_arg(text, after);
        if let Some((cs, ce, after_close)) = extract_command_arg(text, after_opt) {
            result.push_str(&text[last_end..abs_pos]);
            result.push_str(&text[cs..ce]);
            last_end = after_close;
            search_start = after_close;
        } else {
            search_start = after;
        }
    }

    result.push_str(&text[last_end..]);
    result
}

/// Replace \href{url}{text} → text (extract second argument).
fn replace_href_command(text: &str) -> String {
    let pattern = "\\href";
    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;

    let mut search_start = 0;
    while let Some(pos) = text[search_start..].find(pattern) {
        let abs_pos = search_start + pos;
        let after = abs_pos + pattern.len();
        let bytes = text.as_bytes();

        if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
            search_start = after;
            continue;
        }

        if let Some((_, _, after_first)) = extract_command_arg(text, after) {
            if let Some((cs2, ce2, after_second)) = extract_command_arg(text, after_first) {
                result.push_str(&text[last_end..abs_pos]);
                result.push_str(&text[cs2..ce2]);
                last_end = after_second;
                search_start = after_second;
                continue;
            }
        }
        search_start = after;
    }

    result.push_str(&text[last_end..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cr(text: &str) -> String {
        convert_references(text, &HashMap::new())
    }

    // --- Citation resolution with numeric indices ---

    #[test]
    fn test_cite_with_bibitem_resolves_to_number() {
        let input = r"\cite{smith2020} text \bibitem{smith2020} A reference.";
        let result = cr(input);
        assert!(result.contains("[1]"), "cite should resolve to [1]: {result}");
    }

    #[test]
    fn test_multi_cite_resolves() {
        let input = r"\cite{a,b} \bibitem{a} A. \bibitem{b} B.";
        let result = cr(input);
        assert!(result.contains("[1,2]"), "multi-cite should resolve: {result}");
    }

    #[test]
    fn test_cite_unknown_key_fallback() {
        let input = r"\cite{unknown}";
        let result = cr(input);
        assert_eq!(result, "[unknown]");
    }

    #[test]
    fn test_citep() {
        let input = r"\citep{jones2021} \bibitem{jones2021} J.";
        let result = cr(input);
        assert!(result.contains("[1]"), "citep should resolve: {result}");
    }

    #[test]
    fn test_cite_with_optional() {
        let input = r"\cite[p.~42]{smith2020} \bibitem{smith2020} S.";
        let result = cr(input);
        assert!(result.contains("[1]"), "cite with optional should resolve: {result}");
    }

    #[test]
    fn test_bibitem_displays_author_year_label() {
        // Non-numeric optional label triggers author-year mode
        let input = r"\bibitem[Smith2020]{smith2020} A reference.";
        let result = cr(input);
        assert!(result.contains("[Smith2020]"), "bibitem should display author-year label: {result}");
        assert!(result.contains("A reference."));
    }

    #[test]
    fn test_bibitem_sequential_numbering() {
        let input = r"\bibitem{a} First. \bibitem{b} Second.";
        let result = cr(input);
        assert!(result.contains("[1]"), "first bibitem should be [1]: {result}");
        assert!(result.contains("[2]"), "second bibitem should be [2]: {result}");
    }

    // --- Cross-reference resolution ---

    #[test]
    fn test_ref_with_label_resolves() {
        let input = r"\label{fig:x} \ref{fig:x}";
        let result = cr(input);
        assert!(result.contains("1"), "ref should resolve to 1: {result}");
    }

    #[test]
    fn test_eqref_resolves() {
        let input = r"\label{eq:a} \eqref{eq:a}";
        let result = cr(input);
        assert!(result.contains("1"), "eqref should resolve: {result}");
    }

    #[test]
    fn test_ref_unknown_label_fallback() {
        let result = cr(r"See \ref{fig:unknown}");
        assert_eq!(result, "See fig:unknown");
    }

    #[test]
    fn test_label_removed() {
        let result = cr(r"\label{sec:intro} Introduction");
        assert_eq!(result, " Introduction");
    }

    // --- URL / href ---

    #[test]
    fn test_url() {
        let result = cr(r"\url{https://example.com}");
        assert_eq!(result, "https://example.com");
    }

    #[test]
    fn test_href() {
        let result = cr(r"\href{https://example.com}{click here}");
        assert_eq!(result, "click here");
    }

    // --- Boundary checks ---

    #[test]
    fn test_citation_not_matching_longer() {
        let result = cr(r"\citation{x}");
        assert_eq!(result, r"\citation{x}");
    }

    #[test]
    fn test_bibitem_boundary() {
        let result = cr(r"\bibitemize{x}");
        assert_eq!(result, r"\bibitemize{x}");
    }

    // --- Journal macros ---

    #[test]
    fn test_journal_macro_apj() {
        let result = cr(r"\apj, 123, 456");
        assert!(result.contains("ApJ"), "apj should expand: {result}");
    }

    #[test]
    fn test_journal_macro_mnras() {
        let result = cr(r"\mnras, 500, 100");
        assert!(result.contains("MNRAS"), "mnras should expand: {result}");
    }

    #[test]
    fn test_journal_macro_prl() {
        let result = cr(r"\prl, 100, 200");
        assert!(result.contains("Phys. Rev. Lett."), "prl should expand: {result}");
    }

    #[test]
    fn test_citep_two_optional_args() {
        // natbib: \citep[see][p.~5]{key} has two optional args
        let input = r"\citep[see][p.~5]{smith2020} \bibitem{smith2020} S.";
        let result = cr(input);
        assert!(result.contains("[1]"), "citep with two optional args should resolve: {result}");
    }

    #[test]
    fn test_author_year_citation_style() {
        let input = r"\cite{smith2020} \bibitem[Smith et al., 2020]{smith2020} S.";
        let result = cr(input);
        assert!(result.contains("[Smith et al., 2020]"), "author-year cite: {result}");
    }

    #[test]
    fn test_numeric_optional_stays_numeric() {
        // When optional args are just numbers, stay in numeric mode
        let input = r"\cite{a} \bibitem[1]{a} A. \bibitem[2]{b} B.";
        let result = cr(input);
        assert!(result.contains("[1]"), "numeric mode cite: {result}");
    }

    #[test]
    fn test_hyperref() {
        let result = cr(r"\hyperref[sec:intro]{Introduction}");
        assert_eq!(result, "Introduction");
    }

    #[test]
    fn test_hyperref_no_optional() {
        let result = cr(r"\hyperref{text}");
        assert_eq!(result, "text");
    }
}
