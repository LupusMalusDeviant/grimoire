//! Byte spans and 1-based source positions used by the Sigil syntax tree and its diagnostics.
//!
//! Engine ADR-0007 ("Sigil-Quelltextsyntax v1") requires every syntax-tree node to carry a
//! source span and every diagnostic to name a line and column; the ADR also fixes that columns
//! count Unicode scalar values (`char`s), not UTF-8 bytes, starting at 1.

/// A half-open byte range `[start, end)` into a source file's UTF-8 bytes.
///
/// Byte offsets (not `char` counts) so a [`Span`] can slice the original `&str` directly; line
/// and column, where needed for a human, live in [`Position`] instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Span {
    /// Inclusive start offset, in bytes from the start of the file.
    pub start: u32,
    /// Exclusive end offset, in bytes from the start of the file.
    pub end: u32,
}

impl Span {
    /// Builds a span from a start and end byte offset. `end` is clamped up to `start` if it
    /// would otherwise precede it, so a [`Span`] is never inverted.
    #[must_use]
    pub fn new(start: u32, end: u32) -> Self {
        Self {
            start,
            end: end.max(start),
        }
    }

    /// An empty span at a single offset, used for diagnostics anchored at end-of-file.
    #[must_use]
    pub fn empty_at(offset: u32) -> Self {
        Self {
            start: offset,
            end: offset,
        }
    }

    /// Length in bytes.
    #[must_use]
    pub fn len(&self) -> u32 {
        self.end - self.start
    }

    /// Whether this span covers zero bytes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// The smallest span covering both `self` and `other`.
    #[must_use]
    pub fn join(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

/// A 1-based line/column position. Columns count Unicode scalar values (ADR-0007's lexical
/// grammar preamble), so multi-byte UTF-8 characters still advance the column by exactly 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Position {
    /// 1-based line number.
    pub line: u32,
    /// 1-based column number, counted in Unicode scalar values.
    pub column: u32,
}

impl Position {
    /// The position of the first character of a file.
    #[must_use]
    pub fn start_of_file() -> Self {
        Self { line: 1, column: 1 }
    }
}
