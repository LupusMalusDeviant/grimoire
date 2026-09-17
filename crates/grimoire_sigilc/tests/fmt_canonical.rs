//! `sigilc fmt` (Plan 0002 WP4.3): the canonical layout changes whitespace only, is idempotent,
//! refuses broken files, and brings any whitespace variant of a canonical file back to exactly
//! that file.
//!
//! The corpus and the facade fixtures are written in the canonical layout, so each of them is a
//! fixed point of `format_canonical`. The property test takes one of them, perturbs only its
//! whitespace (spaces and tabs between tokens, indentation, CRLF line endings, trailing
//! whitespace, extra blank lines where the layout collapses them) and demands that formatting
//! returns the original bytes. Because it also compiles both variants, it shows that formatting
//! never changes what a pattern compiles to.

mod support;

use std::collections::BTreeMap;

use grimoire_sigilc::compiler::{LoadError, SourceLoader, compile};
use grimoire_sigilc::{FormatError, SyntaxKind, format_canonical, parse};
use proptest::prelude::*;
use support::{clean_sigil_sources, corpus_root, corpus_sigil_files};

#[test]
fn clean_sources_are_already_canonical() {
    for (name, source) in clean_sigil_sources() {
        let formatted =
            format_canonical(&name, &source).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(formatted, source, "{name} is not in the canonical layout");
    }
}

#[test]
fn invalid_corpus_files_are_refused() {
    for (name, source) in corpus_sigil_files() {
        if !name.starts_with("invalid/") {
            continue;
        }
        assert!(
            matches!(
                format_canonical(&name, &source),
                Err(FormatError::SourceHasErrors { .. })
            ),
            "{name} should be refused"
        );
    }
}

/// Where extra blank lines may be inserted without changing the canonical result: right after an
/// opening delimiter, right before a closing one (with no comment in between), or where a blank
/// line already exists.
fn blank_lines_collapse_here(
    previous: Option<SyntaxKind>,
    next: Option<SyntaxKind>,
    followed_by_blank: bool,
) -> bool {
    followed_by_blank
        || matches!(
            previous,
            Some(SyntaxKind::LBrace | SyntaxKind::LParen | SyntaxKind::LBracket)
        )
        || matches!(
            next,
            Some(SyntaxKind::RBrace | SyntaxKind::RParen | SyntaxKind::RBracket)
        )
}

/// Rebuilds `source` with its whitespace perturbed from `noise` (a stream of random choices).
fn perturb_whitespace(source: &str, noise: &[u8]) -> String {
    let tokens: Vec<_> = parse("t.sigil", source).tree.tokens().cloned().collect();
    let mut noise = noise.iter().copied().cycle();
    let mut next_noise = move || noise.next().unwrap_or(0);
    let spaces = |n: u8| -> String {
        (0..(n % 4))
            .map(|i| if (n >> (i + 2)) & 1 == 1 { '\t' } else { ' ' })
            .collect()
    };
    let significant = |index: usize, step: isize| -> Option<SyntaxKind> {
        let mut i = index as isize + step;
        while i >= 0 && (i as usize) < tokens.len() {
            let kind = tokens[i as usize].kind;
            // A comment counts: a blank line next to a comment line is content, not collapsed.
            if !matches!(kind, SyntaxKind::Whitespace | SyntaxKind::Newline) {
                return Some(kind);
            }
            i += step;
        }
        None
    };

    let mut out = String::new();
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            SyntaxKind::Whitespace if !token.text.contains('\n') => {
                // Any horizontal whitespace becomes 1-3 random spaces or tabs.
                let n = next_noise();
                out.push_str(&spaces(n | 1));
            }
            SyntaxKind::Whitespace | SyntaxKind::Newline => {
                let crlf = next_noise() % 2 == 0;
                let newlines = token.text.matches('\n').count();
                let followed_by_blank = newlines > 1
                    || tokens
                        .get(index + 1)
                        .is_some_and(|t| t.kind == SyntaxKind::Newline);
                let extra = if blank_lines_collapse_here(
                    significant(index, -1),
                    significant(index, 1),
                    followed_by_blank,
                ) {
                    usize::from(next_noise() % 3)
                } else {
                    0
                };
                for _ in 0..newlines + extra {
                    out.push_str(&spaces(next_noise()));
                    out.push_str(if crlf { "\r\n" } else { "\n" });
                }
                // Random indentation of whatever follows.
                out.push_str(&spaces(next_noise()));
            }
            SyntaxKind::Eof => {}
            _ => {
                // Between two tokens that touch, sometimes insert whitespace (never before the
                // header's first token, which must stay at line 1, column 1).
                let touches_previous = index > 0
                    && !matches!(
                        tokens[index - 1].kind,
                        SyntaxKind::Whitespace | SyntaxKind::Newline
                    );
                if touches_previous && next_noise() % 3 == 0 {
                    out.push_str(&spaces(next_noise() | 1));
                }
                out.push_str(&token.text);
            }
        }
    }
    out
}

struct DirLoader(std::path::PathBuf);

impl SourceLoader for DirLoader {
    fn load(&self, path: &str) -> Result<String, LoadError> {
        std::fs::read_to_string(self.0.join(path)).map_err(|_| LoadError::NotFound)
    }
}

fn compiled(name: &str, source: &str) -> Option<Vec<u8>> {
    let mut behaviors = BTreeMap::new();
    behaviors.insert("orbit_parent".to_string(), 1);
    let loader = DirLoader(corpus_root().join("valid"));
    compile(name, source, &loader, &behaviors).bytes
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn whitespace_variants_format_back_to_the_canonical_file(
        file_pick in any::<prop::sample::Index>(),
        noise in prop::collection::vec(any::<u8>(), 1..64),
    ) {
        let sources = clean_sigil_sources();
        let (name, source) = file_pick.get(&sources);
        let messy = perturb_whitespace(source, &noise);
        let messy_parse = parse(name, &messy);
        prop_assert!(messy_parse.diagnostics.is_empty(), "{name}: perturbation broke the file: {:#?}\n{messy}", messy_parse.diagnostics);

        let formatted = format_canonical(name, &messy);
        prop_assert_eq!(formatted.as_deref(), Ok(source.as_str()), "{}", name);

        // Formatting never changes what a pattern compiles to.
        if name.starts_with("valid/") {
            let file = name.trim_start_matches("valid/");
            prop_assert_eq!(compiled(file, &messy), compiled(file, source));
        }
    }

    /// Whatever arbitrary text formats at all formats to a fixed point.
    #[test]
    fn formatting_is_idempotent_and_never_panics(source in "\\PC{0,200}") {
        if let Ok(once) = format_canonical("fuzz.sigil", &source) {
            prop_assert_eq!(format_canonical("fuzz.sigil", &once), Ok(once.clone()));
        }
    }

    /// Idempotence on real files with perturbed whitespace and arbitrary extra blank lines.
    #[test]
    fn formatting_perturbed_real_files_is_idempotent(
        file_pick in any::<prop::sample::Index>(),
        noise in prop::collection::vec(any::<u8>(), 1..64),
    ) {
        let sources = clean_sigil_sources();
        let (name, source) = file_pick.get(&sources);
        let messy = perturb_whitespace(source, &noise).replace("\n", "\n\n");
        if let Ok(once) = format_canonical(name, &messy) {
            prop_assert_eq!(format_canonical(name, &once), Ok(once.clone()));
        }
    }
}
