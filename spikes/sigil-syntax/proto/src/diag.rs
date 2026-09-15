//! Diagnostics shared by both front ends, plus a small renderer that shows what a modder sees.

use std::fmt::Write as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// Lexing and parsing (syntax).
    Parse,
    /// Mapping onto the data model: field names, types, units, variants.
    Schema,
    /// Semantic checks on the model: ranges, references, uniqueness, readability.
    Validate,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Parse => "parse",
            Phase::Schema => "schema",
            Phase::Validate => "validate",
        }
    }
}

/// 1-based line and column (columns count Unicode scalar values).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Related {
    pub label: String,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diag {
    pub phase: Phase,
    /// Short machine-readable kind for counting (e.g. `unknown-field`, `range`).
    pub kind: &'static str,
    pub pos: Option<Pos>,
    pub node_path: Option<String>,
    /// Where the position and path came from, when not produced by the parser itself.
    pub cause: String,
    pub hint: Option<String>,
    pub related: Option<Related>,
    /// The verbatim message of the underlying library, if any (RON: `ron::error::Error` text).
    pub raw: Option<String>,
}

impl Diag {
    pub fn new(phase: Phase, kind: &'static str, cause: impl Into<String>) -> Self {
        Diag {
            phase,
            kind,
            pos: None,
            node_path: None,
            cause: cause.into(),
            hint: None,
            related: None,
            raw: None,
        }
    }
    pub fn at(mut self, pos: Option<Pos>) -> Self {
        self.pos = pos;
        self
    }
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.node_path = Some(path.into());
        self
    }
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
    pub fn related(mut self, label: impl Into<String>, pos: Pos) -> Self {
        self.related = Some(Related {
            label: label.into(),
            pos,
        });
        self
    }
}

/// Byte offset -> 1-based line/column (columns in chars).
pub fn pos_of_offset(src: &str, offset: usize) -> Pos {
    let offset = offset.min(src.len());
    let before = &src[..offset];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = src[line_start..offset].chars().count() + 1;
    Pos { line, col }
}

/// 1-based line/column -> byte offset.
pub fn offset_of_pos(src: &str, pos: Pos) -> usize {
    let mut line_start = 0;
    for _ in 1..pos.line {
        match src[line_start..].find('\n') {
            Some(i) => line_start += i + 1,
            None => return src.len(),
        }
    }
    src[line_start..]
        .char_indices()
        .nth(pos.col.saturating_sub(1))
        .map(|(i, _)| line_start + i)
        .unwrap_or(src.len())
}

/// Renders a diagnostic the way a modder would see it in a terminal.
pub fn render(file: &str, src: &str, d: &Diag) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "error[{}/{}]: {}", d.phase.as_str(), d.kind, d.cause);
    match d.pos {
        Some(p) => {
            let _ = writeln!(out, "  --> {}:{}:{}", file, p.line, p.col);
            let text = src.lines().nth(p.line - 1).unwrap_or("");
            let w = p.line.to_string().len();
            let _ = writeln!(out, "{:w$} |", "");
            let _ = writeln!(out, "{} | {}", p.line, text);
            let _ = writeln!(out, "{:w$} | {}^", "", " ".repeat(p.col.saturating_sub(1)));
        }
        None => {
            let _ = writeln!(out, "  --> {} (no position)", file);
        }
    }
    if let Some(r) = &d.related {
        let _ = writeln!(out, "  = note: {} at {}:{}", r.label, r.pos.line, r.pos.col);
    }
    if let Some(p) = &d.node_path {
        let _ = writeln!(out, "  = path: {}", p);
    }
    if let Some(h) = &d.hint {
        let _ = writeln!(out, "  = help: {}", h);
    }
    out
}

pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Closest candidate within an edit distance of 2 (or a pure prefix/suffix extension).
pub fn did_you_mean<'a>(
    found: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    candidates
        .into_iter()
        .map(|c| (levenshtein(found, c), c))
        .filter(|(d, c)| *d <= 2 || c.starts_with(found) || found.starts_with(c))
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

pub fn snake_case(pascal: &str) -> String {
    let mut s = String::new();
    for (i, ch) in pascal.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                s.push('_');
            }
            s.push(ch.to_ascii_lowercase());
        } else {
            s.push(ch);
        }
    }
    s
}

pub fn pascal_case(snake: &str) -> String {
    snake
        .split('_')
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}
