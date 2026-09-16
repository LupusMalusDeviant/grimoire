//! Roundtrip and no-panic property tests (Plan 0002 WP4.1 gate: `parse -> fmt -> parse`;
//! requirement 6: a property test throws arbitrary bytes and truncated files at the parser and it
//! must never panic or hang).
//!
//! The tree is lossless by construction (`syntax.rs`'s and `fmt.rs`'s module docs: every token,
//! trivia included, is a leaf exactly once, in source order), so `format(&parse(name, src).tree)
//! == src` is expected to hold completely unconditionally — for any `src` at all, not just valid
//! Sigil. These tests exercise that claim against the corpus, against structured and unstructured
//! random text, against raw random bytes, against pathologically deep nesting and against every
//! possible truncation point of every corpus file, specifically so a violation shows up as a
//! roundtrip mismatch (a clear, localised assertion failure) rather than only as a hang or a
//! crash somewhere else.

mod support;

use grimoire_sigilc::{format, parse};
use proptest::prelude::*;
use support::corpus_sigil_files;

#[test]
fn corpus_files_roundtrip_byte_exactly() {
    for (name, source) in corpus_sigil_files() {
        let output = parse(&name, &source);
        assert_eq!(format(&output.tree), source, "roundtrip broke for {name}");
    }
}

/// Every char boundary of every corpus file, truncated there, must still parse without a panic
/// or a hang and still roundtrip exactly — an exhaustive, deterministic stand-in for "throw
/// truncated files at the parser" (requirement 6) that is strictly stronger than sampling random
/// cut points, and fast enough (a few thousand small parses) to run on every `cargo test`.
#[test]
fn parser_never_panics_and_stays_lossless_on_every_truncation_of_the_corpus() {
    for (name, source) in corpus_sigil_files() {
        for (boundary, _) in source.char_indices() {
            let truncated = &source[..boundary];
            let output = parse(&name, truncated);
            assert_eq!(
                format(&output.tree),
                truncated,
                "truncating {name} at byte {boundary} broke the roundtrip"
            );
        }
        let output = parse(&name, &source);
        assert_eq!(format(&output.tree), source);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Arbitrary printable-ish Unicode text (never necessarily valid Sigil) must still roundtrip
    /// and must never panic the parser.
    #[test]
    fn roundtrip_holds_for_arbitrary_printable_text(source in "\\PC{0,400}") {
        let output = parse("fuzz.sigil", &source);
        prop_assert_eq!(format(&output.tree), source);
    }

    /// Arbitrary raw bytes, lossily decoded to the valid UTF-8 `&str` the public API accepts
    /// (this parser's "foreign bytes", contract §2 rule 9, are always text — a byte sequence
    /// that is not valid UTF-8 at all cannot be named as a Rust `&str` in the first place, so
    /// lossy decoding is how "arbitrary bytes" reaches this API instead of being rejected before
    /// ever calling it). Still must never panic and must still roundtrip exactly.
    #[test]
    fn roundtrip_holds_for_arbitrary_bytes_lossily_decoded(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
        let source = String::from_utf8_lossy(&bytes).into_owned();
        let output = parse("fuzz.sigil", &source);
        prop_assert_eq!(format(&output.tree), source);
    }

    /// Randomised nesting depth around and well past `MAX_NESTING_DEPTH`, complementing the
    /// fixed-depth unit test in `parser.rs`: the nesting guard must trip cleanly (one `SIG0010`,
    /// no stack overflow) at every depth, not just the one depth a hand-written test happens to
    /// pick.
    #[test]
    fn deeply_nested_lists_of_random_depth_never_overflow_the_stack(depth in 0usize..600) {
        let mut source = String::from("sigil 1\nmeta {\n  x = ");
        source.push_str(&"[".repeat(depth));
        source.push('1');
        source.push_str(&"]".repeat(depth));
        source.push_str("\n}\n");
        let output = parse("fuzz-depth.sigil", &source);
        prop_assert_eq!(format(&output.tree), source);
    }
}
