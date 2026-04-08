/// Find the matching closing brace for an opening brace at `open_pos`.
///
/// Returns `Some((content_start, content_end))` where content is between the braces
/// (exclusive of the braces themselves). `content_end` is the index of the closing `}`.
///
/// Handles arbitrary nesting depth and backslash-escaped braces.
pub fn find_braced_group(s: &str, open_pos: usize) -> Option<(usize, usize)> {
    let bytes = s.as_bytes();
    if bytes.get(open_pos) != Some(&b'{') {
        return None;
    }
    let content_start = open_pos + 1;
    let mut depth: u32 = 1;
    let mut i = content_start;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'\\' => {
                i += 1;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((content_start, i));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Tolerant variant of `find_braced_group`. If no matching `}` is found,
/// returns content up to the next paragraph break (`\n\n`), next `\` command,
/// or end of string, rather than `None`.
///
/// Use this for formatting commands where dropping content entirely is worse
/// than a slightly imprecise boundary (e.g., `\textbf{unclosed text`).
pub fn find_braced_group_tolerant(s: &str, open_pos: usize) -> Option<(usize, usize)> {
    // Try strict matching first
    if let result @ Some(_) = find_braced_group(s, open_pos) {
        return result;
    }
    let bytes = s.as_bytes();
    if bytes.get(open_pos) != Some(&b'{') {
        return None;
    }
    let content_start = open_pos + 1;
    // Fallback: scan to the next paragraph break, next \ command, or end of line
    let mut i = content_start;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' if i + 1 < bytes.len() && bytes[i + 1] == b'\n' => {
                return Some((content_start, i));
            }
            b'\\' if i + 1 < bytes.len() && bytes[i + 1].is_ascii_alphabetic() => {
                return Some((content_start, i));
            }
            _ => {}
        }
        i += 1;
    }
    // Hit end of string
    if i > content_start {
        Some((content_start, i))
    } else {
        None
    }
}

/// After a command name ends at position `cmd_end`, skip optional whitespace,
/// look for `{`, and extract the braced argument.
///
/// Returns `Some((content_start, content_end, after_close))` where:
/// - `content_start..content_end` is the content inside the braces
/// - `after_close` is the position right after the closing `}`
pub fn extract_command_arg(s: &str, cmd_end: usize) -> Option<(usize, usize, usize)> {
    let bytes = s.as_bytes();
    let mut i = cmd_end;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\n') {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'{' {
        return None;
    }
    let (content_start, content_end) = find_braced_group(s, i)?;
    Some((content_start, content_end, content_end + 1))
}

/// Tolerant variant of `extract_command_arg`. Falls back to
/// `find_braced_group_tolerant` when strict brace matching fails.
pub fn extract_command_arg_tolerant(s: &str, cmd_end: usize) -> Option<(usize, usize, usize)> {
    let bytes = s.as_bytes();
    let mut i = cmd_end;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\n') {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'{' {
        return None;
    }
    let (content_start, content_end) = find_braced_group_tolerant(s, i)?;
    // If strict match succeeded, content_end is at '}' — skip past it.
    // If fallback was used, content_end is at the boundary — don't skip.
    let after_close = if content_end < bytes.len() && bytes[content_end] == b'}' {
        content_end + 1
    } else {
        content_end
    };
    Some((content_start, content_end, after_close))
}

/// Skip an optional argument `[...]` if present at position `pos`.
/// Returns the position after the closing `]`, or `pos` if no `[` found.
pub fn skip_optional_arg(s: &str, pos: usize) -> usize {
    let bytes = s.as_bytes();
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
            b'\\' => {
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    if depth == 0 { i } else { pos }
}

/// Extract the content of an optional `[...]` argument starting at `pos`.
///
/// Skips leading whitespace, then looks for `[`. Returns
/// `Some((content_start, content_end, after_close))` where:
/// - `content_start..content_end` is the content inside the brackets
/// - `after_close` is the position right after the closing `]`
///
/// Handles nested brackets and backslash escapes.
pub fn extract_optional_arg(s: &str, pos: usize) -> Option<(usize, usize, usize)> {
    let bytes = s.as_bytes();
    let mut i = pos;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\n') {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'[' {
        return None;
    }
    let content_start = i + 1;
    i = content_start;
    let mut depth = 1u32;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some((content_start, i, i + 1));
                }
            }
            b'\\' => {
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Find a LaTeX command like `\commandname` starting at position `pos` (which should be `\`).
/// Returns the end position of the command name (exclusive), i.e., the index of the first
/// character that is NOT part of the command name.
/// Also handles starred variants like `\section*`.
pub fn find_command_end(s: &str, backslash_pos: usize) -> usize {
    let bytes = s.as_bytes();
    if backslash_pos >= bytes.len() || bytes[backslash_pos] != b'\\' {
        return backslash_pos;
    }
    let mut i = backslash_pos + 1;
    if i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'*' {
            i += 1;
        }
    } else if i < bytes.len() {
        // Single non-letter command like \# \$ \& etc.
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_braces() {
        let s = "{hello}";
        assert_eq!(find_braced_group(s, 0), Some((1, 6)));
        assert_eq!(&s[1..6], "hello");
    }

    #[test]
    fn test_nested_braces() {
        let s = "{a {b {c} d} e}";
        assert_eq!(find_braced_group(s, 0), Some((1, 14)));
        assert_eq!(&s[1..14], "a {b {c} d} e");
    }

    #[test]
    fn test_escaped_brace() {
        let s = r"{hello \} world}";
        assert_eq!(find_braced_group(s, 0), Some((1, 15)));
        assert_eq!(&s[1..15], r"hello \} world");
    }

    #[test]
    fn test_no_opening_brace() {
        assert_eq!(find_braced_group("hello", 0), None);
    }

    #[test]
    fn test_unmatched_brace() {
        assert_eq!(find_braced_group("{hello", 0), None);
    }

    #[test]
    fn test_extract_command_arg() {
        let s = r"\section{Introduction}";
        let cmd_end = find_command_end(s, 0);
        assert_eq!(cmd_end, 8); // after "section"
        let (cs, ce, after) = extract_command_arg(s, cmd_end).unwrap();
        assert_eq!(&s[cs..ce], "Introduction");
        assert_eq!(after, s.len());
    }

    #[test]
    fn test_extract_command_arg_with_whitespace() {
        let s = r"\section  {Introduction}";
        let cmd_end = find_command_end(s, 0);
        let (cs, ce, _) = extract_command_arg(s, cmd_end).unwrap();
        assert_eq!(&s[cs..ce], "Introduction");
    }

    #[test]
    fn test_extract_nested_command_arg() {
        let s = r"\title{The $\mathcal{O}(n)$ Algorithm}";
        let cmd_end = find_command_end(s, 0);
        let (cs, ce, _) = extract_command_arg(s, cmd_end).unwrap();
        assert_eq!(&s[cs..ce], r"The $\mathcal{O}(n)$ Algorithm");
    }

    #[test]
    fn test_skip_optional_arg() {
        let s = r"[short title]{Full Title}";
        let after_opt = skip_optional_arg(s, 0);
        assert_eq!(after_opt, 13); // one past "]"
    }

    #[test]
    fn test_skip_optional_arg_none() {
        let s = r"{Full Title}";
        let after_opt = skip_optional_arg(s, 0);
        assert_eq!(after_opt, 0); // unchanged
    }

    #[test]
    fn test_find_command_end() {
        assert_eq!(find_command_end(r"\section rest", 0), 8);
        assert_eq!(find_command_end(r"\section* rest", 0), 9);
        assert_eq!(find_command_end(r"\# text", 0), 2);
        assert_eq!(find_command_end(r"\begin{doc}", 0), 6);
    }

    #[test]
    fn test_deeply_nested() {
        let s = r"{\textbf{A \emph{very \textit{important}} paper}}";
        let (cs, ce) = find_braced_group(s, 0).unwrap();
        assert_eq!(&s[cs..ce], r"\textbf{A \emph{very \textit{important}} paper}");
    }

    // --- tolerant brace matching ---

    #[test]
    fn test_tolerant_matched_delegates_to_strict() {
        let s = "{hello}";
        assert_eq!(find_braced_group_tolerant(s, 0), Some((1, 6)));
    }

    #[test]
    fn test_tolerant_unmatched_to_paragraph_break() {
        let s = "{unclosed text\n\nmore";
        let (cs, ce) = find_braced_group_tolerant(s, 0).unwrap();
        assert_eq!(&s[cs..ce], "unclosed text");
    }

    #[test]
    fn test_tolerant_unmatched_to_command() {
        let s = "{unclosed \\section rest";
        let (cs, ce) = find_braced_group_tolerant(s, 0).unwrap();
        assert_eq!(&s[cs..ce], "unclosed ");
    }

    #[test]
    fn test_tolerant_unmatched_to_eof() {
        let s = "{unclosed text";
        let (cs, ce) = find_braced_group_tolerant(s, 0).unwrap();
        assert_eq!(&s[cs..ce], "unclosed text");
    }

    #[test]
    fn test_tolerant_no_opening_brace() {
        assert_eq!(find_braced_group_tolerant("hello", 0), None);
    }

    #[test]
    fn test_extract_command_arg_tolerant_matched() {
        let s = r"\cmd{content}";
        let result = extract_command_arg_tolerant(s, 4).unwrap();
        assert_eq!(&s[result.0..result.1], "content");
        assert_eq!(result.2, s.len());
    }

    #[test]
    fn test_extract_command_arg_tolerant_unmatched() {
        let s = r"\cmd{unclosed text";
        let (cs, ce, after) = extract_command_arg_tolerant(s, 4).unwrap();
        assert_eq!(&s[cs..ce], "unclosed text");
        assert_eq!(after, s.len()); // at end of string, no } to skip
    }

    // --- extract_optional_arg ---

    #[test]
    fn test_extract_optional_arg_present() {
        let s = "[content]rest";
        let (cs, ce, after) = extract_optional_arg(s, 0).unwrap();
        assert_eq!(&s[cs..ce], "content");
        assert_eq!(after, 9);
    }

    #[test]
    fn test_extract_optional_arg_nested() {
        let s = "[a [b] c]rest";
        let (cs, ce, after) = extract_optional_arg(s, 0).unwrap();
        assert_eq!(&s[cs..ce], "a [b] c");
        assert_eq!(after, 9);
    }

    #[test]
    fn test_extract_optional_arg_absent() {
        assert_eq!(extract_optional_arg("{content}", 0), None);
    }

    #[test]
    fn test_extract_optional_arg_whitespace() {
        let s = "  [content]";
        let (cs, ce, _) = extract_optional_arg(s, 0).unwrap();
        assert_eq!(&s[cs..ce], "content");
    }
}
