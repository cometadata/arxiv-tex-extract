use crate::symbols::CommandReplacer;
use regex::Regex;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

/// Special character commands with braces (safe for literal replacement).
static SPECIAL_CHARS_BRACED: &[(&str, &str)] = &[
    // Cedilla
    (r"\c{c}", "ç"),
    (r"\c{C}", "Ç"),
    (r"\c{s}", "ş"),
    (r"\c{S}", "Ş"),
    (r"\c{t}", "ţ"),
    (r"\c{T}", "Ţ"),
    (r"\c{e}", "ę"),
    (r"\c{E}", "Ę"),
    (r"\c{a}", "ą"),
    (r"\c{A}", "Ą"),
    // Háček (caron) — explicit entries to avoid ordering issues with bare-accent matching
    (r"\v{C}", "Č"),
    (r"\v{c}", "č"),
    (r"\v{S}", "Š"),
    (r"\v{s}", "š"),
    (r"\v{Z}", "Ž"),
    (r"\v{z}", "ž"),
    (r"\v{r}", "ř"),
    (r"\v{R}", "Ř"),
    (r"\v{n}", "ň"),
    (r"\v{N}", "Ň"),
    (r"\v{e}", "ě"),
    (r"\v{E}", "Ě"),
    (r"\v{d}", "ď"),
    (r"\v{D}", "Ď"),
    (r"\v{t}", "ť"),
    (r"\v{T}", "Ť"),
    // Dotless-i with caron
    (r"\v{\i}", "ǐ"),
];

/// Special character commands without braces (need word boundary via Aho-Corasick).
static SPECIAL_CHARS_BARE: LazyLock<CommandReplacer> = LazyLock::new(|| {
    CommandReplacer::new(&[
        ("\\ss", "ß"),
        ("\\ae", "æ"),
        ("\\AE", "Æ"),
        ("\\oe", "œ"),
        ("\\OE", "Œ"),
        ("\\o", "ø"),
        ("\\O", "Ø"),
        ("\\aa", "å"),
        ("\\AA", "Å"),
        ("\\l", "ł"),
        ("\\L", "Ł"),
        ("\\i", "ı"),
        ("\\j", "ȷ"),
        // Special Latin characters
        ("\\DJ", "Đ"),
        ("\\dj", "đ"),
        ("\\TH", "Þ"),
        ("\\th", "þ"),
        ("\\DH", "Ð"),
        ("\\dh", "ð"),
        ("\\NG", "Ŋ"),
        ("\\ng", "ŋ"),
    ])
});

/// Combining diacritical marks.
static DIACRITICS: &[(&str, char)] = &[
    ("`", '\u{0300}'),  // grave
    ("'", '\u{0301}'),  // acute
    ("^", '\u{0302}'),  // circumflex
    ("~", '\u{0303}'),  // tilde
    ("=", '\u{0304}'),  // macron
    (".", '\u{0307}'),  // dot above
    ("\"", '\u{0308}'), // diaeresis
    ("u", '\u{0306}'),  // breve
    ("v", '\u{030C}'),  // caron
    ("H", '\u{030B}'),  // double acute
    ("r", '\u{030A}'),  // ring above
    ("d", '\u{0323}'),  // dot below
    ("b", '\u{0331}'),  // macron below
    ("k", '\u{0328}'),  // ogonek
];

/// Pre-compiled regex patterns for diacritics.
/// Two forms: \accent{char} and \accent char (for single-letter accents).
struct DiacriticPatterns {
    braced: Vec<(Regex, char)>,
    /// Bare patterns for punctuation-based accents (`` ` ' ^ ~ = . " ``).
    /// These never collide with command names — no boundary checks needed.
    bare_punct: Vec<(Regex, char)>,
    /// Bare patterns for letter-based accents (`u v H r d b k`).
    /// Need pre/post boundary checks to avoid matching inside commands.
    bare_letter: Vec<(Regex, char)>,
}

static DIACRITIC_PATTERNS: LazyLock<DiacriticPatterns> = LazyLock::new(|| {
    let mut braced = Vec::new();
    let mut bare_punct = Vec::new();
    let mut bare_letter = Vec::new();

    for &(accent, combining) in DIACRITICS {
        let escaped = regex::escape(accent);
        let braced_pat = format!(r"\\{}\{{([a-zA-Z])\}}", escaped);
        braced.push((Regex::new(&braced_pat).unwrap(), combining));

        let bare_pat = format!(r"\\{}([a-zA-Z])", escaped);
        let re = Regex::new(&bare_pat).unwrap();

        if accent.chars().next().map_or(false, |c| c.is_ascii_alphabetic()) {
            bare_letter.push((re, combining));
        } else {
            bare_punct.push((re, combining));
        }
    }

    DiacriticPatterns {
        braced,
        bare_punct,
        bare_letter,
    }
});

/// Convert LaTeX diacritics to Unicode.
pub fn convert_diacritics(text: &str) -> String {
    let mut result = text.to_string();

    for &(from, to) in SPECIAL_CHARS_BRACED {
        result = result.replace(from, to);
    }

    result = SPECIAL_CHARS_BARE.replace_all(&result);

    for (re, combining) in &DIACRITIC_PATTERNS.braced {
        result = re
            .replace_all(&result, |caps: &regex::Captures| {
                let base = caps.get(1).unwrap().as_str();
                let composed = format!("{}{}", base, combining);
                composed.nfc().collect::<String>()
            })
            .to_string();
    }

    // Bare punctuation-based accents: no boundary checks needed since
    // `, ', ^, ~, =, ., " can never appear inside command names.
    for (re, combining) in &DIACRITIC_PATTERNS.bare_punct {
        result = re
            .replace_all(&result, |caps: &regex::Captures| {
                let base = caps.get(1).unwrap().as_str();
                let composed = format!("{}{}", base, combining);
                composed.nfc().collect::<String>()
            })
            .to_string();
    }

    // Bare letter-based accents (\v, \b, \d, \u, \r, \k, \H) with boundary
    // checks to prevent matching inside commands like \vskip, \bf, \dots:
    //   1. Skip if `\` is preceded by an ASCII letter (part of a longer command).
    //   2. Skip if captured letter is followed by another ASCII letter.
    for (re, combining) in &DIACRITIC_PATTERNS.bare_letter {
        let bytes = result.as_bytes();
        let combining = *combining;
        result = re
            .replace_all(&result, |caps: &regex::Captures| {
                let full_match = caps.get(0).unwrap();
                let m_start = full_match.start();
                let m_end = full_match.end();

                // Pre-boundary: skip if preceded by ASCII alpha
                if m_start > 0
                    && bytes
                        .get(m_start - 1)
                        .map_or(false, |b| b.is_ascii_alphabetic())
                {
                    return full_match.as_str().to_string();
                }

                // Post-boundary: skip if captured letter is followed by ASCII alpha
                if m_end < bytes.len() && bytes[m_end].is_ascii_alphabetic() {
                    return full_match.as_str().to_string();
                }

                let base = caps.get(1).unwrap().as_str();
                let composed = format!("{}{}", base, combining);
                composed.nfc().collect::<String>()
            })
            .to_string();
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cedilla() {
        assert_eq!(convert_diacritics(r"\c{c}"), "ç");
        assert_eq!(convert_diacritics(r"\c{C}"), "Ç");
    }

    #[test]
    fn test_ss() {
        assert_eq!(convert_diacritics(r"\ss "), "ß ");
        assert_eq!(convert_diacritics(r"\ss"), "ß");
    }

    #[test]
    fn test_ae_oe() {
        assert_eq!(convert_diacritics(r"\ae "), "æ ");
        assert_eq!(convert_diacritics(r"\oe "), "œ ");
    }

    #[test]
    fn test_acute_braced() {
        assert_eq!(convert_diacritics(r"\'{e}"), "é");
    }

    #[test]
    fn test_acute_bare() {
        assert_eq!(convert_diacritics(r"\'e"), "é");
    }

    #[test]
    fn test_grave() {
        assert_eq!(convert_diacritics(r"\`{a}"), "à");
    }

    #[test]
    fn test_circumflex() {
        assert_eq!(convert_diacritics(r"\^{o}"), "ô");
    }

    #[test]
    fn test_diaeresis() {
        assert_eq!(convert_diacritics(r#"\"u"#), "ü");
        assert_eq!(convert_diacritics(r#"\"{u}"#), "ü");
    }

    #[test]
    fn test_tilde() {
        assert_eq!(convert_diacritics(r"\~{n}"), "ñ");
    }

    #[test]
    fn test_caron() {
        assert_eq!(convert_diacritics(r"\v{c}"), "č");
    }

    #[test]
    fn test_o_slash() {
        assert_eq!(convert_diacritics(r"\o "), "ø ");
    }

    #[test]
    fn test_l_stroke() {
        assert_eq!(convert_diacritics(r"\l "), "ł ");
    }

    #[test]
    fn test_dotless_i() {
        assert_eq!(convert_diacritics(r"\i "), "ı ");
    }

    #[test]
    fn test_word_boundary() {
        // \oplus should NOT match \o + plus
        // But Aho-Corasick checks the next char — if it's alpha, it skips
        let result = convert_diacritics(r"\oplus");
        // \o should NOT match here because 'p' follows
        assert_eq!(result, r"\oplus");
    }

    #[test]
    fn test_combined_diacritics() {
        let result = convert_diacritics(r#"Schr\"odinger"#);
        assert_eq!(result, "Schrödinger");
    }

    #[test]
    fn test_bare_v_still_works() {
        // Standalone \v{c} and \vc should still produce caron
        assert_eq!(convert_diacritics(r"\v{c}"), "č");
    }

    #[test]
    fn test_bare_accent_not_inside_command() {
        // \vskip should NOT have \v match the 's'
        // (even if strip_pre_diacritic_commands didn't run, boundary check protects)
        let result = convert_diacritics(r"\vskip");
        assert_eq!(result, r"\vskip");
    }

    #[test]
    fn test_bare_b_not_inside_command() {
        // \baselineskip should NOT have \b match 'a'
        let result = convert_diacritics(r"\baselineskip");
        assert_eq!(result, r"\baselineskip");
    }

    #[test]
    fn test_bare_d_not_dots() {
        // \dots should NOT have \d match 'o'
        let result = convert_diacritics(r"\dots");
        assert_eq!(result, r"\dots");
    }

    #[test]
    fn test_bare_r_not_rm_followed() {
        // \rm followed by more text — \r should NOT match 'm' because 'm'
        // is followed by another letter. (Standalone \rm at EOF relies on
        // strip_pre_diacritic_commands removing it first.)
        let result = convert_diacritics(r"\rmfamily");
        assert_eq!(result, r"\rmfamily");
    }

    #[test]
    fn test_cafe_still_works() {
        assert_eq!(convert_diacritics(r"Caf\'e"), "Café");
    }

    #[test]
    fn test_hacek_uppercase_c() {
        assert_eq!(convert_diacritics(r"\v{C}"), "Č");
    }

    #[test]
    fn test_cadez_name() {
        assert_eq!(convert_diacritics(r"\v{C}ade\v{z}"), "Čadež");
    }

    #[test]
    fn test_hacek_s_z() {
        assert_eq!(convert_diacritics(r"\v{S}"), "Š");
        assert_eq!(convert_diacritics(r"\v{z}"), "ž");
    }

    #[test]
    fn test_hacek_dotless_i() {
        assert_eq!(convert_diacritics(r"\v{\i}"), "ǐ");
    }
}
