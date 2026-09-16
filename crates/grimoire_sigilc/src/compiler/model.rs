//! Schema-aware view of a parsed Sigil file: items, fields and nested members read off the
//! lossless syntax tree from [`crate::parser`] into a shape the rest of the `compiler` module can
//! resolve, validate and lower without re-inspecting [`crate::syntax::SyntaxNode`] children by
//! hand at every call site.
//!
//! This module deliberately stays a thin, generic reading of the tree: it does not itself decide
//! which fields a `block ring { ... }` accepts or what range `radius` allows (that is
//! [`crate::compiler::validate`]'s job, contract §11.1's "was der Compiler zusichert"). What it
//! does decide is the *shape* of a value (a quantity, a reference, a list, ...), which is purely
//! syntactic and therefore stable across every field and nested-member kind.

use std::collections::BTreeMap;

use crate::span::Position;
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

/// Computes the 1-based line/column [`Position`] of byte offset `offset` in `source`.
///
/// Columns count Unicode scalar values (contract-consistent with [`crate::span::Position`] and
/// the lexer/parser's own positions), not UTF-8 bytes. Used by the compiler's own diagnostics,
/// which run over an already-built tree (spans are byte offsets only, `syntax.rs`) rather than
/// during lexing, where a running line/column counter was cheaper to maintain incrementally.
#[must_use]
pub(crate) fn position_at(source: &str, offset: u32) -> Position {
    let offset = offset as usize;
    let mut line = 1u32;
    let mut column = 1u32;
    for (byte_index, ch) in source.char_indices() {
        if byte_index >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    Position { line, column }
}

/// A value read off a [`crate::syntax::SyntaxKind::Field`] or
/// [`crate::syntax::SyntaxKind::RecordField`]'s right-hand side.
///
/// Mirrors `docs/formats/sigil.md`'s `value` grammar production shape-for-shape; see the module
/// docs for why this stays generic rather than typed per field.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FieldValue {
    /// A bare identifier that is not a dotted reference, e.g. `orb` or `sub`.
    Ident(String),
    /// A dotted reference, e.g. `enemy.crimson` or `ring.burst` (one or more segments).
    Ref(Vec<String>),
    /// A double-quoted string literal, already unescaped.
    Str(String),
    /// An integer literal.
    Int(i64),
    /// A float literal (never a bare integer; see [`FieldValue::Int`]).
    Float(f64),
    /// A number with a unit suffix, e.g. `0.09u/t` as `(0.09, "u/t")`.
    Quantity(f64, String),
    /// A `[ ... ]` list value, in written order.
    List(Vec<FieldValue>),
    /// A `( ... )` record value: field name and value pairs, in written order.
    Record(Vec<(String, FieldValue)>),
    /// A `(time | distance | event) (quantity | ref)` trigger value.
    Trigger(String, Box<FieldValue>),
    /// The value could not be read (a parse error already reported by `WP4.1`'s parser produced
    /// an [`crate::syntax::SyntaxKind::ErrorNode`] or nothing at all here). Carrying this instead
    /// of `Option::None` lets validation still see *that* a field was present, so it does not
    /// also report a confusing "missing field" on top of the parser's own diagnostic.
    Invalid,
}

/// A field, keyed by its dotted/indexed path text (`docs/formats/sigil.md`'s node-path
/// convention, e.g. `"block.count"`, `"flags"`), together with where it came from.
#[derive(Debug, Clone)]
pub(crate) struct Located<T> {
    pub value: T,
    pub position: Position,
    /// The canonical path of the file this value's text actually appears in — the entry file for
    /// a plain field, or an imported file's path when the value was inherited, unoverridden,
    /// through an `emitter ... from ...` composition (`crate::compiler::resolve`).
    pub origin_file: String,
    /// Dotted/indexed node path for diagnostics, following `docs/formats/sigil.md`'s convention.
    pub node_path: String,
}

/// An ordered-by-name map of a body's fields (excluding nested `block`/`modifier`/`transform`
/// members, which [`BulletView`]/[`EmitterView`] hold separately).
pub(crate) type FieldMap = BTreeMap<String, Located<FieldValue>>;

/// A `block <kind> { ... }` nested member.
#[derive(Debug, Clone)]
pub(crate) struct BlockView {
    pub kind: String,
    pub kind_position: Position,
    pub fields: FieldMap,
}

/// A `modifier <kind> { ... }` nested member, in written order.
#[derive(Debug, Clone)]
pub(crate) struct ModifierView {
    pub kind: String,
    pub kind_position: Position,
    pub fields: FieldMap,
}

/// A `transform <kind> { ... }` nested member of a [`BulletView`], in written order.
#[derive(Debug, Clone)]
pub(crate) struct TransformView {
    pub kind: String,
    pub kind_position: Position,
    pub fields: FieldMap,
    /// The one nested `block` a `burst` transform carries (contract corpus example
    /// `03-subemitter-cascade.sigil`); `None` for every other transform kind.
    pub block: Option<BlockView>,
}

/// A `bullet <name> { ... }` item.
#[derive(Debug, Clone)]
pub(crate) struct BulletView {
    pub name: String,
    pub position: Position,
    pub fields: FieldMap,
    pub transforms: Vec<TransformView>,
}

/// An `emitter <name> [from <ref>] { ... }` item, before composition
/// ([`crate::compiler::resolve`] applies `from`).
#[derive(Debug, Clone)]
pub(crate) struct EmitterView {
    pub name: String,
    pub position: Position,
    /// `(alias, emitter_name)` of an `from <alias>.<emitter_name>` clause, if present.
    pub from: Option<(String, String, Position)>,
    pub fields: FieldMap,
    pub block: Option<BlockView>,
    pub modifiers: Vec<ModifierView>,
}

/// A whole parsed `.sigil` file, as items rather than a syntax tree.
#[derive(Debug, Clone)]
pub(crate) struct FileView {
    /// `alias -> imported path`, as written (not yet resolved relative to anything).
    pub imports: BTreeMap<String, (String, Position)>,
    pub bullets: BTreeMap<String, BulletView>,
    pub emitters: BTreeMap<String, EmitterView>,
}

impl FileView {
    /// Builds a [`FileView`] from an already-parsed tree. Parse-level diagnostics
    /// ([`crate::parser::parse`]'s own output) are the caller's responsibility to keep and report
    /// separately; a file with parse errors still yields whatever partial item structure the
    /// lossless tree could build (unrecognised fragments become
    /// [`crate::syntax::SyntaxKind::ErrorNode`], read here as [`FieldValue::Invalid`] fields where
    /// they show up in value position, and simply skipped where they show up in item position).
    pub(crate) fn build(source: &str, tree: &SyntaxNode) -> Self {
        let mut imports = BTreeMap::new();
        let mut bullets = BTreeMap::new();
        let mut emitters = BTreeMap::new();

        for child in &tree.children {
            let SyntaxElement::Node(node) = child else {
                continue;
            };
            match node.kind {
                SyntaxKind::ImportItem => {
                    if let Some((alias, target, pos)) = read_import(node, source) {
                        imports.insert(alias, (target, pos));
                    }
                }
                SyntaxKind::BulletItem => {
                    if let Some(bullet) = read_bullet(node, source) {
                        bullets.insert(bullet.name.clone(), bullet);
                    }
                }
                SyntaxKind::EmitterItem => {
                    if let Some(emitter) = read_emitter(node, source) {
                        emitters.insert(emitter.name.clone(), emitter);
                    }
                }
                _ => {}
            }
        }

        Self {
            imports,
            bullets,
            emitters,
        }
    }
}

/// Returns the first significant (non-trivia) direct child at or after `start`.
fn first_significant(children: &[SyntaxElement], start: usize) -> Option<&SyntaxElement> {
    children[start..].iter().find(|element| match element {
        SyntaxElement::Token(token) => !token.kind.is_trivia(),
        SyntaxElement::Node(_) => true,
    })
}

/// Every significant (non-trivia) direct child, in order.
fn significant_children(node: &SyntaxNode) -> impl Iterator<Item = &SyntaxElement> {
    node.children.iter().filter(|element| match element {
        SyntaxElement::Token(token) => !token.kind.is_trivia(),
        SyntaxElement::Node(_) => true,
    })
}

fn ident_text(element: &SyntaxElement) -> Option<&str> {
    match element {
        SyntaxElement::Token(token) if token.kind == SyntaxKind::Ident => Some(&token.text),
        _ => None,
    }
}

fn read_import(node: &SyntaxNode, source: &str) -> Option<(String, String, Position)> {
    // Children in order: "import", StringLit, "as", Ident (trivia interleaved).
    let mut significant = significant_children(node);
    significant.next()?; // "import"
    let path_token = significant.find_map(|e| match e {
        SyntaxElement::Token(token) if token.kind == SyntaxKind::StringLit => Some(token),
        _ => None,
    })?;
    let target = unquote(&path_token.text);
    // Re-iterate to find "as" then the alias identifier (order matters more than which loop).
    let mut after_as = false;
    for element in significant_children(node) {
        if after_as && let Some(alias) = ident_text(element) {
            let position = position_at(source, element.span().start);
            return Some((alias.to_string(), target, position));
        }
        if let SyntaxElement::Token(token) = element
            && token.kind == SyntaxKind::Ident
            && token.text == "as"
        {
            after_as = true;
        }
    }
    None
}

/// Strips the surrounding `"..."` and resolves the four known escapes. Any input that is not a
/// well-formed string (already flagged by `SIG0003`/`SIG0004` at parse time) is best-effort
/// unescaped rather than rejected again here.
fn unquote(text: &str) -> String {
    let inner = text.strip_prefix('"').unwrap_or(text);
    let inner = inner.strip_suffix('"').unwrap_or(inner);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other), // covers `\"` and `\\`; malformed input passes through
                None => {}
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn read_bullet(node: &SyntaxNode, source: &str) -> Option<BulletView> {
    let mut significant = significant_children(node);
    significant.next()?; // "bullet"
    let name_element = significant.next()?;
    let name = ident_text(name_element)?.to_string();
    let position = position_at(source, node.span.start);
    let body = node.child_node(SyntaxKind::Body)?;

    let mut fields = FieldMap::new();
    let mut transforms = Vec::new();
    let mut transform_index = 0u32;
    for member in body.children.iter().filter_map(as_node) {
        match member.kind {
            SyntaxKind::Field => {
                if let Some((path, value, pos)) =
                    read_field(member, source, &format!("bullets.{name}"))
                {
                    fields.insert(
                        path.clone(),
                        Located {
                            value,
                            position: pos,
                            origin_file: String::new(), // filled in by the caller once known
                            node_path: format!("bullets.{name}.{path}"),
                        },
                    );
                }
            }
            SyntaxKind::NestedMember if is_nested_keyword(member, "transform") => {
                let node_path = format!("bullets.{name}.transforms[{transform_index}]");
                if let Some(transform) = read_transform(member, source, &node_path) {
                    transforms.push(transform);
                }
                transform_index += 1;
            }
            _ => {}
        }
    }

    Some(BulletView {
        name,
        position,
        fields,
        transforms,
    })
}

fn read_emitter(node: &SyntaxNode, source: &str) -> Option<EmitterView> {
    let mut significant = significant_children(node).peekable();
    significant.next()?; // "emitter"
    let name_element = significant.next()?;
    let name = ident_text(name_element)?.to_string();
    let position = position_at(source, node.span.start);

    let mut from = None;
    if let Some(next) = significant.peek()
        && matches!(ident_text(next), Some("from"))
    {
        significant.next(); // "from"
        if let Some(SyntaxElement::Node(ref_node)) = significant.next()
            && ref_node.kind == SyntaxKind::RefValue
        {
            let segments: Vec<&str> = ref_node.children.iter().filter_map(ident_text).collect();
            if segments.len() == 2 {
                let pos = position_at(source, ref_node.span.start);
                from = Some((segments[0].to_string(), segments[1].to_string(), pos));
            }
        }
    }

    let body = node.child_node(SyntaxKind::Body)?;
    let mut fields = FieldMap::new();
    let mut block = None;
    let mut modifiers = Vec::new();
    let mut modifier_index = 0u32;
    for member in body.children.iter().filter_map(as_node) {
        match member.kind {
            SyntaxKind::Field => {
                if let Some((path, value, pos)) =
                    read_field(member, source, &format!("emitters.{name}"))
                {
                    fields.insert(
                        path.clone(),
                        Located {
                            value,
                            position: pos,
                            origin_file: String::new(),
                            node_path: format!("emitters.{name}.{path}"),
                        },
                    );
                }
            }
            SyntaxKind::NestedMember => {
                if is_nested_keyword(member, "block") {
                    let node_path = format!("emitters.{name}.block");
                    block = read_block(member, source, &node_path);
                } else if is_nested_keyword(member, "modifier") {
                    let node_path = format!("emitters.{name}.modifiers[{modifier_index}]");
                    if let Some(modifier) = read_modifier(member, source, &node_path) {
                        modifiers.push(modifier);
                    }
                    modifier_index += 1;
                }
            }
            _ => {}
        }
    }

    Some(EmitterView {
        name,
        position,
        from,
        fields,
        block,
        modifiers,
    })
}

fn as_node(element: &SyntaxElement) -> Option<&SyntaxNode> {
    match element {
        SyntaxElement::Node(node) => Some(node),
        SyntaxElement::Token(_) => None,
    }
}

fn is_nested_keyword(nested_member: &SyntaxNode, keyword: &str) -> bool {
    significant_children(nested_member)
        .next()
        .and_then(ident_text)
        == Some(keyword)
}

fn nested_kind_and_position(
    nested_member: &SyntaxNode,
    source: &str,
) -> Option<(String, Position)> {
    let mut significant = significant_children(nested_member);
    significant.next()?; // keyword
    let kind_element = significant.next()?;
    let kind = ident_text(kind_element)?.to_string();
    let position = position_at(source, kind_element.span().start);
    Some((kind, position))
}

fn read_block(node: &SyntaxNode, source: &str, node_path: &str) -> Option<BlockView> {
    let (kind, kind_position) = nested_kind_and_position(node, source)?;
    let body = node.child_node(SyntaxKind::Body)?;
    let fields = read_body_fields(body, source, node_path);
    Some(BlockView {
        kind,
        kind_position,
        fields,
    })
}

fn read_modifier(node: &SyntaxNode, source: &str, node_path: &str) -> Option<ModifierView> {
    let (kind, kind_position) = nested_kind_and_position(node, source)?;
    let body = node.child_node(SyntaxKind::Body)?;
    let fields = read_body_fields(body, source, node_path);
    Some(ModifierView {
        kind,
        kind_position,
        fields,
    })
}

fn read_transform(node: &SyntaxNode, source: &str, node_path: &str) -> Option<TransformView> {
    let (kind, kind_position) = nested_kind_and_position(node, source)?;
    let body = node.child_node(SyntaxKind::Body)?;
    let mut fields = FieldMap::new();
    let mut block = None;
    for member in body.children.iter().filter_map(as_node) {
        match member.kind {
            SyntaxKind::Field => {
                if let Some((path, value, pos)) = read_field(member, source, node_path) {
                    fields.insert(
                        path.clone(),
                        Located {
                            value,
                            position: pos,
                            origin_file: String::new(),
                            node_path: format!("{node_path}.{path}"),
                        },
                    );
                }
            }
            SyntaxKind::NestedMember if is_nested_keyword(member, "block") => {
                block = read_block(member, source, &format!("{node_path}.block"));
            }
            _ => {}
        }
    }
    Some(TransformView {
        kind,
        kind_position,
        fields,
        block,
    })
}

/// Reads every `Field` direct child of `body` into a [`FieldMap`], keyed by the field's own
/// (possibly dotted) path text.
fn read_body_fields(body: &SyntaxNode, source: &str, node_path: &str) -> FieldMap {
    let mut fields = FieldMap::new();
    for member in body.children.iter().filter_map(as_node) {
        if member.kind == SyntaxKind::Field
            && let Some((path, value, pos)) = read_field(member, source, node_path)
        {
            fields.insert(
                path.clone(),
                Located {
                    value,
                    position: pos,
                    origin_file: String::new(),
                    node_path: format!("{node_path}.{path}"),
                },
            );
        }
    }
    fields
}

/// Reads one `Field` node into `(dotted_path_text, value, value_position)`.
fn read_field(
    field: &SyntaxNode,
    source: &str,
    _context_path: &str,
) -> Option<(String, FieldValue, Position)> {
    let path_node = field.child_node(SyntaxKind::Path)?;
    let path_text: String = path_node
        .tokens()
        .filter(|token| !token.kind.is_trivia())
        .map(|token| token.text.as_str())
        .collect();

    // The value is the first significant element after the `Eq` token.
    let eq_index = field.children.iter().position(
        |element| matches!(element, SyntaxElement::Token(t) if t.kind == SyntaxKind::Eq),
    )?;
    let value_element = first_significant(&field.children, eq_index + 1)?;
    let position = position_at(source, value_element.span().start);
    let value = read_value(value_element);
    Some((path_text, value, position))
}

/// Reads one value element (the shape after a field's `=`, or a list/record entry).
fn read_value(element: &SyntaxElement) -> FieldValue {
    match element {
        SyntaxElement::Token(token) => match token.kind {
            SyntaxKind::Quantity => {
                let (number, unit) = crate::lexer::split_quantity(&token.text);
                match number.parse::<f64>() {
                    Ok(value) => FieldValue::Quantity(value, unit.to_string()),
                    Err(_) => FieldValue::Invalid,
                }
            }
            SyntaxKind::FloatLit => token
                .text
                .parse::<f64>()
                .map_or(FieldValue::Invalid, FieldValue::Float),
            SyntaxKind::IntLit => token
                .text
                .parse::<i64>()
                .map_or(FieldValue::Invalid, FieldValue::Int),
            SyntaxKind::StringLit => FieldValue::Str(unquote(&token.text)),
            _ => FieldValue::Invalid,
        },
        SyntaxElement::Node(node) => match node.kind {
            SyntaxKind::RefValue => {
                let segments: Vec<String> = node
                    .children
                    .iter()
                    .filter_map(ident_text)
                    .map(str::to_string)
                    .collect();
                if segments.len() == 1 {
                    FieldValue::Ident(segments.into_iter().next().unwrap_or_default())
                } else if segments.is_empty() {
                    FieldValue::Invalid
                } else {
                    FieldValue::Ref(segments)
                }
            }
            SyntaxKind::ListValue => {
                let values = node
                    .children
                    .iter()
                    .filter(|e| {
                        !matches!(e, SyntaxElement::Token(t) if t.kind.is_trivia()
                            || matches!(t.kind, SyntaxKind::LBracket | SyntaxKind::RBracket | SyntaxKind::Comma))
                    })
                    .map(read_value)
                    .collect();
                FieldValue::List(values)
            }
            SyntaxKind::RecordValue => {
                let fields = node
                    .children
                    .iter()
                    .filter_map(as_node)
                    .filter(|n| n.kind == SyntaxKind::RecordField)
                    .filter_map(read_record_field)
                    .collect();
                FieldValue::Record(fields)
            }
            SyntaxKind::TriggerValue => {
                let mut significant = significant_children(node);
                let Some(keyword) = significant.next().and_then(ident_text) else {
                    return FieldValue::Invalid;
                };
                let Some(argument) = significant.next() else {
                    return FieldValue::Invalid;
                };
                FieldValue::Trigger(keyword.to_string(), Box::new(read_value(argument)))
            }
            _ => FieldValue::Invalid,
        },
    }
}

fn read_record_field(node: &SyntaxNode) -> Option<(String, FieldValue)> {
    let mut significant = significant_children(node);
    let name = ident_text(significant.next()?)?.to_string();
    let eq_index = node.children.iter().position(
        |element| matches!(element, SyntaxElement::Token(t) if t.kind == SyntaxKind::Eq),
    )?;
    let value_element = first_significant(&node.children, eq_index + 1)?;
    Some((name, read_value(value_element)))
}
