use crate::braces::{extract_command_arg, skip_optional_arg};
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

static PROCLAIM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\\proclaim\s*(?:\{([^{}]*)\})?\s*").unwrap()
});

static SUBHEAD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\subhead\s*").unwrap());

static PART_COMMAND: (&str, &str) = ("part", "\n# ");

static PARAGRAPH_COMMANDS: &[&str] = &["paragraph", "subparagraph"];

static TITLE_COMMANDS: &[(&str, &str)] = &[
    ("title", "\n# "),
    ("author", "\n"),
    ("authors", "\n"),
    ("date", "\n"),
    ("address", "\n"),
    ("affiliation", "\n"),
    ("affiliations", "\n"),
];

static MAKETITLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\maketitle").unwrap());

static ABSTRACT_BEGIN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\begin\{abstract\}").unwrap());
static ABSTRACT_END_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\end\{abstract\}").unwrap());

static ACK_BEGIN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\begin\{acknowledge?ments?\}").unwrap());
static ACK_END_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\end\{acknowledge?ments?\}").unwrap());

/// Convert LaTeX structural commands to markdown-style headers.
/// Populates `section_label_map` with label -> section number mappings.
pub fn convert_structure(text: &str, section_label_map: &mut HashMap<String, String>) -> String {
    let mut result = text.to_string();

    result = replace_numbered_sections(&result, section_label_map);

    result = replace_sectioning_command(&result, PART_COMMAND.0, PART_COMMAND.1);

    for cmd in PARAGRAPH_COMMANDS {
        result = replace_paragraph_command(&result, cmd);
    }

    // Must run before the Elsevier handler and TITLE_COMMANDS, which would
    // otherwise consume \author and \affiliation first.
    result = convert_revtex_author_affiliation(&result);

    // Must run before TITLE_COMMANDS consumes \author and \address.
    result = convert_labeled_author_address(&result);

    for &(cmd, prefix) in TITLE_COMMANDS {
        result = replace_sectioning_command(&result, cmd, prefix);
    }

    result = MAKETITLE_RE.replace_all(&result, "").to_string();

    result = ABSTRACT_BEGIN_RE
        .replace_all(&result, "\n**Abstract.** ")
        .to_string();
    result = ABSTRACT_END_RE.replace_all(&result, "\n").to_string();

    result = ACK_BEGIN_RE
        .replace_all(&result, "\n## Acknowledgements\n")
        .to_string();
    result = ACK_END_RE.replace_all(&result, "\n").to_string();

    result = replace_sectioning_command(&result, "abstract", "\n**Abstract.** ");

    result = PROCLAIM_RE.replace_all(&result, |caps: &regex::Captures| {
        match caps.get(1) {
            Some(title) if !title.as_str().is_empty() => {
                format!("\n**{}.** ", title.as_str())
            }
            _ => "\n**Theorem.** ".to_string(),
        }
    }).to_string();
    result = result.replace("\\endproclaim", "\n");

    result = SUBHEAD_RE.replace_all(&result, "\n### ").to_string();
    result = result.replace("\\endsubhead", "\n");

    result = convert_institute_and_inst(&result);

    result
}

const NUMBERED_SECTION_COMMANDS: &[(&str, &str, usize)] = &[
    ("chapter", "\n# ", 0),
    ("section", "\n## ", 1),
    ("subsection", "\n### ", 2),
    ("subsubsection", "\n#### ", 3),
];

struct SectionCounters {
    counters: [usize; 4],
}

impl SectionCounters {
    fn new() -> Self {
        Self { counters: [0; 4] }
    }

    /// Increment counter at `depth`, reset deeper levels, and return the
    /// composite section number (e.g. "1.2.3").
    fn increment(&mut self, depth: usize) -> String {
        self.counters[depth] += 1;
        for d in (depth + 1)..4 {
            self.counters[d] = 0;
        }
        let top = (0..=depth).find(|&d| self.counters[d] > 0).unwrap_or(depth);
        (top..=depth)
            .map(|d| self.counters[d].to_string())
            .collect::<Vec<_>>()
            .join(".")
    }
}

fn replace_numbered_sections(text: &str, section_label_map: &mut HashMap<String, String>) -> String {
    let mut result = String::with_capacity(text.len());
    let mut counters = SectionCounters::new();
    let mut last_end = 0;
    let mut search_start = 0;

    loop {
        let mut best: Option<(usize, usize, &str, usize)> = None;

        for &(cmd, prefix, depth) in NUMBERED_SECTION_COMMANDS {
            let pattern = format!("\\{}", cmd);
            if let Some(pos) = text[search_start..].find(&pattern) {
                let abs_pos = search_start + pos;
                let after = abs_pos + pattern.len();
                let bytes = text.as_bytes();

                let mut cmd_end = after;
                if cmd_end < bytes.len() && bytes[cmd_end] == b'*' {
                    cmd_end += 1;
                }
                if cmd_end < bytes.len() && bytes[cmd_end].is_ascii_alphabetic() {
                    continue;
                }

                if best.is_none() || abs_pos < best.unwrap().0 {
                    let is_starred = after < bytes.len() && bytes[after] == b'*';
                    // Sentinel depth 99 distinguishes starred (unnumbered) from numbered variants
                    let effective_depth = if is_starred { 99 } else { depth };
                    best = Some((abs_pos, cmd_end, prefix, effective_depth));
                }
            }
        }

        let (abs_pos, cmd_end, prefix, depth) = match best {
            Some(b) => b,
            None => break,
        };

        let is_starred = depth == 99;

        let after_opt = skip_optional_arg(text, cmd_end);

        if let Some((cs, ce, after_close)) = extract_command_arg(text, after_opt) {
            result.push_str(&text[last_end..abs_pos]);
            result.push_str(prefix);

            if !is_starred {
                let real_depth = NUMBERED_SECTION_COMMANDS.iter()
                    .find(|&&(_, p, _)| p == prefix)
                    .map(|&(_, _, d)| d)
                    .unwrap_or(1);
                let number = counters.increment(real_depth);
                result.push_str(&number);
                result.push_str(". ");

                let current_section_number = number;
                let mut label_scan = after_close;
                let bytes = text.as_bytes();
                while label_scan < bytes.len()
                    && (bytes[label_scan] == b' ' || bytes[label_scan] == b'\t' || bytes[label_scan] == b'\n')
                {
                    label_scan += 1;
                }
                if text[label_scan..].starts_with("\\label") {
                    let label_cmd_end = label_scan + 6;
                    if let Some((ls, le, _)) = extract_command_arg(text, label_cmd_end) {
                        let label_key = text[ls..le].to_string();
                        section_label_map.insert(label_key, current_section_number);
                    }
                }
            }

            result.push_str(&text[cs..ce]);
            result.push('\n');
            last_end = after_close;
            search_start = after_close;
        } else {
            search_start = cmd_end;
        }
    }

    result.push_str(&text[last_end..]);
    result
}

fn replace_sectioning_command(text: &str, cmd_name: &str, prefix: &str) -> String {
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

        let after_opt = skip_optional_arg(text, cmd_end);

        if let Some((cs, ce, after_close)) = extract_command_arg(text, after_opt) {
            result.push_str(&text[last_end..abs_pos]);
            result.push_str(prefix);
            result.push_str(&text[cs..ce]);
            result.push('\n');
            last_end = after_close;
            search_start = after_close;
        } else {
            search_start = after_opt;
        }
    }

    result.push_str(&text[last_end..]);
    result
}

fn replace_paragraph_command(text: &str, cmd_name: &str) -> String {
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

        let after_opt = skip_optional_arg(text, cmd_end);

        if let Some((cs, ce, after_close)) = extract_command_arg(text, after_opt) {
            result.push_str(&text[last_end..abs_pos]);
            result.push_str("\n**");
            result.push_str(&text[cs..ce]);
            result.push_str("** ");
            last_end = after_close;
            search_start = after_close;
        } else {
            search_start = after_opt;
        }
    }

    result.push_str(&text[last_end..]);
    result
}

/// Detect REVTeX `superscriptaddress` pattern and assign superscript indices
/// linking authors to their affiliations. Only activates when there are
/// multiple `\author` commands AND at least one `\affiliation`, and the
/// `\author` commands lack `[label]` (which would indicate Elsevier style).
fn convert_revtex_author_affiliation(text: &str) -> String {
    let auth_pat = "\\author";
    let affil_pat = "\\affiliation";

    if !text.contains(auth_pat) || !text.contains(affil_pat) {
        return text.to_string();
    }

    let altaffil_pat = "\\altaffiliation";

    #[derive(Debug)]
    enum Entry {
        Author { start: usize, end: usize, content: String },
        Affiliation { start: usize, end: usize, content: String },
        AltAffiliation { start: usize, end: usize, content: String },
    }

    let mut entries: Vec<Entry> = Vec::new();
    let bytes = text.as_bytes();
    let mut search_start = 0;

    loop {
        let next_auth = text[search_start..].find(auth_pat).map(|p| search_start + p);
        let next_affil = text[search_start..].find(affil_pat).map(|p| search_start + p);
        let next_altaffil = text[search_start..].find(altaffil_pat).map(|p| search_start + p);

        // \affiliation is a prefix of \altaffiliation; prefer the longer match
        // when both fire at the same position.
        let next_affil = next_affil.and_then(|f| {
            if let Some(a) = next_altaffil {
                if f == a { None } else { Some(f) }
            } else {
                Some(f)
            }
        });

        #[derive(PartialEq)]
        enum Kind { Author, Affil, AltAffil }
        let candidates: Vec<(usize, Kind)> = [
            next_auth.map(|p| (p, Kind::Author)),
            next_affil.map(|p| (p, Kind::Affil)),
            next_altaffil.map(|p| (p, Kind::AltAffil)),
        ]
        .into_iter()
        .flatten()
        .collect();

        let winner = candidates.iter().min_by_key(|(pos, _)| *pos);
        let (abs_pos, kind) = match winner {
            Some((p, k)) => (*p, k),
            None => break,
        };

        match kind {
            Kind::Author => {
                let after = abs_pos + auth_pat.len();
                if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
                    search_start = after;
                    continue;
                }
                // [label] indicates Elsevier style; defer to that handler
                let ws_pos = skip_ws_bytes(bytes, after);
                if ws_pos < bytes.len() && bytes[ws_pos] == b'[' {
                    return text.to_string();
                }
                if let Some((cs, ce, after_close)) = extract_command_arg(text, after) {
                    entries.push(Entry::Author {
                        start: abs_pos,
                        end: after_close,
                        content: text[cs..ce].to_string(),
                    });
                    search_start = after_close;
                } else {
                    search_start = after;
                }
            }
            Kind::Affil => {
                let after = abs_pos + affil_pat.len();
                if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
                    search_start = after;
                    continue;
                }
                if let Some((cs, ce, after_close)) = extract_command_arg(text, after) {
                    entries.push(Entry::Affiliation {
                        start: abs_pos,
                        end: after_close,
                        content: text[cs..ce].to_string(),
                    });
                    search_start = after_close;
                } else {
                    search_start = after;
                }
            }
            Kind::AltAffil => {
                let after = abs_pos + altaffil_pat.len();
                if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
                    search_start = after;
                    continue;
                }
                let after_opt = {
                    let mut i = after;
                    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\n') {
                        i += 1;
                    }
                    if i < bytes.len() && bytes[i] == b'[' {
                        i += 1;
                        let mut depth = 1u32;
                        while i < bytes.len() && depth > 0 {
                            match bytes[i] { b'[' => depth += 1, b']' => depth -= 1, _ => {} }
                            i += 1;
                        }
                        if depth == 0 { i } else { after }
                    } else {
                        after
                    }
                };
                if let Some((cs, ce, after_close)) = extract_command_arg(text, after_opt) {
                    entries.push(Entry::AltAffiliation {
                        start: abs_pos,
                        end: after_close,
                        content: text[cs..ce].to_string(),
                    });
                    search_start = after_close;
                } else {
                    search_start = after;
                }
            }
        }
    }

    let num_authors = entries.iter().filter(|e| matches!(e, Entry::Author { .. })).count();
    let num_affils = entries.iter().filter(|e| matches!(e, Entry::Affiliation { .. })).count();
    if num_authors < 2 || num_affils == 0 {
        return text.to_string();
    }

    let mut affil_map: HashMap<String, usize> = HashMap::new();
    let mut affil_list: Vec<String> = Vec::new();
    for entry in &entries {
        let content = match entry {
            Entry::Affiliation { content, .. } => Some(content),
            Entry::AltAffiliation { content, .. } => Some(content),
            _ => None,
        };
        if let Some(content) = content {
            let trimmed = content.trim().to_string();
            if !affil_map.contains_key(&trimmed) {
                let idx = affil_list.len() + 1;
                affil_map.insert(trimmed.clone(), idx);
                affil_list.push(trimmed);
            }
        }
    }

    // REVTeX rule: an \affiliation applies to all preceding \author entries
    // since the last \affiliation; \altaffiliation applies only to the
    // immediately preceding author.
    let mut author_affils: Vec<(usize, Vec<usize>)> = Vec::new();
    let mut pending_authors: Vec<usize> = Vec::new();

    for (i, entry) in entries.iter().enumerate() {
        match entry {
            Entry::Author { .. } => {
                pending_authors.push(i);
            }
            Entry::Affiliation { content, .. } => {
                let trimmed = content.trim().to_string();
                if let Some(&idx) = affil_map.get(&trimmed) {
                    for &ai in &pending_authors {
                        if let Some(existing) = author_affils.iter_mut().find(|(ei, _)| *ei == ai) {
                            if !existing.1.contains(&idx) {
                                existing.1.push(idx);
                            }
                        } else {
                            author_affils.push((ai, vec![idx]));
                        }
                    }
                }
                pending_authors.clear();
            }
            Entry::AltAffiliation { content, .. } => {
                let trimmed = content.trim().to_string();
                if let Some(&idx) = affil_map.get(&trimmed) {
                    let last_author_idx = entries[..i].iter().rposition(|e| matches!(e, Entry::Author { .. }));
                    if let Some(ai) = last_author_idx {
                        if let Some(existing) = author_affils.iter_mut().find(|(ei, _)| *ei == ai) {
                            if !existing.1.contains(&idx) {
                                existing.1.push(idx);
                            }
                        } else {
                            author_affils.push((ai, vec![idx]));
                        }
                    }
                }
            }
        }
    }

    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;
    let mut affils_emitted = false;

    for (i, entry) in entries.iter().enumerate() {
        match entry {
            Entry::Author { start, end, content } => {
                result.push_str(&text[last_end..*start]);
                result.push('\n');
                result.push_str(content);
                if let Some((_, indices)) = author_affils.iter().find(|(ei, _)| *ei == i) {
                    for (j, idx) in indices.iter().enumerate() {
                        if j > 0 {
                            result.push(crate::formatting::to_unicode_script(',', true));
                        }
                        for ch in idx.to_string().chars() {
                            result.push(crate::formatting::to_unicode_script(ch, true));
                        }
                    }
                }
                last_end = *end;
            }
            Entry::Affiliation { start, end, content } | Entry::AltAffiliation { start, end, content } => {
                result.push_str(&text[last_end..*start]);
                if !affils_emitted {
                    result.push('\n');
                    for (idx, affil) in affil_list.iter().enumerate() {
                        for ch in (idx + 1).to_string().chars() {
                            result.push(crate::formatting::to_unicode_script(ch, true));
                        }
                        result.push(' ');
                        result.push_str(affil);
                        result.push('\n');
                    }
                    affils_emitted = true;
                }
                let _ = content;
                last_end = *end;
            }
        }
    }

    result.push_str(&text[last_end..]);
    result
}

/// Convert Elsevier-style `\author[label]{Name}` / `\address[label]{Affiliation}`
/// into numbered cross-references. Only activates when `\address` commands have
/// optional `[label]` arguments.
fn convert_labeled_author_address(text: &str) -> String {
    let addr_pat = "\\address";
    let has_labeled_address = {
        let mut found = false;
        let mut ss = 0;
        while let Some(pos) = text[ss..].find(addr_pat) {
            let abs = ss + pos;
            let after = abs + addr_pat.len();
            let bytes = text.as_bytes();
            if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
                ss = after;
                continue;
            }
            let mut i = after;
            while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\n') {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'[' {
                found = true;
                break;
            }
            ss = after;
        }
        found
    };

    if !has_labeled_address {
        return text.to_string();
    }

    let mut label_map: HashMap<String, usize> = HashMap::new();
    let mut addr_entries: Vec<(usize, usize, String)> = Vec::new();
    let mut addr_index = 0usize;

    {
        let mut ss = 0;
        while let Some(pos) = text[ss..].find(addr_pat) {
            let abs = ss + pos;
            let after = abs + addr_pat.len();
            let bytes = text.as_bytes();
            if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
                ss = after;
                continue;
            }
            let opt_start = skip_ws_bytes(bytes, after);
            if opt_start < bytes.len() && bytes[opt_start] == b'[' {
                if let Some((label, after_bracket)) = extract_bracket_content(text, opt_start) {
                    if let Some((cs, ce, after_close)) = extract_command_arg(text, after_bracket) {
                        addr_index += 1;
                        for lbl in label.split(',') {
                            let lbl = lbl.trim();
                            if !lbl.is_empty() {
                                label_map.insert(lbl.to_string(), addr_index);
                            }
                        }
                        let content = text[cs..ce].to_string();
                        addr_entries.push((abs, after_close, content));
                        ss = after_close;
                        continue;
                    }
                }
            }
            ss = after;
        }
    }

    if label_map.is_empty() {
        return text.to_string();
    }

    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;
    let mut addr_idx = 0;
    for (start, end, content) in &addr_entries {
        addr_idx += 1;
        result.push_str(&text[last_end..*start]);
        result.push('\n');
        for ch in addr_idx.to_string().chars() {
            result.push(crate::formatting::to_unicode_script(ch, true));
        }
        result.push(' ');
        result.push_str(content);
        result.push('\n');
        last_end = *end;
    }
    result.push_str(&text[last_end..]);

    let auth_pat = "\\author";
    let mut resolved = String::with_capacity(result.len());
    last_end = 0;
    let mut ss = 0;

    while let Some(pos) = result[ss..].find(auth_pat) {
        let abs = ss + pos;
        let after = abs + auth_pat.len();
        let bytes = result.as_bytes();
        if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
            ss = after;
            continue;
        }
        let opt_start = skip_ws_bytes(bytes, after);
        if opt_start < bytes.len() && bytes[opt_start] == b'[' {
            if let Some((labels_str, after_bracket)) = extract_bracket_content(&result, opt_start) {
                if let Some((cs, ce, after_close)) = extract_command_arg(&result, after_bracket) {
                    resolved.push_str(&result[last_end..abs]);
                    resolved.push('\n');
                    resolved.push_str(&result[cs..ce]);
                    let indices: Vec<String> = labels_str
                        .split(',')
                        .filter_map(|lbl| {
                            let lbl = lbl.trim();
                            label_map.get(lbl).map(|idx| idx.to_string())
                        })
                        .collect();
                    if !indices.is_empty() {
                        let joined = indices.join(",");
                        for ch in joined.chars() {
                            resolved.push(crate::formatting::to_unicode_script(ch, true));
                        }
                    }
                    last_end = after_close;
                    ss = after_close;
                    continue;
                }
            }
        }
        ss = after;
    }
    resolved.push_str(&result[last_end..]);
    resolved
}

fn skip_ws_bytes(bytes: &[u8], pos: usize) -> usize {
    let mut i = pos;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\n') {
        i += 1;
    }
    i
}

fn extract_bracket_content(text: &str, pos: usize) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    if pos >= bytes.len() || bytes[pos] != b'[' {
        return None;
    }
    let mut i = pos + 1;
    let mut depth = 1u32;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'[' => depth += 1,
            b']' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    if depth == 0 {
        Some((text[pos + 1..i - 1].to_string(), i))
    } else {
        None
    }
}

static LABEL_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\label\{([^{}]*)\}").unwrap());

static REF_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\ref\{([^{}]*)\}").unwrap());

/// Convert `\institute{...}` to a numbered affiliation list and resolve
/// `\inst{...}` on author names using the label->index mapping.
fn convert_institute_and_inst(text: &str) -> String {
    let mut label_map: HashMap<String, usize> = HashMap::new();
    let mut result = String::with_capacity(text.len());
    let inst_pattern = "\\institute";
    let mut last_end = 0;
    let mut search_start = 0;

    while let Some(pos) = text[search_start..].find(inst_pattern) {
        let abs_pos = search_start + pos;
        let after = abs_pos + inst_pattern.len();
        let bytes = text.as_bytes();

        if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
            search_start = after;
            continue;
        }

        if let Some((cs, ce, after_close)) = extract_command_arg(text, after) {
            result.push_str(&text[last_end..abs_pos]);
            let content = &text[cs..ce];

            let normalized = {
                let mut s = String::with_capacity(content.len());
                let mut i = 0;
                let cb = content.as_bytes();
                while i < cb.len() {
                    if i + 1 < cb.len() && cb[i] == b'\\' && cb[i + 1] == b'\\' {
                        s.push(' ');
                        i += 2;
                        if i < cb.len() && cb[i] == b'[' {
                            while i < cb.len() && cb[i] != b']' {
                                i += 1;
                            }
                            if i < cb.len() {
                                i += 1;
                            }
                        }
                    } else {
                        s.push(cb[i] as char);
                        i += 1;
                    }
                }
                s
            };

            let use_inst_split = !normalized.contains("\\and") && normalized.contains("\\inst{");

            if use_inst_split {
                let inst_prefix = "\\inst{";
                let mut idx_start = 0;
                let mut entries: Vec<(usize, String)> = Vec::new();
                while let Some(pos) = normalized[idx_start..].find(inst_prefix) {
                    let abs = idx_start + pos;
                    let num_start = abs + inst_prefix.len();
                    if let Some(close) = normalized[num_start..].find('}') {
                        let num_str = normalized[num_start..num_start + close].trim();
                        let idx: usize = num_str.parse().unwrap_or(entries.len() + 1);
                        let text_start = num_start + close + 1;
                        let text_end = if let Some(next) = normalized[text_start..].find(inst_prefix) {
                            text_start + next
                        } else {
                            normalized.len()
                        };
                        let entry_text = normalized[text_start..text_end].trim();
                        if !entry_text.is_empty() {
                            for cap in LABEL_KEY_RE.captures_iter(entry_text) {
                                let key = cap.get(1).unwrap().as_str().to_string();
                                label_map.insert(key, idx);
                            }
                            let clean = LABEL_KEY_RE.replace_all(entry_text, "");
                            let clean = clean.trim();
                            entries.push((idx, clean.to_string()));
                        }
                        idx_start = text_end;
                    } else {
                        break;
                    }
                }
                for (idx, text) in &entries {
                    result.push('\n');
                    for ch in idx.to_string().chars() {
                        result.push(crate::formatting::to_unicode_script(ch, true));
                    }
                    result.push(' ');
                    result.push_str(text);
                }
            } else {
                let parts: Vec<&str> = normalized.split("\\and").collect();
                for (i, part) in parts.iter().enumerate() {
                    let trimmed = part.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let idx = i + 1;
                    for cap in LABEL_KEY_RE.captures_iter(trimmed) {
                        let key = cap.get(1).unwrap().as_str().to_string();
                        label_map.insert(key, idx);
                    }
                    let clean = LABEL_KEY_RE.replace_all(trimmed, "");
                    let clean = clean.trim();
                    result.push('\n');
                    for ch in idx.to_string().chars() {
                        result.push(crate::formatting::to_unicode_script(ch, true));
                    }
                    result.push(' ');
                    result.push_str(clean);
                }
            }
            result.push('\n');
            last_end = after_close;
            search_start = after_close;
        } else {
            search_start = after;
        }
    }
    result.push_str(&text[last_end..]);

    let inst_cmd = "\\inst";
    let mut resolved = String::with_capacity(result.len());
    last_end = 0;
    search_start = 0;

    while let Some(pos) = result[search_start..].find(inst_cmd) {
        let abs_pos = search_start + pos;
        let after = abs_pos + inst_cmd.len();
        let bytes = result.as_bytes();

        if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
            search_start = after;
            continue;
        }

        if let Some((cs, ce, after_close)) = extract_command_arg(&result, after) {
            resolved.push_str(&result[last_end..abs_pos]);
            let content = &result[cs..ce];

            let mut indices: Vec<String> = Vec::new();
            for part in content.split(',') {
                let part = part.trim();
                if let Some(cap) = REF_KEY_RE.captures(part) {
                    let key = cap.get(1).unwrap().as_str();
                    if let Some(&idx) = label_map.get(key) {
                        indices.push(idx.to_string());
                    } else {
                        indices.push(part.to_string());
                    }
                } else if part.chars().all(|c| c.is_ascii_digit()) {
                    indices.push(part.to_string());
                } else {
                    indices.push(part.to_string());
                }
            }

            let joined = indices.join(",");
            let all_numeric = joined
                .chars()
                .all(|c| c.is_ascii_digit() || c == ',');
            if all_numeric {
                for ch in joined.chars() {
                    resolved.push(crate::formatting::to_unicode_script(ch, true));
                }
            } else {
                resolved.push_str(&joined);
            }

            last_end = after_close;
            search_start = after_close;
        } else {
            search_start = after;
        }
    }
    resolved.push_str(&result[last_end..]);
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cs(text: &str) -> String {
        convert_structure(text, &mut HashMap::new())
    }

    #[test]
    fn test_section() {
        let result = cs(r"\section{Introduction}");
        assert_eq!(result, "\n## 1. Introduction\n");
    }

    #[test]
    fn test_section_starred() {
        let result = cs(r"\section*{Introduction}");
        assert_eq!(result, "\n## Introduction\n");
    }

    #[test]
    fn test_section_with_optional() {
        let result = cs(r"\section[short]{Full Title Here}");
        assert_eq!(result, "\n## 1. Full Title Here\n");
    }

    #[test]
    fn test_section_nested_braces() {
        let result = cs(r"\section{The $\mathcal{O}(n)$ Algorithm}");
        assert_eq!(result, "\n## 1. The $\\mathcal{O}(n)$ Algorithm\n");
    }

    #[test]
    fn test_chapter() {
        let result = cs(r"\chapter{Background}");
        assert_eq!(result, "\n# 1. Background\n");
    }

    #[test]
    fn test_subsection() {
        let result = cs(r"\section{A}\subsection{Details}");
        assert!(result.contains("### 1.1. Details"), "subsection: {result}");
    }

    #[test]
    fn test_paragraph() {
        let result = cs(r"\paragraph{Note}");
        assert_eq!(result, "\n**Note** ");
    }

    #[test]
    fn test_title() {
        let result = cs(r"\title{My Paper}");
        assert_eq!(result, "\n# My Paper\n");
    }

    #[test]
    fn test_title_nested() {
        let result = cs(r"\title{\textbf{A \emph{nested} title}}");
        assert_eq!(result, "\n# \\textbf{A \\emph{nested} title}\n");
    }

    #[test]
    fn test_abstract() {
        let result = cs(r"\begin{abstract}Some text\end{abstract}");
        assert_eq!(result, "\n**Abstract.** Some text\n");
    }

    #[test]
    fn test_acknowledgements() {
        let result = cs(r"\begin{acknowledgements}Thanks\end{acknowledgements}");
        assert_eq!(result, "\n## Acknowledgements\nThanks\n");
    }

    #[test]
    fn test_acknowledgments_spelling() {
        let result = cs(r"\begin{acknowledgments}Thanks\end{acknowledgments}");
        assert_eq!(result, "\n## Acknowledgements\nThanks\n");
    }

    #[test]
    fn test_maketitle_removed() {
        let result = cs(r"\maketitle");
        assert_eq!(result, "");
    }

    #[test]
    fn test_multiple_sections() {
        let input = r"\section{A}\subsection{B}\section{C}";
        let result = cs(input);
        assert!(result.contains("## 1. A"), "section 1: {result}");
        assert!(result.contains("### 1.1. B"), "subsection 1.1: {result}");
        assert!(result.contains("## 2. C"), "section 2: {result}");
    }

    #[test]
    fn test_section_numbering_hierarchy() {
        let input = r"\section{A}\subsection{B}\subsection{C}\section{D}";
        let result = cs(input);
        assert!(result.contains("## 1. A"), "sec 1: {result}");
        assert!(result.contains("### 1.1. B"), "subsec 1.1: {result}");
        assert!(result.contains("### 1.2. C"), "subsec 1.2: {result}");
        assert!(result.contains("## 2. D"), "sec 2: {result}");
    }

    #[test]
    fn test_section_label_map_populated() {
        let mut labels = HashMap::new();
        let input = r"\section{Intro}\label{sec:intro}\section{Methods}\label{sec:methods}";
        convert_structure(input, &mut labels);
        assert_eq!(labels.get("sec:intro").map(|s| s.as_str()), Some("1"));
        assert_eq!(labels.get("sec:methods").map(|s| s.as_str()), Some("2"));
    }

    #[test]
    fn test_revtex_superscriptaddress() {
        let input = r"\author{Alice}\author{Bob}\affiliation{MIT}\author{Carol}\affiliation{Stanford}";
        let result = cs(input);
        assert!(result.contains("Alice\u{00B9}"), "Alice should have ¹: {result}");
        assert!(result.contains("Bob\u{00B9}"), "Bob should have ¹: {result}");
        assert!(result.contains("Carol\u{00B2}"), "Carol should have ²: {result}");
        assert!(result.contains("\u{00B9} MIT"), "MIT should have ¹: {result}");
        assert!(result.contains("\u{00B2} Stanford"), "Stanford should have ²: {result}");
    }

    #[test]
    fn test_institute_numbered() {
        let input = r"\institute{Dept A \and Dept B \and Dept C}";
        let result = cs(input);
        assert!(result.contains("\u{00B9} Dept A"), "missing ¹ Dept A: {result}");
        assert!(result.contains("\u{00B2} Dept B"), "missing ² Dept B: {result}");
        assert!(result.contains("\u{00B3} Dept C"), "missing ³ Dept C: {result}");
    }

    #[test]
    fn test_institute_with_labels_and_inst_refs() {
        let input = concat!(
            r"\author{Smith\inst{\ref{inst:A},\ref{inst:B}} \and Jones\inst{\ref{inst:B}}}",
            r"\institute{Dept A\label{inst:A} \and Dept B\label{inst:B}}"
        );
        let result = cs(input);
        assert!(result.contains("Smith\u{00B9},\u{00B2}"), "expected Smith¹,²: {result}");
        assert!(result.contains("Jones\u{00B2}"), "expected Jones²: {result}");
        assert!(result.contains("\u{00B9} Dept A"), "missing ¹ Dept A: {result}");
        assert!(result.contains("\u{00B2} Dept B"), "missing ² Dept B: {result}");
    }

    #[test]
    fn test_inst_bare_numbers() {
        let input = r"\institute{Dept A \and Dept B}\author{Smith\inst{1,2}}";
        let result = cs(input);
        assert!(result.contains("Smith\u{00B9},\u{00B2}"), "expected Smith¹,²: {result}");
    }

    #[test]
    fn test_authors_plural() {
        let result = cs(r"\authors{Smith and Jones}");
        assert!(result.contains("Smith and Jones"), "authors missing: {result}");
    }

    #[test]
    fn test_abstract_command() {
        let result = cs(r"\abstract{Some abstract text}");
        assert!(result.contains("**Abstract.** Some abstract text"), "abstract command: {result}");
    }

    #[test]
    fn test_proclaim_endproclaim() {
        let input = r"\proclaim{Theorem 1} Content here.\endproclaim";
        let result = cs(input);
        assert!(result.contains("**Theorem 1.**"), "proclaim: {result}");
        assert!(result.contains("Content here."), "proclaim content: {result}");
    }

    #[test]
    fn test_subhead_endsubhead() {
        let input = r"\subhead Methods\endsubhead";
        let result = cs(input);
        assert!(result.contains("### Methods"), "subhead: {result}");
    }

    #[test]
    fn test_institute_inst_prefix_format() {
        let input = r"\institute{\inst{1} Laboratoire A \\ \inst{2} Instituut B}";
        let result = cs(input);
        assert!(result.contains("\u{00B9} Laboratoire A"), "missing ¹ Laboratoire A: {result}");
        assert!(result.contains("\u{00B2} Instituut B"), "missing ² Instituut B: {result}");
    }

    #[test]
    fn test_revtex_altaffiliation() {
        let input = r"\author{Alice}\affiliation{MIT}\author{Bob}\altaffiliation{Also at: CERN}\affiliation{Stanford}";
        let result = cs(input);
        assert!(result.contains("Also at: CERN"), "altaffiliation content missing: {result}");
        assert!(result.contains("Bob"), "Bob missing: {result}");
    }
}
