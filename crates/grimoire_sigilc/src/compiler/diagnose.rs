//! Small shared helpers for building [`Diagnostic`]s in the `compiler` module, continuing the
//! stable code sequence from `SIG0012` (`docs/formats/sigil.md`'s diagnostic-code table; contract
//! ADR-0007). Each code's meaning is documented there, not repeated per call site here.

use crate::diagnostics::Diagnostic;
use crate::span::Position;

/// Builds one error diagnostic. A thin wrapper over [`Diagnostic::new`] so every call site in
/// this module reads the same way `crate::parser::Parser::error` does, without needing `&mut
/// self` (schema/resolution/validation code is mostly free functions over a
/// [`crate::compiler::resolve::Workspace`], not one long-lived parser struct).
#[allow(clippy::too_many_arguments)]
pub(crate) fn diagnostic(
    code: &'static str,
    file: &str,
    position: Position,
    node_path: impl Into<String>,
    token: impl Into<String>,
    message: impl Into<String>,
    fix_hint: impl Into<String>,
) -> Diagnostic {
    Diagnostic::new(code, file, position, node_path, token, message, fix_hint)
}

/// Placeholder position for a diagnostic that is not about one specific source location (e.g. an
/// entire file that failed to load, or a cross-file cycle that has no single "the" position).
#[must_use]
pub(crate) fn unresolved_position() -> Position {
    Position { line: 1, column: 1 }
}
