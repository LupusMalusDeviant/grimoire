//! Value listing and lossless single-value rewriting (Plan 0002 WP4.3: `sigilc parse --json
//! --values` and `sigilc set <file> <node-path>=<value>`).
//!
//! Both functions work on the WP4.1 parser's lossless tree. [`list_values`] walks it and names
//! every field value (and, recursively, every element of a list and every field of a record) by its
//! **value path**: the node path convention of `docs/formats/sigil.md` §5 for the field itself,
//! extended with `[<i>]` for the `i`-th list element and `.<name>` for a record field
//! (`emitters.bloom.modifiers[2].keys[1].mul`). [`set_value`] replaces exactly the source bytes of
//! one such value and nothing else: every byte before and after it — comments, blank lines,
//! indentation, member order — is copied unchanged, so the rewrite is lossless by construction
//! rather than by re-printing a tree. The result is re-parsed before it is returned, and a rewrite
//! that would not parse cleanly, or would not read back as exactly the requested value at exactly
//! the same path, is rejected instead of written.

use crate::diagnostics::Diagnostic;
use crate::lexer;
use crate::parser::parse;
use crate::span::{Position, Span};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};

/// Longest value path [`set_value`] accepts, in bytes. Far above any real path (the deepest corpus
/// path is under 50 bytes); it only keeps a hostile argument from being echoed back unbounded.
pub const MAX_NODE_PATH_BYTES: usize = 1024;

/// Longest replacement value [`set_value`] accepts, in bytes (one line of Sigil source).
pub const MAX_VALUE_BYTES: usize = 4096;

/// The syntactic shape of a listed value (`docs/formats/sigil.md` §4, `value`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValueKind {
    /// An integer literal, e.g. `24`.
    Int,
    /// A float literal, e.g. `0.6`.
    Float,
    /// A number with a unit suffix, e.g. `30t`.
    Quantity,
    /// A string literal, e.g. `"Ring Burst"`.
    String,
    /// A dotted reference, e.g. `enemy.crimson`.
    Ref,
    /// A trigger, e.g. `time 50t` or `event phase_end`.
    Trigger,
    /// A `[ ... ]` list; its elements are listed as their own values.
    List,
    /// A `( ... )` record; its fields are listed as their own values.
    Record,
}

impl ValueKind {
    /// The lowercase name used in JSON documents (`"int"`, `"float"`, `"quantity"`, `"string"`,
    /// `"ref"`, `"trigger"`, `"list"`, `"record"`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ValueKind::Int => "int",
            ValueKind::Float => "float",
            ValueKind::Quantity => "quantity",
            ValueKind::String => "string",
            ValueKind::Ref => "ref",
            ValueKind::Trigger => "trigger",
            ValueKind::List => "list",
            ValueKind::Record => "record",
        }
    }
}

/// One value found by [`list_values`]. Only produced by this module.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ValueEntry {
    /// The value path (module docs), e.g. `bullets.orb.glow` or `emitters.jitter.offset.y`.
    pub node_path: String,
    /// The value's syntactic shape.
    pub kind: ValueKind,
    /// The value's exact source text, without surrounding whitespace or comments.
    pub text: String,
    /// For [`ValueKind::Int`], [`ValueKind::Float`] and [`ValueKind::Quantity`]: the numeric
    /// literal as written (`"-0.5"` for `-0.5u`); `None` otherwise. Kept as text on purpose, so no
    /// consumer ever sees a rounded or out-of-range number (contract §2 rule 11).
    pub number: Option<String>,
    /// For [`ValueKind::Quantity`]: the unit suffix as written (`"u/t"`); `None` otherwise.
    pub unit: Option<String>,
    /// Byte span of [`ValueEntry::text`] in the source.
    pub span: Span,
    /// Position of the value's first character.
    pub start: Position,
    /// Position just past the value's last character.
    pub end: Position,
}

/// Lists every field value of a parsed file, in source order, a composite value before its own
/// elements.
///
/// Works on any tree, including one with parse diagnostics (best effort: a construct the parser
/// could not make sense of is skipped, and an item with a missing name uses the parser's own
/// placeholder `?`), so an editor can still show the parameters of a half-typed file.
#[must_use]
pub fn list_values(tree: &SyntaxNode) -> Vec<ValueEntry> {
    let source = tree.text();
    let lines = LineIndex::new(&source);
    let mut walker = Walker {
        source: &source,
        lines: &lines,
        out: Vec::new(),
    };
    for child in &tree.children {
        let SyntaxElement::Node(item) = child else {
            continue;
        };
        let prefix = match item.kind {
            SyntaxKind::MetaItem => "meta".to_string(),
            SyntaxKind::BulletItem => format!("bullets.{}", item_name(item)),
            SyntaxKind::EmitterItem => format!("emitters.{}", item_name(item)),
            _ => continue,
        };
        if let Some(body) = item.child_node(SyntaxKind::Body) {
            walker.body(body, &prefix);
        }
    }
    walker.out
}

/// The result of a successful [`set_value`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SetOutcome {
    /// The complete new source text.
    pub source: String,
    /// The value's previous source text.
    pub old_value: String,
    /// Byte span of the new value in [`SetOutcome::source`].
    pub span: Span,
    /// Whether the source changed at all (`false` when the new value equals the old one).
    pub changed: bool,
}

/// Why [`set_value`] refused a rewrite. The source is never modified in any of these cases.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum SetError {
    /// The node path is not a syntactically valid value path.
    #[error("invalid node path `{node_path}`: {reason}")]
    InvalidNodePath {
        /// The rejected path, as given.
        node_path: String,
        /// What is wrong with it.
        reason: &'static str,
    },
    /// The replacement text is not exactly one single-line Sigil value.
    #[error("invalid value `{value}`: {reason}")]
    InvalidValue {
        /// The rejected value, as given.
        value: String,
        /// What is wrong with it.
        reason: String,
    },
    /// The file does not parse cleanly, so its value paths cannot be trusted.
    #[error("the file has {} parse diagnostic(s); fix them before using `set`", .diagnostics.len())]
    SourceHasErrors {
        /// The file's parse diagnostics.
        diagnostics: Vec<Diagnostic>,
    },
    /// No value in the file has this path.
    #[error("no value at node path `{node_path}`")]
    NotFound {
        /// The path that matched nothing.
        node_path: String,
    },
    /// More than one value has this path (e.g. a field written twice, which the schema pass
    /// rejects but the parser accepts).
    #[error("node path `{node_path}` matches {matches} values")]
    Ambiguous {
        /// The ambiguous path.
        node_path: String,
        /// How many values it matched.
        matches: usize,
    },
    /// The value is valid on its own, but written at this position the file would no longer parse
    /// cleanly or would not read back as this value at this path (e.g. nesting too deep there, or
    /// a token merging with a neighbour).
    #[error("writing `{value}` at `{node_path}` would not leave a cleanly parsing file")]
    ResultHasErrors {
        /// The target path.
        node_path: String,
        /// The value that was to be written.
        value: String,
        /// Diagnostics of the rewritten file (empty if it parsed but did not read back correctly).
        diagnostics: Vec<Diagnostic>,
    },
}

impl SetError {
    /// Stable machine-readable name of the error kind, used as `error.code` in the `sigilc set
    /// --json` document (`docs/formats/sigil.md` §13): `invalid_node_path`, `invalid_value`,
    /// `source_has_errors`, `not_found`, `ambiguous` or `result_has_errors`.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            SetError::InvalidNodePath { .. } => "invalid_node_path",
            SetError::InvalidValue { .. } => "invalid_value",
            SetError::SourceHasErrors { .. } => "source_has_errors",
            SetError::NotFound { .. } => "not_found",
            SetError::Ambiguous { .. } => "ambiguous",
            SetError::ResultHasErrors { .. } => "result_has_errors",
        }
    }

    /// The diagnostics carried by this error, if any.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        match self {
            SetError::SourceHasErrors { diagnostics }
            | SetError::ResultHasErrors { diagnostics, .. } => diagnostics,
            _ => &[],
        }
    }
}

/// Replaces the value at `node_path` in `source` with `value`, touching no other byte.
///
/// `file` only labels diagnostics. `value` must be exactly one Sigil value on one line, with no
/// surrounding whitespace and no comment (`12`, `0.5deg/t`, `enemy.teal`, `[smashable, grazeable]`,
/// `(x = 0u, y = -0.5u)`, `time 30t`, `"text"`). Only values that already exist can be set; a
/// field is never inserted.
///
/// Guarantee (Plan 0002 WP4.3 gate, `tests/set_lossless.rs`): on success,
/// `outcome.source == source[..start] + value + source[end..]` where `start..end` is the old
/// value's span, the new source parses with no diagnostics, and [`list_values`] finds `value` at
/// `node_path` exactly once.
///
/// # Errors
/// Returns a [`SetError`] and leaves nothing modified if the path or the value is malformed, the
/// file has parse diagnostics, the path matches no value or more than one, or the rewritten file
/// would not parse cleanly with the value in place.
pub fn set_value(
    file: &str,
    source: &str,
    node_path: &str,
    value: &str,
) -> Result<SetOutcome, SetError> {
    validate_node_path(node_path)?;
    validate_value(value)?;

    let parsed = parse(file, source);
    if !parsed.diagnostics.is_empty() {
        return Err(SetError::SourceHasErrors {
            diagnostics: parsed.diagnostics,
        });
    }
    let values = list_values(&parsed.tree);
    let mut matches = values.iter().filter(|entry| entry.node_path == node_path);
    let Some(target) = matches.next() else {
        return Err(SetError::NotFound {
            node_path: node_path.to_string(),
        });
    };
    let extra = matches.count();
    if extra > 0 {
        return Err(SetError::Ambiguous {
            node_path: node_path.to_string(),
            matches: extra + 1,
        });
    }

    let start = target.span.start as usize;
    let end = target.span.end as usize;
    let (Some(before), Some(after)) = (source.get(..start), source.get(end..)) else {
        // Spans come from the lossless tree of this very source, so they always sit on char
        // boundaries; reported as a failed rewrite rather than a panic if that ever breaks.
        return Err(result_error(node_path, value, Vec::new()));
    };
    let mut new_source = String::with_capacity(before.len() + value.len() + after.len());
    new_source.push_str(before);
    new_source.push_str(value);
    new_source.push_str(after);

    let reparsed = parse(file, &new_source);
    if !reparsed.diagnostics.is_empty() {
        return Err(result_error(node_path, value, reparsed.diagnostics));
    }
    let new_end = start + value.len();
    let read_back: Vec<ValueEntry> = list_values(&reparsed.tree)
        .into_iter()
        .filter(|entry| entry.node_path == node_path)
        .collect();
    let reads_back = matches!(
        read_back.as_slice(),
        [entry] if entry.text == value
            && entry.span.start as usize == start
            && entry.span.end as usize == new_end
    );
    if !reads_back {
        return Err(result_error(node_path, value, Vec::new()));
    }

    Ok(SetOutcome {
        changed: target.text != value,
        old_value: target.text.clone(),
        span: Span::new(start as u32, new_end as u32),
        source: new_source,
    })
}

fn result_error(node_path: &str, value: &str, diagnostics: Vec<Diagnostic>) -> SetError {
    SetError::ResultHasErrors {
        node_path: node_path.to_string(),
        value: value.to_string(),
        diagnostics,
    }
}

/// Checks the value path grammar: `segment { "." segment }`, `segment = ident { "[" digits "]" }`,
/// `ident = (letter | "_") { letter | digit | "_" }`.
fn validate_node_path(node_path: &str) -> Result<(), SetError> {
    let invalid = |reason: &'static str| SetError::InvalidNodePath {
        node_path: truncate_for_message(node_path),
        reason,
    };
    if node_path.is_empty() {
        return Err(invalid("the path is empty"));
    }
    if node_path.len() > MAX_NODE_PATH_BYTES {
        return Err(invalid("the path is longer than MAX_NODE_PATH_BYTES"));
    }
    for segment in node_path.split('.') {
        let bytes = segment.as_bytes();
        let ident_len = bytes
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || **b == b'_')
            .count();
        if ident_len == 0 {
            return Err(invalid("every segment must start with an identifier"));
        }
        if bytes[0].is_ascii_digit() {
            return Err(invalid("an identifier cannot start with a digit"));
        }
        let mut rest = &bytes[ident_len..];
        while !rest.is_empty() {
            let Some(after_open) = rest.strip_prefix(b"[") else {
                return Err(invalid("expected `[<index>]` or `.` after an identifier"));
            };
            let digits = after_open.iter().take_while(|b| b.is_ascii_digit()).count();
            if digits == 0 {
                return Err(invalid("an index must be a non-negative integer"));
            }
            let Some(after_close) = after_open[digits..].strip_prefix(b"]") else {
                return Err(invalid("an index must be closed with `]`"));
            };
            rest = after_close;
        }
    }
    Ok(())
}

/// Checks that `value` is exactly one single-line Sigil value by parsing it in a minimal file.
fn validate_value(value: &str) -> Result<(), SetError> {
    let invalid = |reason: String| SetError::InvalidValue {
        value: truncate_for_message(value),
        reason,
    };
    if value.is_empty() {
        return Err(invalid("the value is empty".to_string()));
    }
    if value.len() > MAX_VALUE_BYTES {
        return Err(invalid(format!(
            "the value is longer than {MAX_VALUE_BYTES} bytes"
        )));
    }
    if value.contains(['\n', '\r']) {
        return Err(invalid(
            "the value must fit on one line (no line breaks)".to_string(),
        ));
    }
    if value.starts_with([' ', '\t']) || value.ends_with([' ', '\t']) {
        return Err(invalid(
            "the value must not start or end with whitespace".to_string(),
        ));
    }
    let probe = format!("sigil 1\nmeta {{\n  value = {value}\n}}\n");
    let parsed = parse("value", &probe);
    if let Some(first) = parsed.diagnostics.first() {
        return Err(invalid(format!(
            "not a single Sigil value: {}",
            first.message
        )));
    }
    if parsed
        .tree
        .tokens()
        .any(|token| token.kind == SyntaxKind::Comment)
    {
        return Err(invalid("the value must not contain a comment".to_string()));
    }
    let values = list_values(&parsed.tree);
    match values.first() {
        Some(entry) if entry.node_path == "meta.value" && entry.text == value => Ok(()),
        _ => Err(invalid("not a single Sigil value".to_string())),
    }
}

/// Caps an echoed user input at a readable length (character-boundary safe).
fn truncate_for_message(text: &str) -> String {
    const LIMIT: usize = 200;
    if text.len() <= LIMIT {
        return text.to_string();
    }
    let mut cut = LIMIT;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}...", &text[..cut])
}

/// The name of a `bullet`/`emitter` item: its first identifier after the keyword, or the parser's
/// own placeholder `?` when it is missing.
fn item_name(item: &SyntaxNode) -> String {
    item.children
        .iter()
        .filter_map(|child| match child {
            SyntaxElement::Token(token) if token.kind == SyntaxKind::Ident => Some(token),
            _ => None,
        })
        .nth(1)
        .map_or_else(|| "?".to_string(), |token| token.text.clone())
}

struct Walker<'a> {
    source: &'a str,
    lines: &'a LineIndex,
    out: Vec<ValueEntry>,
}

impl Walker<'_> {
    /// Walks one body: fields, and nested `block`/`modifier`/`transform` members with the same
    /// positional numbering the parser uses for diagnostics (`docs/formats/sigil.md` §5).
    fn body(&mut self, body: &SyntaxNode, prefix: &str) {
        let mut modifiers = 0u32;
        let mut transforms = 0u32;
        for child in &body.children {
            let SyntaxElement::Node(node) = child else {
                continue;
            };
            match node.kind {
                SyntaxKind::Field => self.field(node, prefix),
                SyntaxKind::NestedMember => {
                    let keyword = first_token_text(node, SyntaxKind::Ident).unwrap_or_default();
                    let segment = match keyword {
                        "modifier" => {
                            modifiers += 1;
                            format!("modifiers[{}]", modifiers - 1)
                        }
                        "transform" => {
                            transforms += 1;
                            format!("transforms[{}]", transforms - 1)
                        }
                        _ => "block".to_string(),
                    };
                    if let Some(inner) = node.child_node(SyntaxKind::Body) {
                        self.body(inner, &format!("{prefix}.{segment}"));
                    }
                }
                _ => {}
            }
        }
    }

    fn field(&mut self, field: &SyntaxNode, prefix: &str) {
        let Some(path) = field.child_node(SyntaxKind::Path) else {
            return;
        };
        let key: String = path
            .tokens()
            .filter(|token| !token.kind.is_trivia())
            .map(|token| token.text.as_str())
            .collect();
        if let Some(value) = value_after_eq(&field.children) {
            self.value(value, format!("{prefix}.{key}"));
        }
    }

    fn value(&mut self, element: &SyntaxElement, node_path: String) {
        let kind = match element {
            SyntaxElement::Token(token) => match token.kind {
                SyntaxKind::IntLit => ValueKind::Int,
                SyntaxKind::FloatLit => ValueKind::Float,
                SyntaxKind::Quantity => ValueKind::Quantity,
                SyntaxKind::StringLit => ValueKind::String,
                _ => return,
            },
            SyntaxElement::Node(node) => match node.kind {
                SyntaxKind::RefValue => ValueKind::Ref,
                SyntaxKind::TriggerValue => ValueKind::Trigger,
                SyntaxKind::ListValue => ValueKind::List,
                SyntaxKind::RecordValue => ValueKind::Record,
                _ => return,
            },
        };
        let Some(span) = significant_span(element) else {
            return;
        };
        let text = self
            .source
            .get(span.start as usize..span.end as usize)
            .unwrap_or_default()
            .to_string();
        let (number, unit) = match kind {
            ValueKind::Int | ValueKind::Float => (Some(text.clone()), None),
            ValueKind::Quantity => {
                let (number, unit) = lexer::split_quantity(&text);
                (Some(number.to_string()), Some(unit.to_string()))
            }
            _ => (None, None),
        };
        self.out.push(ValueEntry {
            start: self.lines.position(span.start),
            end: self.lines.position(span.end),
            node_path: node_path.clone(),
            kind,
            text,
            number,
            unit,
            span,
        });

        let SyntaxElement::Node(node) = element else {
            return;
        };
        match node.kind {
            SyntaxKind::ListValue => {
                let elements = node.children.iter().filter(|child| {
                    !matches!(
                        child.kind(),
                        SyntaxKind::LBracket | SyntaxKind::RBracket | SyntaxKind::Comma
                    ) && !child.kind().is_trivia()
                });
                for (index, child) in elements.enumerate() {
                    self.value(child, format!("{node_path}[{index}]"));
                }
            }
            SyntaxKind::RecordValue => {
                for child in &node.children {
                    let SyntaxElement::Node(record_field) = child else {
                        continue;
                    };
                    if record_field.kind != SyntaxKind::RecordField {
                        continue;
                    }
                    let Some(name) = first_token_text(record_field, SyntaxKind::Ident) else {
                        continue;
                    };
                    if let Some(value) = value_after_eq(&record_field.children) {
                        self.value(value, format!("{node_path}.{name}"));
                    }
                }
            }
            _ => {}
        }
    }
}

/// The first significant element after the first `=` token among `children` (a field's or record
/// field's value), skipping the leading trivia tokens attached before a scalar value.
fn value_after_eq(children: &[SyntaxElement]) -> Option<&SyntaxElement> {
    let eq = children
        .iter()
        .position(|child| matches!(child, SyntaxElement::Token(t) if t.kind == SyntaxKind::Eq))?;
    children[eq + 1..]
        .iter()
        .find(|child| !child.kind().is_trivia())
}

fn first_token_text(node: &SyntaxNode, kind: SyntaxKind) -> Option<&str> {
    node.children.iter().find_map(|child| match child {
        SyntaxElement::Token(token) if token.kind == kind => Some(token.text.as_str()),
        _ => None,
    })
}

/// The span from an element's first significant token to its last token (a node's leading trivia
/// lives inside the node, so its own span would start too early).
fn significant_span(element: &SyntaxElement) -> Option<Span> {
    match element {
        SyntaxElement::Token(token) => Some(token.span),
        SyntaxElement::Node(node) => {
            let first = node.tokens().find(|token| !token.kind.is_trivia())?;
            let last: &SyntaxToken = node.tokens().last()?;
            Some(Span::new(first.span.start, last.span.end))
        }
    }
}

/// Byte offset to 1-based line/column (columns in Unicode scalar values, as everywhere in Sigil
/// diagnostics).
struct LineIndex {
    source: String,
    line_starts: Vec<u32>,
}

impl LineIndex {
    fn new(source: &str) -> Self {
        let mut line_starts = vec![0u32];
        for (offset, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(offset as u32 + 1);
            }
        }
        Self {
            source: source.to_string(),
            line_starts,
        }
    }

    fn position(&self, offset: u32) -> Position {
        let line_index = match self.line_starts.binary_search(&offset) {
            Ok(index) => index,
            Err(index) => index.saturating_sub(1),
        };
        let line_start = self.line_starts.get(line_index).copied().unwrap_or(0);
        let column = self
            .source
            .get(line_start as usize..offset as usize)
            .map_or(0, |text| text.chars().count());
        Position {
            line: line_index as u32 + 1,
            column: column as u32 + 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "sigil 1\n\
        meta {\n\
        \x20 name = \"Sample\" // trailing comment\n\
        \x20 difficulty = [easy, opener]\n\
        }\n\
        \n\
        emitter jitter {\n\
        \x20 offset = (x = 0u, y = -0.5u)\n\
        \x20 block ring {\n\
        \x20   count = 24\n\
        \x20 }\n\
        \x20 modifier rotate {\n\
        \x20   rate = 0.5deg/t\n\
        \x20 }\n\
        \x20 modifier speed_curve {\n\
        \x20   keys = [\n\
        \x20     (at = 0t, mul = 1.6),\n\
        \x20   ]\n\
        \x20 }\n\
        }\n";

    fn paths(source: &str) -> Vec<(String, &'static str, String)> {
        list_values(&parse("t.sigil", source).tree)
            .into_iter()
            .map(|entry| (entry.node_path, entry.kind.as_str(), entry.text))
            .collect()
    }

    #[test]
    fn lists_fields_list_elements_and_record_fields_in_source_order() {
        let listed = paths(SAMPLE);
        let expected: Vec<(String, &str, String)> = [
            ("meta.name", "string", "\"Sample\""),
            ("meta.difficulty", "list", "[easy, opener]"),
            ("meta.difficulty[0]", "ref", "easy"),
            ("meta.difficulty[1]", "ref", "opener"),
            ("emitters.jitter.offset", "record", "(x = 0u, y = -0.5u)"),
            ("emitters.jitter.offset.x", "quantity", "0u"),
            ("emitters.jitter.offset.y", "quantity", "-0.5u"),
            ("emitters.jitter.block.count", "int", "24"),
            ("emitters.jitter.modifiers[0].rate", "quantity", "0.5deg/t"),
            (
                "emitters.jitter.modifiers[1].keys",
                "list",
                "[\n      (at = 0t, mul = 1.6),\n    ]",
            ),
            (
                "emitters.jitter.modifiers[1].keys[0]",
                "record",
                "(at = 0t, mul = 1.6)",
            ),
            ("emitters.jitter.modifiers[1].keys[0].at", "quantity", "0t"),
            ("emitters.jitter.modifiers[1].keys[0].mul", "float", "1.6"),
        ]
        .into_iter()
        .map(|(path, kind, text)| (path.to_string(), kind, text.to_string()))
        .collect();
        assert_eq!(listed, expected);
    }

    #[test]
    fn value_positions_and_number_parts_are_reported() {
        let values = list_values(&parse("t.sigil", SAMPLE).tree);
        let y = values
            .iter()
            .find(|entry| entry.node_path == "emitters.jitter.offset.y")
            .unwrap();
        assert_eq!(y.number.as_deref(), Some("-0.5"));
        assert_eq!(y.unit.as_deref(), Some("u"));
        assert_eq!(
            y.start,
            Position {
                line: 8,
                column: 25
            }
        );
        assert_eq!(
            y.end,
            Position {
                line: 8,
                column: 30
            }
        );
        let name = values
            .iter()
            .find(|entry| entry.node_path == "meta.name")
            .unwrap();
        assert_eq!(name.number, None);
        assert_eq!(name.unit, None);
    }

    #[test]
    fn set_replaces_only_the_value_bytes() {
        let outcome = set_value("t.sigil", SAMPLE, "emitters.jitter.block.count", "12").unwrap();
        assert_eq!(outcome.source, SAMPLE.replace("count = 24", "count = 12"));
        assert_eq!(outcome.old_value, "24");
        assert!(outcome.changed);

        let outcome = set_value("t.sigil", SAMPLE, "meta.name", "\"Renamed\"").unwrap();
        assert_eq!(
            outcome.source,
            SAMPLE.replace("\"Sample\" // trailing", "\"Renamed\" // trailing")
        );

        let outcome = set_value("t.sigil", SAMPLE, "meta.difficulty[1]", "closer").unwrap();
        assert_eq!(outcome.source, SAMPLE.replace("opener", "closer"));

        let same = set_value("t.sigil", SAMPLE, "meta.difficulty[1]", "opener").unwrap();
        assert_eq!(same.source, SAMPLE);
        assert!(!same.changed);
    }

    #[test]
    fn set_can_replace_a_composite_value_with_a_scalar_and_back() {
        let keys = "emitters.jitter.modifiers[1].keys";
        let original = list_values(&parse("t.sigil", SAMPLE).tree)
            .into_iter()
            .find(|entry| entry.node_path == keys)
            .unwrap()
            .text;
        // The original is multi-line; a replacement is single-line only, so going back to the
        // original goes through a single-line list and ends in a different, still valid file.
        let flat = set_value("t.sigil", SAMPLE, keys, "[(at = 0t, mul = 2.0)]").unwrap();
        assert!(flat.source.contains("keys = [(at = 0t, mul = 2.0)]\n  }"));
        assert_eq!(flat.old_value, original);
    }

    #[test]
    fn set_rejects_malformed_paths_and_values_without_touching_anything() {
        for path in [
            "", ".", "a..b", "a.", "1a", "a[", "a[x]", "a[1", "a[1]x", "a b",
        ] {
            assert_eq!(
                set_value("t.sigil", SAMPLE, path, "1").unwrap_err().code(),
                "invalid_node_path",
                "{path:?}"
            );
        }
        for value in [
            "", " 1", "1 ", "1 2", "1 // c", "[1,", "1\n2", "\"open", "}", "a = 1",
        ] {
            assert_eq!(
                set_value("t.sigil", SAMPLE, "meta.name", value)
                    .unwrap_err()
                    .code(),
                "invalid_value",
                "{value:?}"
            );
        }
        assert_eq!(
            set_value("t.sigil", SAMPLE, "meta.missing", "1")
                .unwrap_err()
                .code(),
            "not_found"
        );
        assert_eq!(
            set_value("t.sigil", SAMPLE, "emitters.jitter.block", "1")
                .unwrap_err()
                .code(),
            "not_found"
        );
    }

    #[test]
    fn set_rejects_ambiguous_paths_and_broken_sources() {
        let twice = "sigil 1\nmeta {\n  density = 1\n  density = 2\n}\n";
        assert_eq!(
            set_value("t.sigil", twice, "meta.density", "3")
                .unwrap_err()
                .code(),
            "ambiguous"
        );
        let broken = "sigil 1\nmeta {\n  density = 1\n";
        let error = set_value("t.sigil", broken, "meta.density", "3").unwrap_err();
        assert_eq!(error.code(), "source_has_errors");
        assert!(!error.diagnostics().is_empty());
    }

    #[test]
    fn set_rejects_a_value_that_would_merge_with_the_following_comment() {
        // `abc// c` is a clean ref followed by a comment; `30t// c` would lex as one malformed
        // quantity token.
        let source = "sigil 1\nmeta {\n  patron = abc// c\n}\n";
        assert!(parse("t.sigil", source).diagnostics.is_empty());
        let error = set_value("t.sigil", source, "meta.patron", "30t").unwrap_err();
        assert_eq!(error.code(), "result_has_errors");
    }

    #[test]
    fn line_index_counts_columns_in_chars() {
        let index = LineIndex::new("ab\nç\u{2603}x\n");
        assert_eq!(index.position(0), Position { line: 1, column: 1 });
        assert_eq!(index.position(3), Position { line: 2, column: 1 });
        // `ç` is 2 bytes, the snowman 3: `x` starts at byte 8, the third char of line 2.
        assert_eq!(index.position(8), Position { line: 2, column: 3 });
    }
}
