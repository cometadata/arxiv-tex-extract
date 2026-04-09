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
        // Quotes
        ("\\textquoteleft", "\u{2018}"),
        ("\\textquoteright", "\u{2019}"),
        ("\\textquotedblleft", "\u{201C}"),
        ("\\textquotedblright", "\u{201D}"),
        // Symbols
        ("\\textdagger", "\u{2020}"),
        ("\\textdaggerdbl", "\u{2021}"),
        ("\\textbullet", "\u{2022}"),
        ("\\textperthousand", "\u{2030}"),
        ("\\textsection", "\u{00A7}"),
        ("\\textparagraph", "\u{00B6}"),
        ("\\textregistered", "\u{00AE}"),
        ("\\textcopyright", "\u{00A9}"),
        ("\\texttrademark", "\u{2122}"),
        // Math in text
        ("\\textpm", "\u{00B1}"),
        ("\\textmp", "\u{2213}"),
        ("\\textsurd", "\u{221A}"),
        ("\\texttimes", "\u{00D7}"),
        // Currency
        ("\\texteuro", "\u{20AC}"),
        ("\\textsterling", "\u{00A3}"),
        ("\\textcent", "\u{00A2}"),
        ("\\textdegree", "\u{00B0}"),
        // Punctuation / special
        ("\\textbar", "|"),
        ("\\textgreater", ">"),
        ("\\textless", "<"),
        ("\\textbraceleft", "{"),
        ("\\textbraceright", "}"),
        ("\\textunderscore", "_"),
        ("\\textvisiblespace", "\u{2423}"),
        ("\\textperiodcentered", "\u{00B7}"),
        // Fractions
        ("\\textonequarter", "\u{00BC}"),
        ("\\textonehalf", "\u{00BD}"),
        ("\\textthreequarters", "\u{00BE}"),
        // Inverted punctuation
        ("\\textexclamdown", "\u{00A1}"),
        ("\\textquestiondown", "\u{00BF}"),
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
        ("\\div", "\u{00F7}"),
        ("\\ast", "\u{2217}"),
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
        ("\\asymp", "\u{224D}"),
        ("\\doteq", "\u{2250}"),
        ("\\nleq", "\u{2270}"),
        ("\\ngeq", "\u{2271}"),
        ("\\nless", "\u{226E}"),
        ("\\ngtr", "\u{226F}"),
        ("\\lneqq", "\u{2268}"),
        ("\\gneqq", "\u{2269}"),
        // Sets
        ("\\subseteq", "\u{2286}"),
        ("\\supseteq", "\u{2287}"),
        ("\\nsubseteq", "\u{2288}"),
        ("\\nsupseteq", "\u{2289}"),
        ("\\sqsubseteq", "\u{2291}"),
        ("\\sqsupseteq", "\u{2292}"),
        ("\\sqsubset", "\u{228F}"),
        ("\\sqsupset", "\u{2290}"),
        ("\\subset", "\u{2282}"),
        ("\\supset", "\u{2283}"),
        ("\\notin", "\u{2209}"),
        ("\\ni", "\u{220B}"),
        ("\\in", "\u{2208}"),
        ("\\infty", "\u{221e}"),
        ("\\cap", "\u{2229}"),
        ("\\cup", "\u{222a}"),
        ("\\emptyset", "\u{2205}"),
        ("\\varnothing", "\u{2205}"),
        // Calculus / logic / misc
        ("\\ell", "\u{2113}"),
        ("\\hbar", "\u{210f}"),
        ("\\wp", "\u{2118}"),
        ("\\Re", "\u{211c}"),
        ("\\Im", "\u{2111}"),
        ("\\partial", "\u{2202}"),
        ("\\nabla", "\u{2207}"),
        ("\\forall", "\u{2200}"),
        ("\\exists", "\u{2203}"),
        ("\\nexists", "\u{2204}"),
        ("\\complement", "\u{2201}"),
        ("\\wedge", "\u{2227}"),
        ("\\vee", "\u{2228}"),
        ("\\neg", "\u{00ac}"),
        ("\\land", "\u{2227}"),
        ("\\lor", "\u{2228}"),
        ("\\lnot", "\u{00AC}"),
        ("\\top", "\u{22A4}"),
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
        ("\\wr", "\u{2240}"),
        ("\\diamond", "\u{22C4}"),
        ("\\bigtriangleup", "\u{25B3}"),
        ("\\bigtriangledown", "\u{25BD}"),
        ("\\trianglelefteq", "\u{22B4}"),
        ("\\trianglerighteq", "\u{22B5}"),
        ("\\ntriangleleft", "\u{22EA}"),
        ("\\ntriangleright", "\u{22EB}"),
        ("\\vartriangleleft", "\u{22B2}"),
        ("\\vartriangleright", "\u{22B3}"),
        ("\\triangleleft", "\u{25C1}"),
        ("\\triangleright", "\u{25B7}"),
        ("\\lhd", "\u{22B2}"),
        ("\\rhd", "\u{22B3}"),
        ("\\amalg", "\u{2A3F}"),
        // Order relations
        ("\\lesssim", "\u{2272}"),
        ("\\gtrsim", "\u{2273}"),
        ("\\simeq", "\u{2243}"),
        ("\\cong", "\u{2245}"),
        ("\\ncong", "\u{2247}"),
        ("\\nsim", "\u{2241}"),
        ("\\prec", "\u{227a}"),
        ("\\succ", "\u{227b}"),
        ("\\nprec", "\u{2280}"),
        ("\\nsucc", "\u{2281}"),
        ("\\preceq", "\u{2aaf}"),
        ("\\succeq", "\u{2ab0}"),
        ("\\bowtie", "\u{22C8}"),
        ("\\smile", "\u{2323}"),
        ("\\frown", "\u{2322}"),
        // Logic / turnstiles
        ("\\vdash", "\u{22a2}"),
        ("\\dashv", "\u{22a3}"),
        ("\\models", "\u{22a8}"),
        ("\\mid", "\u{2223}"),
        ("\\nmid", "\u{2224}"),
        ("\\nparallel", "\u{2226}"),
        // Big operators
        ("\\sum", "\u{2211}"),
        ("\\prod", "\u{220F}"),
        ("\\coprod", "\u{2210}"),
        ("\\int", "\u{222B}"),
        ("\\iint", "\u{222C}"),
        ("\\iiint", "\u{222D}"),
        ("\\oint", "\u{222E}"),
        ("\\bigcup", "\u{22C3}"),
        ("\\bigcap", "\u{22C2}"),
        ("\\bigoplus", "\u{2A01}"),
        ("\\bigotimes", "\u{2A02}"),
        ("\\bigvee", "\u{22C1}"),
        ("\\bigwedge", "\u{22C0}"),
        ("\\bigsqcup", "\u{2A06}"),
        // Delimiters
        ("\\langle", "\u{27e8}"),
        ("\\rangle", "\u{27e9}"),
        ("\\lceil", "\u{2308}"),
        ("\\rceil", "\u{2309}"),
        ("\\lfloor", "\u{230a}"),
        ("\\rfloor", "\u{230b}"),
        ("\\lbrace", "{"),
        ("\\rbrace", "}"),
        ("\\lbrack", "["),
        ("\\rbrack", "]"),
        ("\\vert", "|"),
        ("\\Vert", "\u{2016}"),
        // Aliases
        ("\\thickapprox", "\u{2248}"),
        ("\\thicksim", "\u{223c}"),
        // Shapes
        ("\\triangle", "\u{25b3}"),
        ("\\square", "\u{25a1}"),
        ("\\Diamond", "\u{25C7}"),
        // Arrows
        ("\\Uparrow", "\u{21D1}"),
        ("\\Downarrow", "\u{21D3}"),
        ("\\updownarrow", "\u{2195}"),
        ("\\Updownarrow", "\u{21D5}"),
        ("\\uparrow", "\u{2191}"),
        ("\\downarrow", "\u{2193}"),
        ("\\nearrow", "\u{2197}"),
        ("\\nwarrow", "\u{2196}"),
        ("\\searrow", "\u{2198}"),
        ("\\swarrow", "\u{2199}"),
        ("\\hookrightarrow", "\u{21aa}"),
        ("\\hookleftarrow", "\u{21a9}"),
        ("\\Longrightarrow", "\u{27F9}"),
        ("\\Longleftarrow", "\u{27F8}"),
        ("\\Longleftrightarrow", "\u{27FA}"),
        ("\\longleftrightarrow", "\u{27F7}"),
        ("\\longrightarrow", "\u{27f6}"),
        ("\\longleftarrow", "\u{27f5}"),
        ("\\longmapsto", "\u{27fc}"),
        ("\\rightsquigarrow", "\u{21DD}"),
        ("\\leftrightsquigarrow", "\u{21AD}"),
        ("\\rightharpoonup", "\u{21C0}"),
        ("\\rightharpoondown", "\u{21C1}"),
        ("\\leftharpoonup", "\u{21BC}"),
        ("\\leftharpoondown", "\u{21BD}"),
        ("\\rightleftharpoons", "\u{21CC}"),
        ("\\leftrightharpoons", "\u{21CB}"),
        ("\\twoheadrightarrow", "\u{21A0}"),
        ("\\twoheadleftarrow", "\u{219E}"),
        ("\\rightarrowtail", "\u{21A3}"),
        ("\\leftarrowtail", "\u{21A2}"),
        ("\\curvearrowright", "\u{21B7}"),
        ("\\curvearrowleft", "\u{21B6}"),
        ("\\circlearrowright", "\u{21BB}"),
        ("\\circlearrowleft", "\u{21BA}"),
        ("\\rightarrow", "\u{2192}"),
        ("\\leftarrow", "\u{2190}"),
        ("\\Rightarrow", "\u{21d2}"),
        ("\\Leftarrow", "\u{21d0}"),
        ("\\leftrightarrow", "\u{2194}"),
        ("\\Leftrightarrow", "\u{21d4}"),
        ("\\mapsto", "\u{21a6}"),
        ("\\multimap", "\u{22B8}"),
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
        // Hebrew
        ("\\aleph", "\u{2135}"),
        ("\\beth", "\u{2136}"),
        ("\\gimel", "\u{2137}"),
        ("\\daleth", "\u{2138}"),
        // Angles
        ("\\angle", "\u{2220}"),
        ("\\measuredangle", "\u{2221}"),
        ("\\sphericalangle", "\u{2222}"),
        // Misc
        ("\\therefore", "\u{2234}"),
        ("\\because", "\u{2235}"),
        ("\\sharp", "\u{266F}"),
        ("\\flat", "\u{266D}"),
        ("\\natural", "\u{266E}"),
        ("\\clubsuit", "\u{2663}"),
        ("\\diamondsuit", "\u{2662}"),
        ("\\heartsuit", "\u{2661}"),
        ("\\spadesuit", "\u{2660}"),
        ("\\imath", "\u{0131}"),
        ("\\jmath", "\u{0237}"),
        ("\\degree", "\u{00B0}"),
        ("\\checkmark", "\u{2713}"),
        ("\\prime", "\u{2032}"),
        ("\\backprime", "\u{2035}"),
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
        ("\\varpi", "\u{03D6}"),
        ("\\rho", "\u{03c1}"),
        ("\\varrho", "\u{03F1}"),
        ("\\sigma", "\u{03c3}"),
        ("\\varsigma", "\u{03C2}"),
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
        ("\\digamma", "\u{03DD}"),
        ("\\Digamma", "\u{03DC}"),
        // Upright Greek (same codepoints — Unicode basic Greek block)
        ("\\upalpha", "\u{03b1}"),
        ("\\upbeta", "\u{03b2}"),
        ("\\upgamma", "\u{03b3}"),
        ("\\updelta", "\u{03b4}"),
        ("\\upepsilon", "\u{03b5}"),
        ("\\upzeta", "\u{03b6}"),
        ("\\upeta", "\u{03b7}"),
        ("\\uptheta", "\u{03b8}"),
        ("\\upiota", "\u{03b9}"),
        ("\\upkappa", "\u{03ba}"),
        ("\\uplambda", "\u{03bb}"),
        ("\\upmu", "\u{03bc}"),
        ("\\upnu", "\u{03bd}"),
        ("\\upxi", "\u{03be}"),
        ("\\uppi", "\u{03c0}"),
        ("\\uprho", "\u{03c1}"),
        ("\\upsigma", "\u{03c3}"),
        ("\\uptau", "\u{03c4}"),
        ("\\upupsilon", "\u{03c5}"),
        ("\\upphi", "\u{03c6}"),
        ("\\upchi", "\u{03c7}"),
        ("\\uppsi", "\u{03c8}"),
        ("\\upomega", "\u{03c9}"),
    ])
});

// ---------------------------------------------------------------------------
// Unicode math font converters (Stage 3)
// ---------------------------------------------------------------------------

/// Map `\mathbb{X}` or `\Bbb{X}` to Unicode double-struck characters.
/// Full A-Z, a-z, 0-9 mapping.
fn convert_mathbb(text: &str) -> String {
    static MATHBB_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\\(?:mathbb|Bbb)\{([A-Za-z0-9])\}").unwrap());

    MATHBB_RE
        .replace_all(text, |caps: &regex::Captures| {
            let ch = caps.get(1).unwrap().as_str().chars().next().unwrap();
            mathbb_char(ch).to_string()
        })
        .to_string()
}

fn mathbb_char(ch: char) -> char {
    match ch {
        // Uppercase: holes at C, H, N, P, Q, R, Z have dedicated codepoints
        'A' => '\u{1D538}', 'B' => '\u{1D539}', 'C' => '\u{2102}',
        'D' => '\u{1D53B}', 'E' => '\u{1D53C}', 'F' => '\u{1D53D}',
        'G' => '\u{1D53E}', 'H' => '\u{210D}',  'I' => '\u{1D540}',
        'J' => '\u{1D541}', 'K' => '\u{1D542}', 'L' => '\u{1D543}',
        'M' => '\u{1D544}', 'N' => '\u{2115}',  'O' => '\u{1D546}',
        'P' => '\u{2119}',  'Q' => '\u{211A}',  'R' => '\u{211D}',
        'S' => '\u{1D54A}', 'T' => '\u{1D54B}', 'U' => '\u{1D54C}',
        'V' => '\u{1D54D}', 'W' => '\u{1D54E}', 'X' => '\u{1D54F}',
        'Y' => '\u{1D550}', 'Z' => '\u{2124}',
        // Lowercase: U+1D552-U+1D56B (contiguous)
        c @ 'a'..='z' => char::from_u32(0x1D552 + (c as u32 - 'a' as u32)).unwrap_or(c),
        // Digits: U+1D7D8-U+1D7E1 (contiguous)
        c @ '0'..='9' => char::from_u32(0x1D7D8 + (c as u32 - '0' as u32)).unwrap_or(c),
        c => c,
    }
}

/// Map `\mathcal{X}` or `\mathscr{X}` to Unicode script characters.
fn convert_mathcal_mathscr(text: &str) -> String {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\\math(?:cal|scr)\{([A-Za-z])\}").unwrap());
    RE.replace_all(text, |caps: &regex::Captures| {
        let ch = caps.get(1).unwrap().as_str().chars().next().unwrap();
        mathscript_char(ch).to_string()
    })
    .to_string()
}

fn mathscript_char(ch: char) -> char {
    match ch {
        // Uppercase: holes at B, E, F, H, I, L, M, R
        'A' => '\u{1D49C}', 'B' => '\u{212C}',  'C' => '\u{1D49E}',
        'D' => '\u{1D49F}', 'E' => '\u{2130}',  'F' => '\u{2131}',
        'G' => '\u{1D4A2}', 'H' => '\u{210B}',  'I' => '\u{2110}',
        'J' => '\u{1D4A5}', 'K' => '\u{1D4A6}', 'L' => '\u{2112}',
        'M' => '\u{2133}',  'N' => '\u{1D4A9}', 'O' => '\u{1D4AA}',
        'P' => '\u{1D4AB}', 'Q' => '\u{1D4AC}', 'R' => '\u{211B}',
        'S' => '\u{1D4AE}', 'T' => '\u{1D4AF}', 'U' => '\u{1D4B0}',
        'V' => '\u{1D4B1}', 'W' => '\u{1D4B2}', 'X' => '\u{1D4B3}',
        'Y' => '\u{1D4B4}', 'Z' => '\u{1D4B5}',
        // Lowercase: holes at e, g, o
        'a' => '\u{1D4B6}', 'b' => '\u{1D4B7}', 'c' => '\u{1D4B8}',
        'd' => '\u{1D4B9}', 'e' => '\u{212F}',  'f' => '\u{1D4BB}',
        'g' => '\u{210A}',  'h' => '\u{1D4BD}', 'i' => '\u{1D4BE}',
        'j' => '\u{1D4BF}', 'k' => '\u{1D4C0}', 'l' => '\u{1D4C1}',
        'm' => '\u{1D4C2}', 'n' => '\u{1D4C3}', 'o' => '\u{2134}',
        'p' => '\u{1D4C5}', 'q' => '\u{1D4C6}', 'r' => '\u{1D4C7}',
        's' => '\u{1D4C8}', 't' => '\u{1D4C9}', 'u' => '\u{1D4CA}',
        'v' => '\u{1D4CB}', 'w' => '\u{1D4CC}', 'x' => '\u{1D4CD}',
        'y' => '\u{1D4CE}', 'z' => '\u{1D4CF}',
        c => c,
    }
}

/// Map `\mathfrak{X}` to Unicode fraktur characters.
fn convert_mathfrak(text: &str) -> String {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\\mathfrak\{([A-Za-z])\}").unwrap());
    RE.replace_all(text, |caps: &regex::Captures| {
        let ch = caps.get(1).unwrap().as_str().chars().next().unwrap();
        mathfrak_char(ch).to_string()
    })
    .to_string()
}

fn mathfrak_char(ch: char) -> char {
    match ch {
        // Uppercase: holes at C, H, I, R, Z
        'A' => '\u{1D504}', 'B' => '\u{1D505}', 'C' => '\u{212D}',
        'D' => '\u{1D507}', 'E' => '\u{1D508}', 'F' => '\u{1D509}',
        'G' => '\u{1D50A}', 'H' => '\u{210C}',  'I' => '\u{2111}',
        'J' => '\u{1D50D}', 'K' => '\u{1D50E}', 'L' => '\u{1D50F}',
        'M' => '\u{1D510}', 'N' => '\u{1D511}', 'O' => '\u{1D512}',
        'P' => '\u{1D513}', 'Q' => '\u{1D514}', 'R' => '\u{211C}',
        'S' => '\u{1D516}', 'T' => '\u{1D517}', 'U' => '\u{1D518}',
        'V' => '\u{1D519}', 'W' => '\u{1D51A}', 'X' => '\u{1D51B}',
        'Y' => '\u{1D51C}', 'Z' => '\u{2128}',
        // Lowercase: U+1D51E-U+1D537 (contiguous)
        c @ 'a'..='z' => char::from_u32(0x1D51E + (c as u32 - 'a' as u32)).unwrap_or(c),
        c => c,
    }
}

/// Map `\mathsf{X}` to Unicode sans-serif characters.
fn convert_mathsf(text: &str) -> String {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\\mathsf\{([A-Za-z])\}").unwrap());
    RE.replace_all(text, |caps: &regex::Captures| {
        let ch = caps.get(1).unwrap().as_str().chars().next().unwrap();
        mathsf_char(ch).to_string()
    })
    .to_string()
}

fn mathsf_char(ch: char) -> char {
    match ch {
        // Uppercase: U+1D5A0-U+1D5B9 (contiguous)
        c @ 'A'..='Z' => char::from_u32(0x1D5A0 + (c as u32 - 'A' as u32)).unwrap_or(c),
        // Lowercase: U+1D5BA-U+1D5D3 (contiguous)
        c @ 'a'..='z' => char::from_u32(0x1D5BA + (c as u32 - 'a' as u32)).unwrap_or(c),
        c => c,
    }
}

// ---------------------------------------------------------------------------
// Inverted punctuation (Stage 4f)
// ---------------------------------------------------------------------------

/// Convert `!`` → ¡ and `?`` → ¿ (TeX inverted punctuation convention).
/// Only when NOT followed by another backtick (to avoid interfering with ``).
fn replace_inverted_punct(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut result = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if (bytes[i] == b'!' || bytes[i] == b'?')
            && i + 1 < bytes.len()
            && bytes[i + 1] == b'`'
            && !(i + 2 < bytes.len() && bytes[i + 2] == b'`')
        {
            result.push(if bytes[i] == b'!' { '\u{00A1}' } else { '\u{00BF}' });
            i += 2;
        } else if bytes[i] < 128 {
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

// ---------------------------------------------------------------------------
// Main conversion entry point
// ---------------------------------------------------------------------------

/// Convert LaTeX symbol commands and ligatures to Unicode.
pub fn convert_symbols(text: &str) -> String {
    // Inverted punctuation before ligature conversion
    let mut result = replace_inverted_punct(text);

    // Ligatures
    result = result
        .replace("``", "\u{201c}")
        .replace("''", "\u{201d}")
        .replace("---", "\u{2014}")
        .replace("--", "\u{2013}")
        .replace('~', " ");

    // Escaped characters
    result = result
        .replace("\\&", "&")
        .replace("\\#", "#")
        .replace("\\$", "$")
        .replace("\\%", "%")
        .replace("\\_", "_");

    // Spacing symbols (punctuation-based, not word-boundary-safe for CommandReplacer)
    result = result
        .replace("\\,", "\u{2009}")
        .replace("\\;", "\u{205F}")
        .replace("\\:", "\u{2009}")
        .replace("\\!", "");

    // Punctuation-style commands not handled by CommandReplacer boundary check
    result = result.replace("\\|", "\u{2016}");

    // Command replacers (Aho-Corasick with word boundary)
    result = TEXT_COMMANDS.replace_all(&result);
    result = MATH_SYMBOLS.replace_all(&result);
    result = GREEK_LETTERS.replace_all(&result);

    // Unicode math font converters
    result = convert_mathbb(&result);
    result = convert_mathcal_mathscr(&result);
    result = convert_mathfrak(&result);
    result = convert_mathsf(&result);

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
    fn test_mathbb_full_alphabet() {
        // Uppercase with dedicated codepoints
        assert_eq!(convert_symbols("\\mathbb{C}"), "\u{2102}");
        assert_eq!(convert_symbols("\\mathbb{N}"), "\u{2115}");
        // Uppercase from supplementary plane
        assert_eq!(convert_symbols("\\mathbb{A}"), "\u{1D538}");
        assert_eq!(convert_symbols("\\mathbb{X}"), "\u{1D54F}");
        // Lowercase
        assert_eq!(convert_symbols("\\mathbb{a}"), "\u{1D552}");
        assert_eq!(convert_symbols("\\mathbb{z}"), "\u{1D56B}");
        // Digits
        assert_eq!(convert_symbols("\\mathbb{0}"), "\u{1D7D8}");
        assert_eq!(convert_symbols("\\mathbb{9}"), "\u{1D7E1}");
    }

    #[test]
    fn test_bbb_alias() {
        assert_eq!(convert_symbols("\\Bbb{R}"), "\u{211D}");
    }

    #[test]
    fn test_mathcal() {
        assert_eq!(convert_symbols("\\mathcal{A}"), "\u{1D49C}");
        assert_eq!(convert_symbols("\\mathcal{B}"), "\u{212C}");
        assert_eq!(convert_symbols("\\mathscr{H}"), "\u{210B}");
    }

    #[test]
    fn test_mathfrak() {
        assert_eq!(convert_symbols("\\mathfrak{A}"), "\u{1D504}");
        assert_eq!(convert_symbols("\\mathfrak{C}"), "\u{212D}");
        assert_eq!(convert_symbols("\\mathfrak{a}"), "\u{1D51E}");
    }

    #[test]
    fn test_mathsf() {
        assert_eq!(convert_symbols("\\mathsf{A}"), "\u{1D5A0}");
        assert_eq!(convert_symbols("\\mathsf{a}"), "\u{1D5BA}");
    }

    #[test]
    fn test_big_operators() {
        assert_eq!(convert_symbols("\\sum"), "\u{2211}");
        assert_eq!(convert_symbols("\\prod"), "\u{220F}");
        assert_eq!(convert_symbols("\\int"), "\u{222B}");
        assert_eq!(convert_symbols("\\oint"), "\u{222E}");
    }

    #[test]
    fn test_hebrew() {
        assert_eq!(convert_symbols("\\aleph"), "\u{2135}");
    }

    #[test]
    fn test_spacing_symbols() {
        assert_eq!(convert_symbols("a\\,b"), "a\u{2009}b");
        assert_eq!(convert_symbols("a\\;b"), "a\u{205F}b");
        assert_eq!(convert_symbols("a\\!b"), "ab");
    }

    #[test]
    fn test_inverted_punct() {
        assert_eq!(convert_symbols("!`Hola"), "\u{00A1}Hola");
        assert_eq!(convert_symbols("?`Que"), "\u{00BF}Que");
        // Double backtick should NOT be treated as inverted punct
        assert_eq!(convert_symbols("!``hello''"), "!\u{201c}hello\u{201d}");
    }

    #[test]
    fn test_new_arrows() {
        assert_eq!(convert_symbols("\\Uparrow"), "\u{21D1}");
        assert_eq!(convert_symbols("\\nearrow"), "\u{2197}");
        assert_eq!(convert_symbols("\\Longrightarrow"), "\u{27F9}");
    }

    #[test]
    fn test_text_commands_expanded() {
        assert_eq!(convert_symbols("\\textcopyright"), "\u{00A9}");
        assert_eq!(convert_symbols("\\texteuro"), "\u{20AC}");
        assert_eq!(convert_symbols("\\textonehalf"), "\u{00BD}");
    }

    #[test]
    fn test_greek_variants() {
        assert_eq!(convert_symbols("\\varpi"), "\u{03D6}");
        assert_eq!(convert_symbols("\\varrho"), "\u{03F1}");
        assert_eq!(convert_symbols("\\varsigma"), "\u{03C2}");
        assert_eq!(convert_symbols("\\digamma"), "\u{03DD}");
    }

    #[test]
    fn test_upright_greek() {
        assert_eq!(convert_symbols("\\upalpha"), "\u{03b1}");
        assert_eq!(convert_symbols("\\upomega"), "\u{03c9}");
    }

    #[test]
    fn test_delimiter_names() {
        assert_eq!(convert_symbols("\\lbrace"), "{");
        assert_eq!(convert_symbols("\\rbrace"), "}");
        assert_eq!(convert_symbols("\\vert"), "|");
        assert_eq!(convert_symbols("\\Vert"), "\u{2016}");
    }

    #[test]
    fn test_pipe_delimiter() {
        assert_eq!(convert_symbols("\\|"), "\u{2016}");
    }
}
