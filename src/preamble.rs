use crate::braces::find_braced_group;
use crate::comments::remove_comments;

/// Extract footnote-like content from the preamble (before \begin{document}).
///
/// Collects text from \thanks{...}, \footnote{...}, and \footnotetext{...}
/// that appear before \begin{document}.
pub fn extract_preamble_extras(file_content: &str) -> Vec<String> {
    let mut extras = Vec::new();

    let doc_start = match file_content.find("\\begin{document}") {
        Some(pos) => pos,
        None => return extras,
    };

    let preamble = &file_content[..doc_start];
    let commands = ["\\thanks", "\\footnote", "\\footnotetext", "\\funding"];

    for cmd in &commands {
        let mut search_start = 0;
        while let Some(pos) = preamble[search_start..].find(cmd) {
            let abs_pos = search_start + pos;
            let after_cmd = abs_pos + cmd.len();

            let bytes = preamble.as_bytes();
            if after_cmd < bytes.len() && bytes[after_cmd].is_ascii_alphabetic() {
                // \footnotetext starts with \footnote, so only skip longer names
                // for commands other than \footnotetext itself
                if *cmd != "\\footnotetext" {
                    search_start = after_cmd;
                    continue;
                }
            }

            let mut brace_pos = after_cmd;
            while brace_pos < preamble.len() {
                let b = bytes[brace_pos];
                if b == b' ' || b == b'\t' || b == b'\n' {
                    brace_pos += 1;
                } else {
                    break;
                }
            }

            if brace_pos < preamble.len() && bytes[brace_pos] == b'{' {
                if let Some((cs, ce)) = find_braced_group(preamble, brace_pos) {
                    extras.push(preamble[cs..ce].to_string());
                    search_start = ce + 1;
                    continue;
                }
            }

            search_start = after_cmd;
        }
    }

    extras
}

/// Return byte ranges (start, end) of `\title{...}` and `\author{...}` blocks
/// that were extracted from the preamble, so nested `\thanks` can be skipped.
fn extract_preamble_metadata_ranges(file_content: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();

    let doc_start = match file_content.find("\\begin{document}") {
        Some(pos) => pos,
        None => return ranges,
    };

    let preamble = &file_content[..doc_start];
    let body_start = doc_start + "\\begin{document}".len();
    let body = &file_content[body_start..];
    let body_has_title = body.contains("\\title{") || body.contains("\\title ");
    let body_has_author = body.contains("\\author{") || body.contains("\\author ");

    for &(cmd, skip_if_body) in &[("\\title", body_has_title), ("\\author", body_has_author)] {
        if skip_if_body {
            continue;
        }
        let mut search_start = 0;
        while let Some(pos) = preamble[search_start..].find(cmd) {
            let abs = search_start + pos;
            let after = abs + cmd.len();
            let bytes = preamble.as_bytes();
            if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
                search_start = after;
                continue;
            }
            let mut cmd_end = after;
            if cmd_end < preamble.len() && bytes[cmd_end] == b'*' {
                cmd_end += 1;
            }
            let after_opt = skip_optional_bracket(preamble, cmd_end);
            if let Some((_, ce)) = extract_braced_after(preamble, after_opt) {
                ranges.push((abs, ce + 1));
                search_start = ce + 1;
            } else {
                search_start = after;
            }
        }
    }

    ranges
}

/// Like `extract_preamble_extras` but skips commands whose position falls
/// inside one of the given ranges (e.g., `\thanks` nested inside `\author{...}`).
fn extract_preamble_extras_excluding(
    file_content: &str,
    exclude_ranges: &[(usize, usize)],
) -> Vec<String> {
    let mut extras = Vec::new();

    let doc_start = match file_content.find("\\begin{document}") {
        Some(pos) => pos,
        None => return extras,
    };

    let preamble = &file_content[..doc_start];
    let commands = ["\\thanks", "\\footnote", "\\footnotetext", "\\funding"];

    for cmd in &commands {
        let mut search_start = 0;
        while let Some(pos) = preamble[search_start..].find(cmd) {
            let abs_pos = search_start + pos;
            let after_cmd = abs_pos + cmd.len();

            let bytes = preamble.as_bytes();
            if after_cmd < bytes.len() && bytes[after_cmd].is_ascii_alphabetic() {
                if *cmd != "\\footnotetext" {
                    search_start = after_cmd;
                    continue;
                }
            }

            if exclude_ranges
                .iter()
                .any(|&(rs, re)| abs_pos >= rs && abs_pos < re)
            {
                search_start = after_cmd;
                continue;
            }

            let mut brace_pos = after_cmd;
            while brace_pos < preamble.len() {
                let b = bytes[brace_pos];
                if b == b' ' || b == b'\t' || b == b'\n' {
                    brace_pos += 1;
                } else {
                    break;
                }
            }

            if brace_pos < preamble.len() && bytes[brace_pos] == b'{' {
                if let Some((cs, ce)) = find_braced_group(preamble, brace_pos) {
                    extras.push(preamble[cs..ce].to_string());
                    search_start = ce + 1;
                    continue;
                }
            }

            search_start = after_cmd;
        }
    }

    extras
}

/// Extract a simple `\command{content}` from preamble text, pushing matches to `items`.
fn extract_simple_command(preamble: &str, cmd: &str, items: &mut Vec<String>) {
    let mut search_start = 0;
    while let Some(pos) = preamble[search_start..].find(cmd) {
        let abs = search_start + pos;
        let after = abs + cmd.len();
        let bytes = preamble.as_bytes();
        if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
            search_start = after;
            continue;
        }
        if let Some((cs, ce)) = extract_braced_after(preamble, after) {
            items.push(preamble[cs..ce].to_string());
            search_start = ce + 1;
        } else {
            search_start = after;
        }
    }
}

/// Extract `\title{...}` and `\author{...}` from the preamble when they
/// appear before `\begin{document}` (common in amsart, amsmath classes).
/// Returns formatted lines to prepend to the body.
fn extract_preamble_metadata(file_content: &str) -> Vec<String> {
    let mut items = Vec::new();

    let doc_start = match file_content.find("\\begin{document}") {
        Some(pos) => pos,
        None => return items,
    };

    // Strip comments to prevent commented-out commands from leaking
    let preamble_raw = &file_content[..doc_start];
    let preamble_owned = remove_comments(preamble_raw);
    let preamble = preamble_owned.as_str();

    // Only extract if these aren't also in the body (some documents duplicate them)
    let body_start = doc_start + "\\begin{document}".len();
    let body = &file_content[body_start..];
    let body_has_title = body.contains("\\title{") || body.contains("\\title ");
    let body_has_author = body.contains("\\author{") || body.contains("\\author ");

    let addr_label_map = build_address_label_map(preamble);

    if !body_has_title {
        let mut search_start = 0;
        while let Some(pos) = preamble[search_start..].find("\\title") {
            let abs = search_start + pos;
            let after = abs + 6;
            let bytes = preamble.as_bytes();
            if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
                search_start = after;
                continue;
            }
            let mut cmd_end = after;
            if cmd_end < preamble.len() && bytes[cmd_end] == b'*' {
                cmd_end += 1;
            }
            let after_opt = skip_optional_bracket(preamble, cmd_end);
            if let Some((cs, ce)) = extract_braced_after(preamble, after_opt) {
                items.push(format!("# {}", &preamble[cs..ce]));
                search_start = ce + 1;
            } else {
                search_start = after;
            }
        }
    }

    if !body_has_author {
        let mut search_start = 0;
        while let Some(pos) = preamble[search_start..].find("\\author") {
            let abs = search_start + pos;
            let after = abs + 7;
            let bytes = preamble.as_bytes();
            if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
                search_start = after;
                continue;
            }
            let mut cmd_end = after;
            if cmd_end < preamble.len() && bytes[cmd_end] == b'*' {
                cmd_end += 1;
            }
            let (labels, after_opt) = extract_optional_bracket(preamble, cmd_end);
            if let Some((cs, ce)) = extract_braced_after(preamble, after_opt) {
                let mut entry = preamble[cs..ce].to_string();
                if let Some(ref labels) = labels {
                    let indices: Vec<String> = labels
                        .split(',')
                        .filter_map(|l| addr_label_map.get(l.trim()).map(|i| i.to_string()))
                        .collect();
                    if !indices.is_empty() {
                        let joined = indices.join(",");
                        for ch in joined.chars() {
                            entry.push(crate::formatting::to_unicode_script(ch, true));
                        }
                    }
                }
                items.push(entry);
                search_start = ce + 1;
            } else {
                search_start = after;
            }
        }
    }

    if !body_has_author {
        extract_simple_command(preamble, "\\authors", &mut items);
    }

    extract_simple_command(preamble, "\\affiliations", &mut items);

    {
        let cmd = "\\affiliation";
        let mut search_start = 0;
        while let Some(pos) = preamble[search_start..].find(cmd) {
            let abs = search_start + pos;
            let after = abs + cmd.len();
            let bytes = preamble.as_bytes();
            if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
                search_start = after;
                continue;
            }
            let after_opt = skip_optional_bracket(preamble, after);
            if let Some((cs, ce)) = extract_braced_after(preamble, after_opt) {
                items.push(preamble[cs..ce].to_string());
                search_start = ce + 1;
            } else {
                search_start = after;
            }
        }
    }

    for cmd in &["\\address"] {
        let mut addr_idx = 0usize;
        let mut search_start = 0;
        while let Some(pos) = preamble[search_start..].find(cmd) {
            let abs = search_start + pos;
            let after = abs + cmd.len();
            let bytes = preamble.as_bytes();
            if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
                search_start = after;
                continue;
            }
            let (_, after_opt) = extract_optional_bracket(preamble, after);
            if let Some((cs, ce)) = extract_braced_after(preamble, after_opt) {
                addr_idx += 1;
                if !addr_label_map.is_empty() {
                    let mut entry = String::new();
                    for ch in addr_idx.to_string().chars() {
                        entry.push(crate::formatting::to_unicode_script(ch, true));
                    }
                    entry.push(' ');
                    entry.push_str(&preamble[cs..ce]);
                    items.push(entry);
                } else {
                    items.push(preamble[cs..ce].to_string());
                }
                search_start = ce + 1;
            } else {
                search_start = after;
            }
        }
    }

    {
        let cmd = "\\institute";
        let mut search_start = 0;
        while let Some(pos) = preamble[search_start..].find(cmd) {
            let abs = search_start + pos;
            let after = abs + cmd.len();
            let bytes = preamble.as_bytes();
            if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
                search_start = after;
                continue;
            }
            if let Some((cs, ce)) = extract_braced_after(preamble, after) {
                let content = &preamble[cs..ce];
                let normalized = {
                    let mut s = String::with_capacity(content.len());
                    let mut i = 0;
                    let cb = content.as_bytes();
                    while i < cb.len() {
                        if i + 1 < cb.len() && cb[i] == b'\\' && cb[i + 1] == b'\\' {
                            s.push(' ');
                            i += 2;
                            if i < cb.len() && cb[i] == b'[' {
                                while i < cb.len() && cb[i] != b']' { i += 1; }
                                if i < cb.len() { i += 1; }
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
                    while let Some(p) = normalized[idx_start..].find(inst_prefix) {
                        let a = idx_start + p;
                        let num_start = a + inst_prefix.len();
                        if let Some(close) = normalized[num_start..].find('}') {
                            let text_start = num_start + close + 1;
                            let text_end = if let Some(next) = normalized[text_start..].find(inst_prefix) {
                                text_start + next
                            } else {
                                normalized.len()
                            };
                            let entry_text = normalized[text_start..text_end].trim();
                            if !entry_text.is_empty() {
                                items.push(entry_text.to_string());
                            }
                            idx_start = text_end;
                        } else {
                            break;
                        }
                    }
                } else {
                    for part in normalized.split("\\and") {
                        let trimmed = part.trim();
                        if !trimmed.is_empty() {
                            items.push(trimmed.to_string());
                        }
                    }
                }
                search_start = ce + 1;
            } else {
                search_start = after;
            }
        }
    }

    // Extract \inst{...} (JPSJ class: institution/affiliation block in preamble)
    // Only extract when it appears standalone (not after \author on the same line),
    // i.e., when there is no preceding \institute (Springer class uses \inst differently).
    if !preamble.contains("\\institute") {
        extract_simple_command(preamble, "\\inst", &mut items);
    }

    // Extract \abst{...} (JPSJ class: abstract as preamble command)
    {
        let mut search_start = 0;
        while let Some(pos) = preamble[search_start..].find("\\abst") {
            let abs = search_start + pos;
            let after = abs + 5;
            let bytes = preamble.as_bytes();
            // Boundary: \abst not \abstract
            if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
                // Allow \abst followed by '{' or whitespace, but not \abstract
                let rest = &preamble[after..];
                if !rest.starts_with('{') {
                    search_start = after;
                    continue;
                }
            }
            if let Some((cs, ce)) = extract_braced_after(preamble, after) {
                items.push(format!("**Abstract.** {}", &preamble[cs..ce]));
                search_start = ce + 1;
            } else {
                search_start = after;
            }
        }
    }

    {
        let mut search_start = 0;
        while let Some(pos) = preamble[search_start..].find("\\email") {
            let abs = search_start + pos;
            let after = abs + 6;
            let bytes = preamble.as_bytes();
            if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
                search_start = after;
                continue;
            }
            let after_opt = skip_optional_bracket(preamble, after);
            if let Some((cs, ce)) = extract_braced_after(preamble, after_opt) {
                items.push(preamble[cs..ce].to_string());
                search_start = ce + 1;
            } else {
                search_start = after;
            }
        }
    }

    extract_simple_command(preamble, "\\emailAdd", &mut items);

    items
}

/// Build a map from `\address[label]` optional-arg labels to sequential indices.
fn build_address_label_map(preamble: &str) -> std::collections::HashMap<String, usize> {
    let mut map = std::collections::HashMap::new();
    let mut idx = 0usize;
    let mut search_start = 0;
    let cmd = "\\address";
    while let Some(pos) = preamble[search_start..].find(cmd) {
        let abs = search_start + pos;
        let after = abs + cmd.len();
        let bytes = preamble.as_bytes();
        if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
            search_start = after;
            continue;
        }
        let (labels, after_opt) = extract_optional_bracket(preamble, after);
        if let Some((_, ce)) = extract_braced_after(preamble, after_opt) {
            idx += 1;
            if let Some(ref labels) = labels {
                for lbl in labels.split(',') {
                    let lbl = lbl.trim();
                    if !lbl.is_empty() {
                        map.insert(lbl.to_string(), idx);
                    }
                }
            }
            search_start = ce + 1;
        } else {
            search_start = after;
        }
    }
    map
}

/// Extract optional `[content]` returning `(Some(content), pos_after)` or `(None, original_pos)`.
fn extract_optional_bracket(text: &str, pos: usize) -> (Option<String>, usize) {
    let bytes = text.as_bytes();
    let mut i = pos;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\n') {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'[' {
        return (None, pos);
    }
    let start = i + 1;
    i += 1;
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
        (Some(text[start..i - 1].to_string()), i)
    } else {
        (None, pos)
    }
}

/// Skip optional `[...]` argument after whitespace. Returns position after `]`
/// or unchanged `pos` if no `[` found.
fn skip_optional_bracket(text: &str, pos: usize) -> usize {
    let bytes = text.as_bytes();
    let mut i = pos;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\n') {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'[' {
        return pos;
    }
    i += 1;
    let mut depth = 1u32;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'[' => depth += 1,
            b']' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    if depth == 0 { i } else { pos }
}

/// Skip whitespace after `pos` and extract the content of the next `{...}` group.
/// Returns `Some((content_start, content_end))`.
fn extract_braced_after(text: &str, pos: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut i = pos;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\n') {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'{' {
        find_braced_group(text, i)
    } else {
        None
    }
}

/// Find `\end{document}` that is NOT inside a LaTeX comment (line starting with `%`).
fn find_end_document(text: &str) -> Option<usize> {
    let target = "\\end{document}";
    let mut search_start = 0;
    while let Some(pos) = text[search_start..].find(target) {
        let abs_pos = search_start + pos;
        // Walk backward to the start of the line
        let line_start = text[..abs_pos].rfind('\n').map(|p| p + 1).unwrap_or(0);
        let before = text[line_start..abs_pos].trim_start();
        if !before.starts_with('%') {
            return Some(abs_pos);
        }
        search_start = abs_pos + target.len();
    }
    None
}

/// Extract the document body between \begin{document} and \end{document}.
/// Returns (preamble_extras, body). If no \begin{document} is found, returns
/// the entire content as the body.
///
/// Preamble extras include \thanks/\footnote content and, when \title/\author
/// appear only in the preamble (e.g. amsart class), those metadata items.
pub fn extract_body(file_content: &str) -> (Vec<String>, String) {
    let mut extras = Vec::new();
    extras.extend(extract_preamble_metadata(file_content));
    let metadata_ranges = extract_preamble_metadata_ranges(file_content);
    extras.extend(extract_preamble_extras_excluding(file_content, &metadata_ranges));

    let body = if let Some(start) = file_content.find("\\begin{document}") {
        let after_begin = start + "\\begin{document}".len();
        if let Some(end) = find_end_document(&file_content[after_begin..]) {
            file_content[after_begin..after_begin + end].to_string()
        } else {
            file_content[after_begin..].to_string()
        }
    } else {
        file_content.to_string()
    };

    (extras, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_thanks() {
        let input = r"\title{My Paper}\thanks{Funded by NSF}\begin{document}body\end{document}";
        let extras = extract_preamble_extras(input);
        assert_eq!(extras, vec!["Funded by NSF"]);
    }

    #[test]
    fn test_extract_footnote() {
        let input = r"\author{John\footnote{MIT}}\begin{document}body\end{document}";
        let extras = extract_preamble_extras(input);
        assert_eq!(extras, vec!["MIT"]);
    }

    #[test]
    fn test_extract_multiple() {
        let input = r"\thanks{A}\footnote{B}\footnotetext{C}\begin{document}body\end{document}";
        let extras = extract_preamble_extras(input);
        assert_eq!(extras.len(), 3);
        assert!(extras.contains(&"A".to_string()));
        assert!(extras.contains(&"B".to_string()));
        assert!(extras.contains(&"C".to_string()));
    }

    #[test]
    fn test_no_preamble() {
        let input = "just body text";
        let extras = extract_preamble_extras(input);
        assert!(extras.is_empty());
    }

    #[test]
    fn test_extract_body() {
        let input = r"\begin{document}hello world\end{document}";
        let (extras, body) = extract_body(input);
        assert!(extras.is_empty());
        assert_eq!(body, "hello world");
    }

    #[test]
    fn test_extract_body_no_document() {
        let input = "just plain text";
        let (_, body) = extract_body(input);
        assert_eq!(body, "just plain text");
    }

    #[test]
    fn test_nested_braces_in_thanks() {
        let input = r"\thanks{Funded by \textbf{NSF} grant {123}}\begin{document}body\end{document}";
        let extras = extract_preamble_extras(input);
        assert_eq!(extras, vec![r"Funded by \textbf{NSF} grant {123}"]);
    }

    #[test]
    fn test_preamble_title_author() {
        let input = r"\author{Diego Santoro}\title{My Paper}\begin{document}\begin{abstract}text\end{abstract}\end{document}";
        let (extras, _body) = extract_body(input);
        assert!(extras.iter().any(|e| e.contains("My Paper")), "title missing: {extras:?}");
        assert!(extras.iter().any(|e| e.contains("Diego Santoro")), "author missing: {extras:?}");
    }

    #[test]
    fn test_preamble_title_not_duplicated() {
        // If \title is in the body too, don't extract from preamble
        let input = r"\title{Preamble Title}\begin{document}\title{Body Title}text\end{document}";
        let (extras, _) = extract_body(input);
        assert!(!extras.iter().any(|e| e.contains("Preamble Title")),
                "should not extract preamble title when body has one: {extras:?}");
    }

    #[test]
    fn test_preamble_address_email() {
        let input = r"\author{Name}\address{University}\email{a@b.com}\begin{document}body\end{document}";
        let (extras, _) = extract_body(input);
        assert!(extras.iter().any(|e| e.contains("University")), "address missing: {extras:?}");
        assert!(extras.iter().any(|e| e.contains("a@b.com")), "email missing: {extras:?}");
    }

    #[test]
    fn test_commented_out_preamble_commands_excluded() {
        let input = "\\author{Real Author}\n%\\email{fake@example.com}\n\\begin{document}body\\end{document}";
        let (extras, _) = extract_body(input);
        assert!(!extras.iter().any(|e| e.contains("fake@example.com")),
                "commented-out email should not appear: {extras:?}");
        assert!(extras.iter().any(|e| e.contains("Real Author")),
                "real author should appear: {extras:?}");
    }

    #[test]
    fn test_preamble_affiliation_singular() {
        let input = r"\author{Name}\affiliation{Princeton University}\begin{document}body\end{document}";
        let (extras, _) = extract_body(input);
        assert!(extras.iter().any(|e| e.contains("Princeton University")),
                "affiliation missing: {extras:?}");
    }

    #[test]
    fn test_preamble_affiliation_with_label() {
        let input = r"\author{Name}\affiliation[a]{Hampton University, Hampton, VA}\begin{document}body\end{document}";
        let (extras, _) = extract_body(input);
        assert!(extras.iter().any(|e| e.contains("Hampton University")),
                "labeled affiliation missing: {extras:?}");
    }

    #[test]
    fn test_preamble_affiliation_not_affiliations() {
        // \affiliations (plural) should still work and not be duplicated
        let input = r"\affiliations{Group Affil}\affiliation{Single Affil}\begin{document}body\end{document}";
        let (extras, _) = extract_body(input);
        assert!(extras.iter().any(|e| e.contains("Group Affil")),
                "affiliations (plural) missing: {extras:?}");
        assert!(extras.iter().any(|e| e.contains("Single Affil")),
                "affiliation (singular) missing: {extras:?}");
    }

    #[test]
    fn test_preamble_emailadd() {
        let input = r"\author{Name}\emailAdd{user@example.com}\begin{document}body\end{document}";
        let (extras, _) = extract_body(input);
        assert!(extras.iter().any(|e| e.contains("user@example.com")),
                "emailAdd missing: {extras:?}");
    }

    #[test]
    fn test_preamble_institute_inst_prefix() {
        let input = r"\institute{\inst{1} Lab A \\ \inst{2} Lab B}\begin{document}body\end{document}";
        let (extras, _) = extract_body(input);
        assert!(extras.iter().any(|e| e.contains("Lab A")),
                "institute inst-prefix Lab A missing: {extras:?}");
        assert!(extras.iter().any(|e| e.contains("Lab B")),
                "institute inst-prefix Lab B missing: {extras:?}");
    }

    #[test]
    fn test_preamble_institute_and_split() {
        let input = r"\institute{Dept A \and Dept B}\begin{document}body\end{document}";
        let (extras, _) = extract_body(input);
        assert!(extras.iter().any(|e| e.contains("Dept A")),
                "institute and-split Dept A missing: {extras:?}");
        assert!(extras.iter().any(|e| e.contains("Dept B")),
                "institute and-split Dept B missing: {extras:?}");
    }

    #[test]
    fn test_commented_end_document_skipped() {
        let input = "\\begin{document}before\n%\\end{document}\nafter\\end{document}";
        let (_, body) = extract_body(input);
        assert!(body.contains("before"), "body should contain text before commented end: {body:?}");
        assert!(body.contains("after"), "body should contain text after commented end: {body:?}");
    }

    #[test]
    fn test_uncommented_end_document_unchanged() {
        let input = r"\begin{document}hello world\end{document}";
        let (_, body) = extract_body(input);
        assert_eq!(body, "hello world");
    }

    #[test]
    fn test_text_before_percent_on_same_line() {
        // % after non-comment text on the same line — \end{document} is still valid
        let input = "\\begin{document}body text % comment\n\\end{document}\nextra";
        let (_, body) = extract_body(input);
        assert_eq!(body, "body text % comment\n");
    }

    #[test]
    fn test_preamble_funding_extracted() {
        let input = r"\funding{Supported by grant X}\begin{document}body\end{document}";
        let extras = extract_preamble_extras(input);
        assert_eq!(extras, vec!["Supported by grant X"]);
    }

    #[test]
    fn test_preamble_funding_in_body_extraction() {
        let input = r"\funding{Partly supported by ANR FREDDA}\begin{document}body\end{document}";
        let (extras, _) = extract_body(input);
        assert!(extras.iter().any(|e| e.contains("ANR FREDDA")),
                "funding missing from extras: {extras:?}");
    }
}
