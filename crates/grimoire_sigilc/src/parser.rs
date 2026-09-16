//! Recursive-descent parser for Sigil source text (Plan 0002 WP4.1, engine ADR-0007).
//!
//! [`parse`] never panics and never fails outright: it always returns a full [`SyntaxNode`]
//! (lossless, per `syntax.rs`) together with every [`Diagnostic`] collected along the way
//! (contract §2 rule 9's "never panic" carried over from decoders of foreign bytes to this
//! resilient parser: bad input becomes diagnostics on a side channel, not a fatal `Result`, so
//! more than one problem in a file can be reported at once — WP4.1 requirement 3). Two guards
//! make the "never panics, never hangs" half of that provable by inspection rather than by luck:
//!
//! - **Bounded recursion:** a fixed maximum nesting depth caps how deep bodies, lists and records may
//!   nest before parsing bails out of that subtree with one [`Diagnostic`] instead of recursing
//!   further (`SIG0010`), so adversarial input (e.g. thousands of nested `[`) cannot overflow the
//!   native stack.
//! - **Guaranteed progress:** every loop that repeatedly parses "the next thing" (an item, a body
//!   member, a list element, a record field) checks that the token cursor actually advanced and
//!   forces one token of progress if a sub-parse consumed nothing, so a construct the grammar
//!   does not recognise can only ever produce a diagnostic, never an infinite loop.
//!
//! The parser also builds each diagnostic's **node path** as it descends (`version`, `meta`,
//! `bullets.<name>`, `emitters.<name>.modifiers[<i>].<field>`, ...), following the convention
//! documented in `docs/formats/sigil.md`. This needs no schema or name resolution (out of scope
//! for WP4.1): an item's own keyword and name, and a purely positional count of how many
//! `modifier`/`transform` members a body has seen so far, are enough to reproduce the path table
//! exactly.

use crate::diagnostics::{Diagnostic, RelatedLocation};
use crate::lexer::{self, KNOWN_UNITS, RawToken};
use crate::span::Position;
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};

/// The only Sigil source-syntax version this compiler reads (engine ADR-0007). A file naming any
/// other version raises `SIG0002` and is not parsed further.
pub const SUPPORTED_SIGIL_VERSION: &str = "1";

/// Maximum nesting depth for bodies, lists and records combined, before the parser bails out of
/// that subtree with `SIG0010` instead of recursing further (WP4.1 requirement 6). Generous for
/// any real pattern (the ADR's own cascade-depth limit for emitters is 3) and small enough that a
/// few hundred stack frames, worst case, never come close to overflowing a native stack.
const MAX_NESTING_DEPTH: u32 = 64;

/// The result of parsing one Sigil source file: a lossless tree plus every diagnostic collected.
/// An empty `diagnostics` means the file compiles as far as WP4.1's lexer and parser are
/// concerned (the schema pass, Plan 0002 WP4.2, may still reject it).
#[derive(Debug)]
pub struct ParseOutput {
    /// The root [`SyntaxKind::SourceFile`] node.
    pub tree: SyntaxNode,
    /// Every diagnostic raised while parsing, in the order they were found.
    pub diagnostics: Vec<Diagnostic>,
}

/// Parses `source` as one Sigil file. `file` is used only to label diagnostics (see
/// [`Diagnostic::file`]); it is not read from disk here.
#[must_use]
pub fn parse(file: impl Into<String>, source: &str) -> ParseOutput {
    let tokens = lexer::lex(source);
    let mut parser = Parser {
        tokens,
        pos: 0,
        file: file.into(),
        diagnostics: Vec::new(),
        path_stack: Vec::new(),
    };
    let mut children = Vec::new();
    let header_ok = parser.parse_header(&mut children);
    if header_ok {
        parser.parse_items(&mut children);
    }
    parser.bump_into(&mut children); // the final Eof token, plus any trailing trivia.
    ParseOutput {
        tree: SyntaxNode::from_children(SyntaxKind::SourceFile, children),
        diagnostics: parser.diagnostics,
    }
}

fn is_item_keyword(text: &str) -> bool {
    matches!(text, "import" | "meta" | "bullet" | "emitter")
}

struct Parser {
    tokens: Vec<RawToken>,
    pos: usize,
    file: String,
    diagnostics: Vec<Diagnostic>,
    /// Dotted/indexed path segments of whatever item/nested-member/field the parser currently sits
    /// inside, joined with `.` to build a diagnostic's `node_path` (`docs/formats/sigil.md`).
    path_stack: Vec<String>,
}

impl Parser {
    // ---- Low-level token cursor -----------------------------------------------------------

    /// The kind at raw token index `idx` (trivia included), or [`SyntaxKind::Eof`] past the end.
    fn raw_kind(&self, idx: usize) -> SyntaxKind {
        self.tokens.get(idx).map_or(SyntaxKind::Eof, |t| t.kind)
    }

    /// Index of the `n`-th (0-based) non-trivia token at or after the cursor.
    fn significant_index(&self, n: usize) -> usize {
        let mut idx = self.pos;
        let mut seen = 0usize;
        loop {
            if idx >= self.tokens.len() {
                // The token stream always ends in one `Eof` token, which is never trivia, so in
                // practice this arm is only reached if `n` asks further ahead than the stream is
                // long; the last token (`Eof`) is the only sound answer for "what's out there".
                return self.tokens.len().saturating_sub(1);
            }
            if !self.tokens[idx].kind.is_trivia() {
                if seen == n {
                    return idx;
                }
                seen += 1;
            }
            idx += 1;
        }
    }

    fn peek_kind(&self, n: usize) -> SyntaxKind {
        self.tokens
            .get(self.significant_index(n))
            .map_or(SyntaxKind::Eof, |t| t.kind)
    }

    fn peek_text(&self, n: usize) -> &str {
        self.tokens
            .get(self.significant_index(n))
            .map_or("", |t| t.text.as_str())
    }

    fn peek_pos(&self, n: usize) -> Position {
        self.tokens
            .get(self.significant_index(n))
            .map_or_else(Position::start_of_file, |t| t.start)
    }

    /// Consumes and returns the raw token at the cursor (trivia or not), advancing the cursor.
    /// Past the end of the stream this keeps returning a synthetic `Eof` rather than panicking,
    /// so a parser bug that calls one token too many degrades to "stuck at Eof", never a crash.
    fn bump_raw(&mut self) -> RawToken {
        match self.tokens.get(self.pos) {
            Some(token) => {
                let token = token.clone();
                self.pos += 1;
                token
            }
            None => self.tokens.last().cloned().unwrap_or(RawToken {
                kind: SyntaxKind::Eof,
                text: String::new(),
                span: crate::span::Span::default(),
                start: Position::start_of_file(),
            }),
        }
    }

    /// Drains any leading trivia into `out`, then consumes and returns the next significant
    /// token, also pushed into `out`. This is how every parse function attaches a token's
    /// leading whitespace/comments to whichever node it is currently building (`syntax.rs`'s
    /// module docs: trivia attaches to the following token's node).
    fn bump_into(&mut self, out: &mut Vec<SyntaxElement>) -> SyntaxToken {
        while self.raw_kind(self.pos).is_trivia() {
            let raw = self.bump_raw();
            out.push(SyntaxElement::Token(to_syntax_token(raw)));
        }
        let raw = self.bump_raw();
        let token = to_syntax_token(raw);
        out.push(SyntaxElement::Token(token.clone()));
        token
    }

    /// Raises `SIG0011` for a list or record that never reaches its closing delimiter, whether
    /// that is because the file ran out (`Eof`), or because a `}` or the next item keyword
    /// showed up first — both of which mean *something else* is desynchronised, not this list or
    /// record specifically. Never consumes the token that triggered it: a `}` or item keyword is
    /// left for whatever is looking for it one level up ([`Parser::parse_body`]'s own member
    /// loop, or [`Parser::parse_items`]), so one missing `]`/`)` produces exactly this one
    /// diagnostic instead of a cascade (the enclosing body still finds its `}` right where it
    /// expects it, rather than that `}` having been swallowed as "unexpected" list content).
    fn report_unclosed(
        &mut self,
        open_pos: Position,
        opener_token: &str,
        opener_label: &str,
        closer: &str,
    ) {
        let pos = self.peek_pos(0);
        let found = self.peek_text(0).to_string();
        let message = match self.peek_kind(0) {
            SyntaxKind::Eof => {
                format!(
                    "Unexpected end of file: this {opener_label} is never closed with `{closer}`."
                )
            }
            SyntaxKind::RBrace => format!(
                "`{found}` closes an enclosing body before this {opener_label} is closed with `{closer}`."
            ),
            _ => format!(
                "`{found}` starts a new item before this {opener_label} is closed with `{closer}`."
            ),
        };
        self.error_related(
            "SIG0011",
            pos,
            found,
            message,
            format!("Add `{closer}`."),
            RelatedLocation::new(format!("opening {opener_label}"), open_pos, opener_token),
        );
    }

    /// Consumes tokens until the bracket/brace nesting implied by `opens`/`closes` returns to
    /// `depth <= 0` (starting from `depth`), or EOF. Used only to bail out of a subtree the
    /// nesting guard already refused to parse further into; it does not attempt to build any
    /// structure, only to keep the tree lossless around the bail-out.
    fn skip_balanced(
        &mut self,
        out: &mut Vec<SyntaxElement>,
        opens: &[SyntaxKind],
        closes: &[SyntaxKind],
        mut depth: i32,
    ) {
        loop {
            match self.peek_kind(0) {
                SyntaxKind::Eof => break,
                kind if opens.contains(&kind) => {
                    depth += 1;
                    self.bump_into(out);
                }
                kind if closes.contains(&kind) => {
                    depth -= 1;
                    self.bump_into(out);
                    if depth <= 0 {
                        break;
                    }
                }
                _ => {
                    self.bump_into(out);
                }
            }
        }
    }

    // ---- Diagnostics and node-path bookkeeping ---------------------------------------------

    fn push_path(&mut self, segment: impl Into<String>) {
        self.path_stack.push(segment.into());
    }

    fn pop_path(&mut self) {
        self.path_stack.pop();
    }

    fn current_path(&self) -> String {
        self.path_stack.join(".")
    }

    /// Whether the token at the cursor actually starts a new item, as opposed to merely being
    /// one of the four item keywords used as an ordinary field name (`bullet = orb` is a
    /// perfectly valid field inside an emitter body, per the grammar's contextual-keyword rule —
    /// `docs/formats/sigil.md`, `syntax.rs`'s module docs). [`is_item_keyword`] alone is only
    /// safe to use where no field context exists at all (directly inside [`Parser::parse_items`]
    /// and [`Parser::recover_unexpected_item`]'s own recovery loop, both strictly top-level);
    /// everywhere a body or a value could also be in scope — [`Parser::parse_body`]'s
    /// resynchronisation check, and the same check inside [`Parser::parse_list`] and
    /// [`Parser::parse_record`] — this lookahead is what tells "the next real item begins here"
    /// apart from "this keyword is just this field's/value's name".
    fn looks_like_new_item(&self) -> bool {
        if self.peek_kind(0) != SyntaxKind::Ident {
            return false;
        }
        match self.peek_text(0) {
            "import" => self.peek_kind(1) == SyntaxKind::StringLit,
            "meta" => self.peek_kind(1) == SyntaxKind::LBrace,
            "bullet" | "emitter" => self.peek_kind(1) == SyntaxKind::Ident,
            _ => false,
        }
    }

    fn error(
        &mut self,
        code: &'static str,
        position: Position,
        token: impl Into<String>,
        message: impl Into<String>,
        fix_hint: impl Into<String>,
    ) {
        let path = self.current_path();
        let file = self.file.clone();
        self.diagnostics.push(Diagnostic::new(
            code, file, position, path, token, message, fix_hint,
        ));
    }

    #[allow(clippy::too_many_arguments)]
    fn error_related(
        &mut self,
        code: &'static str,
        position: Position,
        token: impl Into<String>,
        message: impl Into<String>,
        fix_hint: impl Into<String>,
        related: RelatedLocation,
    ) {
        let path = self.current_path();
        let file = self.file.clone();
        self.diagnostics.push(
            Diagnostic::new(code, file, position, path, token, message, fix_hint)
                .with_related(related),
        );
    }

    // ---- Header (engine ADR-0007: checked before anything else) ---------------------------

    /// Parses the mandatory `sigil <N>` header. Returns whether it was `sigil
    /// {SUPPORTED_SIGIL_VERSION}` exactly; on any other outcome exactly one diagnostic is raised
    /// (`SIG0001` for a missing/malformed header, `SIG0002` for an unsupported version) and the
    /// rest of the file is wrapped, unparsed, into one [`SyntaxKind::ErrorNode`] so the tree
    /// stays lossless without the parser guessing at structure it has no version contract for.
    fn parse_header(&mut self, out: &mut Vec<SyntaxElement>) -> bool {
        let mut children = Vec::new();
        let has_bom = self.raw_kind(self.pos) == SyntaxKind::Bom;
        if has_bom {
            self.bump_into(&mut children);
        }
        let starts_with_sigil = !has_bom
            && self.raw_kind(self.pos) == SyntaxKind::Ident
            && self.tokens.get(self.pos).is_some_and(|t| t.text == "sigil");
        if !starts_with_sigil {
            let found = self.peek_text(0).to_string();
            self.push_path("version");
            self.error(
                "SIG0001",
                Position::start_of_file(),
                found,
                "Missing or malformed `sigil <N>` version header: it must be the first line of \
                 the file, in column 1, with no byte-order mark or comment before it."
                    .to_string(),
                format!(
                    "Add `sigil {SUPPORTED_SIGIL_VERSION}` as the very first line of the file."
                ),
            );
            self.pop_path();
            while self.raw_kind(self.pos) != SyntaxKind::Eof {
                self.bump_into(&mut children);
            }
            out.push(SyntaxElement::Node(SyntaxNode::from_children(
                SyntaxKind::ErrorNode,
                children,
            )));
            return false;
        }

        self.push_path("version");
        self.bump_into(&mut children); // "sigil"
        let version_pos = self.peek_pos(0);
        let version_kind = self.peek_kind(0);
        let version_text = self.peek_text(0).to_string();
        let supported =
            version_kind == SyntaxKind::IntLit && version_text == SUPPORTED_SIGIL_VERSION;
        if !supported {
            let message = if version_kind == SyntaxKind::IntLit {
                format!(
                    "Unsupported Sigil version {version_text}; this compiler reads `sigil {SUPPORTED_SIGIL_VERSION}`."
                )
            } else {
                "Expected a positive integer version number after `sigil`.".to_string()
            };
            self.error(
                "SIG0002",
                version_pos,
                version_text,
                message,
                format!(
                    "Change the header to `sigil {SUPPORTED_SIGIL_VERSION}`, or migrate the file with a newer `sigilc`."
                ),
            );
            self.pop_path();
            while self.raw_kind(self.pos) != SyntaxKind::Eof {
                self.bump_into(&mut children);
            }
            out.push(SyntaxElement::Node(SyntaxNode::from_children(
                SyntaxKind::ErrorNode,
                children,
            )));
            return false;
        }
        self.bump_into(&mut children); // the version number itself

        if !matches!(self.peek_kind(0), SyntaxKind::Newline | SyntaxKind::Eof) {
            let pos = self.peek_pos(0);
            let found = self.peek_text(0).to_string();
            self.error(
                "SIG0001",
                pos,
                found,
                "Unexpected content after the version number on the header line.".to_string(),
                format!(
                    "Keep `sigil {SUPPORTED_SIGIL_VERSION}` alone on the first line; move anything else to its own line below."
                ),
            );
            while !matches!(self.peek_kind(0), SyntaxKind::Newline | SyntaxKind::Eof) {
                self.bump_into(&mut children);
            }
        }
        self.pop_path();
        if self.peek_kind(0) == SyntaxKind::Newline {
            self.bump_into(&mut children);
        }
        out.push(SyntaxElement::Node(SyntaxNode::from_children(
            SyntaxKind::Header,
            children,
        )));
        true
    }

    // ---- Items -------------------------------------------------------------------------------

    fn parse_items(&mut self, out: &mut Vec<SyntaxElement>) {
        loop {
            while self.peek_kind(0) == SyntaxKind::Newline {
                self.bump_into(out);
            }
            if self.peek_kind(0) == SyntaxKind::Eof {
                break;
            }
            let text = self.peek_text(0).to_string();
            if self.peek_kind(0) == SyntaxKind::Ident && is_item_keyword(&text) {
                match text.as_str() {
                    "import" => self.parse_import_item(out),
                    "meta" => self.parse_meta_item(out),
                    "bullet" => self.parse_bullet_item(out),
                    "emitter" => self.parse_emitter_item(out),
                    // `is_item_keyword` only accepts these four spellings; kept as a safe
                    // fallback (never a panic) rather than assuming that stays true forever.
                    _ => self.recover_unexpected_item(out),
                }
            } else {
                self.recover_unexpected_item(out);
            }
        }
    }

    /// Consumes tokens that could not start an item, raising `SIG0008`, up to (not including) the
    /// next item keyword or EOF. Always consumes at least one token, so a stray token at item
    /// position can never stall [`Parser::parse_items`]'s loop.
    fn recover_unexpected_item(&mut self, out: &mut Vec<SyntaxElement>) {
        let pos = self.peek_pos(0);
        let found = self.peek_text(0).to_string();
        let message = if found.is_empty() {
            "Expected an item: `import`, `meta`, `bullet` or `emitter`.".to_string()
        } else {
            format!(
                "Expected an item keyword (`import`, `meta`, `bullet` or `emitter`), found `{found}`."
            )
        };
        self.error(
            "SIG0008",
            pos,
            found,
            message,
            "Start a new item with `import`, `meta`, `bullet` or `emitter`, or remove this.",
        );
        let mut children = Vec::new();
        self.bump_into(&mut children);
        loop {
            match self.peek_kind(0) {
                SyntaxKind::Eof => break,
                SyntaxKind::Ident if is_item_keyword(self.peek_text(0)) => break,
                _ => {
                    self.bump_into(&mut children);
                }
            }
        }
        out.push(SyntaxElement::Node(SyntaxNode::from_children(
            SyntaxKind::ErrorNode,
            children,
        )));
    }

    /// Consumes the `NEWLINE {NEWLINE} | EOF` that ends an item. Lenient about anything else
    /// trailing on the line (raises `SIG0008` but does not force extra recovery): the outer item
    /// loop already re-synchronises on the next item keyword either way.
    fn consume_item_end(&mut self, out: &mut Vec<SyntaxElement>) {
        if self.peek_kind(0) == SyntaxKind::Eof {
            return;
        }
        if self.peek_kind(0) == SyntaxKind::Newline {
            while self.peek_kind(0) == SyntaxKind::Newline {
                self.bump_into(out);
            }
            return;
        }
        if self.peek_kind(0) == SyntaxKind::Ident && is_item_keyword(self.peek_text(0)) {
            // Reaching the next item keyword with no separating newline is, in every case this
            // parser produces, already the tail of some other diagnostic's recovery (typically
            // an unclosed body, `SIG0007`, resynchronising here on purpose). Raising a second
            // "expected a newline" diagnostic on top would just restate the same desync from a
            // different angle instead of pointing at anything new, so this one spot stays silent
            // about a missing separator between two items placed on the same line.
            return;
        }
        let pos = self.peek_pos(0);
        let found = self.peek_text(0).to_string();
        self.error(
            "SIG0008",
            pos,
            found.clone(),
            format!("Expected a newline after this item, found `{found}`."),
            "Put the next item on its own line.",
        );
    }

    fn parse_import_item(&mut self, out: &mut Vec<SyntaxElement>) {
        let mut children = Vec::new();
        self.bump_into(&mut children); // "import"
        self.push_path("imports");
        if self.peek_kind(0) == SyntaxKind::StringLit {
            self.bump_into(&mut children);
        } else {
            let pos = self.peek_pos(0);
            let found = self.peek_text(0).to_string();
            self.error(
                "SIG0008",
                pos,
                found,
                "Expected a string literal path after `import`.",
                "Add a quoted path, e.g. `import \"01-ring-burst.sigil\" as ring`.",
            );
        }
        if self.peek_kind(0) == SyntaxKind::Ident && self.peek_text(0) == "as" {
            self.bump_into(&mut children);
        } else {
            let pos = self.peek_pos(0);
            let found = self.peek_text(0).to_string();
            self.error(
                "SIG0008",
                pos,
                found,
                "Expected `as` after the import path.",
                "Add `as <alias>`.",
            );
        }
        if self.peek_kind(0) == SyntaxKind::Ident {
            self.bump_into(&mut children);
        } else {
            let pos = self.peek_pos(0);
            let found = self.peek_text(0).to_string();
            self.error(
                "SIG0008",
                pos,
                found,
                "Expected an alias identifier after `as`.",
                "Add a name, e.g. `as ring`.",
            );
        }
        self.pop_path();
        self.consume_item_end(&mut children);
        out.push(SyntaxElement::Node(SyntaxNode::from_children(
            SyntaxKind::ImportItem,
            children,
        )));
    }

    fn parse_meta_item(&mut self, out: &mut Vec<SyntaxElement>) {
        let mut children = Vec::new();
        self.bump_into(&mut children); // "meta"
        self.push_path("meta");
        self.parse_body(&mut children, 0);
        self.pop_path();
        self.consume_item_end(&mut children);
        out.push(SyntaxElement::Node(SyntaxNode::from_children(
            SyntaxKind::MetaItem,
            children,
        )));
    }

    fn parse_bullet_item(&mut self, out: &mut Vec<SyntaxElement>) {
        let mut children = Vec::new();
        self.bump_into(&mut children); // "bullet"
        let name = self.parse_item_name(&mut children, "bullet");
        self.push_path(format!("bullets.{name}"));
        self.parse_body(&mut children, 0);
        self.pop_path();
        self.consume_item_end(&mut children);
        out.push(SyntaxElement::Node(SyntaxNode::from_children(
            SyntaxKind::BulletItem,
            children,
        )));
    }

    fn parse_emitter_item(&mut self, out: &mut Vec<SyntaxElement>) {
        let mut children = Vec::new();
        self.bump_into(&mut children); // "emitter"
        let name = self.parse_item_name(&mut children, "emitter");
        if self.peek_kind(0) == SyntaxKind::Ident && self.peek_text(0) == "from" {
            self.bump_into(&mut children); // "from"
            if self.peek_kind(0) == SyntaxKind::Ident {
                self.parse_ref(&mut children);
            } else {
                let pos = self.peek_pos(0);
                let found = self.peek_text(0).to_string();
                self.error(
                    "SIG0008",
                    pos,
                    found,
                    "Expected a reference after `from`.",
                    "Name the emitter to inherit from, e.g. `from ring.burst`.",
                );
            }
        }
        self.push_path(format!("emitters.{name}"));
        self.parse_body(&mut children, 0);
        self.pop_path();
        self.consume_item_end(&mut children);
        out.push(SyntaxElement::Node(SyntaxNode::from_children(
            SyntaxKind::EmitterItem,
            children,
        )));
    }

    /// Consumes an item's own name identifier (`bullet <name>`, `emitter <name>`), raising
    /// `SIG0008` and returning a placeholder name if it is missing. `keyword` names the item in
    /// the diagnostic's message.
    fn parse_item_name(&mut self, out: &mut Vec<SyntaxElement>, keyword: &str) -> String {
        if self.peek_kind(0) == SyntaxKind::Ident {
            let name = self.peek_text(0).to_string();
            self.bump_into(out);
            name
        } else {
            let pos = self.peek_pos(0);
            let found = self.peek_text(0).to_string();
            self.error(
                "SIG0008",
                pos,
                found,
                format!("Expected a name after `{keyword}`."),
                format!("Add a name, e.g. `{keyword} example {{ ... }}`."),
            );
            "?".to_string()
        }
    }

    // ---- Bodies and members --------------------------------------------------------------

    /// Parses a brace-delimited body. Always produces a [`SyntaxKind::Body`] node, even when `{`
    /// is missing entirely (an empty one, with the caller left to react to nothing having been
    /// consumed) or the nesting guard trips (an [`SyntaxKind::ErrorNode`] wrapping everything up
    /// to the matching `}`, pushed as if it were the body so callers don't need a special case).
    fn parse_body(&mut self, out: &mut Vec<SyntaxElement>, depth: u32) {
        if self.peek_kind(0) != SyntaxKind::LBrace {
            let pos = self.peek_pos(0);
            let found = self.peek_text(0).to_string();
            self.error(
                "SIG0008",
                pos,
                found,
                "Expected `{` to start the body.",
                "Add `{` here.",
            );
            out.push(SyntaxElement::Node(SyntaxNode::from_children(
                SyntaxKind::Body,
                Vec::new(),
            )));
            return;
        }

        let mut children = Vec::new();
        let open_pos = self.peek_pos(0);
        self.bump_into(&mut children); // "{"

        if depth >= MAX_NESTING_DEPTH {
            self.error(
                "SIG0010",
                open_pos,
                "{",
                "This body nests too deeply.",
                format!("Flatten the structure; Sigil bodies may nest at most {MAX_NESTING_DEPTH} levels deep."),
            );
            self.skip_balanced(
                &mut children,
                &[SyntaxKind::LBrace],
                &[SyntaxKind::RBrace],
                1,
            );
            out.push(SyntaxElement::Node(SyntaxNode::from_children(
                SyntaxKind::ErrorNode,
                children,
            )));
            return;
        }

        let mut modifier_count = 0u32;
        let mut transform_count = 0u32;
        loop {
            while self.peek_kind(0) == SyntaxKind::Newline {
                self.bump_into(&mut children);
            }
            match self.peek_kind(0) {
                SyntaxKind::RBrace => {
                    self.bump_into(&mut children);
                    break;
                }
                SyntaxKind::Eof => {
                    let pos = self.peek_pos(0);
                    self.error_related(
                        "SIG0007",
                        pos,
                        "",
                        "Unexpected end of file: this body is never closed with `}`.",
                        "Add a `}` to close the body.",
                        RelatedLocation::new("opening brace", open_pos, "{"),
                    );
                    break;
                }
                SyntaxKind::Ident if self.looks_like_new_item() => {
                    let pos = self.peek_pos(0);
                    let text = self.peek_text(0).to_string();
                    self.error_related(
                        "SIG0007",
                        pos,
                        text.clone(),
                        format!(
                            "`{text}` cannot start a member here; the `{{` opened above is not closed."
                        ),
                        "Insert `}` on its own line before this.",
                        RelatedLocation::new("opening brace", open_pos, "{"),
                    );
                    break;
                }
                _ => {
                    let before = self.pos;
                    self.parse_member(
                        &mut children,
                        depth,
                        &mut modifier_count,
                        &mut transform_count,
                    );
                    if self.pos == before {
                        let pos = self.peek_pos(0);
                        let text = self.peek_text(0).to_string();
                        self.error(
                            "SIG0008",
                            pos,
                            text,
                            "Unexpected token inside a body: expected a field, a nested \
                             `block`/`modifier`/`transform`, or `}`."
                                .to_string(),
                            "Remove this, or turn it into a field `name = value`.",
                        );
                        self.bump_into(&mut children);
                    } else if !matches!(
                        self.peek_kind(0),
                        SyntaxKind::Newline | SyntaxKind::RBrace | SyntaxKind::Eof
                    ) {
                        // The grammar ends every member with a newline (`body = "{", {NEWLINE},
                        // {member, NEWLINE, {NEWLINE}}, "}"`); a second member-shaped token
                        // right after this one, with no separating newline, is the "multiple
                        // fields on one line" mistake. Reported once here and left unconsumed:
                        // the next loop iteration parses it as its own member normally, so this
                        // one missing newline costs exactly one diagnostic, not a cascade.
                        let pos = self.peek_pos(0);
                        let found = self.peek_text(0).to_string();
                        self.error(
                            "SIG0008",
                            pos,
                            found.clone(),
                            format!("Expected a newline after this member, found `{found}`."),
                            "Put each field or nested member on its own line.",
                        );
                    }
                }
            }
        }
        out.push(SyntaxElement::Node(SyntaxNode::from_children(
            SyntaxKind::Body,
            children,
        )));
    }

    /// Dispatches one body member to [`Parser::parse_nested_member`] or [`Parser::parse_field`],
    /// per the two-token lookahead rule in `docs/formats/sigil.md` ("Member position"). Consumes
    /// nothing if the current token cannot start a member at all (not an identifier); the caller
    /// ([`Parser::parse_body`]) detects that from the cursor not having moved.
    fn parse_member(
        &mut self,
        out: &mut Vec<SyntaxElement>,
        depth: u32,
        modifier_count: &mut u32,
        transform_count: &mut u32,
    ) {
        if self.peek_kind(0) != SyntaxKind::Ident {
            return;
        }
        let text = self.peek_text(0).to_string();
        let starts_nested = matches!(text.as_str(), "block" | "modifier" | "transform")
            && self.peek_kind(1) == SyntaxKind::Ident
            && self.peek_kind(2) == SyntaxKind::LBrace;
        if starts_nested {
            self.parse_nested_member(out, &text, depth, modifier_count, transform_count);
        } else {
            self.parse_field(out, depth);
        }
    }

    /// Parses a `(block | modifier | transform) <name> { ... }` nested member. The node path
    /// segment it contributes follows the convention in `docs/formats/sigil.md`: `block` is
    /// unindexed (a body has at most one), `modifier`/`transform` are indexed by how many of that
    /// same keyword this body has already seen (`modifiers[<i>]`, `transforms[<i>]`).
    fn parse_nested_member(
        &mut self,
        out: &mut Vec<SyntaxElement>,
        keyword: &str,
        depth: u32,
        modifier_count: &mut u32,
        transform_count: &mut u32,
    ) {
        let mut children = Vec::new();
        self.bump_into(&mut children); // keyword
        self.bump_into(&mut children); // name (guaranteed present by the caller's lookahead)
        let segment = match keyword {
            "modifier" => {
                let segment = format!("modifiers[{modifier_count}]");
                *modifier_count += 1;
                segment
            }
            "transform" => {
                let segment = format!("transforms[{transform_count}]");
                *transform_count += 1;
                segment
            }
            _ => "block".to_string(),
        };
        self.push_path(segment);
        self.parse_body(&mut children, depth + 1);
        self.pop_path();
        out.push(SyntaxElement::Node(SyntaxNode::from_children(
            SyntaxKind::NestedMember,
            children,
        )));
    }

    /// Parses a `<path> = <value>` field.
    fn parse_field(&mut self, out: &mut Vec<SyntaxElement>, depth: u32) {
        let mut children = Vec::new();
        let segments = self.parse_path_into(&mut children);
        for segment in &segments {
            self.push_path(segment.clone());
        }
        if self.peek_kind(0) == SyntaxKind::Eq {
            self.bump_into(&mut children);
            self.parse_value(&mut children, depth + 1);
        } else {
            let pos = self.peek_pos(0);
            let found = self.peek_text(0).to_string();
            self.error(
                "SIG0008",
                pos,
                found.clone(),
                format!("Expected `=` after the field name, found `{found}`."),
                "Use `=` to assign the value.",
            );
            // Consume exactly the unexpected token, so the body's own member loop does not trip
            // over the very same token a second time (its own "unexpected token" fallback would
            // otherwise fire right after this one). If it looks like a stray separator typo
            // (`:`, the common RON/JSON habit) rather than the value itself, also try to parse
            // whatever follows as the field's value, so a one-character typo costs exactly one
            // diagnostic instead of losing the value along with it.
            if !matches!(
                self.peek_kind(0),
                SyntaxKind::Newline | SyntaxKind::Eof | SyntaxKind::RBrace
            ) {
                self.bump_into(&mut children);
                if found == ":"
                    && !matches!(
                        self.peek_kind(0),
                        SyntaxKind::Newline | SyntaxKind::Eof | SyntaxKind::RBrace
                    )
                {
                    self.parse_value(&mut children, depth + 1);
                }
            }
        }
        for _ in &segments {
            self.pop_path();
        }
        out.push(SyntaxElement::Node(SyntaxNode::from_children(
            SyntaxKind::Field,
            children,
        )));
    }

    /// Parses a field key: `ident, { ("." ident) | ("[" int_lit "]") }`. Returns the semantic
    /// path segments (e.g. `["block", "count"]` for `block.count`, `["flags[1]"]` for `flags[1]`)
    /// used to extend [`Parser::path_stack`] while parsing the field's value.
    fn parse_path_into(&mut self, out: &mut Vec<SyntaxElement>) -> Vec<String> {
        let mut path_children = Vec::new();
        let mut segments = Vec::new();
        let mut current = self.peek_text(0).to_string();
        self.bump_into(&mut path_children); // first ident (caller guarantees it is present)
        loop {
            match self.peek_kind(0) {
                SyntaxKind::Dot => {
                    segments.push(std::mem::take(&mut current));
                    let mut segment_children = Vec::new();
                    self.bump_into(&mut segment_children); // "."
                    if self.peek_kind(0) == SyntaxKind::Ident {
                        current = self.peek_text(0).to_string();
                        self.bump_into(&mut segment_children);
                    } else {
                        let pos = self.peek_pos(0);
                        let found = self.peek_text(0).to_string();
                        self.error(
                            "SIG0008",
                            pos,
                            found,
                            "Expected an identifier after `.` in a field path.",
                            "",
                        );
                    }
                    path_children.push(SyntaxElement::Node(SyntaxNode::from_children(
                        SyntaxKind::PathSegment,
                        segment_children,
                    )));
                }
                SyntaxKind::LBracket => {
                    let mut segment_children = Vec::new();
                    self.bump_into(&mut segment_children); // "["
                    if self.peek_kind(0) == SyntaxKind::IntLit {
                        let index = self.peek_text(0).to_string();
                        self.bump_into(&mut segment_children);
                        current.push('[');
                        current.push_str(&index);
                        current.push(']');
                    } else {
                        let pos = self.peek_pos(0);
                        let found = self.peek_text(0).to_string();
                        self.error(
                            "SIG0008",
                            pos,
                            found,
                            "Expected an integer index after `[` in a field path.",
                            "",
                        );
                    }
                    if self.peek_kind(0) == SyntaxKind::RBracket {
                        self.bump_into(&mut segment_children);
                    } else {
                        let pos = self.peek_pos(0);
                        let found = self.peek_text(0).to_string();
                        self.error(
                            "SIG0011",
                            pos,
                            found,
                            "Expected `]` to close the index.",
                            "Add `]`.",
                        );
                    }
                    path_children.push(SyntaxElement::Node(SyntaxNode::from_children(
                        SyntaxKind::PathSegment,
                        segment_children,
                    )));
                }
                _ => break,
            }
        }
        segments.push(current);
        out.push(SyntaxElement::Node(SyntaxNode::from_children(
            SyntaxKind::Path,
            path_children,
        )));
        segments
    }

    // ---- Values ------------------------------------------------------------------------------

    /// Parses one value: `quantity | float_lit | int_lit | string | trigger | ref | list |
    /// record`. Consumes nothing on a value that cannot start any of those (the caller decides
    /// how to recover); otherwise always makes progress.
    fn parse_value(&mut self, out: &mut Vec<SyntaxElement>, depth: u32) {
        match self.peek_kind(0) {
            SyntaxKind::Quantity => {
                let pos = self.peek_pos(0);
                let token = self.bump_into(out);
                let (_, unit) = lexer::split_quantity(&token.text);
                if !KNOWN_UNITS.contains(&unit) {
                    self.error(
                        "SIG0006",
                        pos,
                        token.text.clone(),
                        format!("Unknown unit `{unit}` on `{}`.", token.text),
                        format!("Use one of the known units: {}.", KNOWN_UNITS.join(", ")),
                    );
                }
            }
            SyntaxKind::FloatLit | SyntaxKind::IntLit => {
                self.bump_into(out);
            }
            SyntaxKind::StringLit => {
                let pos = self.peek_pos(0);
                let token = self.bump_into(out);
                match lexer::validate_string(&token.text) {
                    Some(lexer::StringProblem::Unterminated) => {
                        self.error(
                            "SIG0003",
                            pos,
                            token.text.clone(),
                            "Unterminated string literal: no closing `\"` before the end of the line.",
                            "Add a closing `\"`.",
                        );
                    }
                    Some(lexer::StringProblem::InvalidEscape(escaped)) => {
                        self.error(
                            "SIG0004",
                            pos,
                            token.text.clone(),
                            format!("Invalid escape sequence `\\{escaped}` in a string literal."),
                            "Valid escapes are `\\\"`, `\\\\`, `\\n` and `\\t`.",
                        );
                    }
                    None => {}
                }
            }
            SyntaxKind::LBracket => self.parse_list(out, depth),
            SyntaxKind::LParen => self.parse_record(out, depth),
            SyntaxKind::Ident => self.parse_ident_value(out, depth),
            _ => {
                let pos = self.peek_pos(0);
                let found = self.peek_text(0).to_string();
                self.error(
                    "SIG0008",
                    pos,
                    found.clone(),
                    format!("Expected a value, found `{found}`."),
                    "Provide a number, quantity, string, reference, list or record.",
                );
            }
        }
    }

    /// An identifier in value position is a `trigger` (`time`/`distance`/`event` followed by a
    /// quantity or another identifier) or, otherwise, a dotted `ref`.
    fn parse_ident_value(&mut self, out: &mut Vec<SyntaxElement>, depth: u32) {
        let text = self.peek_text(0).to_string();
        let is_trigger_keyword = matches!(text.as_str(), "time" | "distance" | "event");
        let looks_like_trigger_argument = matches!(
            self.peek_kind(1),
            SyntaxKind::Quantity | SyntaxKind::IntLit | SyntaxKind::FloatLit | SyntaxKind::Ident
        );
        if is_trigger_keyword && looks_like_trigger_argument {
            let mut children = Vec::new();
            self.bump_into(&mut children); // keyword
            match self.peek_kind(0) {
                SyntaxKind::Ident => self.parse_ref(&mut children),
                _ => self.parse_value(&mut children, depth + 1),
            }
            out.push(SyntaxElement::Node(SyntaxNode::from_children(
                SyntaxKind::TriggerValue,
                children,
            )));
        } else {
            self.parse_ref(out);
        }
    }

    /// Parses `ident { "." ident }` (caller guarantees the first identifier is present).
    fn parse_ref(&mut self, out: &mut Vec<SyntaxElement>) {
        let mut children = Vec::new();
        self.bump_into(&mut children);
        while self.peek_kind(0) == SyntaxKind::Dot && self.peek_kind(1) == SyntaxKind::Ident {
            self.bump_into(&mut children); // "."
            self.bump_into(&mut children); // ident
        }
        out.push(SyntaxElement::Node(SyntaxNode::from_children(
            SyntaxKind::RefValue,
            children,
        )));
    }

    fn parse_list(&mut self, out: &mut Vec<SyntaxElement>, depth: u32) {
        let mut children = Vec::new();
        let open_pos = self.peek_pos(0);
        self.bump_into(&mut children); // "["

        if depth >= MAX_NESTING_DEPTH {
            self.error(
                "SIG0010",
                open_pos,
                "[",
                "This value nests too deeply.",
                format!("Flatten the structure; Sigil values may nest at most {MAX_NESTING_DEPTH} levels deep."),
            );
            self.skip_balanced(
                &mut children,
                &[SyntaxKind::LBracket, SyntaxKind::LParen],
                &[SyntaxKind::RBracket, SyntaxKind::RParen],
                1,
            );
            out.push(SyntaxElement::Node(SyntaxNode::from_children(
                SyntaxKind::ErrorNode,
                children,
            )));
            return;
        }

        let mut expect_value = true;
        loop {
            match self.peek_kind(0) {
                SyntaxKind::RBracket => {
                    self.bump_into(&mut children);
                    break;
                }
                SyntaxKind::Eof | SyntaxKind::RBrace => {
                    self.report_unclosed(open_pos, "[", "list", "]");
                    break;
                }
                SyntaxKind::Ident if self.looks_like_new_item() => {
                    self.report_unclosed(open_pos, "[", "list", "]");
                    break;
                }
                SyntaxKind::Comma => {
                    let pos = self.peek_pos(0);
                    if expect_value {
                        self.error(
                            "SIG0009",
                            pos,
                            ",",
                            "Unexpected `,` in a list: expected a value or `]`.",
                            "Remove the extra comma; a single trailing comma is allowed.",
                        );
                    }
                    self.bump_into(&mut children);
                    expect_value = true;
                }
                _ if expect_value => {
                    let before = self.pos;
                    self.parse_value(&mut children, depth + 1);
                    if self.pos == before {
                        self.bump_into(&mut children);
                    }
                    expect_value = false;
                }
                _ => {
                    let pos = self.peek_pos(0);
                    let found = self.peek_text(0).to_string();
                    self.error(
                        "SIG0008",
                        pos,
                        found.clone(),
                        format!("Expected `,` or `]` in a list, found `{found}`."),
                        "Add a comma between values, or close the list with `]`.",
                    );
                    self.bump_into(&mut children);
                }
            }
        }
        out.push(SyntaxElement::Node(SyntaxNode::from_children(
            SyntaxKind::ListValue,
            children,
        )));
    }

    fn parse_record(&mut self, out: &mut Vec<SyntaxElement>, depth: u32) {
        let mut children = Vec::new();
        let open_pos = self.peek_pos(0);
        self.bump_into(&mut children); // "("

        if depth >= MAX_NESTING_DEPTH {
            self.error(
                "SIG0010",
                open_pos,
                "(",
                "This value nests too deeply.",
                format!("Flatten the structure; Sigil values may nest at most {MAX_NESTING_DEPTH} levels deep."),
            );
            self.skip_balanced(
                &mut children,
                &[SyntaxKind::LBracket, SyntaxKind::LParen],
                &[SyntaxKind::RBracket, SyntaxKind::RParen],
                1,
            );
            out.push(SyntaxElement::Node(SyntaxNode::from_children(
                SyntaxKind::ErrorNode,
                children,
            )));
            return;
        }

        let mut expect_field = true;
        loop {
            match self.peek_kind(0) {
                SyntaxKind::RParen => {
                    self.bump_into(&mut children);
                    break;
                }
                SyntaxKind::Eof | SyntaxKind::RBrace => {
                    self.report_unclosed(open_pos, "(", "record", ")");
                    break;
                }
                SyntaxKind::Ident if self.looks_like_new_item() => {
                    self.report_unclosed(open_pos, "(", "record", ")");
                    break;
                }
                SyntaxKind::Comma => {
                    let pos = self.peek_pos(0);
                    if expect_field {
                        self.error(
                            "SIG0009",
                            pos,
                            ",",
                            "Unexpected `,` in a record: expected a field or `)`.",
                            "Remove the extra comma; a single trailing comma is allowed.",
                        );
                    }
                    self.bump_into(&mut children);
                    expect_field = true;
                }
                SyntaxKind::Ident if expect_field => {
                    self.parse_record_field(&mut children, depth);
                    expect_field = false;
                }
                _ => {
                    let pos = self.peek_pos(0);
                    let found = self.peek_text(0).to_string();
                    let message = if expect_field {
                        format!("Expected a field `name = value` or `)`, found `{found}`.")
                    } else {
                        format!("Expected `,` or `)`, found `{found}`.")
                    };
                    self.error("SIG0008", pos, found, message, "");
                    self.bump_into(&mut children);
                }
            }
        }
        out.push(SyntaxElement::Node(SyntaxNode::from_children(
            SyntaxKind::RecordValue,
            children,
        )));
    }

    fn parse_record_field(&mut self, out: &mut Vec<SyntaxElement>, depth: u32) {
        let mut children = Vec::new();
        self.bump_into(&mut children); // name (caller guarantees an ident is present)
        if self.peek_kind(0) == SyntaxKind::Eq {
            self.bump_into(&mut children);
            self.parse_value(&mut children, depth + 1);
        } else {
            let pos = self.peek_pos(0);
            let found = self.peek_text(0).to_string();
            self.error(
                "SIG0008",
                pos,
                found,
                "Expected `=` after the record field name.",
                "Add `= <value>`.",
            );
        }
        out.push(SyntaxElement::Node(SyntaxNode::from_children(
            SyntaxKind::RecordField,
            children,
        )));
    }
}

fn to_syntax_token(raw: RawToken) -> SyntaxToken {
    SyntaxToken {
        kind: raw.kind,
        text: raw.text,
        span: raw.span,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(source: &str) -> Vec<&'static str> {
        parse("test.sigil", source)
            .diagnostics
            .iter()
            .map(|d| d.code)
            .collect()
    }

    #[test]
    fn valid_minimal_file_has_no_diagnostics() {
        let out = parse("test.sigil", "sigil 1\nmeta {\n  name = \"x\"\n}\n");
        assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
    }

    #[test]
    fn missing_header_is_sig0001() {
        assert_eq!(codes("meta {}\n"), vec!["SIG0001"]);
    }

    #[test]
    fn empty_file_is_sig0001() {
        assert_eq!(codes(""), vec!["SIG0001"]);
    }

    #[test]
    fn wrong_version_is_sig0002_and_only_one_diagnostic() {
        assert_eq!(codes("sigil 2\nmeta {}\n"), vec!["SIG0002"]);
    }

    #[test]
    fn unknown_unit_is_sig0006() {
        let out = parse("test.sigil", "sigil 1\nmeta {\n  x = 30s\n}\n");
        assert_eq!(out.diagnostics.len(), 1);
        assert_eq!(out.diagnostics[0].code, "SIG0006");
        assert_eq!(out.diagnostics[0].node_path, "meta.x");
    }

    #[test]
    fn unterminated_string_value_is_sig0003() {
        assert_eq!(
            codes("sigil 1\nmeta {\n  name = \"Ring\n}\n"),
            vec!["SIG0003"]
        );
    }

    #[test]
    fn invalid_escape_in_string_value_is_sig0004() {
        assert_eq!(
            codes("sigil 1\nmeta {\n  name = \"a\\qb\"\n}\n"),
            vec!["SIG0004"]
        );
    }

    #[test]
    fn unclosed_body_before_next_item_is_sig0007_with_related_span() {
        let out = parse(
            "test.sigil",
            "sigil 1\nbullet seed {\n  radius = 1u\n\nbullet other {\n}\n",
        );
        assert_eq!(out.diagnostics.len(), 1);
        assert_eq!(out.diagnostics[0].code, "SIG0007");
        assert!(out.diagnostics[0].related.is_some());
    }

    #[test]
    fn unclosed_body_at_eof_is_sig0007() {
        assert_eq!(codes("sigil 1\nmeta {\n  x = 1\n"), vec!["SIG0007"]);
    }

    #[test]
    fn doubled_comma_in_list_is_sig0009() {
        let out = parse("test.sigil", "sigil 1\nmeta {\n  x = [1, 2,, 3]\n}\n");
        assert_eq!(out.diagnostics.len(), 1);
        assert_eq!(out.diagnostics[0].code, "SIG0009");
    }

    #[test]
    fn trailing_comma_in_list_is_allowed() {
        assert!(codes("sigil 1\nmeta {\n  x = [1, 2, 3,]\n}\n").is_empty());
    }

    #[test]
    fn unclosed_list_is_sig0011() {
        assert_eq!(codes("sigil 1\nmeta {\n  x = [1, 2\n}\n"), vec!["SIG0011"]);
    }

    #[test]
    fn nested_member_paths_follow_the_convention() {
        let out = parse(
            "test.sigil",
            "sigil 1\nemitter burst {\n  block ring {\n    cuont = 1w\n  }\n  modifier a {\n    x = 1w\n  }\n  modifier b {\n    y = 1w\n  }\n}\n",
        );
        let paths: Vec<&str> = out
            .diagnostics
            .iter()
            .map(|d| d.node_path.as_str())
            .collect();
        assert_eq!(
            paths,
            vec![
                "emitters.burst.block.cuont",
                "emitters.burst.modifiers[0].x",
                "emitters.burst.modifiers[1].y",
            ]
        );
    }

    #[test]
    fn override_field_path_matches_nested_member_path() {
        let out = parse(
            "test.sigil",
            "sigil 1\nemitter finale from ring.burst {\n  block.count = 1w\n}\n",
        );
        assert_eq!(out.diagnostics.len(), 1);
        assert_eq!(out.diagnostics[0].node_path, "emitters.finale.block.count");
    }

    #[test]
    fn deep_list_nesting_is_bounded_not_a_stack_overflow() {
        let mut source = String::from("sigil 1\nmeta {\n  x = ");
        let depth = 5_000;
        source.push_str(&"[".repeat(depth));
        source.push_str(&"]".repeat(depth));
        source.push_str("\n}\n");
        let out = parse("test.sigil", &source); // must return, not overflow the stack.
        assert!(out.diagnostics.iter().any(|d| d.code == "SIG0010"));
    }

    #[test]
    fn parser_terminates_on_arbitrary_short_inputs() {
        for source in [
            "",
            "sigil",
            "sigil 1",
            "{{{{{{",
            "((((((",
            "[[[[[[",
            ",,,,,,",
            "=====",
            "sigil 1\n}",
            "sigil 1\nbullet",
        ] {
            let _ = parse("test.sigil", source);
        }
    }
}
