//! sigil 1 front end: lexer, lossless concrete syntax tree, error-recovering parser, lowering
//! into the generic value tree (`tree::Node`), and lossless value editing (`set`).

use crate::diag::{self, Diag, Phase, Pos};
use crate::tree::{self, Field, Node, NodeKind, Span};

// ------------------------------------------------------------------------------------------
// Lexer
// ------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TK {
    Ident,
    Int,
    Float,
    Qty,
    Str,
    Punct,
    /// Significant newline (bracket depth 0).
    Newline,
    Space,
    /// Newline inside `(` or `[`.
    NlTrivia,
    Comment,
    Bad,
    Eof,
}

impl TK {
    pub fn is_trivia(self) -> bool {
        matches!(self, TK::Space | TK::NlTrivia | TK::Comment)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Token {
    pub kind: TK,
    pub start: usize,
    pub end: usize,
}

/// Longest first.
pub const UNITS: &[&str] = &["deg/t", "deg", "u/t2", "u/t", "u", "t", "beats"];

pub fn lex(src: &str) -> (Vec<Token>, Vec<Diag>) {
    let b = src.as_bytes();
    let mut i = 0;
    let mut depth = 0i32;
    let mut toks = Vec::new();
    let mut diags = Vec::new();
    let at = |off: usize| Some(diag::pos_of_offset(src, off));
    while i < b.len() {
        let c = b[i];
        let start = i;
        let kind = match c {
            b' ' | b'\t' => {
                while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
                    i += 1;
                }
                TK::Space
            }
            b'\r' if b.get(i + 1) == Some(&b'\n') => {
                i += 2;
                if depth > 0 { TK::NlTrivia } else { TK::Newline }
            }
            b'\n' => {
                i += 1;
                if depth > 0 { TK::NlTrivia } else { TK::Newline }
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' && b[i] != b'\r' {
                    i += 1;
                }
                TK::Comment
            }
            b'"' => {
                i += 1;
                let mut closed = false;
                while i < b.len() {
                    match b[i] {
                        b'\\' => i += 2,
                        b'"' => {
                            i += 1;
                            closed = true;
                            break;
                        }
                        b'\n' => break,
                        _ => i += 1,
                    }
                }
                i = i.min(b.len());
                if !closed {
                    diags.push(
                        Diag::new(Phase::Parse, "string", "Unterminated string literal.")
                            .at(at(start))
                            .hint("Close the string with `\"` on the same line."),
                    );
                }
                TK::Str
            }
            b'0'..=b'9' | b'-' if c != b'-' || b.get(i + 1).is_some_and(u8::is_ascii_digit) => {
                if c == b'-' {
                    i += 1;
                }
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                let mut kind = TK::Int;
                if i + 1 < b.len() && b[i] == b'.' && b[i + 1].is_ascii_digit() {
                    i += 1;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                    kind = TK::Float;
                }
                if i < b.len() && b[i].is_ascii_alphabetic() {
                    let rest = &src[i..];
                    let unit = UNITS.iter().find(|u| {
                        rest.starts_with(**u)
                            && !rest[u.len()..]
                                .bytes()
                                .next()
                                .is_some_and(|n| n.is_ascii_alphanumeric() || n == b'_')
                    });
                    match unit {
                        Some(u) => {
                            i += u.len();
                            kind = TK::Qty;
                        }
                        None => {
                            let us = i;
                            while i < b.len()
                                && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'/')
                            {
                                i += 1;
                            }
                            diags.push(
                                Diag::new(
                                    Phase::Parse,
                                    "unit",
                                    format!("Unknown unit `{}`.", &src[us..i]),
                                )
                                .at(at(us))
                                .hint("Units of sigil 1: t, deg, u, u/t, u/t2, deg/t (no wall-clock units such as ms or s)."),
                            );
                            kind = TK::Bad;
                        }
                    }
                }
                kind
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                TK::Ident
            }
            b'{' | b'}' | b'=' | b',' | b'.' => {
                i += 1;
                TK::Punct
            }
            b'(' | b'[' => {
                i += 1;
                depth += 1;
                TK::Punct
            }
            b')' | b']' => {
                i += 1;
                depth = (depth - 1).max(0);
                TK::Punct
            }
            _ => {
                let ch = src[i..].chars().next().unwrap_or('?');
                i += ch.len_utf8();
                diags.push(
                    Diag::new(
                        Phase::Parse,
                        "char",
                        format!("Unexpected character `{ch}`."),
                    )
                    .at(at(start))
                    .hint("Remove the character; strings are written in double quotes."),
                );
                TK::Bad
            }
        };
        toks.push(Token {
            kind,
            start,
            end: i,
        });
    }
    toks.push(Token {
        kind: TK::Eof,
        start: b.len(),
        end: b.len(),
    });
    (toks, diags)
}

// ------------------------------------------------------------------------------------------
// Concrete syntax tree
// ------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NK {
    File,
    Header,
    Import,
    Meta,
    Bullet,
    Emitter,
    Body,
    Field,
    Nested,
    Path,
    Scalar,
    Ref,
    Trigger,
    List,
    Record,
    RecordField,
    Error,
}

#[derive(Debug, Clone)]
pub enum Child {
    Tok(usize),
    Node(CstNode),
}

#[derive(Debug, Clone)]
pub struct CstNode {
    pub kind: NK,
    pub children: Vec<Child>,
}

#[derive(Debug, Clone)]
pub struct Cst {
    pub root: CstNode,
    pub tokens: Vec<Token>,
}

impl Cst {
    /// Concatenation of all tokens in tree order; equals the source iff the tree is lossless.
    pub fn text(&self, src: &str) -> String {
        let mut out = String::new();
        fn go(n: &CstNode, toks: &[Token], src: &str, out: &mut String) {
            for c in &n.children {
                match c {
                    Child::Tok(i) => out.push_str(&src[toks[*i].start..toks[*i].end]),
                    Child::Node(n) => go(n, toks, src, out),
                }
            }
        }
        go(&self.root, &self.tokens, src, &mut out);
        out
    }

    pub fn comment_count(&self) -> usize {
        self.tokens.iter().filter(|t| t.kind == TK::Comment).count()
    }

    /// Significant tokens (no trivia, no newlines, no EOF).
    pub fn significant_tokens(&self) -> usize {
        self.tokens
            .iter()
            .filter(|t| !t.kind.is_trivia() && !matches!(t.kind, TK::Newline | TK::Eof))
            .count()
    }
}

struct Parser<'s> {
    src: &'s str,
    toks: Vec<Token>,
    pos: usize,
    stack: Vec<CstNode>,
    diags: Vec<Diag>,
    recovered: bool,
    ctx: Vec<String>,
}

pub fn parse(src: &str) -> (Cst, Vec<Diag>) {
    let (toks, lex_diags) = lex(src);
    let mut p = Parser {
        src,
        toks,
        pos: 0,
        stack: Vec::new(),
        diags: lex_diags,
        recovered: false,
        ctx: Vec::new(),
    };
    p.start(NK::File);
    p.file();
    while p.pos < p.toks.len() {
        let j = p.pos;
        p.push_tok(j);
        p.pos += 1;
    }
    while p.stack.len() > 1 {
        p.finish();
    }
    let root = p.stack.pop().expect("file node");
    let mut diags = p.diags;
    diags.sort_by_key(|d| d.pos);
    (
        Cst {
            root,
            tokens: p.toks,
        },
        diags,
    )
}

impl<'s> Parser<'s> {
    fn nth(&self, n: usize) -> usize {
        let mut idx = self.pos;
        let mut left = n;
        loop {
            while idx < self.toks.len() - 1 && self.toks[idx].kind.is_trivia() {
                idx += 1;
            }
            if left == 0 || idx >= self.toks.len() - 1 {
                return idx;
            }
            idx += 1;
            left -= 1;
        }
    }
    fn kind(&self, n: usize) -> TK {
        self.toks[self.nth(n)].kind
    }
    fn text(&self, n: usize) -> &'s str {
        let t = self.toks[self.nth(n)];
        &self.src[t.start..t.end]
    }
    fn is_punct(&self, n: usize, p: &str) -> bool {
        self.kind(n) == TK::Punct && self.text(n) == p
    }
    fn tok_pos(&self, n: usize) -> Pos {
        diag::pos_of_offset(self.src, self.toks[self.nth(n)].start)
    }
    fn describe(&self, n: usize) -> String {
        match self.kind(n) {
            TK::Newline => "end of line".to_string(),
            TK::Eof => "end of file".to_string(),
            _ => format!("`{}`", self.text(n)),
        }
    }
    fn push_tok(&mut self, j: usize) {
        self.stack
            .last_mut()
            .expect("open node")
            .children
            .push(Child::Tok(j));
    }
    fn bump(&mut self) {
        let idx = self.nth(0);
        if self.toks[idx].kind == TK::Eof {
            return;
        }
        for j in self.pos..=idx {
            self.push_tok(j);
        }
        self.pos = idx + 1;
    }
    fn start(&mut self, kind: NK) {
        self.stack.push(CstNode {
            kind,
            children: Vec::new(),
        });
    }
    fn finish(&mut self) {
        let n = self.stack.pop().expect("node");
        match self.stack.last_mut() {
            Some(parent) => parent.children.push(Child::Node(n)),
            None => self.stack.push(n),
        }
    }
    fn path(&self) -> String {
        self.ctx.concat()
    }
    fn err(&mut self, kind: &'static str, n: usize, cause: String, hint: Option<&str>) -> usize {
        let path = self.path();
        let mut d = Diag::new(Phase::Parse, kind, cause).at(Some(self.tok_pos(n)));
        if !path.is_empty() {
            d = d.path(path);
        }
        if let Some(h) = hint {
            d = d.hint(h);
        }
        self.diags.push(d);
        self.diags.len() - 1
    }
    fn skip_line(&mut self) {
        self.start(NK::Error);
        while !matches!(self.kind(0), TK::Newline | TK::Eof) {
            self.bump();
        }
        self.finish();
    }

    fn file(&mut self) {
        self.start(NK::Header);
        let first = self.nth(0);
        if self.kind(0) == TK::Ident && self.text(0) == "sigil" && self.toks[first].start == 0 {
            self.bump();
            if self.kind(0) == TK::Int {
                let v = self.text(0).to_string();
                if v != "1" {
                    self.ctx.push("version".into());
                    self.err(
                        "version",
                        0,
                        format!("Unsupported Sigil version {v}; this compiler reads `sigil 1`."),
                        Some("Change the header to `sigil 1` or migrate the file with a newer `sigilc`."),
                    );
                    self.ctx.pop();
                    self.bump();
                    self.finish();
                    self.skip_rest();
                    return;
                }
                self.bump();
            } else {
                let found = self.describe(0);
                self.err(
                    "header",
                    0,
                    format!("Expected the version number after `sigil`, found {found}."),
                    Some("Write `sigil 1` as the first line."),
                );
            }
        } else {
            self.err(
                "header",
                0,
                "Missing header: line 1 must be `sigil 1`.".to_string(),
                Some("Add `sigil 1` as the first line of the file."),
            );
            self.finish();
            self.skip_rest();
            return;
        }
        self.finish();
        if !matches!(self.kind(0), TK::Newline | TK::Eof) {
            let found = self.describe(0);
            self.err(
                "line-end",
                0,
                format!("Expected end of line after the header, found {found}."),
                None,
            );
            self.skip_line();
        }
        loop {
            while self.kind(0) == TK::Newline {
                self.bump();
            }
            if self.kind(0) == TK::Eof {
                break;
            }
            self.recovered = false;
            let word = if self.kind(0) == TK::Ident {
                self.text(0)
            } else {
                ""
            };
            match word {
                "import" => self.import(),
                "meta" => {
                    self.start(NK::Meta);
                    self.bump();
                    self.ctx.push("meta".into());
                    self.body("meta");
                    self.ctx.pop();
                    self.finish();
                }
                "bullet" => self.item(NK::Bullet),
                "emitter" => self.item(NK::Emitter),
                _ => {
                    let found = self.describe(0);
                    self.err(
                        "item",
                        0,
                        format!("Expected an item (`meta`, `import`, `bullet` or `emitter`), found {found}."),
                        Some("Top-level lines start with `meta`, `import`, `bullet` or `emitter`."),
                    );
                    self.skip_line();
                    continue;
                }
            }
            if !self.recovered && !matches!(self.kind(0), TK::Newline | TK::Eof) {
                let found = self.describe(0);
                self.err(
                    "line-end",
                    0,
                    format!("Expected end of line after the item, found {found}."),
                    None,
                );
                self.skip_line();
            }
        }
    }

    fn skip_rest(&mut self) {
        self.start(NK::Error);
        while self.kind(0) != TK::Eof {
            self.bump();
        }
        self.finish();
    }

    fn import(&mut self) {
        self.start(NK::Import);
        self.bump();
        let ok = self.kind(0) == TK::Str
            && {
                self.bump();
                self.kind(0) == TK::Ident && self.text(0) == "as"
            }
            && {
                self.bump();
                self.kind(0) == TK::Ident
            };
        if ok {
            self.bump();
        } else {
            let found = self.describe(0);
            self.err(
                "import",
                0,
                format!("Malformed import, found {found}."),
                Some("Write `import \"file.sigil\" as alias`."),
            );
            self.skip_line();
        }
        self.finish();
    }

    fn item(&mut self, kind: NK) {
        self.start(kind);
        let kw = self.text(0);
        self.bump();
        if self.kind(0) != TK::Ident {
            let found = self.describe(0);
            self.err(
                "item",
                0,
                format!("Expected a name after `{kw}`, found {found}."),
                Some(&format!("Write e.g. `{kw} orb {{`.")),
            );
            self.skip_line();
            self.finish();
            return;
        }
        let name = self.text(0);
        self.bump();
        let prefix = if kind == NK::Bullet {
            "bullets"
        } else {
            "emitters"
        };
        self.ctx.push(format!("{prefix}.{name}"));
        if kind == NK::Emitter && self.kind(0) == TK::Ident && self.text(0) == "from" {
            self.bump();
            if self.kind(0) == TK::Ident {
                self.reference();
            } else {
                let found = self.describe(0);
                self.err(
                    "item",
                    0,
                    format!("Expected `alias.emitter` after `from`, found {found}."),
                    None,
                );
            }
        }
        self.body(&format!("{kw} {name}"));
        self.ctx.pop();
        self.finish();
    }

    fn body(&mut self, owner: &str) {
        if !self.is_punct(0, "{") {
            let found = self.describe(0);
            self.err(
                "expected-brace",
                0,
                format!("Expected `{{` to open the body of `{owner}`, found {found}."),
                Some("Put `{` at the end of the item line."),
            );
            self.skip_line();
            return;
        }
        let open = self.tok_pos(0);
        self.start(NK::Body);
        self.bump();
        let (mut n_mod, mut n_tr) = (0, 0);
        loop {
            while self.kind(0) == TK::Newline {
                self.bump();
            }
            match self.kind(0) {
                TK::Eof => {
                    let i = self.err(
                        "unclosed-brace",
                        0,
                        format!("Unclosed `{{` of `{owner}`: the file ends before `}}`."),
                        Some("Insert `}` on its own line to close the body."),
                    );
                    self.diags[i].related = Some(diag::Related {
                        label: "opening brace".into(),
                        pos: open,
                    });
                    break;
                }
                TK::Punct if self.text(0) == "}" => {
                    self.bump();
                    break;
                }
                TK::Ident => {
                    let t = self.text(0);
                    let field_like =
                        self.kind(1) == TK::Punct && matches!(self.text(1), "=" | "." | "[");
                    if !field_like && matches!(t, "bullet" | "emitter" | "meta" | "import") {
                        let i = self.err(
                            "unclosed-brace",
                            0,
                            format!(
                                "`{t}` cannot start a member inside `{owner}`; the `{{` opened at line {} is not closed.",
                                open.line
                            ),
                            Some("Insert `}` on its own line before this line."),
                        );
                        self.diags[i].related = Some(diag::Related {
                            label: "opening brace".into(),
                            pos: open,
                        });
                        self.recovered = true;
                        break;
                    }
                    if field_like {
                        self.field();
                    } else if matches!(t, "block" | "modifier" | "transform")
                        && self.kind(1) == TK::Ident
                    {
                        let seg = match t {
                            "block" => ".block".to_string(),
                            "modifier" => {
                                n_mod += 1;
                                format!(".modifiers[{}]", n_mod - 1)
                            }
                            _ => {
                                n_tr += 1;
                                format!(".transforms[{}]", n_tr - 1)
                            }
                        };
                        self.ctx.push(seg);
                        self.nested();
                        self.ctx.pop();
                        if self.recovered {
                            break;
                        }
                    } else {
                        let found = self.describe(0);
                        self.err(
                            "member",
                            0,
                            format!(
                                "Expected a field `name = value` or a nested `block`, `modifier` or `transform` inside `{owner}`, found {found}."
                            ),
                            Some("Fields are written `name = value`, one per line."),
                        );
                        self.skip_line();
                        continue;
                    }
                }
                _ => {
                    let found = self.describe(0);
                    self.err(
                        "member",
                        0,
                        format!("Expected a field `name = value` inside `{owner}`, found {found}."),
                        Some("Fields are written `name = value`, one per line."),
                    );
                    self.skip_line();
                    continue;
                }
            }
            if !(matches!(self.kind(0), TK::Newline | TK::Eof) || self.is_punct(0, "}")) {
                let found = self.describe(0);
                self.err(
                    "line-end",
                    0,
                    format!("Expected end of line, found {found}."),
                    Some("Write one member per line."),
                );
                self.skip_line();
            }
        }
        self.finish();
    }

    fn field(&mut self) {
        self.start(NK::Field);
        self.start(NK::Path);
        let mut name = self.text(0).to_string();
        self.bump();
        loop {
            if self.is_punct(0, ".") && self.kind(1) == TK::Ident {
                self.bump();
                name.push('.');
                name.push_str(self.text(0));
                self.bump();
            } else if self.is_punct(0, "[") && self.kind(1) == TK::Int && self.is_punct(2, "]") {
                self.bump();
                name.push_str(&format!("[{}]", self.text(0)));
                self.bump();
                self.bump();
            } else {
                break;
            }
        }
        self.finish();
        if !self.is_punct(0, "=") {
            let found = self.describe(0);
            self.err(
                "expected-eq",
                0,
                format!("Expected `=` after `{name}`, found {found}."),
                Some(&format!("Write `{name} = <value>`.")),
            );
            self.skip_line();
            self.finish();
            return;
        }
        self.bump();
        self.ctx.push(format!(".{name}"));
        self.value(&name);
        self.ctx.pop();
        self.finish();
    }

    fn nested(&mut self) {
        self.start(NK::Nested);
        let kw = self.text(0);
        self.bump();
        let kind = self.text(0);
        self.bump();
        self.body(&format!("{kw} {kind}"));
        self.finish();
    }

    fn value(&mut self, ctx: &str) {
        match self.kind(0) {
            TK::Int | TK::Float | TK::Qty | TK::Str => {
                self.start(NK::Scalar);
                self.bump();
                self.finish();
            }
            TK::Ident
                if matches!(self.text(0), "time" | "distance" | "event")
                    && matches!(self.kind(1), TK::Qty | TK::Ident | TK::Int | TK::Float) =>
            {
                self.start(NK::Trigger);
                self.bump();
                self.value(ctx);
                self.finish();
            }
            TK::Ident => self.reference(),
            TK::Punct if self.text(0) == "[" => self.list(ctx),
            TK::Punct if self.text(0) == "(" => self.record(ctx),
            _ => {
                let found = self.describe(0);
                self.err(
                    "expected-value",
                    0,
                    format!("Expected a value for `{ctx}`, found {found}."),
                    None,
                );
                self.start(NK::Error);
                if self.kind(0) == TK::Bad {
                    self.bump();
                }
                self.finish();
            }
        }
    }

    fn reference(&mut self) {
        self.start(NK::Ref);
        self.bump();
        while self.is_punct(0, ".") && self.kind(1) == TK::Ident {
            self.bump();
            self.bump();
        }
        self.finish();
    }

    fn list(&mut self, ctx: &str) {
        self.start(NK::List);
        let open = self.tok_pos(0);
        self.bump();
        loop {
            if self.is_punct(0, "]") {
                self.bump();
                break;
            }
            if self.kind(0) == TK::Eof || self.is_punct(0, "}") || self.kind(0) == TK::Newline {
                let i = self.err(
                    "unclosed-bracket",
                    0,
                    format!("Unclosed `[` of list `{ctx}`."),
                    Some("Insert `]` to close the list."),
                );
                self.diags[i].related = Some(diag::Related {
                    label: "opening bracket".into(),
                    pos: open,
                });
                break;
            }
            if self.is_punct(0, ",") {
                self.err(
                    "comma",
                    0,
                    format!("Unexpected `,` in list `{ctx}`: expected a value or `]`."),
                    Some("Remove the extra comma; a single trailing comma is allowed."),
                );
                self.start(NK::Error);
                self.bump();
                self.finish();
                continue;
            }
            self.value(ctx);
            if self.is_punct(0, ",") {
                self.bump();
            } else if !self.is_punct(0, "]") {
                let found = self.describe(0);
                self.err(
                    "expected-comma",
                    0,
                    format!("Expected `,` or `]` in list `{ctx}`, found {found}."),
                    Some("Separate list elements with `,`."),
                );
                self.start(NK::Error);
                while !(self.is_punct(0, "]")
                    || self.is_punct(0, "}")
                    || matches!(self.kind(0), TK::Eof | TK::Newline))
                {
                    self.bump();
                }
                self.finish();
            }
        }
        self.finish();
    }

    fn record(&mut self, ctx: &str) {
        self.start(NK::Record);
        let open = self.tok_pos(0);
        self.bump();
        loop {
            if self.is_punct(0, ")") {
                self.bump();
                break;
            }
            if self.kind(0) == TK::Eof || self.is_punct(0, "}") || self.kind(0) == TK::Newline {
                let i = self.err(
                    "unclosed-paren",
                    0,
                    format!("Unclosed `(` of record `{ctx}`."),
                    Some("Insert `)` to close the record."),
                );
                self.diags[i].related = Some(diag::Related {
                    label: "opening parenthesis".into(),
                    pos: open,
                });
                break;
            }
            if self.is_punct(0, ",") {
                self.err(
                    "comma",
                    0,
                    format!("Unexpected `,` in record `{ctx}`: expected `name = value` or `)`."),
                    Some("Remove the extra comma; a single trailing comma is allowed."),
                );
                self.start(NK::Error);
                self.bump();
                self.finish();
                continue;
            }
            if self.kind(0) == TK::Ident && self.is_punct(1, "=") {
                self.start(NK::RecordField);
                let k = self.text(0);
                self.bump();
                self.bump();
                self.ctx.push(format!(".{k}"));
                self.value(k);
                self.ctx.pop();
                self.finish();
            } else {
                let found = self.describe(0);
                self.err(
                    "record-field",
                    0,
                    format!("Expected `name = value` in record `{ctx}`, found {found}."),
                    Some("Write e.g. `(x = 0u, y = 0u)`."),
                );
                self.start(NK::Error);
                while !(self.is_punct(0, ")")
                    || self.is_punct(0, ",")
                    || self.is_punct(0, "}")
                    || matches!(self.kind(0), TK::Eof | TK::Newline))
                {
                    self.bump();
                }
                self.finish();
            }
            if self.is_punct(0, ",") {
                self.bump();
            } else if !self.is_punct(0, ")") && !(self.kind(0) == TK::Eof || self.is_punct(0, "}"))
            {
                let found = self.describe(0);
                self.err(
                    "expected-comma",
                    0,
                    format!("Expected `,` or `)` in record `{ctx}`, found {found}."),
                    None,
                );
                self.start(NK::Error);
                while !(self.is_punct(0, ")")
                    || self.is_punct(0, "}")
                    || matches!(self.kind(0), TK::Eof | TK::Newline))
                {
                    self.bump();
                }
                self.finish();
            }
        }
        self.finish();
    }
}

// ------------------------------------------------------------------------------------------
// Lowering CST -> value tree
// ------------------------------------------------------------------------------------------

enum Sig<'a> {
    Tok(Token),
    Node(&'a CstNode),
}

fn sig<'a>(n: &'a CstNode, toks: &[Token]) -> Vec<Sig<'a>> {
    n.children
        .iter()
        .filter_map(|c| match c {
            Child::Tok(i) => {
                let t = toks[*i];
                (!t.kind.is_trivia() && !matches!(t.kind, TK::Newline | TK::Eof))
                    .then_some(Sig::Tok(t))
            }
            Child::Node(n) => Some(Sig::Node(n)),
        })
        .collect()
}

fn node_span(n: &CstNode, toks: &[Token]) -> Span {
    let mut first = None;
    let mut last = None;
    fn go(n: &CstNode, toks: &[Token], first: &mut Option<usize>, last: &mut Option<usize>) {
        for c in &n.children {
            match c {
                Child::Tok(i) => {
                    let t = toks[*i];
                    if !t.kind.is_trivia() && !matches!(t.kind, TK::Newline | TK::Eof) {
                        first.get_or_insert(t.start);
                        *last = Some(t.end);
                    }
                }
                Child::Node(n) => go(n, toks, first, last),
            }
        }
    }
    go(n, toks, &mut first, &mut last);
    Span {
        start: first.unwrap_or(0),
        end: last.unwrap_or(0),
    }
}

fn has_error(n: &CstNode) -> bool {
    n.kind == NK::Error
        || n.children.iter().any(|c| match c {
            Child::Node(n) => has_error(n),
            Child::Tok(_) => false,
        })
}

struct Lower<'a> {
    src: &'a str,
    toks: &'a [Token],
    diags: Vec<Diag>,
}

pub fn lower(src: &str, cst: &Cst) -> (Node, Vec<Diag>) {
    let mut l = Lower {
        src,
        toks: &cst.tokens,
        diags: Vec::new(),
    };
    let root = l.file(&cst.root);
    (root, l.diags)
}

fn span_of(t: Token) -> Span {
    Span {
        start: t.start,
        end: t.end,
    }
}

impl<'a> Lower<'a> {
    fn t(&self, t: Token) -> &'a str {
        &self.src[t.start..t.end]
    }

    fn leaf(&self, kind: NodeKind, span: Span, path: String) -> Node {
        Node {
            kind,
            span,
            path,
            text: self.src[span.start..span.end].to_string(),
        }
    }

    fn file(&mut self, root: &CstNode) -> Node {
        let mut fields = Vec::new();
        let mut imports = Vec::new();
        let mut metas = Vec::new();
        let mut bullets = Vec::new();
        let mut emitters = Vec::new();
        for c in sig(root, self.toks) {
            let Sig::Node(n) = c else { continue };
            match n.kind {
                NK::Header => {
                    let s = sig(n, self.toks);
                    if let (Some(Sig::Tok(kw)), Some(Sig::Tok(v))) = (s.first(), s.get(1)) {
                        let num = self.t(*v).parse().unwrap_or(0);
                        fields.push(Field {
                            key: "version".into(),
                            key_span: span_of(*kw),
                            value: self.leaf(NodeKind::Int(num), span_of(*v), "version".into()),
                        });
                    }
                }
                NK::Import if !has_error(n) => {
                    let s = sig(n, self.toks);
                    if let (Some(Sig::Tok(kw)), Some(Sig::Tok(p)), Some(Sig::Tok(a))) =
                        (s.first(), s.get(1), s.get(3))
                    {
                        let alias = self.t(*a).to_string();
                        let path = format!("imports.{alias}");
                        imports.push(Node {
                            kind: NodeKind::Record {
                                tag: None,
                                label: format!("import `{alias}`"),
                                fields: vec![
                                    Field {
                                        key: "path".into(),
                                        key_span: span_of(*kw),
                                        value: self.leaf(
                                            NodeKind::Str(unescape(self.t(*p))),
                                            span_of(*p),
                                            format!("{path}.path"),
                                        ),
                                    },
                                    Field {
                                        key: "alias".into(),
                                        key_span: span_of(*a),
                                        value: self.leaf(
                                            NodeKind::Ref(alias.clone()),
                                            span_of(*a),
                                            format!("{path}.alias"),
                                        ),
                                    },
                                ],
                            },
                            span: span_of(*a),
                            path,
                            text: String::new(),
                        });
                    }
                }
                NK::Meta => {
                    let s = sig(n, self.toks);
                    let (Some(Sig::Tok(kw)), Some(Sig::Node(body))) = (s.first(), s.get(1)) else {
                        continue;
                    };
                    let mut mf = Vec::new();
                    self.body_fields(body, "meta", &mut mf, false);
                    metas.push(Field {
                        key: "meta".into(),
                        key_span: span_of(*kw),
                        value: Node {
                            kind: NodeKind::Record {
                                tag: None,
                                label: "meta".into(),
                                fields: mf,
                            },
                            span: span_of(*kw),
                            path: "meta".into(),
                            text: String::new(),
                        },
                    });
                }
                NK::Bullet | NK::Emitter => {
                    if let Some(item) = self.item(n) {
                        if n.kind == NK::Bullet {
                            bullets.push(item);
                        } else {
                            emitters.push(item);
                        }
                    }
                }
                _ => {}
            }
        }
        if !imports.is_empty() {
            fields.push(self.list_field("imports", imports));
        }
        fields.extend(metas);
        fields.push(self.list_field("bullets", bullets));
        fields.push(self.list_field("emitters", emitters));
        Node {
            kind: NodeKind::Record {
                tag: None,
                label: "the file".into(),
                fields,
            },
            span: Span::default(),
            path: String::new(),
            text: String::new(),
        }
    }

    fn list_field(&self, key: &str, items: Vec<Node>) -> Field {
        Field {
            key: key.into(),
            key_span: Span::default(),
            value: Node {
                kind: NodeKind::List(items),
                span: Span::default(),
                path: key.into(),
                text: String::new(),
            },
        }
    }

    fn item(&mut self, n: &CstNode) -> Option<Node> {
        let s = sig(n, self.toks);
        let Some(Sig::Tok(kw)) = s.first() else {
            return None;
        };
        let Some(Sig::Tok(name_tok)) = s.get(1) else {
            return None;
        };
        let kw_text = self.t(*kw);
        let name = self.t(*name_tok).to_string();
        let prefix = if n.kind == NK::Bullet {
            "bullets"
        } else {
            "emitters"
        };
        let path = format!("{prefix}.{name}");
        let label = format!("{kw_text} `{name}`");
        let mut fields = vec![Field {
            key: "name".into(),
            key_span: span_of(*name_tok),
            value: self.leaf(
                NodeKind::Ref(name.clone()),
                span_of(*name_tok),
                format!("{path}.name"),
            ),
        }];
        let body = s.iter().find_map(|c| match c {
            Sig::Node(b) if b.kind == NK::Body => Some(*b),
            _ => None,
        })?;
        let from = s.iter().find_map(|c| match c {
            Sig::Node(r) if r.kind == NK::Ref => Some(*r),
            _ => None,
        });
        let tag = if let Some(r) = from {
            let sp = node_span(r, self.toks);
            fields.push(Field {
                key: "from".into(),
                key_span: sp,
                value: self.leaf(
                    NodeKind::Ref(self.src[sp.start..sp.end].to_string()),
                    sp,
                    format!("{path}.from"),
                ),
            });
            let mut ov = Vec::new();
            for c in sig(body, self.toks) {
                let Sig::Node(m) = c else { continue };
                match m.kind {
                    NK::Field if !has_error(m) => {
                        let ms = sig(m, self.toks);
                        let (Some(Sig::Node(p)), Some(Sig::Node(v))) = (ms.first(), ms.get(2))
                        else {
                            continue;
                        };
                        let psp = node_span(p, self.toks);
                        let key = self.src[psp.start..psp.end].to_string();
                        let vsp = node_span(v, self.toks);
                        let vtext = &self.src[vsp.start..vsp.end];
                        ov.push(Field {
                            key: key.clone(),
                            key_span: psp,
                            value: Node {
                                kind: NodeKind::Raw(format!("{}\0{}", vsp.start, vtext)),
                                span: vsp,
                                path: format!("{path}.{key}"),
                                text: vtext.to_string(),
                            },
                        });
                    }
                    NK::Nested => {
                        let sp = node_span(m, self.toks);
                        self.diags.push(
                            Diag::new(
                                Phase::Schema,
                                "override",
                                format!("`emitter {name} from ...` only takes override fields, not nested members."),
                            )
                            .at(Some(diag::pos_of_offset(self.src, sp.start)))
                            .hint("Override single values with paths, e.g. `block.count = 12`.")
                            .path(path.clone()),
                        );
                    }
                    _ => {}
                }
            }
            fields.push(Field {
                key: "overrides".into(),
                key_span: span_of(*kw),
                value: Node {
                    kind: NodeKind::Record {
                        tag: None,
                        label: "overrides".into(),
                        fields: ov,
                    },
                    span: span_of(*name_tok),
                    path: path.clone(),
                    text: String::new(),
                },
            });
            Some(("use".to_string(), span_of(*kw)))
        } else {
            self.body_fields(body, &path, &mut fields, true);
            (n.kind == NK::Emitter).then(|| ("emitter".to_string(), span_of(*kw)))
        };
        Some(Node {
            kind: NodeKind::Record { tag, label, fields },
            span: span_of(*name_tok),
            path,
            text: String::new(),
        })
    }

    fn body_fields(&mut self, body: &CstNode, owner: &str, fields: &mut Vec<Field>, _item: bool) {
        let mut modifiers = Vec::new();
        let mut transforms = Vec::new();
        let mut first_mod = Span::default();
        let mut first_tr = Span::default();
        for c in sig(body, self.toks) {
            let Sig::Node(m) = c else { continue };
            match m.kind {
                NK::Field => {
                    let ms = sig(m, self.toks);
                    let (Some(Sig::Node(p)), Some(Sig::Node(v))) = (ms.first(), ms.get(2)) else {
                        continue;
                    };
                    let psp = node_span(p, self.toks);
                    let key = self.src[psp.start..psp.end].to_string();
                    if key.contains('.') || key.contains('[') {
                        self.diags.push(
                            Diag::new(
                                Phase::Schema,
                                "dotted-path",
                                format!("The path `{key}` is only allowed in overrides of `emitter ... from ...`."),
                            )
                            .at(Some(diag::pos_of_offset(self.src, psp.start)))
                            .hint("Write the value inside the nested member instead.")
                            .path(format!("{owner}.{key}")),
                        );
                        continue;
                    }
                    let path = format!("{owner}.{key}");
                    if let Some(value) = self.value(v, path, &key) {
                        fields.push(Field {
                            key,
                            key_span: psp,
                            value,
                        });
                    }
                }
                NK::Nested => {
                    let ms = sig(m, self.toks);
                    let (Some(Sig::Tok(kw)), Some(Sig::Tok(kind)), Some(Sig::Node(b))) =
                        (ms.first(), ms.get(1), ms.get(2))
                    else {
                        continue;
                    };
                    let kw_text = self.t(*kw);
                    let kind_text = self.t(*kind).to_string();
                    let (key, path) = match kw_text {
                        "block" => ("block".to_string(), format!("{owner}.block")),
                        "modifier" => (
                            "modifiers".to_string(),
                            format!("{owner}.modifiers[{}]", modifiers.len()),
                        ),
                        _ => (
                            "transforms".to_string(),
                            format!("{owner}.transforms[{}]", transforms.len()),
                        ),
                    };
                    let mut nf = Vec::new();
                    self.body_fields(b, &path, &mut nf, false);
                    let node = Node {
                        kind: NodeKind::Record {
                            tag: Some((kind_text.clone(), span_of(*kind))),
                            label: format!("{kw_text} `{kind_text}`"),
                            fields: nf,
                        },
                        span: span_of(*kind),
                        path,
                        text: String::new(),
                    };
                    match key.as_str() {
                        "block" => fields.push(Field {
                            key,
                            key_span: span_of(*kw),
                            value: node,
                        }),
                        "modifiers" => {
                            if modifiers.is_empty() {
                                first_mod = span_of(*kw);
                            }
                            modifiers.push(node);
                        }
                        _ => {
                            if transforms.is_empty() {
                                first_tr = span_of(*kw);
                            }
                            transforms.push(node);
                        }
                    }
                }
                _ => {}
            }
        }
        for (key, items, span) in [
            ("modifiers", modifiers, first_mod),
            ("transforms", transforms, first_tr),
        ] {
            if !items.is_empty() {
                fields.push(Field {
                    key: key.into(),
                    key_span: span,
                    value: Node {
                        kind: NodeKind::List(items),
                        span,
                        path: format!("{owner}.{key}"),
                        text: String::new(),
                    },
                });
            }
        }
    }

    fn value(&mut self, n: &CstNode, path: String, ctx: &str) -> Option<Node> {
        let span = node_span(n, self.toks);
        let s = sig(n, self.toks);
        let kind = match n.kind {
            NK::Scalar => {
                let Some(Sig::Tok(t)) = s.first() else {
                    return None;
                };
                let text = self.t(*t);
                match t.kind {
                    TK::Int => match text.parse::<i64>() {
                        Ok(i) => NodeKind::Int(i),
                        Err(_) => NodeKind::Float(text.parse().unwrap_or(0.0)),
                    },
                    TK::Float => NodeKind::Float(text.parse().unwrap_or(0.0)),
                    TK::Qty => {
                        let split = text
                            .find(|c: char| c.is_ascii_alphabetic())
                            .unwrap_or(text.len());
                        let num = &text[..split];
                        NodeKind::Qty {
                            num: num.parse().unwrap_or(0.0),
                            int: !num.contains('.'),
                            unit: text[split..].to_string(),
                        }
                    }
                    TK::Str => NodeKind::Str(unescape(text)),
                    _ => return None,
                }
            }
            NK::Ref => NodeKind::Ref(self.src[span.start..span.end].to_string()),
            NK::Trigger => {
                let (Some(Sig::Tok(kw)), Some(Sig::Node(arg))) = (s.first(), s.get(1)) else {
                    return None;
                };
                let arg = self.value(arg, path.clone(), ctx)?;
                NodeKind::Trigger {
                    kind: self.t(*kw).to_string(),
                    kind_span: span_of(*kw),
                    arg: Box::new(arg),
                }
            }
            NK::List => {
                let mut items = Vec::new();
                for c in &s {
                    if let Sig::Node(e) = c {
                        if e.kind == NK::Error {
                            continue;
                        }
                        let p = format!("{path}[{}]", items.len());
                        if let Some(v) = self.value(e, p, ctx) {
                            items.push(v);
                        }
                    }
                }
                NodeKind::List(items)
            }
            NK::Record => {
                let mut fields = Vec::new();
                for c in &s {
                    if let Sig::Node(rf) = c {
                        if rf.kind != NK::RecordField {
                            continue;
                        }
                        let rs = sig(rf, self.toks);
                        let (Some(Sig::Tok(k)), Some(Sig::Node(v))) = (rs.first(), rs.get(2))
                        else {
                            continue;
                        };
                        let key = self.t(*k).to_string();
                        if let Some(value) = self.value(v, format!("{path}.{key}"), &key) {
                            fields.push(Field {
                                key,
                                key_span: span_of(*k),
                                value,
                            });
                        }
                    }
                }
                NodeKind::Record {
                    tag: None,
                    label: format!("`{ctx}`"),
                    fields,
                }
            }
            _ => return None,
        };
        Some(self.leaf(kind, span, path))
    }
}

fn unescape(quoted: &str) -> String {
    let inner = quoted.strip_prefix('"').unwrap_or(quoted);
    let inner = inner.strip_suffix('"').unwrap_or(inner);
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parses a standalone value (override text) at `offset` of `src` into a node.
pub fn parse_value_at(src: &str, offset: usize, len: usize, path: &str) -> Option<Node> {
    // Re-parse the whole file prefix-free: wrap the text in a synthetic field so the normal
    // parser runs, then shift spans back to the file.
    let text = &src[offset..offset + len];
    let synthetic = format!("sigil 1\nmeta {{\n  v = {text}\n}}\n");
    let shift = offset as isize - "sigil 1\nmeta {\n  v = ".len() as isize;
    let (cst, _) = parse(&synthetic);
    let (root, _) = lower(&synthetic, &cst);
    let mut node = tree::locate(&root, "meta.v")?.clone();
    fn fix(n: &mut Node, shift: isize, path: &str) {
        n.span.start = (n.span.start as isize + shift) as usize;
        n.span.end = (n.span.end as isize + shift) as usize;
        n.path = n.path.replacen("meta.v", path, 1);
        match &mut n.kind {
            NodeKind::List(items) => items.iter_mut().for_each(|i| fix(i, shift, path)),
            NodeKind::Record { fields, .. } => fields.iter_mut().for_each(|f| {
                f.key_span.start = (f.key_span.start as isize + shift) as usize;
                f.key_span.end = (f.key_span.end as isize + shift) as usize;
                fix(&mut f.value, shift, path)
            }),
            NodeKind::Trigger { arg, kind_span, .. } => {
                kind_span.start = (kind_span.start as isize + shift) as usize;
                kind_span.end = (kind_span.end as isize + shift) as usize;
                fix(arg, shift, path)
            }
            _ => {}
        }
    }
    fix(&mut node, shift, path);
    Some(node)
}

/// Lossless edit: replaces the value at `path` with `new_text`, every other byte stays.
pub fn set(src: &str, path: &str, new_text: &str) -> Result<String, String> {
    let (cst, diags) = parse(src);
    if let Some(d) = diags.first() {
        return Err(format!("file does not parse: {}", d.cause));
    }
    let (root, _) = lower(src, &cst);
    let node = tree::locate(&root, path).ok_or_else(|| format!("path `{path}` not found"))?;
    Ok(format!(
        "{}{}{}",
        &src[..node.span.start],
        new_text,
        &src[node.span.end..]
    ))
}
