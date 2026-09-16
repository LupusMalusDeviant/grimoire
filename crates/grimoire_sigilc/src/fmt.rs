//! The Sigil formatter (Plan 0002 WP4.1, engine ADR-0007).
//!
//! [`format()`] reproduces a syntax tree's exact source text: because every token the lexer
//! produces, trivia included, ends up as exactly one leaf of the tree (`syntax.rs`'s module
//! docs), formatting an *unmodified* tree is nothing more than walking those leaves in order and
//! concatenating their text. That triviality is the point: it is the proof that the tree is
//! lossless, exercised as the roundtrip property `parse -> fmt -> parse` (`tests/roundtrip.rs`),
//! and it is the foundation WP4.3's `sigilc set` builds on to rewrite a single value's token
//! without disturbing any other byte of the file — rewrite one leaf in place and this same
//! concatenation reproduces everything else unchanged.

use crate::syntax::SyntaxNode;

/// Renders `tree` back to source text by concatenating every token's exact text, in order.
///
/// For a tree produced by [`crate::parser::parse`] and never modified, `format(&parse(name,
/// src).tree) == src` for any `src` (the roundtrip property WP4.1's gate requires).
#[must_use]
pub fn format(tree: &SyntaxNode) -> String {
    tree.text()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

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
}
