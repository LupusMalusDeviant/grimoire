//! The lossless concrete syntax tree ("green tree") for Sigil source files.
//!
//! Engine ADR-0007 requires a lossless syntax tree: comments, blank lines, indentation and
//! token order all survive a parse, so a formatter can reproduce the exact source bytes and a
//! later tool (`sigilc set`, Plan 0002 WP4.3) can rewrite a single value without disturbing
//! anything else. The tree achieves this the simple way: every byte of the source is lexed into
//! exactly one token (trivia included — whitespace, comments and the newlines that separate
//! items and members), and every token, in source order, ends up exactly once as a leaf of the
//! tree. [`crate::fmt::format`] reproduces the source by walking those leaves in order and
//! concatenating their text, so the roundtrip property (Plan 0002 WP4.1, gate) falls out of the
//! tree's construction rather than needing a separate reconstruction step.
//!
//! Trivia attaches to the node of the token that follows it (the same convention `rust-analyzer`
//! uses for its own lossless tree): the leading whitespace and comments before a token become
//! children of that token's own node, inserted just before it.

use crate::span::Span;

/// The syntax kind of a token or node in the Sigil concrete syntax tree.
///
/// Contextual keywords (`sigil`, `import`, `as`, `meta`, `bullet`, `emitter`, `from`, `block`,
/// `modifier`, `transform`, `time`, `distance`, `event`) are lexed as plain [`SyntaxKind::Ident`]
/// tokens, exactly as the grammar in `docs/formats/sigil.md` describes: they are keywords only in
/// specific grammar positions and ordinary identifiers everywhere else (for example `bullet =
/// orb` inside an emitter body). The parser tells them apart by inspecting the identifier text at
/// the positions the grammar defines.
///
/// `#[non_exhaustive]`: the grammar is versioned (`sigil 1`) and later versions, or WP4.3's
/// `sigilc set`/`fmt` tooling, may add syntax kinds. External code matches on this enum with a
/// wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SyntaxKind {
    // ---- Trivia (never significant to the grammar; always skippable while parsing) ----
    /// Spaces, tabs, and any newline that falls inside `(...)` or `[...]`, where the grammar
    /// demotes newlines to trivia so a list or record may span multiple lines.
    Whitespace,
    /// A `//` line comment, not including its terminating newline.
    Comment,

    // ---- Significant tokens ----
    /// An identifier, including every contextual keyword (see the type-level docs).
    Ident,
    /// An integer literal, e.g. `24` or `-3`.
    IntLit,
    /// A float literal, e.g. `0.6` or `-0.5`.
    FloatLit,
    /// A number immediately followed, with no space, by a unit suffix, e.g. `30t` or `0.09u/t`.
    Quantity,
    /// A double-quoted string literal, e.g. `"Ring Burst"`.
    StringLit,
    /// A newline token at bracket depth 0: significant, separating items and body members.
    Newline,
    /// `{`
    LBrace,
    /// `}`
    RBrace,
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `[`
    LBracket,
    /// `]`
    RBracket,
    /// `=`
    Eq,
    /// `,`
    Comma,
    /// `.`
    Dot,
    /// A byte-order mark at the very start of the file; the grammar forbids it before the
    /// header, but it is still kept as a token so the tree stays lossless.
    Bom,
    /// A single character (or malformed construct) the lexer could not classify as any other
    /// token kind. Still a normal leaf, so the tree stays lossless even for garbage input.
    Unknown,
    /// The single end-of-file marker, always the last token of a [`SyntaxKind::SourceFile`].
    Eof,

    // ---- Nodes ----
    /// The root node: the whole file.
    SourceFile,
    /// The mandatory `sigil <N>` header.
    Header,
    /// An `import "<path>" as <alias>` item.
    ImportItem,
    /// The `meta { ... }` item.
    MetaItem,
    /// A `bullet <name> { ... }` item.
    BulletItem,
    /// An `emitter <name> [from <ref>] { ... }` item.
    EmitterItem,
    /// A brace-delimited body of an item or a nested member.
    Body,
    /// A `<path> = <value>` field inside a body.
    Field,
    /// A dotted/indexed field key, e.g. `block.count` or `flags[1]`.
    Path,
    /// One `.ident` or `[index]` continuation of a [`SyntaxKind::Path`].
    PathSegment,
    /// A `(block | modifier | transform) <name> { ... }` nested member.
    NestedMember,
    /// A `[ ... ]` list value.
    ListValue,
    /// A `( ... )` record value.
    RecordValue,
    /// One `ident = value` field inside a [`SyntaxKind::RecordValue`].
    RecordField,
    /// A `(time | distance | event) (quantity | ref)` trigger value.
    TriggerValue,
    /// A dotted reference value, e.g. `enemy.crimson`.
    RefValue,
    /// A run of tokens the parser could not fit into any grammar production. Keeps the tree
    /// lossless around a parse error instead of dropping the offending tokens.
    ErrorNode,
}

impl SyntaxKind {
    /// Whether a token of this kind is trivia: never itself part of the grammar, always
    /// attached to the following token's node instead of being parsed.
    #[must_use]
    pub fn is_trivia(self) -> bool {
        matches!(self, SyntaxKind::Whitespace | SyntaxKind::Comment)
    }
}

/// A single leaf of the syntax tree: a token together with its exact source text and span.
///
/// `text` is stored verbatim (escapes in a string literal are not unescaped, a quantity's unit
/// suffix is not split off) precisely so the tree stays lossless: concatenating every token's
/// `text` in tree order reproduces the source byte-for-byte (see [`crate::fmt::format`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxToken {
    /// The token's kind.
    pub kind: SyntaxKind,
    /// The token's exact source text.
    pub text: String,
    /// The token's byte span in the source file.
    pub span: Span,
}

/// An interior node of the syntax tree: a kind, a span covering all of its children, and the
/// ordered list of child tokens and nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxNode {
    /// The node's kind.
    pub kind: SyntaxKind,
    /// The node's byte span; the join of every child's span.
    pub span: Span,
    /// The node's children, in source order. Includes trivia tokens.
    pub children: Vec<SyntaxElement>,
}

/// One child of a [`SyntaxNode`]: either a leaf token or another node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaxElement {
    /// A leaf token, trivia or significant.
    Token(SyntaxToken),
    /// An interior node.
    Node(SyntaxNode),
}

impl SyntaxElement {
    /// The span of this element, whether it is a token or a node.
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            SyntaxElement::Token(token) => token.span,
            SyntaxElement::Node(node) => node.span,
        }
    }

    /// The kind of this element, whether it is a token or a node.
    #[must_use]
    pub fn kind(&self) -> SyntaxKind {
        match self {
            SyntaxElement::Token(token) => token.kind,
            SyntaxElement::Node(node) => node.kind,
        }
    }
}

impl SyntaxNode {
    /// Builds a span-less-computed node from already-built children, joining their spans (or an
    /// empty span at offset 0 if `children` happens to be empty — this never occurs from the
    /// parser, which always feeds at least the tokens it consumed for a node, but a total
    /// function is cheaper to keep correct than a partial one, per contract §2 rule 6).
    #[must_use]
    pub fn from_children(kind: SyntaxKind, children: Vec<SyntaxElement>) -> Self {
        let span = children
            .iter()
            .map(SyntaxElement::span)
            .reduce(Span::join)
            .unwrap_or_default();
        Self {
            kind,
            span,
            children,
        }
    }

    /// Depth-first, left-to-right iterator over every token (trivia included) under this node.
    ///
    /// This is the basis of the lossless roundtrip: [`crate::fmt::format`] concatenates exactly
    /// this sequence's `text` fields.
    pub fn tokens(&self) -> impl Iterator<Item = &SyntaxToken> + '_ {
        SyntaxTokenIter {
            stack: vec![self.children.iter()],
        }
    }

    /// The concatenated text of every token under this node, i.e. this node's exact source
    /// slice reproduced from the tree rather than sliced from the original string.
    #[must_use]
    pub fn text(&self) -> String {
        self.tokens().map(|token| token.text.as_str()).collect()
    }

    /// The first direct child node of the given `kind`, if any. Does not recurse into
    /// grandchildren; callers that need to look further descend explicitly, since node paths
    /// (§ the ADR's node-path table) are built top-down while parsing, not by searching the
    /// finished tree.
    #[must_use]
    pub fn child_node(&self, kind: SyntaxKind) -> Option<&SyntaxNode> {
        self.children.iter().find_map(|child| match child {
            SyntaxElement::Node(node) if node.kind == kind => Some(node),
            _ => None,
        })
    }

    /// The first direct child token of the given `kind`, if any (skips other nodes and tokens).
    #[must_use]
    pub fn child_token(&self, kind: SyntaxKind) -> Option<&SyntaxToken> {
        self.children.iter().find_map(|child| match child {
            SyntaxElement::Token(token) if token.kind == kind => Some(token),
            _ => None,
        })
    }
}

/// Iterator over every token under a node, depth-first left-to-right. A small explicit stack of
/// slice iterators rather than recursion, so walking a pathologically deep tree (see the nesting
/// guard in `parser.rs`) never risks a native stack overflow of its own.
struct SyntaxTokenIter<'a> {
    stack: Vec<std::slice::Iter<'a, SyntaxElement>>,
}

impl<'a> Iterator for SyntaxTokenIter<'a> {
    type Item = &'a SyntaxToken;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let top = self.stack.last_mut()?;
            match top.next() {
                Some(SyntaxElement::Token(token)) => return Some(token),
                Some(SyntaxElement::Node(node)) => self.stack.push(node.children.iter()),
                None => {
                    self.stack.pop();
                }
            }
        }
    }
}
