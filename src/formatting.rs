use crate::braces::{extract_command_arg, extract_command_arg_tolerant, skip_optional_arg};
use regex::Regex;
use std::sync::LazyLock;

/// Commands that wrap content in emphasis: \emph{x} → *x*
const EMPH_COMMANDS: &[&str] = &["emph", "textit", "textsl"];

/// Commands that wrap content in bold: \textbf{x} → **x**
const BOLD_COMMANDS: &[&str] = &["textbf", "mathbf", "boldsymbol", "bm"];

/// Commands that unwrap to just their content: \textrm{x} → x
const UNWRAP_COMMANDS: &[&str] = &[
    "textrm", "textsc", "texttt", "textsf", "text", "textnormal",
    "mathrm", "mathit", "mathcal", "mathbb", "mathfrak", "mathsf",
    "operatorname",
    "cal", "Bbb",
    "hat", "tilde", "widetilde", "widehat", "bar", "overline",
    "underline", "vec", "dot", "ddot", "acute", "grave", "check", "breve",
    "mbox", "hbox", "vbox", "fbox",
    "country",
    "citenamefont", "bibnamefont", "bibfnamefont",
    "newblock", "xspace",
    "normalfont", "bfseries", "itshape",
    "Vert", "lVert", "rVert",
    "centering",
    // soul package
    "uline", "uuline", "sout", "xout",
];

/// Two-argument commands where we want the SECOND argument: \textcolor{red}{text} → text
const TWO_ARG_SECOND: &[&str] = &["textcolor", "colorbox"];

/// Three-argument commands where we want the THIRD argument: \multicolumn{n}{align}{content} → content
const THREE_ARG_THIRD: &[&str] = &["multicolumn"];

/// Commands that extract content with spaces: \footnote{x} → " x "
const INLINE_COMMANDS: &[&str] = &[
    "footnote", "footnotetext",
    "thanks",
    "authornote", "cortext", "corref", "fntext", "ead",
    "corauth",
    "collaboration", "onbehalf",
];

/// Commands to strip entirely (cross-reference markers, etc.)
const STRIP_COMMANDS: &[&str] = &[
    "corauthref", "fnref", "thanksref",
];

/// Font size commands to remove
const FONT_SIZE_COMMANDS: &[&str] = &[
    "tiny", "scriptsize", "footnotesize", "small", "normalsize",
    "large", "Large", "LARGE", "huge", "Huge",
];

/// Commands that just a space before uppercase: \authornote → " "
const SPACE_BEFORE_UPPER: &[&str] = &[
    "authornote", "email", "affiliation", "address",
    "institute", "institution", "cortext",
];

static FOOTNOTE_MARK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\footnotemark").unwrap());

/// Convert LaTeX inline formatting to plaintext/markdown.
pub fn convert_formatting(text: &str) -> String {
    let mut result = text.to_string();

    for cmd in EMPH_COMMANDS {
        result = replace_single_arg_command(&result, cmd, "*", "*");
    }

    for cmd in BOLD_COMMANDS {
        result = replace_single_arg_command(&result, cmd, "**", "**");
    }

    for cmd in UNWRAP_COMMANDS {
        result = replace_single_arg_command(&result, cmd, "", "");
    }

    for cmd in TWO_ARG_SECOND {
        result = replace_two_arg_command(&result, cmd);
    }

    for cmd in THREE_ARG_THIRD {
        result = replace_three_arg_command(&result, cmd);
    }

    for cmd in FONT_SIZE_COMMANDS {
        result = remove_standalone_command(&result, cmd);
    }

    for cmd in INLINE_COMMANDS {
        result = replace_single_arg_command(&result, cmd, " ", " ");
    }

    for cmd in STRIP_COMMANDS {
        result = strip_command_entirely(&result, cmd);
    }

    result = FOOTNOTE_MARK_RE.replace_all(&result, "").to_string();

    result = replace_single_arg_command(&result, "operatorname*", "", "");

    result = replace_with_unicode_script(&result, "textsuperscript", true);
    result = replace_with_unicode_script(&result, "textsubscript", false);
    // Note: \inst{N} is handled in structure.rs (convert_institute_and_inst)
    // where it has access to the \institute label→index mapping.

    for cmd in SPACE_BEFORE_UPPER {
        result = replace_cmd_before_upper(&result, cmd);
    }

    result
}

/// Convert `\textsuperscript{content}` or `\textsubscript{content}` to
/// Unicode super/subscript characters where possible. Characters without
/// Unicode equivalents are passed through as-is.
fn replace_with_unicode_script(text: &str, cmd_name: &str, is_super: bool) -> String {
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
            let content = &text[cs..ce];
            // Only convert to Unicode super/subscript if content is
            // digits, commas, spaces, hyphens, or symbols with mappings.
            // Otherwise fall back to plain unwrap (e.g. \inst{\ref{...}}
            // where \ref resolved to a label key rather than a number).
            let convertible = content
                .chars()
                .all(|c| c.is_ascii_digit() || c == ',' || c == ' ' || c == '-'
                     || c == '+' || c == '=' || c == '(' || c == ')'
                     || c == '*' || c == 'n' || c == 'i');
            if convertible {
                for ch in content.chars() {
                    result.push(to_unicode_script(ch, is_super));
                }
            } else {
                result.push_str(content);
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

/// Map a character to its Unicode superscript or subscript equivalent.
/// Returns the original character if no mapping exists.
pub fn to_unicode_script(ch: char, is_super: bool) -> char {
    if is_super {
        match ch {
            '0' => '\u{2070}',
            '1' => '\u{00B9}',
            '2' => '\u{00B2}',
            '3' => '\u{00B3}',
            '4' => '\u{2074}',
            '5' => '\u{2075}',
            '6' => '\u{2076}',
            '7' => '\u{2077}',
            '8' => '\u{2078}',
            '9' => '\u{2079}',
            '+' => '\u{207A}',
            '-' => '\u{207B}',
            '=' => '\u{207C}',
            '(' => '\u{207D}',
            ')' => '\u{207E}',
            'n' => '\u{207F}',
            'i' => '\u{2071}',
            '*' => '\u{2217}', // asterisk operator
            _ => ch,
        }
    } else {
        match ch {
            '0' => '\u{2080}',
            '1' => '\u{2081}',
            '2' => '\u{2082}',
            '3' => '\u{2083}',
            '4' => '\u{2084}',
            '5' => '\u{2085}',
            '6' => '\u{2086}',
            '7' => '\u{2087}',
            '8' => '\u{2088}',
            '9' => '\u{2089}',
            '+' => '\u{208A}',
            '-' => '\u{208B}',
            '=' => '\u{208C}',
            '(' => '\u{208D}',
            ')' => '\u{208E}',
            _ => ch,
        }
    }
}

/// Replace `\cmd{content}` with `prefix` + content + `suffix`.
/// Uses find_braced_group for arbitrarily nested content.
fn replace_single_arg_command(text: &str, cmd_name: &str, prefix: &str, suffix: &str) -> String {
    let pattern = format!("\\{}", cmd_name);
    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;

    let mut search_start = 0;
    while let Some(pos) = text[search_start..].find(&pattern) {
        let abs_pos = search_start + pos;
        let after = abs_pos + pattern.len();
        let bytes = text.as_bytes();

        let cmd_end = after;
        if !cmd_name.ends_with('*') && cmd_end < bytes.len() && bytes[cmd_end] == b'*' {
            search_start = cmd_end + 1;
            continue;
        }

        if cmd_end < bytes.len() && bytes[cmd_end].is_ascii_alphabetic() {
            search_start = cmd_end;
            continue;
        }

        let after_opt = skip_optional_arg(text, cmd_end);
        if let Some((cs, ce, after_close)) = extract_command_arg_tolerant(text, after_opt) {
            result.push_str(&text[last_end..abs_pos]);
            result.push_str(prefix);
            result.push_str(&text[cs..ce]);
            result.push_str(suffix);
            last_end = after_close;
            search_start = after_close;
        } else {
            search_start = cmd_end;
        }
    }

    result.push_str(&text[last_end..]);
    result
}

/// Replace `\cmd{arg1}{arg2}` with arg2 (extracts second argument).
fn replace_two_arg_command(text: &str, cmd_name: &str) -> String {
    let pattern = format!("\\{}", cmd_name);
    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;

    let mut search_start = 0;
    while let Some(pos) = text[search_start..].find(&pattern) {
        let abs_pos = search_start + pos;
        let after = abs_pos + pattern.len();
        let bytes = text.as_bytes();

        let mut cmd_end = after;
        if cmd_end < bytes.len() && bytes[cmd_end] == b'*' {
            cmd_end += 1;
        }
        if cmd_end < bytes.len() && bytes[cmd_end].is_ascii_alphabetic() {
            search_start = cmd_end;
            continue;
        }

        if let Some((_, _, after_first)) = extract_command_arg(text, cmd_end) {
            if let Some((cs2, ce2, after_second)) = extract_command_arg(text, after_first) {
                result.push_str(&text[last_end..abs_pos]);
                result.push_str(&text[cs2..ce2]);
                last_end = after_second;
                search_start = after_second;
                continue;
            }
        }
        search_start = cmd_end;
    }

    result.push_str(&text[last_end..]);
    result
}

/// Replace `\cmd{arg1}{arg2}{arg3}` with arg3 (extracts third argument).
fn replace_three_arg_command(text: &str, cmd_name: &str) -> String {
    let pattern = format!("\\{}", cmd_name);
    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;

    let mut search_start = 0;
    while let Some(pos) = text[search_start..].find(&pattern) {
        let abs_pos = search_start + pos;
        let after = abs_pos + pattern.len();
        let bytes = text.as_bytes();

        let mut cmd_end = after;
        if cmd_end < bytes.len() && bytes[cmd_end] == b'*' {
            cmd_end += 1;
        }
        if cmd_end < bytes.len() && bytes[cmd_end].is_ascii_alphabetic() {
            search_start = cmd_end;
            continue;
        }

        if let Some((_, _, after1)) = extract_command_arg(text, cmd_end) {
            if let Some((_, _, after2)) = extract_command_arg(text, after1) {
                if let Some((cs3, ce3, after3)) = extract_command_arg(text, after2) {
                    result.push_str(&text[last_end..abs_pos]);
                    result.push_str(&text[cs3..ce3]);
                    last_end = after3;
                    search_start = after3;
                    continue;
                }
            }
        }
        search_start = cmd_end;
    }

    result.push_str(&text[last_end..]);
    result
}

/// Remove a standalone command like `\large` (no braces, just the command).
fn remove_standalone_command(text: &str, cmd_name: &str) -> String {
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

/// Strip `\cmd{arg}` entirely (command + braced arg → nothing).
fn strip_command_entirely(text: &str, cmd_name: &str) -> String {
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

        if let Some((_, _, after_close)) = extract_command_arg(text, after) {
            result.push_str(&text[last_end..abs_pos]);
            last_end = after_close;
            search_start = after_close;
        } else {
            search_start = after;
        }
    }

    result.push_str(&text[last_end..]);
    result
}

/// Replace `\cmd` followed by uppercase letter with a space.
fn replace_cmd_before_upper(text: &str, cmd_name: &str) -> String {
    let pattern = format!("\\{}", cmd_name);
    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;

    let mut search_start = 0;
    while let Some(pos) = text[search_start..].find(&pattern) {
        let abs_pos = search_start + pos;
        let after = abs_pos + pattern.len();
        let bytes = text.as_bytes();

        if after < bytes.len() && bytes[after].is_ascii_uppercase() {
            result.push_str(&text[last_end..abs_pos]);
            result.push(' ');
            last_end = after;
            search_start = after;
        } else {
            search_start = after;
        }
    }

    result.push_str(&text[last_end..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emph() {
        let result = convert_formatting(r"\emph{hello}");
        assert_eq!(result, "*hello*");
    }

    #[test]
    fn test_textbf() {
        let result = convert_formatting(r"\textbf{bold}");
        assert_eq!(result, "**bold**");
    }

    #[test]
    fn test_textrm() {
        let result = convert_formatting(r"\textrm{normal}");
        assert_eq!(result, "normal");
    }

    #[test]
    fn test_nested_formatting() {
        let result = convert_formatting(r"\textbf{\emph{nested}}");
        assert_eq!(result, "***nested***");
    }

    #[test]
    fn test_textcolor_extracts_second_arg() {
        let result = convert_formatting(r"\textcolor{red}{visible text}");
        assert_eq!(result, "visible text");
    }

    #[test]
    fn test_colorbox_extracts_second_arg() {
        let result = convert_formatting(r"\colorbox{yellow}{highlighted}");
        assert_eq!(result, "highlighted");
    }

    #[test]
    fn test_multicolumn_extracts_third_arg() {
        let result = convert_formatting(r"\multicolumn{3}{c}{Header}");
        assert_eq!(result, "Header");
    }

    #[test]
    fn test_footnote() {
        let result = convert_formatting(r"text\footnote{note}more");
        assert_eq!(result, "text note more");
    }

    #[test]
    fn test_thanks() {
        let result = convert_formatting(r"author\thanks{grant info}");
        assert_eq!(result, "author grant info ");
    }

    #[test]
    fn test_font_size_removed() {
        let result = convert_formatting(r"\large text");
        assert_eq!(result, " text");
    }

    #[test]
    fn test_nested_braces_in_formatting() {
        let result = convert_formatting(r"\emph{a {b} c}");
        assert_eq!(result, "*a {b} c*");
    }

    #[test]
    fn test_corauthref_stripped() {
        let result = convert_formatting(r"Name \corauthref{cor2} rest");
        assert_eq!(result, "Name  rest");
    }

    #[test]
    fn test_textsuperscript_digits() {
        let result = convert_formatting(r"Zhou\textsuperscript{1}");
        assert_eq!(result, "Zhou\u{00B9}");
    }

    #[test]
    fn test_textsuperscript_multi() {
        let result = convert_formatting(r"x\textsuperscript{23}");
        assert_eq!(result, "x\u{00B2}\u{00B3}");
    }

    #[test]
    fn test_textsubscript_digits() {
        let result = convert_formatting(r"H\textsubscript{2}O");
        assert_eq!(result, "H\u{2082}O");
    }

    #[test]
    fn test_textsuperscript_letter_passthrough() {
        // Letters without Unicode superscript equivalents pass through
        let result = convert_formatting(r"\textsuperscript{a}");
        assert_eq!(result, "a");
    }

    #[test]
    fn test_tolerant_unclosed_brace() {
        // Unclosed brace should still extract content rather than lose it
        let result = convert_formatting("\\textbf{unclosed text");
        assert!(
            result.contains("unclosed text"),
            "tolerant should preserve content: {result}"
        );
    }
}
