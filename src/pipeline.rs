use crate::cleanup::{cleanup, strip_pre_diacritic_commands};
use crate::comments::remove_comments;
use crate::diacritics::convert_diacritics;
use crate::environments::convert_environments;
use crate::formatting::convert_formatting;
use crate::input_resolve::{resolve_and_order, TexFile};
use crate::macros::{
    collect_macros, collect_newtheorems, collect_parametric_macros,
    expand_macros_cancellable, expand_parametric_macros_cancellable,
    normalize_shorthands, ParametricMacro,
};
use crate::preamble::extract_body;
use crate::references::convert_references;
use crate::structure::convert_structure;
use crate::symbols::convert_symbols;
use crate::timing::StageTimings;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

/// State threaded through the pipeline to share data between stages.
pub struct PipelineContext {
    pub macros: HashMap<String, String>,
    pub parametric_macros: Vec<ParametricMacro>,
    pub custom_theorems: HashMap<String, String>,
    pub section_label_map: HashMap<String, String>,
}

/// Output of the timed extraction pipeline.
pub struct ExtractionOutput {
    pub text: Option<String>,
    pub timings: StageTimings,
}

/// Run the full extraction pipeline on a collection of .tex files.
///
/// Returns the extracted text, or None if extraction produces empty output.
/// This is the original API — timing data is discarded.
pub fn extract_text(tex_files: &[TexFile]) -> Option<String> {
    extract_text_timed(tex_files).text
}

/// Run the full extraction pipeline, returning both text and per-stage timings.
pub fn extract_text_timed(tex_files: &[TexFile]) -> ExtractionOutput {
    extract_text_timed_cancellable(tex_files, None)
}

/// Cancellable variant of [`extract_text_timed`]. When `cancel` flips to
/// `true`, the pipeline stops between files and macro-expansion passes
/// and returns whatever it has produced so far.
pub fn extract_text_timed_cancellable(
    tex_files: &[TexFile],
    cancel: Option<&AtomicBool>,
) -> ExtractionOutput {
    if tex_files.is_empty() {
        return ExtractionOutput {
            text: None,
            timings: StageTimings::new(),
        };
    }

    let mut ctx = PipelineContext {
        macros: HashMap::new(),
        parametric_macros: Vec::new(),
        custom_theorems: HashMap::new(),
        section_label_map: HashMap::new(),
    };
    for f in tex_files {
        if let Some(c) = cancel {
            if c.load(Ordering::Relaxed) {
                return ExtractionOutput { text: None, timings: StageTimings::new() };
            }
        }
        ctx.macros.extend(collect_macros(&f.content));
        ctx.parametric_macros
            .extend(collect_parametric_macros(&f.content));
        ctx.custom_theorems.extend(collect_newtheorems(&f.content));
    }

    let mut timings = StageTimings::new();

    let ordered_files = resolve_and_order(tex_files);

    let mut parts = Vec::new();
    for file in &ordered_files {
        if let Some(c) = cancel {
            if c.load(Ordering::Relaxed) {
                break;
            }
        }
        match clean_tex_file(&file.content, &mut ctx, &mut timings, cancel) {
            Some(cleaned) if !cleaned.is_empty() => parts.push(cleaned),
            _ => {}
        }
    }

    let text = if parts.is_empty() {
        None
    } else {
        let result = parts.join("\n");
        if result.trim().is_empty() {
            None
        } else {
            Some(result)
        }
    };

    ExtractionOutput { text, timings }
}

/// Convert a single .tex file to clean readable text.
///
/// Timings are passed separately from the pipeline context to avoid
/// borrow-checker conflicts: `timings.time()` borrows `timings` mutably,
/// while the closures inside need shared borrows of `ctx.macros`,
/// `ctx.section_label_map`, etc. Keeping them in separate variables lets
/// the borrow checker see there is no overlap.
fn clean_tex_file(
    file_content: &str,
    ctx: &mut PipelineContext,
    timings: &mut StageTimings,
    cancel: Option<&AtomicBool>,
) -> Option<String> {
    let (preamble_extras, body) = extract_body(file_content);

    let mut body = if preamble_extras.is_empty() {
        body
    } else {
        format!("{}\n{}", preamble_extras.join("\n"), body)
    };

    body = timings.time("remove_comments", || remove_comments(&body));
    body = timings.time("normalize_shorthands", || normalize_shorthands(&body));
    body = timings.time("expand_macros", || {
        expand_macros_cancellable(&body, &ctx.macros, cancel)
    });
    body = timings.time("expand_parametric", || {
        expand_parametric_macros_cancellable(&body, &ctx.parametric_macros, cancel)
    });
    if let Some(c) = cancel {
        if c.load(Ordering::Relaxed) { return None; }
    }
    body = timings.time("convert_structure", || {
        convert_structure(&body, &mut ctx.section_label_map)
    });
    body = timings.time("convert_references", || {
        convert_references(&body, &ctx.section_label_map)
    });
    body = timings.time("convert_formatting", || convert_formatting(&body));
    body = timings.time("convert_environments", || {
        convert_environments(&body, &ctx.custom_theorems)
    });
    body = timings.time("strip_pre_diacritic", || {
        strip_pre_diacritic_commands(&body)
    });
    body = timings.time("convert_diacritics", || convert_diacritics(&body));
    body = timings.time("convert_symbols", || convert_symbols(&body));
    body = timings.time("cleanup", || cleanup(&body));

    if body.trim().is_empty() {
        None
    } else {
        Some(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_document() {
        let files = vec![TexFile {
            name: "main.tex".into(),
            content: r"\documentclass{article}
\begin{document}
\section{Introduction}
Hello world.
\end{document}"
                .into(),
        }];
        let result = extract_text(&files).unwrap();
        assert!(result.contains("1. Introduction"), "numbered section: {result}");
        assert!(result.contains("Hello world."));
    }

    #[test]
    fn test_with_macros() {
        let files = vec![TexFile {
            name: "main.tex".into(),
            content: r"\documentclass{article}
\newcommand{\myname}{John}
\begin{document}
Hello \myname!
\end{document}"
                .into(),
        }];
        let result = extract_text(&files).unwrap();
        assert!(result.contains("Hello John!"));
    }

    #[test]
    fn test_with_funding() {
        let files = vec![TexFile {
            name: "main.tex".into(),
            content: r"\documentclass{article}
\begin{document}
\section{Acknowledgements}
This work was funded by NSF grant 12345.
\end{document}"
                .into(),
        }];
        let result = extract_text(&files).unwrap();
        assert!(result.contains("funded by NSF grant 12345"));
    }

    #[test]
    fn test_preamble_thanks() {
        let files = vec![TexFile {
            name: "main.tex".into(),
            content: r"\documentclass{article}
\title{My Paper}
\thanks{Supported by NIH grant R01}
\begin{document}
Body text.
\end{document}"
                .into(),
        }];
        let result = extract_text(&files).unwrap();
        assert!(result.contains("Supported by NIH grant R01"));
    }

    #[test]
    fn test_empty_input() {
        let files: Vec<TexFile> = vec![];
        assert!(extract_text(&files).is_none());
    }

    #[test]
    fn test_empty_output() {
        let files = vec![TexFile {
            name: "main.tex".into(),
            content: "".into(),
        }];
        assert!(extract_text(&files).is_none());
    }

    #[test]
    fn test_full_bibliography() {
        let files = vec![TexFile {
            name: "main.tex".into(),
            content: r"\documentclass{article}
\begin{document}
Body text.
\begin{thebibliography}{99}
\bibitem[1]{ref1} Author One, Title One, 2020.
\bibitem[2]{ref2} Author Two, Title Two, 2021.
\end{thebibliography}
\end{document}"
                .into(),
        }];
        let result = extract_text(&files).unwrap();
        assert!(result.contains("References"));
        assert!(result.contains("[1]"), "first bibitem should be [1]: {result}");
        assert!(result.contains("Author One"));
        assert!(result.contains("[2]"), "second bibitem should be [2]: {result}");
        assert!(result.contains("Author Two"));
    }

    #[test]
    fn test_tabular_content_preserved() {
        let files = vec![TexFile {
            name: "main.tex".into(),
            content: r"\documentclass{article}
\begin{document}
\begin{table}[h]
\caption{Results}
\begin{tabular}{lcc}
Method & Precision & Recall \\
Ours & 0.95 & 0.87 \\
\end{tabular}
\end{table}
\end{document}"
                .into(),
        }];
        let result = extract_text(&files).unwrap();
        assert!(result.contains("Results"));
        assert!(result.contains("Method"));
        assert!(result.contains("Precision"));
        assert!(result.contains("0.95"));
        assert!(!result.contains('&'));
    }

    #[test]
    fn test_custom_newtheorem_pipeline() {
        let files = vec![TexFile {
            name: "main.tex".into(),
            content: r"\documentclass{article}
\newtheorem{dfn}{Definition}
\begin{document}
\begin{dfn}[Key]A definition.\end{dfn}
\end{document}"
                .into(),
        }];
        let result = extract_text(&files).unwrap();
        assert!(result.contains("**Definition (Key).**"), "custom theorem: {result}");
        assert!(result.contains("A definition."));
    }

    #[test]
    fn test_section_label_crossref() {
        let files = vec![TexFile {
            name: "main.tex".into(),
            content: r"\documentclass{article}
\begin{document}
\section{Intro}
\label{sec:intro}
\section{Methods}
\label{sec:methods}
See Section \ref{sec:methods}.
\end{document}"
                .into(),
        }];
        let result = extract_text(&files).unwrap();
        assert!(result.contains("See Section 2"), "section cross-ref: {result}");
    }

    #[test]
    fn test_extract_text_timed() {
        let files = vec![TexFile {
            name: "main.tex".into(),
            content: r"\documentclass{article}
\begin{document}
\section{Intro}
Hello.
\end{document}"
                .into(),
        }];
        let output = extract_text_timed(&files);
        assert!(output.text.is_some());
        assert!(output.text.unwrap().contains("Hello."));

        let timings = output.timings;
        assert!(
            timings.entries().len() >= 8,
            "expected >=8 stages, got {}",
            timings.entries().len()
        );
        assert!(timings.total_us() > 0);

        let names: Vec<&str> = timings.entries().iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"remove_comments"));
        assert!(names.contains(&"expand_macros"));
        assert!(names.contains(&"expand_parametric"));
        assert!(names.contains(&"convert_structure"));
        assert!(names.contains(&"cleanup"));
    }

    #[test]
    fn test_parametric_macro_single_arg() {
        let files = vec![TexFile {
            name: "main.tex".into(),
            content: r"\documentclass{article}
\newcommand{\vect}[1]{\mathbf{#1}}
\begin{document}
The vector \vect{x} is important.
\end{document}"
                .into(),
        }];
        let result = extract_text(&files).unwrap();
        assert!(!result.contains("\\vect"), "\\vect should be expanded: {result}");
    }

    #[test]
    fn test_parametric_macro_two_args() {
        let files = vec![TexFile {
            name: "main.tex".into(),
            content: r"\documentclass{article}
\newcommand{\inner}[2]{\langle #1, #2 \rangle}
\begin{document}
Value: \inner{a}{b}.
\end{document}"
                .into(),
        }];
        let result = extract_text(&files).unwrap();
        assert!(!result.contains("\\inner"), "\\inner should be expanded: {result}");
        assert!(result.contains("a") && result.contains("b"), "args present: {result}");
    }

    #[test]
    fn test_parametric_macro_with_optional_default() {
        let files = vec![TexFile {
            name: "main.tex".into(),
            content: r"\documentclass{article}
\newcommand{\greet}[2][Hello]{#1, #2!}
\begin{document}
\greet{World}
\greet[Hi]{World}
\end{document}"
                .into(),
        }];
        let result = extract_text(&files).unwrap();
        assert!(result.contains("Hello, World!"), "default opt: {result}");
        assert!(result.contains("Hi, World!"), "explicit opt: {result}");
    }

    #[test]
    fn test_parametric_def_macro() {
        let files = vec![TexFile {
            name: "main.tex".into(),
            content: r"\documentclass{article}
\def\norm#1{\left\|#1\right\|}
\begin{document}
Result: \norm{x}.
\end{document}"
                .into(),
        }];
        let result = extract_text(&files).unwrap();
        assert!(!result.contains("\\norm"), "\\norm should be expanded: {result}");
    }
}
