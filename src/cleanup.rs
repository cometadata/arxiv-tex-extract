use crate::braces::{extract_command_arg, find_command_end};
use crate::environments::MATH_ENVS;
use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

/// Spacing commands to replace with a space (handled in cleanup).
const SPACING_COMMANDS: &[&str] = &[
    "quad", "qquad", "hfill", "vfill", "hspace", "vspace",
    "bigskip", "medskip", "smallskip", "noindent", "indent",
    "newline", "linebreak", "pagebreak", "newpage", "clearpage",
    "cleardoublepage",
    "hline", "cline", "toprule", "midrule", "bottomrule",
    "nonumber", "notag", "allowbreak",
];

/// Font switch commands to remove.
const FONT_SWITCHES: &[&str] = &[
    "it", "bf", "rm", "tt", "sc", "sl", "sf", "em",
    "normalfont", "bfseries", "itshape", "mdseries",
    "upshape", "slshape", "scshape", "sffamily",
    "rmfamily", "ttfamily",
];

// ---------------------------------------------------------------------------
// Pre-diacritic stripping (runs BEFORE convert_diacritics)
// ---------------------------------------------------------------------------

/// TeX spacing commands that take a dimension argument and collide with
/// single-letter diacritic accents (e.g. `\vskip` vs `\v`, `\kern` vs `\k`).
static SPACING_DIM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\\(?:vskip|hskip|vglue|hglue|kern|mkern|mskip|penalty|widowpenalty|clubpenalty|displaywidowpenalty)\s*=?\s*-?\.?\d[\d.]*\s*(?:cm|mm|pt|em|ex|in|bp|dd|cc|sp|pc|mu|fil(?:l(?:l)?)?)?"
    )
    .unwrap()
});

/// TeX length registers that take `=` + dimension.
static LENGTH_ASSIGN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\\(?:baselineskip|parskip|parindent|lineskip|lineskiplimit|topskip|abovedisplayskip|belowdisplayskip|abovedisplayshortskip|belowdisplayshortskip|columnsep|columnseprule|tabcolsep|arraycolsep|fboxsep|fboxrule|jot)\s*=?\s*-?\.?\d[\d.]*\s*(?:cm|mm|pt|em|ex|in|bp|dd|cc|sp|pc|mu|fil(?:l(?:l)?)?)?"
    )
    .unwrap()
});

/// No-op / structural commands that can be safely removed.
const NOOP_COMMANDS: &[&str] = &[
    "relax", "par", "empty", "protect", "raggedright", "raggedleft",
    "centering", "sloppy", "fussy", "samepage", "nopagebreak",
    "tableofcontents", "listoffigures", "listoftables",
    "makeatletter", "makeatother", "appendix",
];

/// Commands whose entire invocation (command + N braced args) should be removed.
/// (command_name, number_of_braced_args_to_consume)
const STRIP_WITH_ARGS: &[(&str, usize)] = &[
    ("pagestyle", 1),
    ("thispagestyle", 1),
    ("bibliographystyle", 1),
    ("nocite", 1),
    ("stepcounter", 1),
    ("theoremstyle", 1),
    ("enlargethispage", 1),
    ("setcounter", 2),
    ("addtocounter", 2),
    ("setlength", 2),
    ("addtolength", 2),
    ("numberwithin", 2),
    ("DeclareMathOperator", 2),
    ("DeclareMathOperator*", 2),
    ("newtheorem", 2),
    ("newtheorem*", 2),
    ("newenvironment", 3),
    ("renewenvironment", 3),
    ("addcontentsline", 3),
    ("xymatrix", 1),
    // Configuration commands whose args should not leak as text
    ("hypersetup", 1),
    ("definecolor", 3),
    ("colorlet", 2),
    ("lstset", 1),
    ("markboth", 2),
    ("markright", 1),
    ("DeclareRobustCommand", 2),
    ("pagenumbering", 1),
];

/// Strip commands that collide with diacritic patterns.
///
/// Must run BEFORE `convert_diacritics()` so that commands like `\vskip`,
/// `\bf`, `\baselineskip` are gone before bare-accent regexes fire.
pub fn strip_pre_diacritic_commands(text: &str) -> String {
    let mut result = text.to_string();

    result = SPACING_DIM_RE.replace_all(&result, " ").to_string();
    result = LENGTH_ASSIGN_RE.replace_all(&result, " ").to_string();

    for cmd in FONT_SWITCHES {
        result = remove_font_switch(&result, cmd);
    }

    for cmd in NOOP_COMMANDS {
        result = remove_font_switch(&result, cmd);
    }

    for &(cmd, n_args) in STRIP_WITH_ARGS {
        result = strip_command_with_args(&result, cmd, n_args);
    }

    result
}

/// Remove `\cmd{arg1}{arg2}...` consuming exactly `n_args` braced groups.
fn strip_command_with_args(text: &str, cmd_name: &str, n_args: usize) -> String {
    let pattern = format!("\\{}", cmd_name);
    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;

    let mut search_start = 0;
    while let Some(pos) = text[search_start..].find(&pattern) {
        let abs_pos = search_start + pos;
        let after = abs_pos + pattern.len();
        let bytes = text.as_bytes();

        if !cmd_name.ends_with('*') && after < bytes.len() && bytes[after].is_ascii_alphabetic() {
            search_start = after;
            continue;
        }

        let mut cursor = after;
        let mut ok = true;
        for _ in 0..n_args {
            if let Some((_, _, after_close)) = extract_command_arg(text, cursor) {
                cursor = after_close;
            } else {
                ok = false;
                break;
            }
        }

        if ok {
            result.push_str(&text[last_end..abs_pos]);
            last_end = cursor;
            search_start = cursor;
        } else {
            search_start = after;
        }
    }

    result.push_str(&text[last_end..]);
    result
}

// ---------------------------------------------------------------------------
// Math-region detection (used to protect math from cleanup transforms)
// ---------------------------------------------------------------------------

/// Identify byte ranges of math regions: `$...$`, `$$...$$`, `\(...\)`, `\[...\]`,
/// and named math environments like `\begin{equation}...\end{equation}`.
/// Returns a sorted, non-overlapping `Vec<(start, end)>` (end is exclusive).
fn find_math_regions(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut regions = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'$' => {
                    i += 2;
                    continue;
                }
                b'(' => {
                    let start = i;
                    i += 2;
                    while i + 1 < bytes.len() {
                        if bytes[i] == b'\\' && bytes[i + 1] == b')' {
                            regions.push((start, i + 2));
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                    continue;
                }
                b'[' => {
                    let start = i;
                    i += 2;
                    while i + 1 < bytes.len() {
                        if bytes[i] == b'\\' && bytes[i + 1] == b']' {
                            regions.push((start, i + 2));
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                    continue;
                }
                _ => {
                    i += 2;
                    continue;
                }
            }
        }
        if bytes[i] == b'$' {
            let start = i;
            let double = i + 1 < bytes.len() && bytes[i + 1] == b'$';
            let delim_len = if double { 2 } else { 1 };
            i += delim_len;
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'$' {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'$' {
                    if double {
                        if i + 1 < bytes.len() && bytes[i + 1] == b'$' {
                            regions.push((start, i + 2));
                            i += 2;
                            break;
                        }
                    } else {
                        regions.push((start, i + 1));
                        i += 1;
                        break;
                    }
                }
                i += 1;
            }
            continue;
        }
        i += 1;
    }

    // Also detect named math environments (safety net for unconverted envs).
    // Uses the canonical MATH_ENVS list from environments.rs.
    for env in MATH_ENVS {
        let begin_pat = format!("\\begin{{{env}}}");
        let end_pat = format!("\\end{{{env}}}");
        let mut search = 0;
        while let Some(bp) = text[search..].find(&begin_pat) {
            let abs_begin = search + bp;
            if let Some(ep) = text[abs_begin..].find(&end_pat) {
                let abs_end = abs_begin + ep + end_pat.len();
                regions.push((abs_begin, abs_end));
                search = abs_end;
            } else {
                break;
            }
        }
    }

    // Sort and merge overlapping regions
    regions.sort_by_key(|&(s, _)| s);
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(regions.len());
    for (s, e) in regions {
        if let Some(last) = merged.last_mut() {
            if s <= last.1 {
                last.1 = last.1.max(e);
                continue;
            }
        }
        merged.push((s, e));
    }
    merged
}

/// Check if byte position `pos` falls inside any math region (binary search).
fn in_math(pos: usize, regions: &[(usize, usize)]) -> bool {
    regions
        .binary_search_by(|&(start, end)| {
            if pos < start {
                std::cmp::Ordering::Greater
            } else if pos >= end {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// Math commands to preserve (not strip as orphan commands).
static MATH_KEEP: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "frac", "sum", "int", "prod", "sqrt", "lim", "log", "ln",
        "sin", "cos", "tan", "sec", "csc", "cot",
        "exp", "min", "max", "sup", "inf", "arg", "det", "dim",
        "gcd", "hom", "ker", "deg", "Pr", "oint", "iint", "iiint",
        "bigcup", "bigcap", "bigoplus", "bigotimes", "coprod",
        "mathbb", "mathscr", "mathcal", "mathrm", "mathbf",
    ]
    .into_iter()
    .collect()
});

static LINE_BREAK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\\\(\[[^\]]*\])?").unwrap());

static PHANTOM_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\[hv]?phantom\{[^{}]*\}").unwrap());

static RULE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\rule\s*(?:\[[^\]]*\])?\s*\{[^{}]*\}\{[^{}]*\}").unwrap());

static NEWCOMMAND_LEAKED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\\(?:re)?newcommand\*?\{[^}]*\}(?:\[[^\]]*\])?\{(?:[^{}]|\{[^{}]*\})*\}").unwrap()
});

static DEF_LEAKED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\def\\[a-zA-Z]+\{(?:[^{}]|\{[^{}]*\})*\}").unwrap());

static MULTI_SPACE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^\S\n]+").unwrap());

static MULTI_NEWLINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());

/// Strip stray backslashes: a `\` NOT followed by an ASCII letter, another `\`, or `$`.
fn strip_stray_backslashes(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut result = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            if i + 1 < bytes.len() {
                let next = bytes[i + 1];
                if next.is_ascii_alphabetic() || next == b'\\' || next == b'$' {
                    result.push('\\');
                }
            }
            i += 1;
        } else {
            if bytes[i] < 128 {
                result.push(bytes[i] as char);
            } else {
                let c = &text[i..];
                if let Some(ch) = c.chars().next() {
                    result.push(ch);
                    i += ch.len_utf8();
                    continue;
                }
            }
            i += 1;
        }
    }
    result
}

/// Reflow paragraphs: join single-newline-separated lines within paragraphs.
/// Preserves: paragraph breaks (\n\n), headings (#), list items (- ), display math ($$),
/// bold labels (**), and lines that are very short (likely intentional breaks).
fn reflow_paragraphs(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let lines: Vec<&str> = text.split('\n').collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        result.push_str(line);

        if i + 1 >= lines.len() {
            break;
        }

        let next = lines[i + 1];

        let should_join = !line.is_empty()
            && !next.is_empty()
            && !line.starts_with('#')
            && !next.starts_with('#')
            && !line.starts_with("- ")
            && !next.starts_with("- ")
            && !line.starts_with("$$")
            && !next.starts_with("$$")
            && !next.starts_with("**")
            && !line.ends_with("$$");

        if should_join {
            result.push(' ');
        } else {
            result.push('\n');
        }

        i += 1;
    }

    result
}

/// Final cleanup pass.
pub fn cleanup(text: &str) -> String {
    let mut result = text.to_string();

    for cmd in SPACING_COMMANDS {
        result = remove_spacing_command(&result, cmd);
    }

    result = LINE_BREAK_RE.replace_all(&result, "\n").to_string();
    result = PHANTOM_RE.replace_all(&result, "").to_string();
    result = RULE_RE.replace_all(&result, "").to_string();

    // Second pass catches font switches that leak through nested unwrapping
    for cmd in FONT_SWITCHES {
        result = remove_font_switch(&result, cmd);
    }

    result = result.replace("\\protect", "");
    result = NEWCOMMAND_LEAKED_RE.replace_all(&result, "").to_string();
    result = DEF_LEAKED_RE.replace_all(&result, "").to_string();

    for _ in 0..2 {
        result = unwrap_generic_commands(&result);
    }

    result = strip_grouping_braces(&result);
    result = strip_orphan_commands(&result);
    result = strip_stray_backslashes(&result);
    result = MULTI_SPACE_RE.replace_all(&result, " ").to_string();
    result = MULTI_NEWLINE_RE.replace_all(&result, "\n\n").to_string();

    result = result
        .lines()
        .map(|line| line.trim())
        .collect::<Vec<_>>()
        .join("\n");

    result = result.trim().to_string();
    result = reflow_paragraphs(&result);

    result
}

/// Remove a spacing command like `\quad`, `\hspace{1cm}`, etc.
fn remove_spacing_command(text: &str, cmd_name: &str) -> String {
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

        result.push_str(&text[last_end..abs_pos]);
        result.push(' ');

        let mut skip_to = after;
        if skip_to < bytes.len() && bytes[skip_to] == b'*' {
            skip_to += 1;
        }
        if skip_to < bytes.len() && bytes[skip_to] == b'{' {
            if let Some((_, ce)) = crate::braces::find_braced_group(text, skip_to) {
                skip_to = ce + 1;
            }
        }

        last_end = skip_to;
        search_start = skip_to;
    }

    result.push_str(&text[last_end..]);
    result
}

/// Remove a standalone font switch command like \it, \bf.
fn remove_font_switch(text: &str, cmd_name: &str) -> String {
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

        result.push_str(&text[last_end..abs_pos]);
        last_end = after;
        search_start = after;
    }

    result.push_str(&text[last_end..]);
    result
}

/// Unwrap generic \command{content} → content.
/// Scans left-to-right, finds \command patterns, extracts braced args.
/// Skips commands whose start position falls inside a math region.
fn unwrap_generic_commands(text: &str) -> String {
    let math_regions = find_math_regions(text);
    let mut result = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_alphabetic() {
            if !in_math(i, &math_regions) {
                let cmd_end = find_command_end(text, i);

                if let Some((cs, ce, after_close)) = extract_command_arg(text, cmd_end) {
                    result.push_str(&text[cs..ce]);
                    i = after_close;
                    continue;
                }
            }
        }

        if bytes[i] < 128 {
            result.push(bytes[i] as char);
            i += 1;
        } else {
            let c = &text[i..];
            if let Some(ch) = c.chars().next() {
                result.push(ch);
                i += ch.len_utf8();
            } else {
                i += 1;
            }
        }
    }

    result
}

/// Strip grouping braces `{content}` → `content`.
/// Only strips braces that are not preceded by a backslash.
/// Skips braces inside math regions.
fn strip_grouping_braces(text: &str) -> String {
    let math_regions = find_math_regions(text);
    let mut result = String::with_capacity(text.len());
    let mut prev_was_backslash = false;
    let mut byte_pos = 0;

    for ch in text.chars() {
        if (ch == '{' || ch == '}') && !prev_was_backslash && !in_math(byte_pos, &math_regions) {
            prev_was_backslash = false;
            byte_pos += ch.len_utf8();
            continue;
        }
        prev_was_backslash = ch == '\\';
        result.push(ch);
        byte_pos += ch.len_utf8();
    }

    result
}

/// Strip orphan \commands that aren't math operators.
fn strip_orphan_commands(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut result = String::with_capacity(text.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_alphabetic() {
            let name_start = i + 1;
            let mut name_end = name_start;
            while name_end < bytes.len() && bytes[name_end].is_ascii_alphabetic() {
                name_end += 1;
            }
            let mut cmd_end = name_end;
            if cmd_end < bytes.len() && bytes[cmd_end] == b'*' {
                cmd_end += 1;
            }

            let cmd_name = &text[name_start..name_end];

            if cmd_end < bytes.len()
                && (bytes[cmd_end] == b'{' || bytes[cmd_end].is_ascii_alphabetic())
            {
                result.push_str(&text[i..cmd_end]);
                i = cmd_end;
                continue;
            }

            if MATH_KEEP.contains(cmd_name) {
                result.push_str(&text[i..cmd_end]);
            } else {
                result.push(' ');
            }
            i = cmd_end;
        } else {
            if bytes[i] < 128 {
                result.push(bytes[i] as char);
            } else {
                let c = &text[i..];
                if let Some(ch) = c.chars().next() {
                    result.push(ch);
                    i += ch.len_utf8();
                    continue;
                }
            }
            i += 1;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spacing_commands() {
        assert_eq!(cleanup(r"\quad text"), "text");
        assert_eq!(cleanup(r"\hspace{1cm}text"), "text");
    }

    #[test]
    fn test_line_break() {
        let result = cleanup(r"line1\\line2");
        assert!(result.contains("line1"));
        assert!(result.contains("line2"));
    }

    #[test]
    fn test_phantom_removed() {
        assert_eq!(cleanup(r"\phantom{x}text"), "text");
    }

    #[test]
    fn test_font_switch() {
        let result = cleanup(r"{\it italic}");
        assert_eq!(result, "italic");
    }

    #[test]
    fn test_protect_removed() {
        let result = cleanup(r"\protect\foo{bar}");
        assert_eq!(result, "bar");
    }

    #[test]
    fn test_generic_unwrap() {
        let result = cleanup(r"\unknown{content}");
        assert_eq!(result, "content");
    }

    #[test]
    fn test_grouping_braces() {
        let result = cleanup("some {grouped} text");
        assert_eq!(result, "some grouped text");
    }

    #[test]
    fn test_orphan_command_removed() {
        let result = cleanup(r"\somecommand rest");
        assert!(result.contains("rest"));
        assert!(!result.contains("somecommand"));
    }

    #[test]
    fn test_math_command_preserved() {
        let result = cleanup(r"\frac rest");
        assert!(result.contains("\\frac"));
    }

    #[test]
    fn test_whitespace_collapse() {
        let result = cleanup("a   b   c");
        assert_eq!(result, "a b c");
    }

    #[test]
    fn test_blank_line_collapse() {
        let result = cleanup("a\n\n\n\nb");
        assert_eq!(result, "a\n\nb");
    }

    #[test]
    fn test_leaked_newcommand() {
        let result = cleanup(r"\newcommand{\foo}{bar} text");
        assert_eq!(result.trim(), "text");
    }

    #[test]
    fn test_math_subscript_preserved() {
        let result = cleanup(r"$X_{max}$");
        assert_eq!(result, r"$X_{max}$");
    }

    #[test]
    fn test_math_frac_preserved() {
        let result = cleanup(r"$\frac{1}{2}$");
        assert_eq!(result, r"$\frac{1}{2}$");
    }

    #[test]
    fn test_math_sqrt_preserved() {
        let result = cleanup(r"$\sqrt{x+1}$");
        assert_eq!(result, r"$\sqrt{x+1}$");
    }

    #[test]
    fn test_display_math_frac_preserved() {
        let result = cleanup(r"$$\frac{a}{b}$$");
        assert_eq!(result, r"$$\frac{a}{b}$$");
    }

    #[test]
    fn test_non_math_still_unwraps() {
        let result = cleanup(r"\unknown{content}");
        assert_eq!(result, "content");
    }

    #[test]
    fn test_inline_math_paren_content_preserved() {
        // \( and \) lose backslashes via strip_stray_backslashes,
        // but the math content inside must be preserved
        let result = cleanup(r"\(\frac{x}{y}\)");
        assert!(result.contains(r"\frac{x}{y}"));
    }

    #[test]
    fn test_find_math_regions_basic() {
        let regions = find_math_regions(r"text $x_{max}$ more $$\frac{a}{b}$$");
        assert_eq!(regions.len(), 2);
    }

    #[test]
    fn test_escaped_dollar_not_math() {
        let regions = find_math_regions(r"costs \$5 not math");
        assert_eq!(regions.len(), 0);
    }

    #[test]
    fn test_find_math_regions_named_env() {
        let input = r"text \begin{equation}x^2\end{equation} more";
        let regions = find_math_regions(input);
        assert_eq!(regions.len(), 1);
        let (s, e) = regions[0];
        assert!(input[s..e].contains("x^2"));
    }

    #[test]
    fn test_find_math_regions_merge_overlapping() {
        // $$ inside a named env should merge into one region
        let input = r"\begin{equation}$$x$$\end{equation}";
        let regions = find_math_regions(input);
        assert_eq!(regions.len(), 1, "overlapping regions should merge");
    }

    #[test]
    fn test_reflow_joins_paragraph_lines() {
        let input = "This is a line\nthat continues here.";
        let result = reflow_paragraphs(input);
        assert_eq!(result, "This is a line that continues here.");
    }

    #[test]
    fn test_reflow_preserves_paragraph_breaks() {
        let input = "First paragraph.\n\nSecond paragraph.";
        let result = reflow_paragraphs(input);
        assert_eq!(result, "First paragraph.\n\nSecond paragraph.");
    }

    #[test]
    fn test_reflow_preserves_headings() {
        let input = "Some text.\n## Heading\nMore text.";
        let result = reflow_paragraphs(input);
        assert!(result.contains("\n## Heading\n"), "heading preserved: {result}");
    }

    #[test]
    fn test_reflow_preserves_list_items() {
        let input = "- Item one\n- Item two";
        let result = reflow_paragraphs(input);
        assert_eq!(result, "- Item one\n- Item two");
    }

    #[test]
    fn test_reflow_preserves_display_math() {
        let input = "Text before\n$$\nE = mc^2\n$$\nText after";
        let result = reflow_paragraphs(input);
        assert!(result.contains("\n$$\n"), "display math preserved: {result}");
    }

    #[test]
    fn test_pre_diacritic_vskip() {
        let result = strip_pre_diacritic_commands(r"\vskip 1cm some text");
        assert!(result.contains("some text"));
        assert!(!result.contains("vskip"));
    }

    #[test]
    fn test_pre_diacritic_baselineskip() {
        let result = strip_pre_diacritic_commands(r"\baselineskip=16pt some text");
        assert!(result.contains("some text"));
        assert!(!result.contains("baselineskip"));
    }

    #[test]
    fn test_pre_diacritic_font_switches() {
        let result = strip_pre_diacritic_commands(r"{\bf bold text}");
        assert!(result.contains("bold text"));
        assert!(!result.contains("\\bf"));
    }

    #[test]
    fn test_pre_diacritic_noop() {
        let result = strip_pre_diacritic_commands(r"\relax\par some text");
        assert!(result.contains("some text"));
        assert!(!result.contains("relax"));
        assert!(!result.contains("\\par"));
    }

    #[test]
    fn test_pre_diacritic_pagestyle() {
        let result = strip_pre_diacritic_commands(r"\pagestyle{plain} text");
        assert!(result.contains("text"));
        assert!(!result.contains("pagestyle"));
        assert!(!result.contains("plain"));
    }

    #[test]
    fn test_pre_diacritic_setcounter() {
        let result = strip_pre_diacritic_commands(r"\setcounter{page}{1} text");
        assert!(result.contains("text"));
        assert!(!result.contains("setcounter"));
    }

    #[test]
    fn test_pre_diacritic_kern() {
        let result = strip_pre_diacritic_commands(r"\kern 3pt text");
        assert!(result.contains("text"));
        assert!(!result.contains("kern"));
    }

    #[test]
    fn test_pre_diacritic_penalty() {
        let result = strip_pre_diacritic_commands(r"\penalty 10000 text");
        assert!(result.contains("text"));
        assert!(!result.contains("penalty"));
    }

    #[test]
    fn test_addcontentsline_stripped() {
        let result = strip_pre_diacritic_commands(r"\addcontentsline{toc}{section}{Intro} text");
        assert!(result.contains("text"));
        assert!(!result.contains("addcontentsline"));
        assert!(!result.contains("toc"));
    }
}
