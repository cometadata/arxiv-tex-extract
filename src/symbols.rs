use aho_corasick::AhoCorasick;
use regex::Regex;
use std::sync::LazyLock;

/// A single-pass multi-pattern replacer using Aho-Corasick.
/// Checks that the character after each match is not ASCII alphabetic
/// (replicating Python's `(?![a-zA-Z])` lookahead).
pub struct CommandReplacer {
    automaton: AhoCorasick,
    #[allow(dead_code)]
    patterns: Vec<String>,
    replacements: Vec<String>,
}

impl CommandReplacer {
    /// Build a new replacer from (pattern, replacement) pairs.
    /// Patterns are matched with leftmost-longest semantics.
    pub fn new(pairs: &[(&str, &str)]) -> Self {
        let patterns: Vec<String> = pairs.iter().map(|(p, _)| p.to_string()).collect();
        let replacements: Vec<String> = pairs.iter().map(|(_, r)| r.to_string()).collect();
        let automaton = AhoCorasick::builder()
            .match_kind(aho_corasick::MatchKind::LeftmostLongest)
            .build(&patterns)
            .expect("failed to build Aho-Corasick automaton");
        Self {
            automaton,
            patterns,
            replacements,
        }
    }

    /// Replace all matching patterns in `text`, but only when the match is NOT
    /// followed by an ASCII alphabetic character (word boundary check).
    pub fn replace_all(&self, text: &str) -> String {
        let bytes = text.as_bytes();
        let mut result = String::with_capacity(text.len());
        let mut last_end = 0;

        for mat in self.automaton.find_iter(text) {
            let match_end = mat.end();
            if match_end < bytes.len() && bytes[match_end].is_ascii_alphabetic() {
                continue;
            }
            result.push_str(&text[last_end..mat.start()]);
            result.push_str(&self.replacements[mat.pattern().as_usize()]);
            last_end = match_end;
        }
        result.push_str(&text[last_end..]);
        result
    }
}

// ---------------------------------------------------------------------------
// Symbol tables
// ---------------------------------------------------------------------------

static TEXT_COMMANDS: LazyLock<CommandReplacer> = LazyLock::new(|| {
    CommandReplacer::new(&[
        ("\\textbackslash", "\\"),
        ("\\textasciitilde", "~"),
        ("\\textasciicircum", "^"),
        ("\\textemdash", "\u{2014}"),
        ("\\textendash", "\u{2013}"),
        ("\\textellipsis", "\u{2026}"),
    ])
});

static MATH_SYMBOLS: LazyLock<CommandReplacer> = LazyLock::new(|| {
    CommandReplacer::new(&[
        // Dots
        ("\\ldots", "\u{2026}"),
        ("\\dots", "\u{2026}"),
        ("\\cdots", "\u{22ef}"),
        // Arithmetic
        ("\\times", "\u{00d7}"),
        ("\\pm", "\u{00b1}"),
        ("\\mp", "\u{2213}"),
        // Relations
        ("\\leqslant", "\u{2a7d}"),
        ("\\geqslant", "\u{2a7e}"),
        ("\\leq", "\u{2264}"),
        ("\\geq", "\u{2265}"),
        ("\\le", "\u{2264}"),
        ("\\ge", "\u{2265}"),
        ("\\gg", "\u{226b}"),
        ("\\ll", "\u{226a}"),
        ("\\neq", "\u{2260}"),
        ("\\ne", "\u{2260}"),
        ("\\approx", "\u{2248}"),
        ("\\propto", "\u{221d}"),
        ("\\sim", "\u{223c}"),
        ("\\equiv", "\u{2261}"),
        ("\\perp", "\u{22a5}"),
        ("\\parallel", "\u{2225}"),
        // Sets
        ("\\subseteq", "\u{2286}"),
        ("\\supseteq", "\u{2287}"),
        ("\\subset", "\u{2282}"),
        ("\\supset", "\u{2283}"),
        ("\\notin", "\u{2209}"),
        ("\\in", "\u{2208}"),
        ("\\infty", "\u{221e}"),
        ("\\cap", "\u{2229}"),
        ("\\cup", "\u{222a}"),
        ("\\emptyset", "\u{2205}"),
        ("\\varnothing", "\u{2205}"),
        // Calculus / logic / misc
        ("\\ell", "\u{2113}"),
        ("\\hbar", "\u{210f}"),
        ("\\Re", "\u{211c}"),
        ("\\Im", "\u{2111}"),
        ("\\partial", "\u{2202}"),
        ("\\nabla", "\u{2207}"),
        ("\\forall", "\u{2200}"),
        ("\\exists", "\u{2203}"),
        ("\\wedge", "\u{2227}"),
        ("\\vee", "\u{2228}"),
        ("\\neg", "\u{00ac}"),
        // Binary ops
        ("\\oplus", "\u{2295}"),
        ("\\otimes", "\u{2297}"),
        ("\\odot", "\u{2299}"),
        ("\\circ", "\u{2218}"),
        ("\\cdot", "\u{00b7}"),
        ("\\bullet", "\u{2022}"),
        ("\\star", "\u{22c6}"),
        ("\\dagger", "\u{2020}"),
        ("\\ddagger", "\u{2021}"),
        // Order relations
        ("\\lesssim", "\u{2272}"),
        ("\\gtrsim", "\u{2273}"),
        ("\\simeq", "\u{2243}"),
        ("\\cong", "\u{2245}"),
        ("\\prec", "\u{227a}"),
        ("\\succ", "\u{227b}"),
        ("\\preceq", "\u{2aaf}"),
        ("\\succeq", "\u{2ab0}"),
        // Logic / turnstiles
        ("\\vdash", "\u{22a2}"),
        ("\\dashv", "\u{22a3}"),
        ("\\models", "\u{22a8}"),
        ("\\mid", "\u{2223}"),
        // Delimiters
        ("\\langle", "\u{27e8}"),
        ("\\rangle", "\u{27e9}"),
        ("\\lceil", "\u{2308}"),
        ("\\rceil", "\u{2309}"),
        ("\\lfloor", "\u{230a}"),
        ("\\rfloor", "\u{230b}"),
        // Aliases
        ("\\thickapprox", "\u{2248}"),
        ("\\thicksim", "\u{223c}"),
        // Shapes
        ("\\triangle", "\u{25b3}"),
        ("\\square", "\u{25a1}"),
        // Arrows
        ("\\uparrow", "\u{2191}"),
        ("\\downarrow", "\u{2193}"),
        ("\\hookrightarrow", "\u{21aa}"),
        ("\\hookleftarrow", "\u{21a9}"),
        ("\\longrightarrow", "\u{27f6}"),
        ("\\longleftarrow", "\u{27f5}"),
        ("\\longmapsto", "\u{27fc}"),
        ("\\rightarrow", "\u{2192}"),
        ("\\leftarrow", "\u{2190}"),
        ("\\Rightarrow", "\u{21d2}"),
        ("\\Leftarrow", "\u{21d0}"),
        ("\\leftrightarrow", "\u{2194}"),
        ("\\Leftrightarrow", "\u{21d4}"),
        ("\\mapsto", "\u{21a6}"),
        ("\\to", "\u{2192}"),
        // Set operations
        ("\\setminus", "\u{2216}"),
        ("\\backslash", "\u{005C}"),
        // Punctuation / delimiters
        ("\\colon", ":"),
        ("\\lvert", "|"),
        ("\\rvert", "|"),
        ("\\lVert", "\u{2016}"),
        ("\\rVert", "\u{2016}"),
        // Limit variants
        ("\\varprojlim", "proj lim"),
        ("\\varinjlim", "inj lim"),
        // Package-defined symbols
        ("\\lambdabar", "\u{019B}"),
    ])
});

static GREEK_LETTERS: LazyLock<CommandReplacer> = LazyLock::new(|| {
    CommandReplacer::new(&[
        ("\\alpha", "\u{03b1}"),
        ("\\beta", "\u{03b2}"),
        ("\\gamma", "\u{03b3}"),
        ("\\delta", "\u{03b4}"),
        ("\\epsilon", "\u{03b5}"),
        ("\\varepsilon", "\u{03b5}"),
        ("\\zeta", "\u{03b6}"),
        ("\\eta", "\u{03b7}"),
        ("\\theta", "\u{03b8}"),
        ("\\vartheta", "\u{03d1}"),
        ("\\iota", "\u{03b9}"),
        ("\\kappa", "\u{03ba}"),
        ("\\lambda", "\u{03bb}"),
        ("\\mu", "\u{03bc}"),
        ("\\nu", "\u{03bd}"),
        ("\\xi", "\u{03be}"),
        ("\\pi", "\u{03c0}"),
        ("\\rho", "\u{03c1}"),
        ("\\sigma", "\u{03c3}"),
        ("\\tau", "\u{03c4}"),
        ("\\upsilon", "\u{03c5}"),
        ("\\phi", "\u{03c6}"),
        ("\\varphi", "\u{03c6}"),
        ("\\chi", "\u{03c7}"),
        ("\\psi", "\u{03c8}"),
        ("\\omega", "\u{03c9}"),
        ("\\Gamma", "\u{0393}"),
        ("\\Delta", "\u{0394}"),
        ("\\Theta", "\u{0398}"),
        ("\\Lambda", "\u{039b}"),
        ("\\Xi", "\u{039e}"),
        ("\\Pi", "\u{03a0}"),
        ("\\Sigma", "\u{03a3}"),
        ("\\Upsilon", "\u{03a5}"),
        ("\\Phi", "\u{03a6}"),
        ("\\Psi", "\u{03a8}"),
        ("\\Omega", "\u{03a9}"),
    ])
});

/// Map `\mathbb{X}` to Unicode blackboard-bold characters.
fn convert_mathbb(text: &str) -> String {
    static MATHBB_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\\mathbb\{([A-Za-z])\}").unwrap());

    MATHBB_RE
        .replace_all(text, |caps: &regex::Captures| {
            let letter = caps.get(1).unwrap().as_str();
            match letter {
                "R" => "\u{211D}".to_string(), // ℝ
                "Z" => "\u{2124}".to_string(), // ℤ
                "N" => "\u{2115}".to_string(), // ℕ
                "C" => "\u{2102}".to_string(), // ℂ
                "Q" => "\u{211A}".to_string(), // ℚ
                "P" => "\u{2119}".to_string(), // ℙ
                "H" => "\u{210D}".to_string(), // ℍ
                "F" => "\u{1D53D}".to_string(), // 𝔽
                other => format!("{}", other),   // fallback: just the letter
            }
        })
        .to_string()
}

/// Convert LaTeX symbol commands and ligatures to Unicode.
pub fn convert_symbols(text: &str) -> String {
    let mut result = text
        .replace("``", "\u{201c}")
        .replace("''", "\u{201d}")
        .replace("---", "\u{2014}")
        .replace("--", "\u{2013}")
        .replace('~', " ");

    result = result
        .replace("\\&", "&")
        .replace("\\#", "#")
        .replace("\\$", "$")
        .replace("\\%", "%")
        .replace("\\_", "_");

    result = TEXT_COMMANDS.replace_all(&result);
    result = MATH_SYMBOLS.replace_all(&result);
    result = GREEK_LETTERS.replace_all(&result);
    result = convert_mathbb(&result);

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_replacer_basic() {
        let r = CommandReplacer::new(&[("\\alpha", "α"), ("\\beta", "β")]);
        assert_eq!(r.replace_all("\\alpha + \\beta"), "α + β");
    }

    #[test]
    fn test_command_replacer_boundary() {
        let r = CommandReplacer::new(&[("\\in", "∈"), ("\\infty", "∞")]);
        // \infty should match as a whole, not as \in + fty
        assert_eq!(r.replace_all("\\infty"), "∞");
        // \in followed by non-alpha
        assert_eq!(r.replace_all("x \\in S"), "x ∈ S");
        // \in followed by alpha (not via \infty) — should NOT match
        assert_eq!(r.replace_all("\\input"), "\\input");
    }

    #[test]
    fn test_command_replacer_at_eof() {
        let r = CommandReplacer::new(&[("\\alpha", "α")]);
        assert_eq!(r.replace_all("\\alpha"), "α");
    }

    #[test]
    fn test_command_replacer_followed_by_digit() {
        let r = CommandReplacer::new(&[("\\alpha", "α")]);
        // Digits are not [a-zA-Z], so boundary check passes
        assert_eq!(r.replace_all("\\alpha123"), "α123");
    }

    #[test]
    fn test_command_replacer_followed_by_backslash() {
        let r = CommandReplacer::new(&[("\\alpha", "α"), ("\\beta", "β")]);
        // \alpha followed by \beta — backslash is not [a-zA-Z], so \alpha matches
        assert_eq!(r.replace_all("\\alpha\\beta"), "αβ");
    }

    #[test]
    fn test_convert_symbols_ligatures() {
        assert_eq!(convert_symbols("``hello''"), "\u{201c}hello\u{201d}");
        assert_eq!(convert_symbols("a---b"), "a\u{2014}b");
        assert_eq!(convert_symbols("a--b"), "a\u{2013}b");
        assert_eq!(convert_symbols("a~b"), "a b");
    }

    #[test]
    fn test_convert_symbols_escaped() {
        assert_eq!(convert_symbols("10\\% tax"), "10% tax");
        assert_eq!(convert_symbols("A \\& B"), "A & B");
    }

    #[test]
    fn test_convert_symbols_greek() {
        let result = convert_symbols("\\alpha + \\beta = \\gamma");
        assert_eq!(result, "α + β = γ");
    }

    #[test]
    fn test_convert_symbols_math() {
        let result = convert_symbols("x \\leq y \\rightarrow z");
        assert_eq!(result, "x ≤ y → z");
    }

    #[test]
    fn test_subset_vs_subseteq() {
        let result = convert_symbols("A \\subseteq B \\subset C");
        assert_eq!(result, "A ⊆ B ⊂ C");
    }

    #[test]
    fn test_lesssim() {
        assert_eq!(convert_symbols("\\lesssim"), "\u{2272}");
    }

    #[test]
    fn test_vdash() {
        assert_eq!(convert_symbols("\\vdash"), "\u{22a2}");
    }

    #[test]
    fn test_langle_rangle() {
        let result = convert_symbols("\\langle x \\rangle");
        assert_eq!(result, "\u{27e8} x \u{27e9}");
    }

    #[test]
    fn test_hookrightarrow() {
        assert_eq!(convert_symbols("\\hookrightarrow"), "\u{21aa}");
    }

    #[test]
    fn test_triangle_square() {
        assert_eq!(convert_symbols("\\triangle"), "\u{25b3}");
        assert_eq!(convert_symbols("\\square"), "\u{25a1}");
    }

    #[test]
    fn test_setminus() {
        assert_eq!(convert_symbols("A \\setminus B"), "A \u{2216} B");
    }

    #[test]
    fn test_colon() {
        assert_eq!(convert_symbols("f\\colon A"), "f: A");
    }

    #[test]
    fn test_vert_delimiters() {
        assert_eq!(convert_symbols("\\lvert x \\rvert"), "| x |");
        assert_eq!(convert_symbols("\\lVert x \\rVert"), "\u{2016} x \u{2016}");
    }

    #[test]
    fn test_varprojlim_varinjlim() {
        assert_eq!(convert_symbols("\\varprojlim"), "proj lim");
        assert_eq!(convert_symbols("\\varinjlim"), "inj lim");
    }

    #[test]
    fn test_mathbb_r() {
        assert_eq!(convert_symbols("\\mathbb{R}"), "\u{211D}");
    }

    #[test]
    fn test_mathbb_z() {
        assert_eq!(convert_symbols("\\mathbb{Z}"), "\u{2124}");
    }

    #[test]
    fn test_mathbb_unknown() {
        // Letters without special Unicode map → just the letter
        assert_eq!(convert_symbols("\\mathbb{X}"), "X");
    }
}
