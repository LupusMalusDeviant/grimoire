//! The Sigil formatters (Plan 0002 WP4.1 and WP4.3, engine ADR-0007).
//!
//! [`format()`] reproduces a syntax tree's exact source text: because every token the lexer
//! produces, trivia included, ends up as exactly one leaf of the tree (`syntax.rs`'s module
//! docs), formatting an *unmodified* tree is nothing more than walking those leaves in order and
//! concatenating their text. That triviality is the point: it is the proof that the tree is
//! lossless, exercised as the roundtrip property `parse -> fmt -> parse` (`tests/roundtrip.rs`),
//! and it is the foundation WP4.3's `sigilc set` ([`crate::edit::set_value`]) builds on to rewrite
//! a single value without disturbing any other byte of the file.
//!
//! [`format_canonical`] is what `sigilc fmt` writes (WP4.3): the same tokens in the same order,
//! with only the whitespace between them normalised (`docs/formats/sigil.md` §13.7). It never
//! adds, removes or reorders a significant token or a comment, and never touches the text inside
//! a token (a string literal; a comment's text up to its trailing whitespace). It re-parses its
//! own output and refuses to return anything whose tokens differ from the input's, so a formatter
//! bug surfaces as an error, never as a silently changed pattern.

use crate::diagnostics::Diagnostic;
use crate::parser::parse;
use crate::syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

/// Renders `tree` back to source text by concatenating every token's exact text, in order.
///
/// For a tree produced by [`crate::parser::parse`] and never modified, `format(&parse(name,
/// src).tree) == src` for any `src` (the roundtrip property WP4.1's gate requires).
#[must_use]
pub fn format(tree: &SyntaxNode) -> String {
    tree.text()
}

/// Why [`format_canonical`] refused to format a file. The input is never modified.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum FormatError {
    /// The file does not parse cleanly; re-laying out a broken construct could move it somewhere
    /// it means something else.
    #[error("the file has {} parse diagnostic(s); fix them before formatting", .diagnostics.len())]
    SourceHasErrors {
        /// The file's parse diagnostics.
        diagnostics: Vec<Diagnostic>,
    },
    /// The formatted text would not parse cleanly to the same tokens and comments. Never expected;
    /// it guards against a formatter bug corrupting a file.
    #[error("canonical formatting would change the file's tokens; the file was left unchanged")]
    TokensChanged,
}

/// Formats `source` in the canonical Sigil layout (`docs/formats/sigil.md` §13.7).
///
/// Only whitespace changes: two-space indentation per open body, list and record; one space around
/// `=`, after `,`, before `{` and before a trailing comment; no space inside `(...)`/`[...]`,
/// around `.`, before `,` or between a path and its `[index]`; line breaks exactly where the source
/// has them, with runs of blank lines collapsed to one, no blank line right after a line ending in
/// an opening delimiter or right before a closing one, `\n` line endings, no trailing whitespace
/// and exactly one final newline. `file` only labels diagnostics. The result is idempotent:
/// `format_canonical(format_canonical(x)) == format_canonical(x)`.
///
/// # Errors
/// [`FormatError::SourceHasErrors`] if `source` has parse diagnostics;
/// [`FormatError::TokensChanged`] if the formatted text would not re-parse cleanly to the same
/// significant tokens and comments.
pub fn format_canonical(file: &str, source: &str) -> Result<String, FormatError> {
    let parsed = parse(file, source);
    if !parsed.diagnostics.is_empty() {
        return Err(FormatError::SourceHasErrors {
            diagnostics: parsed.diagnostics,
        });
    }

    let mut layout = Layout::default();
    for token in parsed.tree.tokens() {
        layout.push(token);
    }
    let formatted = layout.finish();

    let reparsed = parse(file, &formatted);
    if !reparsed.diagnostics.is_empty()
        || token_signature(&parsed.tree) != token_signature(&reparsed.tree)
    {
        return Err(FormatError::TokensChanged);
    }
    Ok(formatted)
}

/// Every significant token (kind and exact text) and every comment (without trailing whitespace)
/// in order: what canonical formatting must preserve. Newlines are left out; where they may and
/// may not go is checked by the re-parse having no diagnostics.
fn token_signature(tree: &SyntaxNode) -> Vec<(SyntaxKind, String)> {
    tree.tokens()
        .filter(|token| {
            !matches!(
                token.kind,
                SyntaxKind::Whitespace | SyntaxKind::Newline | SyntaxKind::Eof
            )
        })
        .map(|token| (token.kind, comment_text(token).to_string()))
        .collect()
}

fn comment_text(token: &SyntaxToken) -> &str {
    if token.kind == SyntaxKind::Comment {
        token.text.trim_end_matches([' ', '\t'])
    } else {
        &token.text
    }
}

/// Line-oriented builder for [`format_canonical`].
#[derive(Default)]
struct Layout {
    out: String,
    /// Line breaks seen in the input since the last emitted token.
    pending_breaks: u32,
    /// Kind of the last token emitted, if any.
    previous: Option<SyntaxKind>,
    /// Kind of the last significant (non-comment) token emitted on the current output line.
    last_significant_on_line: Option<SyntaxKind>,
    brace_depth: u32,
    bracket_depth: u32,
}

impl Layout {
    fn push(&mut self, token: &SyntaxToken) {
        match token.kind {
            SyntaxKind::Newline => self.pending_breaks += 1,
            SyntaxKind::Whitespace => {
                // Spaces and tabs are recomputed; a line break inside `(...)`/`[...]` is kept.
                self.pending_breaks += token.text.matches('\n').count() as u32;
            }
            SyntaxKind::Eof => {}
            kind => self.emit(kind, comment_text(token)),
        }
    }

    fn emit(&mut self, kind: SyntaxKind, text: &str) {
        let is_closer = matches!(
            kind,
            SyntaxKind::RBrace | SyntaxKind::RParen | SyntaxKind::RBracket
        );
        // A comment runs to the end of its line, so whatever follows it starts a new line even if
        // the input separated the two only by a lone `\r`.
        let starts_line = self.previous.is_none()
            || self.pending_breaks > 0
            || self.previous == Some(SyntaxKind::Comment);

        if starts_line {
            if self.previous.is_some() {
                self.out.push('\n');
                let after_opener = matches!(
                    self.last_significant_on_line,
                    Some(SyntaxKind::LBrace | SyntaxKind::LParen | SyntaxKind::LBracket)
                );
                if self.pending_breaks > 1 && !after_opener && !is_closer {
                    self.out.push('\n');
                }
            }
            let level =
                (self.brace_depth + self.bracket_depth).saturating_sub(u32::from(is_closer));
            for _ in 0..level {
                self.out.push_str("  ");
            }
            self.last_significant_on_line = None;
        } else if let Some(previous) = self.previous {
            self.out.push_str(separator(previous, kind));
        }

        self.out.push_str(text);
        match kind {
            SyntaxKind::LBrace => self.brace_depth += 1,
            SyntaxKind::RBrace => self.brace_depth = self.brace_depth.saturating_sub(1),
            SyntaxKind::LParen | SyntaxKind::LBracket => self.bracket_depth += 1,
            SyntaxKind::RParen | SyntaxKind::RBracket => {
                self.bracket_depth = self.bracket_depth.saturating_sub(1);
            }
            _ => {}
        }
        if kind != SyntaxKind::Comment {
            self.last_significant_on_line = Some(kind);
        }
        self.previous = Some(kind);
        self.pending_breaks = 0;
    }

    fn finish(mut self) -> String {
        if self.previous.is_some() {
            self.out.push('\n');
        }
        self.out
    }
}

/// The whitespace between two tokens on the same output line.
fn separator(previous: SyntaxKind, next: SyntaxKind) -> &'static str {
    use SyntaxKind::{
        Comma, Comment, Dot, Ident, LBrace, LBracket, LParen, RBrace, RBracket, RParen,
    };
    match (previous, next) {
        (_, Comment) => " ",
        (_, Comma | RParen | RBracket | Dot)
        | (LParen | LBracket | Dot, _)
        | (Ident | RBracket, LBracket)
        | (LBrace, RBrace) => "",
        _ => " ",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_reproduces_the_source_for_a_valid_file() {
        let source = "sigil 1\nmeta {\n  name = \"Ring\" // trailing comment\n\n  x = 1\n}\n";
        let out = parse("test.sigil", source);
        assert_eq!(format(&out.tree), source);
    }

    #[test]
    fn format_reproduces_the_source_even_with_parse_errors() {
        for source in [
            "sigil 2\nmeta {}\n",
            "meta {}\n",
            "sigil 1\nbullet seed {\n  x = 1\n\nbullet other {\n}\n",
            "sigil 1\nmeta {\n  x = [1, 2,, 3]\n}\n",
            "",
            "sigil 1\n\u{2603}\n",
        ] {
            let out = parse("test.sigil", source);
            assert_eq!(format(&out.tree), source, "roundtrip broke for {source:?}");
        }
    }

    #[test]
    fn canonical_layout_normalises_only_whitespace() {
        let messy = "sigil   1   //  header note   \r\n\
                     \r\n\
                     \r\n\
                     emitter   finale   from ring . burst{\r\n\
                     \r\n\
                     \t delay=240t\t// late\r\n\
                     block . count  =  12\r\n\
                     offset=( x=0u ,y = -0.5u , )\r\n\
                     \x20     flags[1] = [ a,b ,\r\n\
                     \x20  c ]\r\n\
                     \x20 block ring { count = 8 }\r\n\
                     \r\n\
                     \x20 modifier rotate {}\r\n\
                     \r\n\
                     }";
        let expected = "sigil 1 //  header note\n\
                        \n\
                        emitter finale from ring.burst {\n\
                        \x20 delay = 240t // late\n\
                        \x20 block.count = 12\n\
                        \x20 offset = (x = 0u, y = -0.5u,)\n\
                        \x20 flags[1] = [a, b,\n\
                        \x20   c]\n\
                        \x20 block ring { count = 8 }\n\
                        \n\
                        \x20 modifier rotate {}\n\
                        }\n";
        assert_eq!(format_canonical("t.sigil", messy).unwrap(), expected);
        assert_eq!(format_canonical("t.sigil", expected).unwrap(), expected);
    }

    #[test]
    fn canonical_layout_indents_multi_line_lists_and_comments() {
        let source = "sigil 1\nemitter e {\nmodifier speed_curve {\n// tempo\nkeys = [\n(at = 0t, mul = 1.6),\n\n\n(at = 20t, mul = 0.3), // slow\n]\n\n}\n}\n";
        let expected = "sigil 1\nemitter e {\n  modifier speed_curve {\n    // tempo\n    keys = [\n      (at = 0t, mul = 1.6),\n\n      (at = 20t, mul = 0.3), // slow\n    ]\n  }\n}\n";
        assert_eq!(format_canonical("t.sigil", source).unwrap(), expected);
    }

    #[test]
    fn canonical_layout_refuses_files_with_diagnostics() {
        let error = format_canonical("t.sigil", "sigil 1\nmeta {\n").unwrap_err();
        assert!(
            matches!(error, FormatError::SourceHasErrors { ref diagnostics } if !diagnostics.is_empty())
        );
    }

    #[test]
    fn a_comment_followed_by_a_lone_carriage_return_still_ends_its_line() {
        let source = "sigil 1\nmeta {\n  flags = [a, // first\rb]\n}\n";
        assert!(parse("t.sigil", source).diagnostics.is_empty());
        let formatted = format_canonical("t.sigil", source).unwrap();
        assert_eq!(
            formatted,
            "sigil 1\nmeta {\n  flags = [a, // first\n    b]\n}\n"
        );
    }
}
