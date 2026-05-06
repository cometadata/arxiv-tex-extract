use crate::braces::{extract_command_arg, extract_command_arg_tolerant, extract_optional_arg, skip_optional_arg};
use crate::cleanup::{find_math_regions, in_math};
use regex::Regex;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

const EMPH_COMMANDS: &[&str] = &["emph", "textit", "textsl"];

const BOLD_COMMANDS: &[&str] = &["textbf", "mathbf", "boldsymbol", "bm"];

/// NOTE: math accents (hat, tilde, bar, vec, dot, ddot, acute, grave, check, breve,
/// overline, underline) are handled by convert_math_accents() instead.
/// Math font styles (mathcal, mathbb, mathfrak, mathsf, Bbb) are handled by
/// convert_symbols() in symbols.rs instead.
const UNWRAP_COMMANDS: &[&str] = &[
    "textrm", "textsc", "texttt", "textsf", "text", "textnormal",
    "mathrm", "mathit",
    "operatorname",
    "cal",
    "widetilde", "widehat",
    "ensuremath",
    "mbox", "hbox", "vbox", "fbox",
    "country",
    "citenamefont", "bibnamefont", "bibfnamefont",
    "newblock", "xspace",
    "normalfont", "bfseries", "itshape",
    "Vert", "lVert", "rVert",
    "centering",
    "uline", "uuline", "sout", "xout",
];

const TWO_ARG_SECOND: &[&str] = &["textcolor", "colorbox", "texorpdfstring"];

const THREE_ARG_THIRD: &[&str] = &["multicolumn"];

const INLINE_COMMANDS: &[&str] = &[
    "footnote", "footnotetext",
    "thanks",
    "authornote", "cortext", "corref", "fntext", "ead",
    "corauth",
    "collaboration", "onbehalf",
    "endnote",
];

const STRIP_COMMANDS: &[&str] = &[
    "corauthref", "fnref", "thanksref",
];

const FONT_SIZE_COMMANDS: &[&str] = &[
    "tiny", "scriptsize", "footnotesize", "small", "normalsize",
    "large", "Large", "LARGE", "huge", "Huge",
];

const SPACE_BEFORE_UPPER: &[&str] = &[
    "authornote", "email", "affiliation", "address",
    "institute", "institution", "cortext",
];

static FOOTNOTE_MARK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\footnotemark").unwrap());

/// Handle `\verb|...|` and `\verb*|...|` — delimiter-aware inline verbatim.
/// The delimiter is the first non-whitespace character after `\verb` (or `\verb*`).
fn convert_inline_verb(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut last_end = 0;
    let mut search_start = 0;

    while let Some(pos) = text[search_start..].find("\\verb") {
        let abs_pos = search_start + pos;
        let mut after = abs_pos + 5;

        if after < bytes.len() && bytes[after] == b'*' {
            after += 1;
        }

        // Boundary check: skip \verbatim, \verbose, etc.
        if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
            search_start = after;
            continue;
        }

        // Delimiter may be multi-byte UTF-8
        let Some(delim_char) = text[after..].chars().next() else {
            search_start = after;
            continue;
        };
        let content_start = after + delim_char.len_utf8();

        if let Some(end_offset) = text[content_start..].find(delim_char) {
            let content_end = content_start + end_offset;
            result.push_str(&text[last_end..abs_pos]);
            result.push_str(&text[content_start..content_end]);
            last_end = content_end + delim_char.len_utf8();
            search_start = last_end;
        } else {
            search_start = content_start;
        }
    }

    result.push_str(&text[last_end..]);
    result
}

/// Handle `\lstinline|...|` and `\lstinline{...}` — listings package inline code.
fn convert_lstinline(text: &str) -> String {
    let pattern = "\\lstinline";
    let mut result = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut last_end = 0;
    let mut search_start = 0;

    while let Some(pos) = text[search_start..].find(pattern) {
        let abs_pos = search_start + pos;
        let after = abs_pos + pattern.len();

        if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
            search_start = after;
            continue;
        }

        if after >= bytes.len() {
            search_start = after;
            continue;
        }

        if bytes[after] == b'{' {
            if let Some((cs, ce, after_close)) = extract_command_arg(text, after) {
                result.push_str(&text[last_end..abs_pos]);
                result.push_str(&text[cs..ce]);
                last_end = after_close;
                search_start = after_close;
                continue;
            }
        }

        let after_opt = skip_optional_arg(text, after);

        if let Some(delim_char) = text[after_opt..].chars().next().filter(|&c| c != '{') {
            let content_start = after_opt + delim_char.len_utf8();
            if let Some(end_offset) = text[content_start..].find(delim_char) {
                let content_end = content_start + end_offset;
                result.push_str(&text[last_end..abs_pos]);
                result.push_str(&text[content_start..content_end]);
                last_end = content_end + delim_char.len_utf8();
                search_start = last_end;
                continue;
            }
        } else if after_opt < bytes.len() {
            if let Some((cs, ce, after_close)) = extract_command_arg(text, after_opt) {
                result.push_str(&text[last_end..abs_pos]);
                result.push_str(&text[cs..ce]);
                last_end = after_close;
                search_start = after_close;
                continue;
            }
        }

        search_start = after;
    }

    result.push_str(&text[last_end..]);
    result
}

/// Handle `\mintinline{lang}{code}` — minted package inline code.
fn convert_mintinline(text: &str) -> String {
    let pattern = "\\mintinline";
    let mut result = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut last_end = 0;
    let mut search_start = 0;

    while let Some(pos) = text[search_start..].find(pattern) {
        let abs_pos = search_start + pos;
        let after = abs_pos + pattern.len();

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

const MATH_ACCENT_MAP: &[(&str, char)] = &[
    ("hat",       '\u{0302}'),
    ("tilde",     '\u{0303}'),
    ("bar",       '\u{0305}'),
    ("vec",       '\u{20D7}'),
    ("dot",       '\u{0307}'),
    ("ddot",      '\u{0308}'),
    ("acute",     '\u{0301}'),
    ("grave",     '\u{0300}'),
    ("check",     '\u{030C}'),
    ("breve",     '\u{0306}'),
    ("overline",  '\u{0305}'),
    ("underline", '\u{0332}'),
    ("dddot",     '\u{20DB}'),
    ("ddddot",    '\u{20DC}'),
];

fn convert_math_accents(text: &str) -> String {
    let mut result = text.to_string();
    for &(cmd_name, combining) in MATH_ACCENT_MAP {
        result = apply_math_accent(&result, cmd_name, combining);
    }
    result
}

fn apply_math_accent(text: &str, cmd_name: &str, combining: char) -> String {
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
            let content = &text[cs..ce];
            result.push_str(&text[last_end..abs_pos]);

            let chars: Vec<char> = content.chars().collect();
            if chars.len() == 1 {
                let composed = format!("{}{}", chars[0], combining);
                result.push_str(&composed.nfc().collect::<String>());
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

static FRAC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\(?:d|t|nice|c)?frac").unwrap());

static BINOM_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\(?:t|d)?binom").unwrap());

fn convert_text_fractions(text: &str) -> String {
    let math_regions = find_math_regions(text);
    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;

    for mat in FRAC_RE.find_iter(text) {
        let start = mat.start();
        if start < last_end {
            continue;
        }
        if in_math(start, &math_regions) {
            continue;
        }

        let after = mat.end();
        if let Some((cs1, ce1, after1)) = extract_command_arg(text, after) {
            if let Some((cs2, ce2, after2)) = extract_command_arg(text, after1) {
                result.push_str(&text[last_end..start]);
                result.push_str(&text[cs1..ce1]);
                result.push('/');
                result.push_str(&text[cs2..ce2]);
                last_end = after2;
            }
        }
    }

    result.push_str(&text[last_end..]);
    result
}

fn convert_text_sqrt(text: &str) -> String {
    let math_regions = find_math_regions(text);
    let pattern = "\\sqrt";
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
        if abs_pos < last_end {
            search_start = after;
            continue;
        }
        if in_math(abs_pos, &math_regions) {
            search_start = after;
            continue;
        }

        let mut cursor = after;
        let mut opt_arg = None;
        if cursor < bytes.len() && bytes[cursor] == b'[' {
            if let Some((os, oe, after_opt)) = extract_optional_arg(text, cursor) {
                opt_arg = Some(&text[os..oe]);
                cursor = after_opt;
            }
        }

        if let Some((cs, ce, after_close)) = extract_command_arg(text, cursor) {
            result.push_str(&text[last_end..abs_pos]);
            result.push('\u{221A}');
            if let Some(n) = opt_arg {
                result.push('[');
                result.push_str(n);
                result.push(']');
            }
            result.push('(');
            result.push_str(&text[cs..ce]);
            result.push(')');
            last_end = after_close;
            search_start = after_close;
        } else {
            search_start = after;
        }
    }

    result.push_str(&text[last_end..]);
    result
}

fn convert_text_binom(text: &str) -> String {
    let math_regions = find_math_regions(text);
    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;

    for mat in BINOM_RE.find_iter(text) {
        let start = mat.start();
        if start < last_end {
            continue;
        }
        if in_math(start, &math_regions) {
            continue;
        }

        let after = mat.end();
        if let Some((cs1, ce1, after1)) = extract_command_arg(text, after) {
            if let Some((cs2, ce2, after2)) = extract_command_arg(text, after1) {
                result.push_str(&text[last_end..start]);
                result.push_str("C(");
                result.push_str(&text[cs1..ce1]);
                result.push(',');
                result.push_str(&text[cs2..ce2]);
                result.push(')');
                last_end = after2;
            }
        }
    }

    result.push_str(&text[last_end..]);
    result
}

/// Convert LaTeX inline formatting to plaintext/markdown.
pub fn convert_formatting(text: &str) -> String {
    let mut result = text.to_string();

    // Inline verbatim must be extracted before any other processing
    result = convert_inline_verb(&result);
    result = convert_lstinline(&result);
    result = convert_mintinline(&result);

    // Math accents must apply combining marks before generic unwrap
    result = convert_math_accents(&result);

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

    result = convert_text_fractions(&result);
    result = convert_text_sqrt(&result);
    result = convert_text_binom(&result);

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
    // \inst{N} is handled in structure.rs (convert_institute_and_inst)
    // where it has access to the \institute label→index mapping.

    result = convert_quantum_notation(&result);

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
            // Only convert if content is digits/symbols with known mappings;
            // otherwise plain unwrap (e.g. \inst{\ref{...}} where \ref
            // resolved to a label key rather than a number).
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
            '*' => '\u{2217}',
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

/// Convert \ket{x} → |x⟩, \bra{x} → ⟨x|, \braket{x}{y} → ⟨x|y⟩, \ketbra{x}{y} → |x⟩⟨y|
fn convert_quantum_notation(text: &str) -> String {
    let mut result = text.to_string();

    // Two-argument commands must be matched first (longer patterns first)
    result = convert_braket(&result);
    result = convert_ketbra(&result);

    result = replace_single_arg_command(&result, "ket", "|\u{200B}", "\u{27E9}");
    result = replace_single_arg_command(&result, "bra", "\u{27E8}", "|\u{200B}");

    result
}

fn convert_braket(text: &str) -> String {
    let pattern = "\\braket";
    let mut result = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut last_end = 0;
    let mut search_start = 0;

    while let Some(pos) = text[search_start..].find(pattern) {
        let abs_pos = search_start + pos;
        let after = abs_pos + pattern.len();

        if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
            search_start = after;
            continue;
        }

        if let Some((cs1, ce1, after1)) = extract_command_arg(text, after) {
            if let Some((cs2, ce2, after2)) = extract_command_arg(text, after1) {
                result.push_str(&text[last_end..abs_pos]);
                result.push('\u{27E8}');
                result.push_str(&text[cs1..ce1]);
                result.push('|');
                result.push_str(&text[cs2..ce2]);
                result.push('\u{27E9}');
                last_end = after2;
                search_start = after2;
                continue;
            }
            // Single-arg fallback: \braket{x} → ⟨x⟩
            result.push_str(&text[last_end..abs_pos]);
            result.push('\u{27E8}');
            result.push_str(&text[cs1..ce1]);
            result.push('\u{27E9}');
            last_end = after1;
            search_start = after1;
            continue;
        }
        search_start = after;
    }

    result.push_str(&text[last_end..]);
    result
}

fn convert_ketbra(text: &str) -> String {
    let pattern = "\\ketbra";
    let mut result = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut last_end = 0;
    let mut search_start = 0;

    while let Some(pos) = text[search_start..].find(pattern) {
        let abs_pos = search_start + pos;
        let after = abs_pos + pattern.len();

        if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
            search_start = after;
            continue;
        }

        if let Some((cs1, ce1, after1)) = extract_command_arg(text, after) {
            if let Some((cs2, ce2, after2)) = extract_command_arg(text, after1) {
                result.push_str(&text[last_end..abs_pos]);
                result.push('|');
                result.push_str(&text[cs1..ce1]);
                result.push('\u{27E9}');
                result.push('\u{27E8}');
                result.push_str(&text[cs2..ce2]);
                result.push('|');
                last_end = after2;
                search_start = after2;
                continue;
            }
        }
        search_start = after;
    }

    result.push_str(&text[last_end..]);
    result
}

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
        let result = convert_formatting(r"\textsuperscript{a}");
        assert_eq!(result, "a");
    }

    #[test]
    fn test_tolerant_unclosed_brace() {
        let result = convert_formatting("\\textbf{unclosed text");
        assert!(
            result.contains("unclosed text"),
            "tolerant should preserve content: {result}"
        );
    }

    #[test]
    fn test_ensuremath_unwrap() {
        let result = convert_formatting(r"\ensuremath{x}");
        assert_eq!(result, "x");
    }

    #[test]
    fn test_texorpdfstring() {
        let result = convert_formatting(r"\texorpdfstring{$\alpha$}{alpha}");
        assert_eq!(result, "alpha");
    }

    #[test]
    fn test_frac_outside_math() {
        let result = convert_formatting(r"\frac{a}{b}");
        assert_eq!(result, "a/b");
    }

    #[test]
    fn test_dfrac_outside_math() {
        let result = convert_formatting(r"\dfrac{1}{2}");
        assert_eq!(result, "1/2");
    }

    #[test]
    fn test_frac_inside_math_preserved() {
        let result = convert_formatting(r"$\frac{a}{b}$");
        assert_eq!(result, r"$\frac{a}{b}$");
    }

    #[test]
    fn test_sqrt_outside_math() {
        let result = convert_formatting(r"\sqrt{x}");
        assert_eq!(result, "\u{221A}(x)");
    }

    #[test]
    fn test_sqrt_with_optional() {
        let result = convert_formatting(r"\sqrt[3]{x}");
        assert_eq!(result, "\u{221A}[3](x)");
    }

    #[test]
    fn test_sqrt_inside_math_preserved() {
        let result = convert_formatting(r"$\sqrt{x}$");
        assert_eq!(result, r"$\sqrt{x}$");
    }

    #[test]
    fn test_binom_outside_math() {
        let result = convert_formatting(r"\binom{n}{k}");
        assert_eq!(result, "C(n,k)");
    }

    #[test]
    fn test_binom_inside_math_preserved() {
        let result = convert_formatting(r"$\binom{n}{k}$");
        assert_eq!(result, r"$\binom{n}{k}$");
    }

    #[test]
    fn test_hat_single_char() {
        let result = convert_formatting(r"\hat{x}");
        assert_eq!(result, "x\u{0302}");
    }

    #[test]
    fn test_vec_single_char() {
        let result = convert_formatting(r"\vec{v}");
        assert_eq!(result, "v\u{20D7}");
    }

    #[test]
    fn test_accent_multi_char_unwraps() {
        let result = convert_formatting(r"\hat{abc}");
        assert_eq!(result, "abc");
    }

    #[test]
    fn test_overline_single_char() {
        let result = convert_formatting(r"\overline{x}");
        assert_eq!(result, "x\u{0305}");
    }

    #[test]
    fn test_underline_single_char() {
        let result = convert_formatting(r"\underline{x}");
        assert_eq!(result, "x\u{0332}");
    }

    #[test]
    fn test_verb_pipe() {
        let result = convert_formatting(r"\verb|hello world|");
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_verb_star() {
        let result = convert_formatting(r"\verb*|code here|");
        assert_eq!(result, "code here");
    }

    #[test]
    fn test_verb_bang_delimiter() {
        let result = convert_formatting(r"\verb!special!");
        assert_eq!(result, "special");
    }

    #[test]
    fn test_verb_boundary() {
        // \verbatim should NOT match as \verb
        let result = convert_formatting(r"\verbatim{text}");
        assert!(result.contains("verbatim") || result.contains("text"));
    }

    #[test]
    fn test_verb_multibyte_delimiter() {
        // Multi-byte UTF-8 char adjacent to \verb must not panic
        let input = "before \\verb·code· after";
        let result = convert_formatting(input);
        assert_eq!(result, "before code after");
    }

    #[test]
    fn test_lstinline_braced() {
        let result = convert_formatting(r"\lstinline{x = 1}");
        assert_eq!(result, "x = 1");
    }

    #[test]
    fn test_lstinline_delimiter() {
        let result = convert_formatting(r"\lstinline|x = 1|");
        assert_eq!(result, "x = 1");
    }

    #[test]
    fn test_lstinline_multibyte_delimiter() {
        // Multi-byte UTF-8 char as lstinline delimiter must not panic
        let input = "before \\lstinline·code· after";
        let result = convert_formatting(input);
        assert_eq!(result, "before code after");
    }

    #[test]
    fn test_mintinline() {
        let result = convert_formatting(r"\mintinline{python}{x = 1}");
        assert_eq!(result, "x = 1");
    }

    #[test]
    fn test_ket() {
        let result = convert_formatting(r"\ket{0}");
        assert!(result.contains("|") && result.contains("\u{27E9}"), "ket: {result}");
    }

    #[test]
    fn test_bra() {
        let result = convert_formatting(r"\bra{0}");
        assert!(result.contains("\u{27E8}") && result.contains("|"), "bra: {result}");
    }

    #[test]
    fn test_braket() {
        let result = convert_formatting(r"\braket{x}{y}");
        assert!(result.contains("\u{27E8}") && result.contains("|") && result.contains("\u{27E9}"), "braket: {result}");
    }

    #[test]
    fn test_ketbra() {
        let result = convert_formatting(r"\ketbra{x}{y}");
        assert!(result.contains("|x\u{27E9}") && result.contains("\u{27E8}y|"), "ketbra: {result}");
    }

    #[test]
    fn test_dddot_single_char() {
        let result = convert_formatting(r"\dddot{x}");
        assert!(result.contains("x") && result.contains("\u{20DB}"), "dddot: {result}");
    }

    #[test]
    fn test_ddddot_single_char() {
        let result = convert_formatting(r"\ddddot{x}");
        assert!(result.contains("x") && result.contains("\u{20DC}"), "ddddot: {result}");
    }

    #[test]
    fn test_endnote() {
        let result = convert_formatting(r"text\endnote{a note}more");
        assert_eq!(result, "text a note more");
    }
}
