use crate::braces::{extract_command_arg, skip_optional_arg};
use crate::timing::deadline_expired;
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Instant;

static ITEM_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\item\s*(?:\[([^\]]*)\])?").unwrap());

static INCLUDEGRAPHICS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\includegraphics\s*(?:\[[^\]]*\])?\s*\{[^{}]*\}").unwrap());

static CENTERING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\centering").unwrap());

static DISPLAY_MATH_OPEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\\[").unwrap());

static DISPLAY_MATH_CLOSE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\]").unwrap());

/// List environments
const LIST_ENVS: &[&str] = &[
    "itemize", "enumerate", "description",
    // paralist package variants
    "compactitem", "compactenum", "compactdesc", "inparaenum",
];

/// Figure/table environments (including starred)
const FLOAT_ENVS: &[&str] = &[
    "figure", "figure*", "table", "table*", "wrapfigure", "wraptable",
    "subfigure", "subfloat", "sidewaysfigure", "sidewaystable",
];

/// Quote environments
const QUOTE_ENVS: &[&str] = &["quote", "quotation", "verse"];

/// Theorem-like environments
const THEOREM_ENVS: &[&str] = &[
    "theorem", "lemma", "proposition", "corollary", "conjecture",
    "definition", "example", "remark", "proof", "claim", "observation",
    "notation", "assumption", "hypothesis", "property", "fact",
    "question", "problem", "exercise", "solution", "case", "condition",
];

/// Tabular environments (content grids with & separators)
const TABULAR_ENVS: &[&str] = &[
    "tabular", "tabular*", "tabularx", "tabulary",
    "longtable", "longtable*", "supertabular",
    "array",
];

/// Math display environments.
/// Also used by `cleanup::find_math_regions` to protect math content from cleanup transforms.
pub(crate) const MATH_ENVS: &[&str] = &[
    "equation", "equation*", "align", "align*", "gather", "gather*",
    "multline", "multline*", "eqnarray", "eqnarray*",
    "displaymath", "flalign", "flalign*",
    "alignat", "alignat*",
    "math", "dmath", "dmath*",
    // Inner amsmath environments
    "split", "cases", "aligned", "gathered", "alignedat",
    "subequations",
    // Matrix environments
    "pmatrix", "bmatrix", "vmatrix", "Bmatrix", "Vmatrix",
    "smallmatrix", "psmallmatrix", "bsmallmatrix",
];

/// Algorithm / pseudocode environments
const ALGORITHM_ENVS: &[&str] = &[
    "algorithm", "algorithm*", "algorithmic", "algorithm2e",
    "procedure", "function",
];

/// Algorithm commands to convert to readable form
static ALGO_COMMAND_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\\(?:State|If|Else|ElsIf|EndIf|While|EndWhile|For|EndFor|ForAll|EndForAll|Return|Require|Ensure|Input|Output|Procedure|EndProcedure|Function|EndFunction|Loop|EndLoop|Repeat|Until|Comment)\b"
    ).unwrap()
});

/// Keyword environments
const KEYWORD_ENVS: &[&str] = &["keywords", "keyword", "IEEEkeywords"];

/// Layout environments (just unwrap to content)
const LAYOUT_ENVS: &[&str] = &[
    "minipage", "adjustbox", "adjustwidth",
    "spacing", "singlespace", "doublespace", "onehalfspace",
    "multicols", "multicols*", "widetext",
    "titlepage", "landscape",
];

/// Verbatim / code environments (remove markers, keep content)
const VERBATIM_ENVS: &[&str] = &["verbatim", "Verbatim", "lstlisting", "minted", "alltt"];

/// Environments to discard entirely (content and markers)
const DISCARD_ENVS: &[&str] = &[
    "filecontents", "filecontents*",
    // Drawing / diagram environments (produce garbage text)
    "tikzpicture", "tikzcd", "pgfpicture", "pspicture", "picture",
];

/// Convert LaTeX environments to readable form.
/// `custom_theorems` maps env names (e.g. "dfn") to display names (e.g. "Definition").
pub fn convert_environments(text: &str, custom_theorems: &HashMap<String, String>, deadline: Option<Instant>) -> String {
    let mut result = text.to_string();

    for env in LIST_ENVS {
        result = remove_env_markers(&result, env);
    }
    result = convert_items(&result);

    result = convert_all_floats(&result);
    if deadline_expired(deadline) { return result; }

    for env in QUOTE_ENVS {
        result = remove_env_markers(&result, env);
    }

    result = remove_env_markers(&result, "center");
    result = CENTERING_RE.replace_all(&result, "").to_string();

    for env in THEOREM_ENVS {
        result = convert_theorem_env(&result, env);
    }

    for (env_name, display_name) in custom_theorems {
        if !THEOREM_ENVS.contains(&env_name.as_str()) {
            result = convert_custom_theorem_env(&result, env_name, display_name);
        }
    }
    if deadline_expired(deadline) { return result; }

    let mut eq_counter = 0usize;
    for env in MATH_ENVS {
        if env.ends_with('*') || *env == "displaymath" {
            result = replace_env_with(&result, env, "\n$$\n", "\n$$\n");
        } else {
            result = replace_math_env_numbered(&result, env, &mut eq_counter);
        }
    }

    result = clean_math_display_interiors(&result);
    if deadline_expired(deadline) { return result; }

    result = DISPLAY_MATH_OPEN_RE.replace_all(&result, "\n$$$$\n").to_string();
    result = DISPLAY_MATH_CLOSE_RE.replace_all(&result, "\n$$$$\n").to_string();

    for env in TABULAR_ENVS {
        result = convert_tabular_env(&result, env);
    }

    for env in ALGORITHM_ENVS {
        result = remove_env_markers(&result, env);
    }
    result = ALGO_COMMAND_RE.replace_all(&result, "").to_string();
    if deadline_expired(deadline) { return result; }

    for env in KEYWORD_ENVS {
        result = convert_keyword_env(&result, env);
    }

    for env in LAYOUT_ENVS {
        result = remove_env_markers_with_args(&result, env);
    }

    for env in VERBATIM_ENVS {
        result = remove_env_markers(&result, env);
    }

    result = convert_bibliography_env(&result);

    for env in DISCARD_ENVS {
        result = discard_env(&result, env);
    }

    result = remove_remaining_envs(&result);

    result
}

/// Discard an entire environment (markers + content → nothing).
fn discard_env(text: &str, env_name: &str) -> String {
    let begin_pat = format!("\\begin{{{}}}", env_name);
    let end_pat = format!("\\end{{{}}}", env_name);

    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;
    let mut search = 0;

    while let Some(bp) = text[search..].find(&begin_pat) {
        let abs_begin = search + bp;
        if let Some(ep) = text[abs_begin..].find(&end_pat) {
            let abs_end = abs_begin + ep + end_pat.len();
            result.push_str(&text[last_end..abs_begin]);
            last_end = abs_end;
            search = abs_end;
        } else {
            break;
        }
    }

    result.push_str(&text[last_end..]);
    result
}

/// Remove \begin{env} and \end{env} markers, replacing with newlines.
fn remove_env_markers(text: &str, env_name: &str) -> String {
    let begin_pat = format!("\\begin{{{}}}", env_name);
    let end_pat = format!("\\end{{{}}}", env_name);

    let working = text.to_string();
    let mut temp = String::with_capacity(working.len());
    let mut temp_last = 0;

    let mut ss = 0;
    while let Some(pos) = working[ss..].find(&begin_pat) {
        let abs = ss + pos;
        let after = abs + begin_pat.len();
        temp.push_str(&working[temp_last..abs]);
        temp.push('\n');
        let skip_to = skip_optional_trailing(working.as_bytes(), after);
        temp_last = skip_to;
        ss = skip_to;
    }
    temp.push_str(&working[temp_last..]);

    temp.replace(&end_pat, "\n")
}

/// Skip trailing `[options]` after an environment begin marker.
fn skip_optional_trailing(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    if i < bytes.len() && bytes[i] == b'[' {
        let mut depth = 1;
        i += 1;
        while i < bytes.len() && depth > 0 {
            match bytes[i] {
                b'[' => depth += 1,
                b']' => depth -= 1,
                _ => {}
            }
            i += 1;
        }
    }
    i
}

/// Convert \item[label] to \n- label
fn convert_items(text: &str) -> String {
    ITEM_RE
        .replace_all(text, |caps: &regex::Captures| {
            match caps.get(1) {
                Some(label) if !label.as_str().is_empty() => format!("\n- {} ", label.as_str()),
                _ => "\n- ".to_string(),
            }
        })
        .to_string()
}

/// Determine the float type category for numbering purposes.
fn float_type(env_name: &str) -> &str {
    match env_name {
        "figure" | "figure*" | "wrapfigure" | "subfigure" | "subfloat" | "sidewaysfigure" => "figure",
        "table" | "table*" | "wraptable" | "sidewaystable" => "table",
        _ => env_name,
    }
}

/// Process all float environments (figure, table, etc.) as units.
/// Maintains per-type counters and prepends `Figure N:` / `Table N:` to captions.
fn convert_all_floats(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;
    let mut counters: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    // Find all float begins in order
    let mut search_start = 0;
    while search_start < text.len() {
        // Find the next float environment begin
        let mut best_match: Option<(usize, &str, usize)> = None; // (abs_pos, env_name, after_begin)
        for &env in FLOAT_ENVS {
            let begin_pat = format!("\\begin{{{}}}", env);
            if let Some(pos) = text[search_start..].find(&begin_pat) {
                let abs = search_start + pos;
                let after = abs + begin_pat.len();
                if best_match.is_none() || abs < best_match.unwrap().0 {
                    best_match = Some((abs, env, after));
                }
            }
        }

        let (abs_begin, env_name, after_begin) = match best_match {
            Some(m) => m,
            None => break,
        };

        // Find matching \end{env}
        let end_pat = format!("\\end{{{}}}", env_name);
        let content_start = skip_optional_trailing(text.as_bytes(), after_begin);
        if let Some(end_rel) = text[content_start..].find(&end_pat) {
            let abs_end = content_start + end_rel;
            let after_end = abs_end + end_pat.len();

            result.push_str(&text[last_end..abs_begin]);

            let mut content = text[content_start..abs_end].to_string();
            content = INCLUDEGRAPHICS_RE.replace_all(&content, "").to_string();
            content = CENTERING_RE.replace_all(&content, "").to_string();

            let ftype = float_type(env_name);
            let counter = counters.entry(ftype.to_string()).or_insert(0);
            content = extract_and_prefix_captions(&content, ftype, counter);

            result.push('\n');
            result.push_str(content.trim());
            result.push('\n');

            last_end = after_end;
            search_start = after_end;
        } else {
            search_start = after_begin;
        }
    }

    result.push_str(&text[last_end..]);

    let result = INCLUDEGRAPHICS_RE.replace_all(&result, "").to_string();
    result
}

/// Extract `\caption{...}` from float content, prefix with `Figure N:` or `Table N:`.
fn extract_and_prefix_captions(content: &str, ftype: &str, counter: &mut usize) -> String {
    let pattern = "\\caption";
    let mut result = String::with_capacity(content.len());
    let mut last_end = 0;

    let label = {
        let mut chars = ftype.chars();
        match chars.next() {
            Some(c) => format!("{}{}", c.to_uppercase(), chars.as_str()),
            None => String::new(),
        }
    };

    let mut search_start = 0;
    while let Some(pos) = content[search_start..].find(pattern) {
        let abs_pos = search_start + pos;
        let after = abs_pos + pattern.len();
        let bytes = content.as_bytes();

        let mut cmd_end = after;
        if cmd_end < bytes.len() && bytes[cmd_end] == b'*' {
            cmd_end += 1;
        }
        if cmd_end < bytes.len() && bytes[cmd_end].is_ascii_alphabetic() {
            search_start = cmd_end;
            continue;
        }

        let after_opt = skip_optional_arg(content, cmd_end);
        if let Some((cs, ce, after_close)) = extract_command_arg(content, after_opt) {
            *counter += 1;
            result.push_str(&content[last_end..abs_pos]);
            result.push('\n');
            result.push_str(&label);
            result.push(' ');
            result.push_str(&counter.to_string());
            result.push_str(": ");
            result.push_str(&content[cs..ce]);
            result.push('\n');
            last_end = after_close;
            search_start = after_close;
        } else {
            search_start = cmd_end;
        }
    }

    result.push_str(&content[last_end..]);
    result
}

/// Convert theorem-like environments: \begin{theorem}[name] → **Theorem (name).**
fn convert_theorem_env(text: &str, env_name: &str) -> String {
    let begin_pat = format!("\\begin{{{}}}", env_name);
    let begin_pat_star = format!("\\begin{{{}*}}", env_name);
    let end_pat = format!("\\end{{{}}}", env_name);
    let end_pat_star = format!("\\end{{{}*}}", env_name);

    let label = {
        let mut chars = env_name.chars();
        match chars.next() {
            Some(c) => format!("{}{}", c.to_uppercase(), chars.as_str()),
            None => String::new(),
        }
    };

    let mut result = text.to_string();

    for pat in &[&begin_pat, &begin_pat_star] {
        let mut new_result = String::with_capacity(result.len());
        let mut last_end = 0;
        let mut ss = 0;

        while let Some(pos) = result[ss..].find(pat.as_str()) {
            let abs = ss + pos;
            let after = abs + pat.len();
            new_result.push_str(&result[last_end..abs]);

            let after_ws = skip_ws(result.as_bytes(), after);
            if after_ws < result.len() && result.as_bytes()[after_ws] == b'[' {
                // Find the closing ]
                if let Some(close) = result[after_ws + 1..].find(']') {
                    let name = &result[after_ws + 1..after_ws + 1 + close];
                    new_result.push_str(&format!("\n**{} ({}).** ", label, name));
                    last_end = after_ws + 1 + close + 1;
                    ss = last_end;
                    continue;
                }
            }

            new_result.push_str(&format!("\n**{}.** ", label));
            last_end = after;
            ss = after;
        }

        new_result.push_str(&result[last_end..]);
        result = new_result;
    }

    // For proof environments, append □ (QED symbol) if not already present
    if env_name == "proof" {
        result = append_qed_to_proof(&result, &end_pat);
        result = append_qed_to_proof(&result, &end_pat_star);
    } else {
        result = result.replace(&end_pat, "\n");
        result = result.replace(&end_pat_star, "\n");
    }

    result
}

/// Replace \end{proof} with " □\n" if the preceding body doesn't contain \qed or \qedsymbol.
fn append_qed_to_proof(text: &str, end_pat: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;
    let mut search = 0;

    while let Some(pos) = text[search..].find(end_pat) {
        let abs_pos = search + pos;
        let after = abs_pos + end_pat.len();

        // Check if the text between last_end and abs_pos contains \qed
        let body = &text[last_end..abs_pos];
        let has_qed = body.contains("\\qed") || body.contains("\\qedsymbol");

        result.push_str(&text[last_end..abs_pos]);
        if !has_qed {
            result.push_str(" \u{25A1}");
        }
        result.push('\n');

        last_end = after;
        search = after;
    }

    result.push_str(&text[last_end..]);
    result
}

/// Convert a custom theorem environment using a preamble-defined display name.
fn convert_custom_theorem_env(text: &str, env_name: &str, display_name: &str) -> String {
    let begin_pat = format!("\\begin{{{}}}", env_name);
    let begin_pat_star = format!("\\begin{{{}*}}", env_name);
    let end_pat = format!("\\end{{{}}}", env_name);
    let end_pat_star = format!("\\end{{{}*}}", env_name);

    let mut result = text.to_string();

    for pat in &[&begin_pat, &begin_pat_star] {
        let mut new_result = String::with_capacity(result.len());
        let mut last_end = 0;
        let mut ss = 0;

        while let Some(pos) = result[ss..].find(pat.as_str()) {
            let abs = ss + pos;
            let after = abs + pat.len();
            new_result.push_str(&result[last_end..abs]);

            let after_ws = skip_ws(result.as_bytes(), after);
            if after_ws < result.len() && result.as_bytes()[after_ws] == b'[' {
                if let Some(close) = result[after_ws + 1..].find(']') {
                    let name = &result[after_ws + 1..after_ws + 1 + close];
                    new_result.push_str(&format!("\n**{} ({}).** ", display_name, name));
                    last_end = after_ws + 1 + close + 1;
                    ss = last_end;
                    continue;
                }
            }

            new_result.push_str(&format!("\n**{}.** ", display_name));
            last_end = after;
            ss = after;
        }

        new_result.push_str(&result[last_end..]);
        result = new_result;
    }

    result = result.replace(&end_pat, "\n");
    result = result.replace(&end_pat_star, "\n");

    result
}

fn skip_ws(bytes: &[u8], pos: usize) -> usize {
    let mut i = pos;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\n') {
        i += 1;
    }
    i
}

/// Replace a numbered math environment with `$$ content (N) $$`.
fn replace_math_env_numbered(text: &str, env_name: &str, counter: &mut usize) -> String {
    let begin_pat = format!("\\begin{{{}}}", env_name);
    let end_pat = format!("\\end{{{}}}", env_name);

    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;
    let mut search_start = 0;

    while let Some(begin_pos) = text[search_start..].find(&begin_pat) {
        let abs_begin = search_start + begin_pos;
        let content_start = abs_begin + begin_pat.len();

        if let Some(end_pos) = text[content_start..].find(&end_pat) {
            let abs_end = content_start + end_pos;
            let after_end = abs_end + end_pat.len();

            *counter += 1;

            result.push_str(&text[last_end..abs_begin]);
            result.push_str("\n$$\n");
            result.push_str(&text[content_start..abs_end]);
            result.push_str(&format!(" ({})", counter));
            result.push_str("\n$$\n");

            last_end = after_end;
            search_start = after_end;
        } else {
            search_start = content_start;
        }
    }

    result.push_str(&text[last_end..]);
    result
}

/// Clean `&` and `\\[dim]` inside `$$...$$` math display blocks.
/// Replaces `&` with space and `\\[optional dim]` with newline.
fn clean_math_display_interiors(text: &str) -> String {
    static MATH_LINE_BREAK_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\\\\(\[[^\]]*\])?").unwrap());

    let mut result = String::with_capacity(text.len());
    let mut search_start = 0;

    while let Some(open_pos) = text[search_start..].find("\n$$\n") {
        let abs_open = search_start + open_pos;
        let content_start = abs_open + 4; // skip \n$$\n

        if let Some(close_pos) = text[content_start..].find("\n$$\n") {
            let abs_close = content_start + close_pos;
            // Copy everything before this $$ block
            result.push_str(&text[search_start..content_start]);
            // Clean the interior
            let interior = &text[content_start..abs_close];
            let cleaned = interior.replace('&', " ");
            let cleaned = MATH_LINE_BREAK_RE.replace_all(&cleaned, "\n");
            result.push_str(&cleaned);
            search_start = abs_close;
        } else {
            break;
        }
    }

    result.push_str(&text[search_start..]);
    result
}

/// Replace \begin{env} with open_str and \end{env} with close_str.
fn replace_env_with(text: &str, env_name: &str, open_str: &str, close_str: &str) -> String {
    let begin_pat = format!("\\begin{{{}}}", env_name);
    let end_pat = format!("\\end{{{}}}", env_name);
    text.replace(&begin_pat, open_str).replace(&end_pat, close_str)
}

/// Skip trailing braced groups `{...}` and optional brackets `[...]` after an
/// environment begin marker. Used for `\begin{tabular}{col_spec}` and
/// `\begin{thebibliography}{widest-label}`.
fn skip_trailing_braces_and_options(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() && bytes[i] == b'{' {
        let mut depth = 1;
        i += 1;
        while i < bytes.len() && depth > 0 {
            match bytes[i] {
                b'{' => depth += 1,
                b'}' => depth -= 1,
                _ => {}
            }
            i += 1;
        }
    }
    if i < bytes.len() && bytes[i] == b'[' {
        let mut depth = 1;
        i += 1;
        while i < bytes.len() && depth > 0 {
            match bytes[i] {
                b'[' => depth += 1,
                b']' => depth -= 1,
                _ => {}
            }
            i += 1;
        }
    }
    i
}

/// Convert tabular environments: strip markers (including column spec),
/// replace `&` column separators with spaces.
fn convert_tabular_env(text: &str, env_name: &str) -> String {
    let begin_pat = format!("\\begin{{{}}}", env_name);
    let end_pat = format!("\\end{{{}}}", env_name);

    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;
    let mut search_start = 0;

    while let Some(begin_pos) = text[search_start..].find(&begin_pat) {
        let abs_begin = search_start + begin_pos;
        let after_begin = abs_begin + begin_pat.len();

        let after_opts = skip_trailing_braces_and_options(text.as_bytes(), after_begin);

        if let Some(end_pos) = text[after_opts..].find(&end_pat) {
            let abs_end = after_opts + end_pos;
            let after_end = abs_end + end_pat.len();

            result.push_str(&text[last_end..abs_begin]);
            result.push('\n');

            let cell_content = &text[after_opts..abs_end];
            result.push_str(&cell_content.replace('&', " "));
            result.push('\n');

            last_end = after_end;
            search_start = after_end;
        } else {
            search_start = after_begin;
        }
    }

    result.push_str(&text[last_end..]);
    result
}

/// Convert `\begin{thebibliography}{widest-label}` to a References header.
fn convert_bibliography_env(text: &str) -> String {
    let begin_pat = "\\begin{thebibliography}";
    let end_pat = "\\end{thebibliography}";

    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;
    let mut search_start = 0;

    while let Some(pos) = text[search_start..].find(begin_pat) {
        let abs_pos = search_start + pos;
        let after = abs_pos + begin_pat.len();

        result.push_str(&text[last_end..abs_pos]);
        result.push_str("\n## References\n");

        let skip_to = skip_trailing_braces_and_options(text.as_bytes(), after);

        last_end = skip_to;
        search_start = skip_to;
    }

    result.push_str(&text[last_end..]);
    result.replace(end_pat, "\n")
}

/// Convert `\begin{keywords}` → `**Keywords:** ` prefix.
fn convert_keyword_env(text: &str, env_name: &str) -> String {
    let begin_pat = format!("\\begin{{{}}}", env_name);
    let end_pat = format!("\\end{{{}}}", env_name);
    text.replace(&begin_pat, "\n**Keywords:** ")
        .replace(&end_pat, "\n")
}

/// Remove `\begin{env}` markers, consuming trailing braced/optional args
/// (needed for environments like `\begin{minipage}{0.5\textwidth}`).
fn remove_env_markers_with_args(text: &str, env_name: &str) -> String {
    let begin_pat = format!("\\begin{{{}}}", env_name);
    let end_pat = format!("\\end{{{}}}", env_name);

    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;
    let mut ss = 0;

    while let Some(pos) = text[ss..].find(&begin_pat) {
        let abs = ss + pos;
        let after = abs + begin_pat.len();
        result.push_str(&text[last_end..abs]);
        result.push('\n');
        let skip_to = skip_trailing_braces_and_options(text.as_bytes(), after);
        last_end = skip_to;
        ss = skip_to;
    }
    result.push_str(&text[last_end..]);

    result.replace(&end_pat, "\n")
}

/// Remove remaining \begin{...} and \end{...} for unrecognized environments.
fn remove_remaining_envs(text: &str) -> String {
    static BEGIN_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\\begin\{[^{}]*\}(?:\{[^{}]*\})*(?:\[[^\]]*\])?").unwrap());
    static END_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\\end\{[^{}]*\}").unwrap());

    let result = BEGIN_RE.replace_all(text, "\n");
    END_RE.replace_all(&result, "\n").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ce(text: &str) -> String {
        convert_environments(text, &HashMap::new(), None)
    }

    #[test]
    fn test_itemize() {
        let input = r"\begin{itemize}\item First\item Second\end{itemize}";
        let result = ce(input);
        assert!(result.contains("- First"));
        assert!(result.contains("- Second"));
    }

    #[test]
    fn test_item_with_label() {
        let input = r"\item[Note] Important";
        let result = convert_items(input);
        assert_eq!(result, "\n- Note  Important");
    }

    #[test]
    fn test_item_without_label() {
        let input = r"\item Plain item";
        let result = convert_items(input);
        assert_eq!(result, "\n- Plain item");
    }

    #[test]
    fn test_figure() {
        let input = r"\begin{figure}[htbp]\caption{My figure}\end{figure}";
        let result = ce(input);
        assert!(result.contains("Figure 1: My figure"), "figure caption: {result}");
        assert!(!result.contains("\\begin"));
        assert!(!result.contains("\\end"));
    }

    #[test]
    fn test_theorem() {
        let input = r"\begin{theorem}[Main Result]Content here.\end{theorem}";
        let result = ce(input);
        assert!(result.contains("**Theorem (Main Result).**"));
        assert!(result.contains("Content here."));
    }

    #[test]
    fn test_theorem_no_name() {
        let input = r"\begin{theorem}Content.\end{theorem}";
        let result = ce(input);
        assert!(result.contains("**Theorem.**"));
    }

    #[test]
    fn test_proof() {
        let input = r"\begin{proof}We show that...\end{proof}";
        let result = ce(input);
        assert!(result.contains("**Proof.**"));
    }

    #[test]
    fn test_equation() {
        let input = r"\begin{equation}E = mc^2\end{equation}";
        let result = ce(input);
        assert!(result.contains("$$"));
        assert!(result.contains("E = mc^2"));
        assert!(result.contains("(1)"), "numbered equation: {result}");
    }

    #[test]
    fn test_equation_star_unnumbered() {
        let input = r"\begin{equation*}E = mc^2\end{equation*}";
        let result = ce(input);
        assert!(result.contains("$$"));
        assert!(result.contains("E = mc^2"));
        assert!(!result.contains("(1)"), "starred equation should be unnumbered: {result}");
    }

    #[test]
    fn test_display_math_shortcut() {
        let input = r"\[x = 1\]";
        let result = ce(input);
        assert!(result.contains("$$"));
        assert!(result.contains("x = 1"));
    }

    #[test]
    fn test_unknown_environment() {
        let input = r"\begin{custom}content\end{custom}";
        let result = ce(input);
        assert!(result.contains("content"));
        assert!(!result.contains("\\begin"));
        assert!(!result.contains("\\end"));
    }

    #[test]
    fn test_includegraphics_removed() {
        let input = r"\includegraphics[width=0.5\textwidth]{fig.png}";
        let result = ce(input);
        assert_eq!(result.trim(), "");
    }

    #[test]
    fn test_tabular_basic() {
        let input = r"\begin{tabular}{|l|c|} A & B \\ C & D \end{tabular}";
        let result = ce(input);
        assert!(result.contains("A"));
        assert!(result.contains("B"));
        assert!(!result.contains('&'));
        assert!(!result.contains("\\begin"));
        assert!(!result.contains("\\end"));
    }

    #[test]
    fn test_tabular_inside_table_float() {
        let input = r"\begin{table}[h]\caption{Results}\begin{tabular}{ll} X & Y \end{tabular}\end{table}";
        let result = ce(input);
        assert!(result.contains("Table 1: Results"), "table caption: {result}");
        assert!(result.contains("X"));
        assert!(result.contains("Y"));
        assert!(!result.contains('&'));
    }

    #[test]
    fn test_longtable() {
        let input = r"\begin{longtable}{ccc} A & B & C \\ D & E & F \end{longtable}";
        let result = ce(input);
        assert!(result.contains("A"));
        assert!(result.contains("F"));
        assert!(!result.contains('&'));
    }

    #[test]
    fn test_thebibliography() {
        let input = r"\begin{thebibliography}{99}Some references here.\end{thebibliography}";
        let result = ce(input);
        assert!(result.contains("## References"));
        assert!(result.contains("Some references here."));
        assert!(!result.contains("99"));
        assert!(!result.contains("\\begin"));
    }

    #[test]
    fn test_algorithm_env() {
        let input = r"\begin{algorithmic}\State x = 1\Return x\end{algorithmic}";
        let result = ce(input);
        assert!(result.contains("x = 1"));
        assert!(result.contains("x"));
        assert!(!result.contains("\\State"));
        assert!(!result.contains("\\Return"));
        assert!(!result.contains("\\begin"));
    }

    #[test]
    fn test_keywords_env() {
        let input = r"\begin{keywords}machine learning, NLP\end{keywords}";
        let result = ce(input);
        assert!(result.contains("**Keywords:**"));
        assert!(result.contains("machine learning"));
    }

    #[test]
    fn test_ieee_keywords() {
        let input = r"\begin{IEEEkeywords}deep learning\end{IEEEkeywords}";
        let result = ce(input);
        assert!(result.contains("**Keywords:**"));
        assert!(result.contains("deep learning"));
    }

    #[test]
    fn test_minipage_env() {
        let input = r"\begin{minipage}{0.5\textwidth}Content here\end{minipage}";
        let result = ce(input);
        assert!(result.contains("Content here"));
        assert!(!result.contains("\\begin"));
        assert!(!result.contains("minipage"));
    }

    #[test]
    fn test_verbatim_env() {
        let input = r"\begin{verbatim}print('hello')\end{verbatim}";
        let result = ce(input);
        assert!(result.contains("print('hello')"));
        assert!(!result.contains("\\begin"));
    }

    #[test]
    fn test_multicols_env() {
        let input = r"\begin{multicols}{2}Two column content\end{multicols}";
        let result = ce(input);
        assert!(result.contains("Two column content"));
        assert!(!result.contains("\\begin"));
    }

    #[test]
    fn test_figure_sequential_numbering() {
        let input = r"\begin{figure}\caption{First}\end{figure}\begin{figure}\caption{Second}\end{figure}";
        let result = ce(input);
        assert!(result.contains("Figure 1: First"), "first figure: {result}");
        assert!(result.contains("Figure 2: Second"), "second figure: {result}");
    }

    #[test]
    fn test_table_caption_prefix() {
        let input = r"\begin{table}\caption{My table}\end{table}";
        let result = ce(input);
        assert!(result.contains("Table 1: My table"), "table caption: {result}");
    }

    #[test]
    fn test_question_env() {
        let input = r"\begin{question}What is this?\end{question}";
        let result = ce(input);
        assert!(result.contains("**Question.**"), "question env: {result}");
    }

    #[test]
    fn test_eqnarray_no_ampersands() {
        let input = r"\begin{eqnarray}a &=& b \\ c &=& d\end{eqnarray}";
        let result = ce(input);
        assert!(result.contains("$$"));
        assert!(!result.contains('&'), "eqnarray should have no &: {result}");
        assert!(result.contains("a"));
        assert!(result.contains("b"));
    }

    #[test]
    fn test_array_col_spec_stripped() {
        let input = r"\begin{array}{cc} x & y \\ z & w \end{array}";
        let result = ce(input);
        assert!(!result.contains('&'), "array should have no &: {result}");
        assert!(result.contains("x"));
        assert!(result.contains("w"));
    }

    #[test]
    fn test_custom_theorem_env() {
        let mut custom = HashMap::new();
        custom.insert("dfn".to_string(), "Definition".to_string());
        let input = r"\begin{dfn}[Key]A definition.\end{dfn}";
        let result = convert_environments(input, &custom, None);
        assert!(result.contains("**Definition (Key).**"), "custom theorem: {result}");
        assert!(result.contains("A definition."));
    }

    #[test]
    fn test_custom_theorem_no_name() {
        let mut custom = HashMap::new();
        custom.insert("cor".to_string(), "Corollary".to_string());
        let input = r"\begin{cor}A corollary.\end{cor}";
        let result = convert_environments(input, &custom, None);
        assert!(result.contains("**Corollary.**"), "custom theorem: {result}");
    }

    // --- Stage 5a: proof QED ---

    #[test]
    fn test_proof_qed_appended() {
        let input = r"\begin{proof}This is a proof.\end{proof}";
        let result = ce(input);
        assert!(result.contains("**Proof.**"), "proof label: {result}");
        assert!(result.contains("\u{25A1}"), "QED symbol: {result}");
    }

    #[test]
    fn test_proof_qed_not_duplicated() {
        let input = r"\begin{proof}This has \qed already.\end{proof}";
        let result = ce(input);
        assert!(result.contains("**Proof.**"), "proof label: {result}");
        assert!(!result.contains("\u{25A1}"), "should not add QED when \\qed present: {result}");
    }

    #[test]
    fn test_proof_qedsymbol_not_duplicated() {
        let input = r"\begin{proof}Has \qedsymbol here.\end{proof}";
        let result = ce(input);
        assert!(!result.contains("\u{25A1}"), "should not add QED when \\qedsymbol present: {result}");
    }

    // --- Stage 5b: discard environments ---

    #[test]
    fn test_filecontents_discarded() {
        let input = "before\n\\begin{filecontents}{file.sty}\nsome content\n\\end{filecontents}\nafter";
        let result = ce(input);
        assert!(result.contains("before"));
        assert!(result.contains("after"));
        assert!(!result.contains("some content"));
    }

    #[test]
    fn test_filecontents_star_discarded() {
        let input = "before\n\\begin{filecontents*}{f.tex}\nstuff\n\\end{filecontents*}\nafter";
        let result = ce(input);
        assert!(result.contains("before"));
        assert!(result.contains("after"));
        assert!(!result.contains("stuff"));
    }

    #[test]
    fn test_tikzpicture_discarded() {
        let input = r"\begin{tikzpicture}
\draw (0,0) -- (1,1);
\end{tikzpicture} text after";
        let result = convert_environments(input, &std::collections::HashMap::new(), None);
        assert!(!result.contains("draw"), "tikz should be discarded: {result}");
        assert!(result.contains("text after"));
    }

    #[test]
    fn test_compactitem_list() {
        let input = r"\begin{compactitem}
\item First
\item Second
\end{compactitem}";
        let result = convert_environments(input, &std::collections::HashMap::new(), None);
        assert!(result.contains("First") && result.contains("Second"));
    }
}
