/// Remove LaTeX comments from text.
///
/// Handles:
/// - Full-line comments (lines starting with %)
/// - Inline comments (% to end of line, not preceded by odd backslashes)
/// - Line joining: in LaTeX, `word%\njoined` → `wordjoined`
/// - Correct `\\%` handling: even backslashes = comment, odd = escaped %
pub fn remove_comments(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' {
            let backslash_count = count_preceding_backslashes(bytes, i);
            if backslash_count % 2 == 0 {
                i += 1;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b'\n' {
                    i += 1;
                }
                continue;
            }
            result.push('%');
            i += 1;
        } else if bytes[i] == b'\\' {
            result.push('\\');
            i += 1;
            if i < bytes.len() {
                if bytes[i] >= 128 {
                    let c = &text[i..];
                    if let Some(ch) = c.chars().next() {
                        result.push(ch);
                        i += ch.len_utf8();
                    } else {
                        i += 1;
                    }
                } else {
                    result.push(bytes[i] as char);
                    i += 1;
                }
            }
        } else if bytes[i] >= 128 {
            let c = &text[i..];
            if let Some(ch) = c.chars().next() {
                result.push(ch);
                i += ch.len_utf8();
            } else {
                i += 1;
            }
        } else {
            result.push(bytes[i] as char);
            i += 1;
        }
    }

    result
}

/// Count how many consecutive backslashes appear immediately before position `pos`.
fn count_preceding_backslashes(bytes: &[u8], pos: usize) -> usize {
    let mut count = 0;
    let mut j = pos;
    while j > 0 {
        j -= 1;
        if bytes[j] == b'\\' {
            count += 1;
        } else {
            break;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_line_comment() {
        assert_eq!(remove_comments("% this is a comment\nhello"), "hello");
    }

    #[test]
    fn test_inline_comment() {
        assert_eq!(remove_comments("hello % comment\nworld"), "hello world");
    }

    #[test]
    fn test_escaped_percent() {
        assert_eq!(remove_comments("10\\% tax"), "10\\% tax");
    }

    #[test]
    fn test_double_backslash_percent() {
        // \\% = escaped backslash + comment
        assert_eq!(remove_comments("line\\\\% comment\nnext"), "line\\\\next");
    }

    #[test]
    fn test_line_joining() {
        // word%\njoined → wordjoined
        assert_eq!(remove_comments("word%\njoined"), "wordjoined");
    }

    #[test]
    fn test_bare_percent_at_eol() {
        // Just % at end of line with nothing after
        assert_eq!(remove_comments("text%\n"), "text");
    }

    #[test]
    fn test_bare_percent_at_eof() {
        assert_eq!(remove_comments("text%"), "text");
    }

    #[test]
    fn test_no_comments() {
        assert_eq!(remove_comments("hello world"), "hello world");
    }

    #[test]
    fn test_multiple_comments() {
        let input = "a % first\nb % second\nc";
        assert_eq!(remove_comments(input), "a b c");
    }

    #[test]
    fn test_comment_preserves_non_ascii() {
        // This is a simplified test — real UTF-8 handling is covered by
        // the byte-level scanning
        assert_eq!(remove_comments("café % comment\nworld"), "café world");
    }
}
