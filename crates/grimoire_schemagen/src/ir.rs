//! Intermediate representation (IR) of a parsed schema description (project ADR-0011).
//!
//! [`parser::parse`](crate::parser::parse) turns schema source text into a [`Schema`]; the Rust
//! emitter ([`crate::emit_rust`]) and the docs emitter ([`crate::emit_docs`]) each turn a
//! [`Schema`] into their own output, and never look at the source text. A future C# emitter
//! (Plan-0002 WP9.1) is meant to be the same shape: a third module that consumes [`Schema`]
//! without changing the parser or the other emitters, which is why the IR carries no detail that
//! is specific to one target language.

/// One parsed schema description: the message/struct/enum vocabulary of a single wire or file
/// format (contract §12 or §13), plus enough metadata for every emitter to work from the IR
/// alone.
#[derive(Clone, Debug)]
pub struct Schema {
    /// Short schema name (e.g. `debug_protocol_v1`), used to derive output file names and the
    /// docs page title.
    pub name: String,
    /// Format or protocol version this schema describes (documentation only; the emitted code
    /// never redefines the crate's own version constant, to avoid a second source of truth).
    pub version: u32,
    /// One-paragraph description of the format, copied verbatim into generated doc comments and
    /// the docs table header.
    pub doc: Vec<String>,
    /// Repository-relative path of the schema source file, for a traceability comment in every
    /// generated file (deliberately not a timestamp or tool version: both would make the "is
    /// generated code current" CI check platform- or run-dependent, project ADR-0011 "Negativ").
    pub source_path: String,
    /// How the emitted `encode`/`decode` functions report errors (contract §2 rule 9).
    pub error_type: ErrorType,
    /// Enums and structs, in declaration order (never re-sorted: contract §2 rule 10 forbids
    /// depending on the iteration order of an unordered container, and preserving source order
    /// also keeps regenerated diffs small when a schema file only grows).
    pub items: Vec<Item>,
    /// The message catalogue (contract §13 "Nachrichtenkatalog"); empty for schemas that only
    /// describe a file format (e.g. the pack manifest) rather than a message stream.
    pub messages: Vec<MessageEntry>,
}

/// Where the encode/decode error type of a schema's emitted code comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorType {
    /// Emit a local, self-contained error enum named `<Schema>Error` in the generated file.
    /// Used by schemas whose generated code is not (yet) wired into the crate's own error type.
    SelfContained,
    /// Reuse an existing error type at this Rust path (e.g. `crate::frame::ProtocolError`). The
    /// referenced type must already declare the five rule-9 variants every emitter assumes:
    /// `UnexpectedEnd { offset, needed, available }`, `TrailingBytes { extra }`,
    /// `InvalidUtf8 { field }`, `FieldTooLong { field, len, max }` and
    /// `InvalidEnum { field, value }` (field names and shapes exactly as listed, contract §13).
    /// If a schema using this error type also declares a [`TypeRef::VecUsing`] field, the
    /// referenced type must additionally declare a sixth variant,
    /// `CountMismatch { field: &'static str, count_field: &'static str, declared: usize, actual: usize }`
    /// (see [`crate::emit_rust`]'s `VecUsing` handling); no schema with an external error type
    /// declares one today, so this requirement is not yet exercised.
    External(String),
}

/// One top-level declaration: either an enum or a struct.
#[derive(Clone, Debug)]
pub enum Item {
    /// An enum declaration.
    Enum(EnumDef),
    /// A struct declaration.
    Struct(StructDef),
}

impl Item {
    /// The declared name, regardless of item kind.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Item::Enum(e) => &e.name,
            Item::Struct(s) => &s.name,
        }
    }
}

/// The wire width of an enum's discriminant (contract §13: "Enums als `u8`, außer explizit anders
/// angegeben").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnumWidth {
    /// One byte on the wire.
    U8,
    /// Two bytes on the wire, little-endian.
    U16,
}

/// One `enum` declaration: a closed set of named, explicitly numbered variants.
#[derive(Clone, Debug)]
pub struct EnumDef {
    /// Rust-facing enum name (`PascalCase` in every schema file in this repo).
    pub name: String,
    /// Wire width of the discriminant.
    pub width: EnumWidth,
    /// `true` emits `#[non_exhaustive]` on the generated enum (contract §2 rule 13: a growable
    /// public enum that may gain variants later, e.g. `ErrorCode`).
    pub non_exhaustive: bool,
    /// If set, a wire discriminant that matches no known variant decodes to this variant instead
    /// of failing with `InvalidEnum` (contract §13: "`ErrorCode` ... unbekannter Code ->
    /// `Malformed`" — an `Error` message must stay decodable even when it carries a code from a
    /// newer protocol version, since `Error`'s id and layout are frozen across all versions).
    /// Must name a variant declared in this same enum; [`crate::parser::parse`] rejects a
    /// `fallback` naming an undeclared variant as a schema-parse error, so a typo here is caught
    /// at schema-parse time rather than surfacing as a Rust compile error in the generated file.
    pub fallback: Option<String>,
    /// Doc comment lines for the enum itself.
    pub doc: Vec<String>,
    /// Variants, in declaration order (also the order the docs table renders them in).
    pub variants: Vec<EnumVariant>,
}

/// One variant of an [`EnumDef`], with its explicit wire discriminant.
#[derive(Clone, Debug)]
pub struct EnumVariant {
    /// Rust-facing variant name.
    pub name: String,
    /// Explicit wire discriminant (never inferred: contract §13 lists an explicit numeric value
    /// for every `ErrorCode`/`PeerRole` variant, and a schema without one is a schema bug).
    pub value: u32,
    /// Doc comment lines for the variant.
    pub doc: Vec<String>,
}

/// One `struct` declaration: an ordered list of named, typed fields with their own wire codec.
#[derive(Clone, Debug)]
pub struct StructDef {
    /// Rust-facing struct name.
    pub name: String,
    /// `true` emits `#[non_exhaustive]` plus a `#[derive(Default)]` on the generated struct
    /// (contract §2 rule 13 and §13: `Stats`, `StatsScope` and `SwapAck` are built outside
    /// `grimoire_debug` via `Default` and field assignment because they are expected to grow
    /// fields later, e.g. `Stats` with Plan-0002 WP6.3).
    pub growable: bool,
    /// Doc comment lines for the struct itself.
    pub doc: Vec<String>,
    /// Fields, in wire order (the order fields are encoded and decoded in).
    pub fields: Vec<FieldDef>,
}

/// One field of a [`StructDef`].
#[derive(Clone, Debug)]
pub struct FieldDef {
    /// Rust-facing field name (`snake_case`).
    pub name: String,
    /// Wire type.
    pub ty: TypeRef,
    /// Doc comment lines for the field.
    pub doc: Vec<String>,
}

/// A field or element wire type, per the schema type grammar (see [`crate::parser`]).
///
/// This is the full type vocabulary the schema language supports; not every emitter need support
/// every variant (the Rust emitter supports all of them, since every payload and manifest type in
/// this repo's two schemas needs them), but adding a target language that cannot represent, say,
/// [`TypeRef::Hash64`] natively only affects that one emitter, never this enum or the parser.
#[derive(Clone, Debug, PartialEq)]
pub enum TypeRef {
    /// Unsigned 8-bit integer.
    U8,
    /// Unsigned 16-bit integer, little-endian on the wire.
    U16,
    /// Unsigned 32-bit integer, little-endian on the wire.
    U32,
    /// Unsigned 64-bit integer, little-endian on the wire.
    U64,
    /// IEEE-754 single-precision float, bit-exact little-endian on the wire (contract §13).
    F32,
    /// Boolean, encoded as one `u8` (`0`/`1`; any other byte is `InvalidEnum`, contract §13).
    Bool,
    /// Semantic alias of `u64` reserved for a content/state hash (contract §2 rule 11's `hash64`
    /// JSON convention). Binary layout is identical to [`TypeRef::U64`]; only a future JSON
    /// emitter (Plan-0002 WP9.2) treats it differently (16 lowercase hex digits, never a JSON
    /// number). Unused by the two schemas in this PR, kept for that later emitter.
    Hash64,
    /// Semantic alias of `u64` reserved for a stable content id (contract §2 rule 11's `id64`
    /// convention). Same note as [`TypeRef::Hash64`].
    Id64,
    /// `Str≤max`: a `u32` length prefix followed by that many bytes of UTF-8, at most `max` bytes
    /// (contract §13).
    Str {
        /// Maximum length in bytes (checked before allocating, contract §2 rule 9).
        max: u32,
    },
    /// `Str16≤max`: a `u16` length prefix followed by that many bytes of UTF-8, at most `max`
    /// bytes (contract §12).
    Str16 {
        /// Maximum length in bytes.
        max: u32,
    },
    /// `Bytes≤max`: a `u32` length prefix followed by that many raw bytes (contract §13).
    Bytes {
        /// Maximum length in bytes.
        max: u32,
    },
    /// `Vec≤max<T>`: a `u32` count prefix followed by that many `elem`-typed values, at most `max`
    /// elements (contract §13).
    Vec {
        /// Maximum element count.
        max: u32,
        /// Element type.
        elem: Box<TypeRef>,
    },
    /// `Vec≤max<T>` whose element count is **not** wire-encoded separately: it is the value of an
    /// earlier `u8`/`u16`/`u32`/`u64` field of the same struct (contract §12's manifest: "je
    /// Eintrag in TOC-Reihenfolge der Pfad" reuses the manifest's own `entry_count`, rather than
    /// writing a second, redundant count next to the path list). The bytes remaining are still
    /// checked against `max` before allocating (contract §2 rule 9), exactly like [`Self::Vec`];
    /// only where the count comes from differs.
    VecUsing {
        /// Name of the earlier field in the same struct whose already-decoded value is this
        /// vector's element count.
        count_field: String,
        /// Maximum element count.
        max: u32,
        /// Element type.
        elem: Box<TypeRef>,
    },
    /// `Option<T>`: a `u8` tag (`0` = absent, `1` = present) followed by `inner` if present
    /// (contract §13). Any tag byte other than `0`/`1` is `InvalidEnum`.
    Option(Box<TypeRef>),
    /// `[T; len]`: a fixed-size array with no length prefix on the wire (contract §13, e.g.
    /// `[u8; 32]`, `Option<[f32; 2]>`).
    Array {
        /// Element type.
        elem: Box<TypeRef>,
        /// Fixed element count.
        len: u32,
    },
    /// Reference to an [`EnumDef`] or [`StructDef`] declared elsewhere in the same schema.
    /// [`crate::parser::parse`] resolves and validates every reference before returning a
    /// [`Schema`], so an emitter can assume the name exists and look it up by name.
    Named(String),
}

/// Direction a message travels in the debug protocol (contract §13's "Richtung" column: T =
/// tool, E = engine).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Tool to engine only (`T→E`).
    ToolToEngine,
    /// Engine to tool only (`E→T`).
    EngineToTool,
    /// Both directions (`T↔E`).
    Both,
}

impl Direction {
    /// Contract-style short label (`T→E`, `E→T`, `T↔E`) for the docs table.
    #[must_use]
    pub fn contract_label(self) -> &'static str {
        match self {
            Direction::ToolToEngine => "T→E",
            Direction::EngineToTool => "E→T",
            Direction::Both => "T↔E",
        }
    }
}

/// One row of the message catalogue (contract §13): a stable numeric id, its direction, and the
/// payload struct it carries. Building the actual `Message` enum, its `id()`/`to_frame`/
/// `from_frame` dispatch and the handshake that negotiates it is WP8.2, not this crate: a
/// [`MessageEntry`] only records the catalogue metadata the docs table and a future WP8.2 need,
/// so that catalogue stays generated from the same source as the payload types instead of being
/// hand-copied out of the contract a second time.
#[derive(Clone, Debug)]
pub struct MessageEntry {
    /// Message name (matches the payload struct name for every message in this repo's schemas,
    /// except `Error`, whose payload struct is `ErrorMsg` because `Error` collides with
    /// `std::error::Error`-adjacent naming conventions — contract §13 already names the struct
    /// `ErrorMsg` for exactly this reason).
    pub name: String,
    /// Wire message id (contract §13 "Nachrichtenkatalog" `ID` column).
    pub id: u32,
    /// Direction the message travels in.
    pub direction: Direction,
    /// Name of the [`StructDef`] carrying this message's payload.
    pub payload: String,
    /// Doc comment lines (typically a short restatement of the catalogue row).
    pub doc: Vec<String>,
}
