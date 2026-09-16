//! Diagnostics for the Sigil lexer and parser (Plan 0002 WP4.1).
//!
//! Every diagnostic names a file, a 1-based line and column, a node path, a cause, a fix hint
//! and a stable code (engine ADR-0007). Codes are documented in `docs/formats/sigil.md`'s
//! diagnostic-code table and are stable from their first release: a code, once shipped, keeps
//! its meaning, even if the wording of `message`/`fix_hint` is later improved.
//!
//! [`Diagnostic`] renders as human-readable text ([`Diagnostic::render_text`]) or as part of a
//! [`DiagnosticsDocument`] serialised to JSON (contract §2 rule 11: `sigilc --json` is one of the
//! named JSON interfaces, so its documents carry `schema_version` and keep line/column as JSON
//! numbers well under 2^53).

use serde::Serialize;

use crate::span::Position;

/// Schema version of [`DiagnosticsDocument`]'s JSON shape (contract §2 rule 11). Bump this,
/// additively if possible, whenever a field is added or a meaning changes.
pub const DIAGNOSTICS_SCHEMA_VERSION: u32 = 1;

/// Severity of a [`Diagnostic`].
///
/// WP4.1's lexer and parser only ever raise [`Severity::Error`] (every code in the table is a
/// condition that stops the file from compiling); the variant exists now because the schema pass
/// (Plan 0002 WP4.2) needs warnings (e.g. deprecated syntax during a migration window) and a
/// later addition here is additive, not a breaking rename of an already-shipped shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Severity {
    /// The file does not compile as written.
    Error,
    /// The file compiles, but the construct is discouraged or scheduled for removal.
    Warning,
}

/// A secondary location a [`Diagnostic`] points to in addition to its primary one, e.g. the
/// opening brace an "unclosed body" diagnostic's primary location is missing a match for.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct RelatedLocation {
    /// Short label explaining what this location is, e.g. `"opening brace"`.
    pub label: String,
    /// 1-based line number.
    pub line: u32,
    /// 1-based column number (Unicode scalar values).
    pub column: u32,
    /// The exact source text at this location.
    pub token: String,
}

impl RelatedLocation {
    /// Builds a related location.
    #[must_use]
    pub fn new(label: impl Into<String>, position: Position, token: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            line: position.line,
            column: position.column,
            token: token.into(),
        }
    }
}

/// One diagnostic: a file, a 1-based line and column, a node path, a cause, a fix hint and a
/// stable code (engine ADR-0007).
///
/// `#[non_exhaustive]` (contract §2 rule 13): this struct has grown once already while WP4.1 was
/// implemented (`related` was added after the first draft) and will grow again once the schema
/// pass (WP4.2) needs e.g. a list of allowed alternatives. Build one with [`Diagnostic::new`],
/// not a struct literal.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Diagnostic {
    /// Stable diagnostic code, e.g. `"SIG0007"`. See `docs/formats/sigil.md` for the full table.
    pub code: &'static str,
    /// Always [`Severity::Error`] for the lexer and parser (see the type's docs).
    pub severity: Severity,
    /// The file this diagnostic belongs to, exactly as the caller of [`crate::parser::parse`]
    /// named it (a corpus test passes a corpus-relative name; the CLI passes through whatever
    /// path the user typed), so two runs against the same logical file always agree byte-for-byte
    /// regardless of where the repository happens to be checked out.
    pub file: String,
    /// 1-based line number of the primary location.
    pub line: u32,
    /// 1-based column number of the primary location (Unicode scalar values).
    pub column: u32,
    /// Dotted/indexed node path of the primary location, following the convention in
    /// `docs/formats/sigil.md` (`version`, `meta`, `bullets.<name>`, `emitters.<name>.block.count`,
    /// `emitters.<name>.modifiers[<i>].<field>`, ...). Empty only when no item has been entered
    /// yet (a diagnostic about the header itself uses the literal path `"version"`).
    pub node_path: String,
    /// The exact source text at the primary location, for a text renderer to underline or a
    /// tool to match against.
    pub token: String,
    /// Human- and agent-readable cause of the diagnostic (English, contract §2 rule 1).
    pub message: String,
    /// A concrete suggested fix, where one exists.
    pub fix_hint: String,
    /// A secondary location, e.g. the opening delimiter an "unclosed" diagnostic's primary
    /// location has no match for.
    pub related: Option<RelatedLocation>,
}

impl Diagnostic {
    /// Builds a diagnostic with no related location. Use [`Diagnostic::with_related`] to attach
    /// one.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        code: &'static str,
        file: impl Into<String>,
        position: Position,
        node_path: impl Into<String>,
        token: impl Into<String>,
        message: impl Into<String>,
        fix_hint: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: Severity::Error,
            file: file.into(),
            line: position.line,
            column: position.column,
            node_path: node_path.into(),
            token: token.into(),
            message: message.into(),
            fix_hint: fix_hint.into(),
            related: None,
        }
    }

    /// Attaches a related secondary location, replacing any previous one.
    #[must_use]
    pub fn with_related(mut self, related: RelatedLocation) -> Self {
        self.related = Some(related);
        self
    }

    /// Renders this diagnostic as a single human-readable line plus, when present, one line for
    /// the related location: `<file>:<line>:<column>: error[<code>]: <message>` followed by
    /// `  at <node_path>` (when non-empty), `  fix: <fix_hint>` (when non-empty) and `  related:
    /// <label> at <file>:<line>:<column>: <token>` (when present).
    #[must_use]
    pub fn render_text(&self) -> String {
        let severity = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        let mut out = format!(
            "{file}:{line}:{column}: {severity}[{code}]: {message}",
            file = self.file,
            line = self.line,
            column = self.column,
            severity = severity,
            code = self.code,
            message = self.message,
        );
        if !self.node_path.is_empty() {
            out.push_str(&format!("\n  at {}", self.node_path));
        }
        if !self.fix_hint.is_empty() {
            out.push_str(&format!("\n  fix: {}", self.fix_hint));
        }
        if let Some(related) = &self.related {
            out.push_str(&format!(
                "\n  related: {label} at {file}:{line}:{column}: `{token}`",
                label = related.label,
                file = self.file,
                line = related.line,
                column = related.column,
                token = related.token,
            ));
        }
        out
    }
}

/// The JSON document `sigilc parse --json` writes for one file (contract §2 rule 11): a
/// `schema_version`, the file name and every diagnostic collected for it, in the order they were
/// raised.
///
/// `#[non_exhaustive]`: a later field (e.g. a top-level `ok: bool` convenience, or timing) is an
/// additive change under the vertrag's change protocol (§2b), not a breaking one, as long as
/// existing fields keep their meaning.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct DiagnosticsDocument {
    /// Always [`DIAGNOSTICS_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The file these diagnostics belong to (see [`Diagnostic::file`]).
    pub file: String,
    /// Every diagnostic collected while parsing `file`, in the order they were raised. Empty for
    /// a file with no parse errors.
    pub diagnostics: Vec<Diagnostic>,
}

impl DiagnosticsDocument {
    /// Builds a diagnostics document for `file` from a finished list of diagnostics.
    #[must_use]
    pub fn new(file: impl Into<String>, diagnostics: Vec<Diagnostic>) -> Self {
        Self {
            schema_version: DIAGNOSTICS_SCHEMA_VERSION,
            file: file.into(),
            diagnostics,
        }
    }

    /// Serialises this document as pretty-printed JSON, UTF-8, no BOM (contract §2 rule 11).
    /// `serde_json`'s struct serialisation preserves declaration order (rule 11's "objects keys
    /// written in fixed order"), so no explicit key ordering is needed here.
    ///
    /// # Errors
    /// Returns `serde_json`'s error if a `String` field somehow contains data `serde_json`
    /// cannot encode; in practice this never happens for the plain UTF-8 strings this crate
    /// builds diagnostics from.
    pub fn to_json_pretty(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }
}
