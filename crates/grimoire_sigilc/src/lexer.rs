//! Tokenizer for Sigil source text (Plan 0002 WP4.1, engine ADR-0007).
//!
//! The lexer never fails and never panics: every byte of the input ends up in exactly one
//! [`RawToken`] (trivia included), right up to a final [`SyntaxKind::Eof`] token, however
//! malformed the input is. Unrecognised bytes become one-character [`SyntaxKind::Unknown`]
//! tokens rather than being skipped or raising an error, and an unterminated string or an
//! unrecognised unit suffix still becomes a token (a [`SyntaxKind::StringLit`] or
//! [`SyntaxKind::Quantity`]) with a marker in the token's own text region so the *parser* can
//! raise a diagnostic with the right node-path context (contract §2 rule 9: this "decoder" of
//! foreign bytes never panics; the "error" it raises is a diagnostic collected on the side, not a
//! fatal `Result`, so the parser can keep going and report more than one problem per file, WP4.1
//! requirement 3). This split keeps the lexer itself trivial to prove terminating: it only ever
//! moves forward.
//!
//! Newlines are significant (a real [`SyntaxKind::Newline`] token) only at bracket depth 0; a
//! newline found inside `(...)` or `[...]` is folded into [`SyntaxKind::Whitespace`] instead, so
//! a list or record literal may span multiple lines (the grammar in `docs/formats/sigil.md`).
//! Braces never change the bracket depth, so a body's members stay one-per-line even though a
//! body can itself sit inside... nothing, in v1 — bodies are never values, only items and nested
//! members have one — but the rule is applied uniformly regardless.

use crate::span::{Position, Span};
use crate::syntax::SyntaxKind;

/// A single lexed token: its kind, its exact source text, its byte span and the 1-based
/// position of its first character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawToken {
    pub kind: SyntaxKind,
    pub text: String,
    pub span: Span,
    pub start: Position,
}

/// The fixed set of unit suffixes the grammar recognises (`docs/formats/sigil.md`). `beats` is
/// lexically valid but reserved: a later schema pass (Plan 0002 WP4.2) rejects it, not the lexer
/// or parser.
pub(crate) const KNOWN_UNITS: &[&str] = &["deg/t", "deg", "u/t2", "u/t", "u", "t", "beats"];

/// Characters a unit suffix may be made of. Deliberately generous (lowercase ASCII letters,
/// digits and `/`): the lexer consumes a *maximal* run of these right after a number with no
/// intervening space, then leaves it to the caller to decide whether the run is one of
/// [`KNOWN_UNITS`]. Maximal munch means `0.09u/t` is never split into `u` plus a stray `/t`, and
/// an unknown suffix like `30s` still becomes one `Quantity` token (`"30s"`) that a diagnostic can
/// point at directly, rather than two tokens that would each look fine in isolation.
fn is_unit_char(ch: char) -> bool {
    ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '/'
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_ident_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

struct Cursor {
    chars: Vec<char>,
    /// `byte_offsets[i]` is the byte offset of `chars[i]`; `byte_offsets[chars.len()]` is the
    /// total byte length, so a span can always be closed off without a bounds special-case.
    byte_offsets: Vec<u32>,
    idx: usize,
    line: u32,
    column: u32,
    /// Incremented on `(`/`[`, decremented on `)`/`]`; `{`/`}` never change it (see module docs).
    bracket_depth: i32,
}

impl Cursor {
    fn new(source: &str) -> Self {
        let mut chars = Vec::new();
        let mut byte_offsets = Vec::new();
        for (offset, ch) in source.char_indices() {
            byte_offsets.push(offset as u32);
            chars.push(ch);
        }
        byte_offsets.push(source.len() as u32);
        Self {
            chars,
            byte_offsets,
            idx: 0,
            line: 1,
            column: 1,
            bracket_depth: 0,
        }
    }

    fn pos(&self) -> Position {
        Position {
            line: self.line,
            column: self.column,
        }
    }

    fn byte_pos(&self) -> u32 {
        // `idx` is always in `0..=chars.len()`, and `byte_offsets` has exactly one more entry
        // than `chars`, so this index is always in bounds; `.get` plus a fallback to the total
        // length keeps that an invariant rather than a potential panic.
        self.byte_offsets
            .get(self.idx)
            .copied()
            .unwrap_or_else(|| self.byte_offsets[self.byte_offsets.len() - 1])
    }

    fn peek(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.idx + ahead).copied()
    }

    /// Consumes and returns the current character, advancing line/column bookkeeping. Column
    /// counts Unicode scalar values (ADR-0007); a `\n` starts a new line instead of merely
    /// advancing the column, whether or not it was preceded by `\r`.
    fn bump(&mut self) -> Option<char> {
        let ch = self.chars.get(self.idx).copied()?;
        self.idx += 1;
        if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(ch)
    }
}

/// Lexes `source` into a flat token stream ending in one [`SyntaxKind::Eof`] token.
///
/// Concatenating every returned token's `text` reproduces `source` exactly (the tree built on
/// top of these tokens is lossless by construction, `syntax.rs`'s module docs). Never panics,
/// never loops without making progress: every branch below consumes at least one character (or,
/// for `Eof`, terminates the loop).
pub(crate) fn lex(source: &str) -> Vec<RawToken> {
    let mut cursor = Cursor::new(source);
    let mut tokens = Vec::new();

    // A byte-order mark is invalid before the header (ADR-0007), but still becomes one token so
    // the tree stays lossless; the parser raises the header diagnostic.
    if cursor.peek(0) == Some('\u{FEFF}') {
        tokens.push(lex_one_char(&mut cursor, SyntaxKind::Bom));
    }

    loop {
        let Some(ch) = cursor.peek(0) else {
            let eof_start = cursor.pos();
            let eof_span = Span::empty_at(cursor.byte_pos());
            tokens.push(RawToken {
                kind: SyntaxKind::Eof,
                text: String::new(),
                span: eof_span,
                start: eof_start,
            });
            return tokens;
        };
        tokens.push(lex_token(&mut cursor, ch));
    }
}

fn lex_token(cursor: &mut Cursor, ch: char) -> RawToken {
    match ch {
        ' ' | '\t' => lex_whitespace_run(cursor),
        '\r' | '\n' => lex_newline(cursor),
        '/' if cursor.peek(1) == Some('/') => lex_comment(cursor),
        '{' => lex_one_char(cursor, SyntaxKind::LBrace),
        '}' => lex_one_char(cursor, SyntaxKind::RBrace),
        '(' => {
            cursor.bracket_depth += 1;
            lex_one_char(cursor, SyntaxKind::LParen)
        }
        ')' => {
            cursor.bracket_depth -= 1;
            lex_one_char(cursor, SyntaxKind::RParen)
        }
        '[' => {
            cursor.bracket_depth += 1;
            lex_one_char(cursor, SyntaxKind::LBracket)
        }
        ']' => {
            cursor.bracket_depth -= 1;
            lex_one_char(cursor, SyntaxKind::RBracket)
        }
        '=' => lex_one_char(cursor, SyntaxKind::Eq),
        ',' => lex_one_char(cursor, SyntaxKind::Comma),
        '.' => lex_one_char(cursor, SyntaxKind::Dot),
        '"' => lex_string(cursor),
        '-' if cursor.peek(1).is_some_and(|c| c.is_ascii_digit()) => lex_number(cursor),
        c if c.is_ascii_digit() => lex_number(cursor),
        c if is_ident_start(c) => lex_ident(cursor),
        _ => lex_one_char(cursor, SyntaxKind::Unknown),
    }
}

/// Pushes the cursor's current character onto `text` and advances past it, if there is one.
/// Centralising this (rather than `cursor.bump().unwrap_or(some_fallback_char)` at each call
/// site) matters for correctness, not just style: a fallback char would silently substitute the
/// wrong byte if a caller's "there must be a character here" reasoning were ever wrong, and with
/// arbitrary fuzz input a genuine NUL byte is a real character that must still be pushed, not a
/// sentinel for "nothing was there".
fn bump_into(cursor: &mut Cursor, text: &mut String) -> bool {
    if let Some(ch) = cursor.bump() {
        text.push(ch);
        true
    } else {
        false
    }
}

fn lex_one_char(cursor: &mut Cursor, kind: SyntaxKind) -> RawToken {
    let start = cursor.pos();
    let start_byte = cursor.byte_pos();
    let mut text = String::new();
    bump_into(cursor, &mut text);
    RawToken {
        kind,
        text,
        span: Span::new(start_byte, cursor.byte_pos()),
        start,
    }
}

fn lex_whitespace_run(cursor: &mut Cursor) -> RawToken {
    let start = cursor.pos();
    let start_byte = cursor.byte_pos();
    let mut text = String::new();
    while matches!(cursor.peek(0), Some(' ') | Some('\t')) {
        if let Some(ch) = cursor.bump() {
            text.push(ch);
        }
    }
    RawToken {
        kind: SyntaxKind::Whitespace,
        text,
        span: Span::new(start_byte, cursor.byte_pos()),
        start,
    }
}

/// Lexes one newline (`\n` or `\r\n`). Significant ([`SyntaxKind::Newline`]) at bracket depth 0,
/// otherwise folded into [`SyntaxKind::Whitespace`] trivia so lists and records may span lines.
/// A lone `\r` not followed by `\n` is not a grammar newline at all (the grammar's lexical
/// section defines `newline` as `[CR] LF`); it is still consumed as one character of trivia
/// rather than left for the next call to misclassify, keeping the "always makes progress"
/// invariant simple to see.
fn lex_newline(cursor: &mut Cursor) -> RawToken {
    let start = cursor.pos();
    let start_byte = cursor.byte_pos();
    let mut text = String::new();
    if cursor.peek(0) == Some('\r') {
        bump_into(cursor, &mut text);
        if cursor.peek(0) == Some('\n') {
            bump_into(cursor, &mut text);
        }
    } else if cursor.peek(0) == Some('\n') {
        bump_into(cursor, &mut text);
    }
    let kind = if text.ends_with('\n') && cursor.bracket_depth <= 0 {
        SyntaxKind::Newline
    } else {
        SyntaxKind::Whitespace
    };
    RawToken {
        kind,
        text,
        span: Span::new(start_byte, cursor.byte_pos()),
        start,
    }
}

fn lex_comment(cursor: &mut Cursor) -> RawToken {
    let start = cursor.pos();
    let start_byte = cursor.byte_pos();
    let mut text = String::new();
    // The two `/` characters of the marker.
    bump_into(cursor, &mut text);
    bump_into(cursor, &mut text);
    while let Some(ch) = cursor.peek(0) {
        if ch == '\n' || ch == '\r' {
            break;
        }
        bump_into(cursor, &mut text);
    }
    RawToken {
        kind: SyntaxKind::Comment,
        text,
        span: Span::new(start_byte, cursor.byte_pos()),
        start,
    }
}

/// Lexes a string literal. An unterminated string (no closing `"` before a newline or EOF) still
/// becomes one [`SyntaxKind::StringLit`] token, covering everything up to (not including) the
/// newline or EOF that stopped it, so the parser can report `SIG0003` at a useful span instead
/// of the lexer either panicking or silently absorbing the rest of the file.
fn lex_string(cursor: &mut Cursor) -> RawToken {
    let start = cursor.pos();
    let start_byte = cursor.byte_pos();
    let mut text = String::new();
    bump_into(cursor, &mut text); // opening quote
    loop {
        match cursor.peek(0) {
            None | Some('\n') | Some('\r') => break, // unterminated
            Some('"') => {
                bump_into(cursor, &mut text);
                break;
            }
            Some('\\') => {
                bump_into(cursor, &mut text);
                if let Some(escaped) = cursor.peek(0)
                    && escaped != '\n'
                    && escaped != '\r'
                {
                    bump_into(cursor, &mut text);
                }
            }
            Some(_) => {
                bump_into(cursor, &mut text);
            }
        }
    }
    RawToken {
        kind: SyntaxKind::StringLit,
        text,
        span: Span::new(start_byte, cursor.byte_pos()),
        start,
    }
}

/// Lexes a number (`int_lit` or `float_lit`) and, if a unit suffix immediately follows with no
/// space, extends the token into a [`SyntaxKind::Quantity`] (see [`is_unit_char`] for why a
/// maximal run is consumed regardless of whether it turns out to name a known unit).
fn lex_number(cursor: &mut Cursor) -> RawToken {
    let start = cursor.pos();
    let start_byte = cursor.byte_pos();
    let mut text = String::new();
    if cursor.peek(0) == Some('-') {
        bump_into(cursor, &mut text);
    }
    while cursor.peek(0).is_some_and(|c| c.is_ascii_digit()) {
        bump_into(cursor, &mut text);
    }
    let mut is_float = false;
    if cursor.peek(0) == Some('.') && cursor.peek(1).is_some_and(|c| c.is_ascii_digit()) {
        is_float = true;
        bump_into(cursor, &mut text);
        while cursor.peek(0).is_some_and(|c| c.is_ascii_digit()) {
            bump_into(cursor, &mut text);
        }
    }
    let mut kind = if is_float {
        SyntaxKind::FloatLit
    } else {
        SyntaxKind::IntLit
    };
    if cursor.peek(0).is_some_and(|c| c.is_ascii_lowercase()) {
        kind = SyntaxKind::Quantity;
        while cursor.peek(0).is_some_and(is_unit_char) {
            bump_into(cursor, &mut text);
        }
    }
    RawToken {
        kind,
        text,
        span: Span::new(start_byte, cursor.byte_pos()),
        start,
    }
}

fn lex_ident(cursor: &mut Cursor) -> RawToken {
    let start = cursor.pos();
    let start_byte = cursor.byte_pos();
    let mut text = String::new();
    while cursor.peek(0).is_some_and(is_ident_continue) {
        bump_into(cursor, &mut text);
    }
    RawToken {
        kind: SyntaxKind::Ident,
        text,
        span: Span::new(start_byte, cursor.byte_pos()),
        start,
    }
}

/// Splits a [`SyntaxKind::Quantity`] token's text into its numeric prefix and unit suffix, e.g.
/// `"0.09u/t"` into `("0.09", "u/t")`. Used by the parser to validate the suffix against
/// [`KNOWN_UNITS`] with the right node-path context (module docs).
///
/// Never panics: a `Quantity` token's text always has at least one trailing unit character by
/// construction ([`lex_number`] only assigns [`SyntaxKind::Quantity`] after consuming at least
/// one), but this function falls back to treating the whole text as the numeric part rather than
/// relying on that invariant if it is ever violated.
pub(crate) fn split_quantity(text: &str) -> (&str, &str) {
    let split_at = text
        .find(|c: char| c.is_ascii_lowercase())
        .unwrap_or(text.len());
    text.split_at(split_at)
}

/// A problem with a [`SyntaxKind::StringLit`] token's text, found by [`validate_string`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StringProblem {
    /// No closing `"` before the line (or the file) ended.
    Unterminated,
    /// A `\` was followed by a character other than `"`, `\`, `n` or `t` (the grammar's `escape`
    /// rule).
    InvalidEscape(char),
}

/// Checks a [`SyntaxKind::StringLit`] token's text against the grammar's `string`/`escape` rules.
/// The lexer itself never rejects a string (module docs: it always makes a token, valid or not),
/// so this is what lets the *parser* raise `SIG0003`/`SIG0004` with the right node-path context.
///
/// A leading `\` is always followed by at least one more character in a `StringLit` token's text
/// ([`lex_string`] only starts an escape when a character is actually there to pair it with), and
/// the token always starts with `"` ([`lex_string`] is only ever entered on `"`); this walks that
/// text rather than relying on either invariant, so a future change to the lexer can only ever
/// make this stricter or looser, never panic.
pub(crate) fn validate_string(text: &str) -> Option<StringProblem> {
    let mut chars = text.chars();
    chars.next(); // the opening quote.
    let mut in_escape = false;
    let mut closed = false;
    for ch in chars {
        if in_escape {
            in_escape = false;
            if !matches!(ch, '"' | '\\' | 'n' | 't') {
                return Some(StringProblem::InvalidEscape(ch));
            }
            continue;
        }
        match ch {
            '\\' => in_escape = true,
            '"' => closed = true,
            _ => {}
        }
    }
    if closed {
        None
    } else {
        Some(StringProblem::Unterminated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<SyntaxKind> {
        lex(source).into_iter().map(|t| t.kind).collect()
    }

    fn texts(source: &str) -> Vec<String> {
        lex(source).into_iter().map(|t| t.text).collect()
    }

    #[test]
    fn concatenated_token_text_reproduces_the_source() {
        let source = "sigil 1\nmeta {\n  name = \"Ring\" // c\n}\n";
        assert_eq!(texts(source).concat(), source);
    }

    #[test]
    fn newline_is_significant_at_depth_zero_but_trivia_inside_brackets() {
        let source = "a\n[\n1,\n2\n]\nb";
        let toks = lex(source);
        // Reconstruct: still lossless regardless of classification.
        assert_eq!(
            toks.iter().map(|t| t.text.as_str()).collect::<String>(),
            source
        );
        let newline_count = toks
            .iter()
            .filter(|t| t.kind == SyntaxKind::Newline)
            .count();
        // Significant: the newline between `a` and `[` and the one between `]` and `b` (bracket
        // depth 0 both times). Trivia: the three newlines inside `[...]` (bracket depth 1).
        assert_eq!(newline_count, 2);
    }

    #[test]
    fn quantity_token_keeps_unit_suffix_attached() {
        let toks = lex("0.09u/t\n");
        assert_eq!(toks[0].kind, SyntaxKind::Quantity);
        assert_eq!(toks[0].text, "0.09u/t");
        assert_eq!(split_quantity(&toks[0].text), ("0.09", "u/t"));
    }

    #[test]
    fn unknown_unit_suffix_still_becomes_one_quantity_token() {
        let toks = lex("30s\n");
        assert_eq!(toks[0].kind, SyntaxKind::Quantity);
        assert_eq!(toks[0].text, "30s");
    }

    #[test]
    fn unterminated_string_stops_at_the_line_end() {
        let toks = lex("\"abc\ndef");
        assert_eq!(toks[0].kind, SyntaxKind::StringLit);
        assert_eq!(toks[0].text, "\"abc");
        assert_eq!(
            validate_string(&toks[0].text),
            Some(StringProblem::Unterminated)
        );
    }

    #[test]
    fn terminated_string_with_valid_escapes_has_no_problem() {
        assert_eq!(validate_string("\"a\\n\\t\\\"\\\\b\""), None);
    }

    #[test]
    fn invalid_escape_is_reported() {
        assert_eq!(
            validate_string("\"a\\qb\""),
            Some(StringProblem::InvalidEscape('q'))
        );
    }

    #[test]
    fn a_backslash_escaping_the_closing_quote_does_not_count_as_closed() {
        // `"\"` (quote, backslash, quote): the trailing quote is consumed as the *target* of the
        // escape, not as a closing delimiter, so this string is still unterminated.
        let toks = lex("\"\\\"");
        assert_eq!(toks[0].kind, SyntaxKind::StringLit);
        assert_eq!(
            validate_string(&toks[0].text),
            Some(StringProblem::Unterminated)
        );
    }

    #[test]
    fn negative_numbers_lex_as_one_token() {
        assert_eq!(kinds("-0.5u")[0], SyntaxKind::Quantity);
        assert_eq!(texts("-0.5u")[0], "-0.5u");
        assert_eq!(kinds("-3")[0], SyntaxKind::IntLit);
    }

    #[test]
    fn every_byte_is_covered_by_exactly_one_token_span() {
        let source = "sigil 1\n// c\nmeta {\n  x = [1, 2,]\n}\n";
        let toks = lex(source);
        let mut expected_start = 0u32;
        for tok in &toks {
            assert_eq!(
                tok.span.start, expected_start,
                "gap or overlap before a token"
            );
            expected_start = tok.span.end;
        }
        assert_eq!(expected_start, source.len() as u32);
    }

    #[test]
    fn lexer_terminates_and_stays_lossless_on_arbitrary_bytes() {
        // Not a substitute for the proptest in `tests/no_panics.rs`, just a fast smoke test.
        for source in ["", "\0\0\0", "🦀🦀🦀", "((((((", "\"", "sigil", "----"] {
            let toks = lex(source);
            assert_eq!(
                toks.iter().map(|t| t.text.as_str()).collect::<String>(),
                source
            );
            assert_eq!(toks.last().map(|t| t.kind), Some(SyntaxKind::Eof));
        }
    }
}
