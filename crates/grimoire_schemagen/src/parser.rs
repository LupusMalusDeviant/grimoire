//! Parser for the schema description language (project ADR-0011, WP8.1).
//!
//! # The language
//!
//! A schema file is plain ASCII/UTF-8 text (typical extension `.gschema`, files live under
//! `schema/` per project ADR-0011's "Vorschlag" point 4). `#` starts a line comment. Commas are
//! punctuation the parser accepts anywhere between fields/variants and never requires, so a file
//! reads close to Rust without being it (avoids implying this is actual Rust source, since it is
//! parsed by a hand-written parser rather than `rustc`/`syn`). A semicolon has exactly one job:
//! separating a fixed array's element type from its length, `[T; N]`.
//!
//! Every file starts with three directives, in order, the last optionally followed by one or
//! more bare string literals — the schema's own doc comment:
//!
//! ```text
//! schema "debug_protocol_v1"
//! version 1
//! error_type "crate::frame::ProtocolError"   # or: error_type "self"
//! "Payload types of the Grimoire debug protocol v1 (engine contract §13)."
//! ```
//!
//! `error_type` names an existing Rust error type the generated `decode` functions return
//! ([`crate::ir::ErrorType::External`]); the special value `"self"` instead makes the Rust
//! emitter generate a matching, self-contained error enum in the output file
//! ([`crate::ir::ErrorType::SelfContained`]). See [`crate::ir::ErrorType`] for the exact variant
//! shapes either path assumes. The schema's own doc comment uses a bare trailing string (like a
//! field's or variant's doc, see below) rather than a `doc "..."` line, specifically so it can
//! never be confused with, or swallow, the next declaration's `doc "..."` lines — the two forms
//! look different on purpose.
//!
//! After the directives, any number of `enum`, `struct` and `message` declarations follow, each
//! optionally preceded by one or more `doc "..."` lines:
//!
//! ```text
//! doc "Tool or engine side of a debug-link connection."
//! enum PeerRole : u8 {
//!     Tool = 0 "Tool-side peer (editor, CLI, C# client)."
//!     Engine = 1 "Engine-side peer."
//! }
//!
//! struct Hello {
//!     protocol_version: u16 "Protocol version the sender speaks."
//!     role: PeerRole "Tool or Engine."
//!     token: [u8; 32] "Debug-link token; all zero from the engine."
//!     stats_interval_frames: u16 "0 = no Stats messages."
//! }
//!
//! message Hello = 0x0001 dir both payload Hello "Handshake request/response."
//! ```
//!
//! A field's or variant's doc string(s) come immediately after its declaration; several string
//! literals in a row become several doc-comment lines.
//!
//! A `struct` may carry the modifier `growable` right after its name, and an `enum` the modifiers
//! `non_exhaustive` and/or `fallback <Variant>` right after its discriminant width — see
//! [`crate::ir::StructDef::growable`] and [`crate::ir::EnumDef::non_exhaustive`]/
//! [`crate::ir::EnumDef::fallback`] for what each one changes in the generated code.
//!
//! ## Types
//!
//! `u8 u16 u32 u64 f32 bool hash64 id64` (contract §13's catalogue never uses a signed integer,
//! so the language does not have one), `str(N)`, `str16(N)`, `bytes(N)`, `vec(N) <Type>`,
//! `vec_using(<field>, N) <Type>` (element count taken from an earlier field of the same struct
//! instead of its own wire count, e.g. contract §12's manifest), `option(<Type>)`,
//! `[<Type>; N]`, or a bare identifier naming another `enum`/`struct` declared in the same file.
//! See [`crate::ir::TypeRef`] for what each one means on the wire.

use crate::ir::{
    Direction, EnumDef, EnumVariant, EnumWidth, ErrorType, FieldDef, Item, MessageEntry, Schema,
    StructDef, TypeRef,
};
use std::fmt;

/// A schema file failed to parse, or parsed to something the emitters could not use (e.g. a type
/// referencing an undeclared name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaError {
    /// 1-based line number the problem was found on (best effort: for a validation error found
    /// after parsing, e.g. an unresolved reference, this points at the referencing declaration).
    pub line: usize,
    /// Human-readable description.
    pub message: String,
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for SchemaError {}

/// Parses `source` (the contents of a `.gschema` file) into a [`Schema`], resolving and
/// validating every [`TypeRef::Named`] reference against the file's own `enum`/`struct`
/// declarations.
///
/// # Errors
/// Returns [`SchemaError`] for any lexical, syntactic or reference error. Never panics on any
/// input (this is a developer tool invoked from a checked-in `.gschema` file, not a decoder of
/// foreign bytes under contract §2 rule 9, but a hand-authored schema can still contain typos,
/// and a panic would be a worse failure mode than a message with a line number).
pub fn parse(source: &str) -> Result<Schema, SchemaError> {
    let tokens = lex(source)?;
    let mut parser = Parser { tokens, pos: 0 };
    let schema = parser.parse_schema()?;
    validate(&schema)?;
    Ok(schema)
}

// --- Lexer -------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    Number(u64),
    Str(String),
    Punct(char),
}

struct Spanned {
    token: Token,
    line: usize,
}

fn lex(source: &str) -> Result<Vec<Spanned>, SchemaError> {
    let mut out = Vec::new();
    let bytes: Vec<char> = source.chars().collect();
    let mut i = 0usize;
    let mut line = 1usize;

    while i < bytes.len() {
        let c = bytes[i];
        match c {
            '\n' => {
                line += 1;
                i += 1;
            }
            c if c.is_whitespace() => {
                i += 1;
            }
            '#' => {
                while i < bytes.len() && bytes[i] != '\n' {
                    i += 1;
                }
            }
            ',' => {
                // Punctuation the parser never requires; accepted anywhere and dropped here so
                // the parser does not need to special-case it between every pair of tokens.
                i += 1;
            }
            '{' | '}' | '(' | ')' | '[' | ']' | ':' | '=' | ';' => {
                out.push(Spanned {
                    token: Token::Punct(c),
                    line,
                });
                i += 1;
            }
            '"' => {
                let start_line = line;
                i += 1;
                let mut s = String::new();
                loop {
                    if i >= bytes.len() {
                        return Err(SchemaError {
                            line: start_line,
                            message: "unterminated string literal".to_owned(),
                        });
                    }
                    match bytes[i] {
                        '"' => {
                            i += 1;
                            break;
                        }
                        '\\' if i + 1 < bytes.len() => {
                            let escaped = bytes[i + 1];
                            s.push(match escaped {
                                '"' => '"',
                                '\\' => '\\',
                                'n' => '\n',
                                other => {
                                    return Err(SchemaError {
                                        line,
                                        message: format!("unknown escape '\\{other}'"),
                                    });
                                }
                            });
                            i += 2;
                        }
                        '\n' => {
                            return Err(SchemaError {
                                line,
                                message: "string literal must not span a newline".to_owned(),
                            });
                        }
                        other => {
                            s.push(other);
                            i += 1;
                        }
                    }
                }
                out.push(Spanned {
                    token: Token::Str(s),
                    line: start_line,
                });
            }
            c if c.is_ascii_digit() => {
                let start = i;
                let hex = bytes.get(i + 1) == Some(&'x');
                if hex {
                    i += 2;
                    while i < bytes.len() && (bytes[i].is_ascii_hexdigit() || bytes[i] == '_') {
                        i += 1;
                    }
                } else {
                    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == '_') {
                        i += 1;
                    }
                }
                let text: String = bytes[start..i].iter().collect();
                let digits = text.replace('_', "");
                let value = if hex {
                    u64::from_str_radix(digits.trim_start_matches("0x"), 16)
                } else {
                    digits.parse::<u64>()
                }
                .map_err(|error| SchemaError {
                    line,
                    message: format!("invalid number literal {text:?}: {error}"),
                })?;
                out.push(Spanned {
                    token: Token::Number(value),
                    line,
                });
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == '_') {
                    i += 1;
                }
                let text: String = bytes[start..i].iter().collect();
                out.push(Spanned {
                    token: Token::Ident(text),
                    line,
                });
            }
            other => {
                return Err(SchemaError {
                    line,
                    message: format!("unexpected character {other:?}"),
                });
            }
        }
    }

    Ok(out)
}

// --- Recursive-descent parser --------------------------------------------------------------

struct Parser {
    tokens: Vec<Spanned>,
    pos: usize,
}

impl Parser {
    fn line(&self) -> usize {
        self.tokens
            .get(self.pos)
            .or_else(|| self.tokens.last())
            .map_or(1, |t| t.line)
    }

    fn err(&self, message: impl Into<String>) -> SchemaError {
        SchemaError {
            line: self.line(),
            message: message.into(),
        }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos).map(|t| &t.token)
    }

    fn advance(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).map(|s| s.token.clone());
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn expect_ident(&mut self) -> Result<String, SchemaError> {
        match self.advance() {
            Some(Token::Ident(s)) => Ok(s),
            other => Err(self.err(format!("expected an identifier, found {other:?}"))),
        }
    }

    fn expect_keyword(&mut self, keyword: &str) -> Result<(), SchemaError> {
        match self.advance() {
            Some(Token::Ident(s)) if s == keyword => Ok(()),
            other => Err(self.err(format!("expected keyword {keyword:?}, found {other:?}"))),
        }
    }

    fn expect_string(&mut self) -> Result<String, SchemaError> {
        match self.advance() {
            Some(Token::Str(s)) => Ok(s),
            other => Err(self.err(format!("expected a string literal, found {other:?}"))),
        }
    }

    fn expect_number(&mut self) -> Result<u64, SchemaError> {
        match self.advance() {
            Some(Token::Number(n)) => Ok(n),
            other => Err(self.err(format!("expected a number, found {other:?}"))),
        }
    }

    fn expect_punct(&mut self, c: char) -> Result<(), SchemaError> {
        match self.advance() {
            Some(Token::Punct(p)) if p == c => Ok(()),
            other => Err(self.err(format!("expected {c:?}, found {other:?}"))),
        }
    }

    /// Consumes zero or more consecutive string literals as doc-comment lines.
    fn take_doc_strings(&mut self) -> Vec<String> {
        let mut doc = Vec::new();
        while let Some(Token::Str(_)) = self.peek() {
            if let Some(Token::Str(s)) = self.advance() {
                doc.push(s);
            }
        }
        doc
    }

    fn parse_schema(&mut self) -> Result<Schema, SchemaError> {
        self.expect_keyword("schema")?;
        let name = self.expect_string()?;
        self.expect_keyword("version")?;
        let version = self.expect_number()?;
        let version = u32::try_from(version)
            .map_err(|_| self.err(format!("version {version} does not fit a u32")))?;
        self.expect_keyword("error_type")?;
        let error_type_raw = self.expect_string()?;
        let error_type = if error_type_raw == "self" {
            ErrorType::SelfContained
        } else {
            ErrorType::External(error_type_raw)
        };
        // Bare trailing string literals here (not `doc "..."` lines) so the schema's own doc
        // comment has an unambiguous end: the item loop below only recognises `doc "..."` right
        // before `enum`/`struct`/`message`, so it can never be confused with, or swallow, this.
        let doc = self.take_doc_strings();

        let mut items = Vec::new();
        let mut messages = Vec::new();

        loop {
            let pending_doc = self.take_doc_strings_before_item();
            match self.peek() {
                None => break,
                Some(Token::Ident(kw)) if kw == "enum" => {
                    items.push(Item::Enum(self.parse_enum(pending_doc)?));
                }
                Some(Token::Ident(kw)) if kw == "struct" => {
                    items.push(Item::Struct(self.parse_struct(pending_doc)?));
                }
                Some(Token::Ident(kw)) if kw == "message" => {
                    messages.push(self.parse_message(pending_doc)?);
                }
                other => {
                    return Err(self.err(format!(
                        "expected 'enum', 'struct' or 'message', found {other:?}"
                    )));
                }
            }
        }

        Ok(Schema {
            name,
            version,
            doc,
            source_path: String::new(), // filled in by the caller, which knows the file path
            error_type,
            items,
            messages,
        })
    }

    /// `doc` lines may precede an item as `doc "..."` statements (distinct from the trailing
    /// string literals on a field/variant/message, which document that one declaration).
    fn take_doc_strings_before_item(&mut self) -> Vec<String> {
        let mut doc = Vec::new();
        loop {
            match self.peek() {
                Some(Token::Ident(kw)) if kw == "doc" => {
                    self.pos += 1;
                    if let Ok(s) = self.expect_string() {
                        doc.push(s);
                    }
                }
                _ => break,
            }
        }
        doc
    }

    fn parse_enum(&mut self, doc: Vec<String>) -> Result<EnumDef, SchemaError> {
        self.expect_keyword("enum")?;
        let name = self.expect_ident()?;
        self.expect_punct(':')?;
        let width_ident = self.expect_ident()?;
        let width = match width_ident.as_str() {
            "u8" => EnumWidth::U8,
            "u16" => EnumWidth::U16,
            other => {
                return Err(self.err(format!(
                    "enum discriminant width must be 'u8' or 'u16', found {other:?}"
                )));
            }
        };
        // Optional modifiers, in any combination, before the opening brace: `non_exhaustive` and
        // `fallback <Variant>` (contract §2 rule 13, §13 "unbekannter Code -> Malformed").
        let mut non_exhaustive = false;
        let mut fallback = None;
        loop {
            match self.peek() {
                Some(Token::Ident(kw)) if kw == "non_exhaustive" => {
                    self.pos += 1;
                    non_exhaustive = true;
                }
                Some(Token::Ident(kw)) if kw == "fallback" => {
                    self.pos += 1;
                    fallback = Some(self.expect_ident()?);
                }
                _ => break,
            }
        }
        self.expect_punct('{')?;
        let mut variants = Vec::new();
        while !matches!(self.peek(), Some(Token::Punct('}'))) {
            let vname = self.expect_ident()?;
            self.expect_punct('=')?;
            let value = self.expect_number()?;
            let value = u32::try_from(value)
                .map_err(|_| self.err(format!("variant value {value} does not fit a u32")))?;
            let vdoc = self.take_doc_strings();
            variants.push(EnumVariant {
                name: vname,
                value,
                doc: vdoc,
            });
        }
        self.expect_punct('}')?;
        if let Some(target) = &fallback
            && !variants.iter().any(|v| &v.name == target)
        {
            return Err(self.err(format!(
                "enum {name:?}'s fallback variant {target:?} is not declared in this enum"
            )));
        }
        Ok(EnumDef {
            name,
            width,
            non_exhaustive,
            fallback,
            doc,
            variants,
        })
    }

    fn parse_struct(&mut self, doc: Vec<String>) -> Result<StructDef, SchemaError> {
        self.expect_keyword("struct")?;
        let name = self.expect_ident()?;
        let growable = matches!(self.peek(), Some(Token::Ident(kw)) if kw == "growable");
        if growable {
            self.pos += 1;
        }
        self.expect_punct('{')?;
        let mut fields = Vec::new();
        while !matches!(self.peek(), Some(Token::Punct('}'))) {
            let fname = self.expect_ident()?;
            self.expect_punct(':')?;
            let ty = self.parse_type()?;
            let fdoc = self.take_doc_strings();
            fields.push(FieldDef {
                name: fname,
                ty,
                doc: fdoc,
            });
        }
        self.expect_punct('}')?;
        Ok(StructDef {
            name,
            growable,
            doc,
            fields,
        })
    }

    fn parse_message(&mut self, doc: Vec<String>) -> Result<MessageEntry, SchemaError> {
        self.expect_keyword("message")?;
        let name = self.expect_ident()?;
        self.expect_punct('=')?;
        let id = self.expect_number()?;
        let id = u32::try_from(id)
            .map_err(|_| self.err(format!("message id {id} does not fit a u32")))?;
        self.expect_keyword("dir")?;
        let dir_ident = self.expect_ident()?;
        let direction = match dir_ident.as_str() {
            "both" => Direction::Both,
            "tool_to_engine" => Direction::ToolToEngine,
            "engine_to_tool" => Direction::EngineToTool,
            other => {
                return Err(self.err(format!(
                    "message direction must be 'both', 'tool_to_engine' or 'engine_to_tool', found {other:?}"
                )));
            }
        };
        self.expect_keyword("payload")?;
        let payload = self.expect_ident()?;
        let trailing_doc = self.take_doc_strings();
        let doc = if doc.is_empty() { trailing_doc } else { doc };
        Ok(MessageEntry {
            name,
            id,
            direction,
            payload,
            doc,
        })
    }

    fn parse_type(&mut self) -> Result<TypeRef, SchemaError> {
        match self.peek() {
            Some(Token::Punct('[')) => {
                self.pos += 1;
                let elem = self.parse_type()?;
                self.expect_punct(';')?;
                let len = self.expect_number()?;
                let len = u32::try_from(len)
                    .map_err(|_| self.err(format!("array length {len} does not fit a u32")))?;
                self.expect_punct(']')?;
                Ok(TypeRef::Array {
                    elem: Box::new(elem),
                    len,
                })
            }
            Some(Token::Ident(_)) => {
                let ident = self.expect_ident()?;
                match ident.as_str() {
                    "u8" => Ok(TypeRef::U8),
                    "u16" => Ok(TypeRef::U16),
                    "u32" => Ok(TypeRef::U32),
                    "u64" => Ok(TypeRef::U64),
                    "f32" => Ok(TypeRef::F32),
                    "bool" => Ok(TypeRef::Bool),
                    "hash64" => Ok(TypeRef::Hash64),
                    "id64" => Ok(TypeRef::Id64),
                    "str" => {
                        self.expect_punct('(')?;
                        let max = self.expect_number()?;
                        let max = u32::try_from(max).map_err(|_| {
                            self.err(format!("str max length {max} does not fit a u32"))
                        })?;
                        self.expect_punct(')')?;
                        Ok(TypeRef::Str { max })
                    }
                    "str16" => {
                        self.expect_punct('(')?;
                        let max = self.expect_number()?;
                        let max = u32::try_from(max).map_err(|_| {
                            self.err(format!("str16 max length {max} does not fit a u32"))
                        })?;
                        self.expect_punct(')')?;
                        Ok(TypeRef::Str16 { max })
                    }
                    "bytes" => {
                        self.expect_punct('(')?;
                        let max = self.expect_number()?;
                        let max = u32::try_from(max).map_err(|_| {
                            self.err(format!("bytes max length {max} does not fit a u32"))
                        })?;
                        self.expect_punct(')')?;
                        Ok(TypeRef::Bytes { max })
                    }
                    "vec" => {
                        self.expect_punct('(')?;
                        let max = self.expect_number()?;
                        let max = u32::try_from(max).map_err(|_| {
                            self.err(format!("vec max count {max} does not fit a u32"))
                        })?;
                        self.expect_punct(')')?;
                        let elem = self.parse_type()?;
                        Ok(TypeRef::Vec {
                            max,
                            elem: Box::new(elem),
                        })
                    }
                    "vec_using" => {
                        self.expect_punct('(')?;
                        let count_field = self.expect_ident()?;
                        let max = self.expect_number()?;
                        let max = u32::try_from(max).map_err(|_| {
                            self.err(format!("vec_using max count {max} does not fit a u32"))
                        })?;
                        self.expect_punct(')')?;
                        let elem = self.parse_type()?;
                        Ok(TypeRef::VecUsing {
                            count_field,
                            max,
                            elem: Box::new(elem),
                        })
                    }
                    "option" => {
                        self.expect_punct('(')?;
                        let inner = self.parse_type()?;
                        self.expect_punct(')')?;
                        Ok(TypeRef::Option(Box::new(inner)))
                    }
                    other => Ok(TypeRef::Named(other.to_owned())),
                }
            }
            other => Err(self.err(format!("expected a type, found {other:?}"))),
        }
    }
}

// --- Post-parse validation -----------------------------------------------------------------

/// Walks every [`TypeRef::Named`] reachable from any field, enum, or message payload and checks
/// it resolves to a declared `enum` or `struct` in the same schema; also rejects duplicate item
/// names and message payload references to an undeclared struct.
fn validate(schema: &Schema) -> Result<(), SchemaError> {
    let mut seen = std::collections::BTreeSet::new();
    for item in &schema.items {
        if !seen.insert(item.name().to_owned()) {
            return Err(SchemaError {
                line: 0,
                message: format!("duplicate item name {:?}", item.name()),
            });
        }
    }

    for item in &schema.items {
        if let Item::Struct(s) = item {
            for (index, field) in s.fields.iter().enumerate() {
                validate_type(schema, &s.name, &field.name, &field.ty)?;
                if let TypeRef::VecUsing { count_field, .. } = &field.ty {
                    let earlier = &s.fields[..index];
                    let source = earlier.iter().find(|f| &f.name == count_field);
                    match source {
                        None => {
                            return Err(SchemaError {
                                line: 0,
                                message: format!(
                                    "{}.{} is `vec_using({count_field}, ..)`, but {count_field:?} is not an earlier field of the same struct",
                                    s.name, field.name
                                ),
                            });
                        }
                        Some(source)
                            if !matches!(
                                source.ty,
                                TypeRef::U8 | TypeRef::U16 | TypeRef::U32 | TypeRef::U64
                            ) =>
                        {
                            return Err(SchemaError {
                                line: 0,
                                message: format!(
                                    "{}.{} is `vec_using({count_field}, ..)`, but {count_field:?} is not an unsigned integer field",
                                    s.name, field.name
                                ),
                            });
                        }
                        Some(_) => {}
                    }
                }
            }
        }
    }

    for message in &schema.messages {
        if !schema.items.iter().any(|item| match item {
            Item::Struct(s) => s.name == message.payload,
            Item::Enum(_) => false,
        }) {
            return Err(SchemaError {
                line: 0,
                message: format!(
                    "message {:?} references undeclared payload struct {:?}",
                    message.name, message.payload
                ),
            });
        }
    }

    Ok(())
}

fn validate_type(
    schema: &Schema,
    struct_name: &str,
    field_name: &str,
    ty: &TypeRef,
) -> Result<(), SchemaError> {
    match ty {
        TypeRef::Named(name) => {
            if schema.items.iter().any(|item| item.name() == name) {
                Ok(())
            } else {
                Err(SchemaError {
                    line: 0,
                    message: format!(
                        "{struct_name}.{field_name} references undeclared type {name:?}"
                    ),
                })
            }
        }
        TypeRef::Vec { elem, .. }
        | TypeRef::VecUsing { elem, .. }
        | TypeRef::Option(elem)
        | TypeRef::Array { elem, .. } => validate_type(schema, struct_name, field_name, elem),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_minimal_schema() {
        let source = r#"
            schema "example"
            version 1
            error_type "self"

            enum PeerRole : u8 {
                Tool = 0 "Tool side."
                Engine = 1 "Engine side."
            }

            struct Hello {
                protocol_version: u16 "Version."
                role: PeerRole "Role."
                token: [u8; 32] "Token."
                scopes: vec(64) u16 "Scope ids."
                target: option([f32; 2]) "Optional target."
            }

            message Hello = 0x0001 dir both payload Hello "Handshake."
        "#;
        let schema = parse(source).expect("schema should parse");
        assert_eq!(schema.name, "example");
        assert_eq!(schema.version, 1);
        assert_eq!(schema.error_type, ErrorType::SelfContained);
        assert_eq!(schema.items.len(), 2);
        assert_eq!(schema.messages.len(), 1);
        assert_eq!(schema.messages[0].id, 1);
        assert_eq!(schema.messages[0].direction, Direction::Both);
    }

    #[test]
    fn rejects_unknown_type_reference() {
        let source = r#"
            schema "example"
            version 1
            error_type "self"
            struct Broken {
                x: DoesNotExist "bad"
            }
        "#;
        let error = parse(source).unwrap_err();
        assert!(error.message.contains("DoesNotExist"), "{error}");
    }

    #[test]
    fn rejects_duplicate_item_names() {
        let source = r#"
            schema "example"
            version 1
            error_type "self"
            struct Dup { x: u8 "x" }
            struct Dup { y: u8 "y" }
        "#;
        let error = parse(source).unwrap_err();
        assert!(error.message.contains("duplicate"), "{error}");
    }

    #[test]
    fn rejects_message_with_undeclared_payload() {
        let source = r#"
            schema "example"
            version 1
            error_type "self"
            message Foo = 1 dir both payload Missing
        "#;
        let error = parse(source).unwrap_err();
        assert!(error.message.contains("Missing"), "{error}");
    }

    #[test]
    fn struct_growable_and_enum_non_exhaustive_fallback_modifiers() {
        let source = r#"
            schema "example"
            version 1
            error_type "self"

            enum ErrorCode : u16 non_exhaustive fallback Malformed {
                Malformed = 4 "Malformed."
                Busy = 8 "Busy."
            }

            struct Stats growable {
                frame: u64 "Frame."
            }
        "#;
        let schema = parse(source).expect("should parse");
        match &schema.items[0] {
            Item::Enum(e) => {
                assert!(e.non_exhaustive);
                assert_eq!(e.fallback.as_deref(), Some("Malformed"));
            }
            other => panic!("unexpected item {other:?}"),
        }
        match &schema.items[1] {
            Item::Struct(s) => assert!(s.growable),
            other => panic!("unexpected item {other:?}"),
        }
    }

    #[test]
    fn enum_fallback_must_name_a_declared_variant() {
        let source = r#"
            schema "example"
            version 1
            error_type "self"
            enum E : u8 fallback Missing {
                A = 0 "a"
            }
        "#;
        let error = parse(source).unwrap_err();
        assert!(error.message.contains("Missing"), "{error}");
    }

    #[test]
    fn semantic_hash_and_id_types() {
        let source = r#"
            schema "example"
            version 1
            error_type "self"
            struct S {
                h: hash64 "hash"
                i: id64 "id"
            }
        "#;
        let schema = parse(source).expect("should parse");
        match &schema.items[0] {
            Item::Struct(s) => {
                assert_eq!(s.fields[0].ty, TypeRef::Hash64);
                assert_eq!(s.fields[1].ty, TypeRef::Id64);
            }
            other => panic!("unexpected item {other:?}"),
        }
    }

    #[test]
    fn vec_using_reuses_an_earlier_integer_field_as_its_count() {
        let source = r#"
            schema "example"
            version 1
            error_type "self"
            struct Manifest {
                entry_count: u32 "count"
                paths: vec_using(entry_count, 65536) str16(255) "paths"
            }
        "#;
        let schema = parse(source).expect("should parse");
        match &schema.items[0] {
            Item::Struct(s) => match &s.fields[1].ty {
                TypeRef::VecUsing {
                    count_field, max, ..
                } => {
                    assert_eq!(count_field, "entry_count");
                    assert_eq!(*max, 65536);
                }
                other => panic!("unexpected type {other:?}"),
            },
            other => panic!("unexpected item {other:?}"),
        }
    }

    #[test]
    fn vec_using_rejects_an_unknown_count_field() {
        let source = r#"
            schema "example"
            version 1
            error_type "self"
            struct Manifest {
                paths: vec_using(missing_field, 65536) str16(255) "paths"
            }
        "#;
        let error = parse(source).unwrap_err();
        assert!(error.message.contains("missing_field"), "{error}");
    }

    #[test]
    fn vec_using_rejects_a_non_integer_count_field() {
        let source = r#"
            schema "example"
            version 1
            error_type "self"
            struct Manifest {
                name: str(64) "not an integer"
                paths: vec_using(name, 65536) str16(255) "paths"
            }
        "#;
        let error = parse(source).unwrap_err();
        assert!(error.message.contains("not an unsigned integer"), "{error}");
    }

    #[test]
    fn hex_and_underscore_number_literals() {
        let source = r#"
            schema "example"
            version 1
            error_type "self"
            struct S { x: bytes(8_388_608) "big" }
            message M = 0x0020 dir tool_to_engine payload S
        "#;
        let schema = parse(source).expect("should parse");
        assert_eq!(schema.messages[0].id, 0x0020);
        match &schema.items[0] {
            Item::Struct(s) => match &s.fields[0].ty {
                TypeRef::Bytes { max } => assert_eq!(*max, 8_388_608),
                other => panic!("unexpected type {other:?}"),
            },
            other => panic!("unexpected item {other:?}"),
        }
    }
}
