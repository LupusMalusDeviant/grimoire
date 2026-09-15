//! RON front end: `ron` 0.12.2 + serde derive on the shared model. Positions and messages
//! come from `ron`; node paths from the scanner in `ron_cst`; fix hints from the structured
//! error codes of `ron` (thin layer written for the spike).

use crate::check::Locator;
use crate::diag::{self, Diag, Phase, Pos};
use crate::model::{self, Sigil};
use crate::ron_cst;
use ron::error::{Error as E, SpannedError};
use ron::value::RawValue;

pub type RonSigil = Sigil<Box<RawValue>>;

pub fn parse(src: &str) -> Result<RonSigil, Diag> {
    ron::Options::default()
        .from_str::<RonSigil>(src)
        .map_err(|e| from_spanned(src, &e))
}

/// The verbatim `ron` error of a file and the position `ron` reports, without any adapter.
pub fn raw_error(src: &str) -> Option<(String, Pos)> {
    ron::Options::default()
        .from_str::<RonSigil>(src)
        .err()
        .map(|e| {
            (
                e.code.to_string(),
                Pos {
                    line: e.span.start.line,
                    col: e.span.start.col,
                },
            )
        })
}

pub fn from_spanned(src: &str, e: &SpannedError) -> Diag {
    let pos = Pos {
        line: e.span.start.line,
        col: e.span.start.col,
    };
    let offset = diag::offset_of_pos(src, pos);
    let path = ron_cst::path_at(src, offset);
    let mut d = from_code(&e.code);
    d.pos = Some(pos);
    d.node_path = path;
    match &e.code {
        E::MissingStructField { field, outer } => {
            relocate_missing_field(src, &mut d, field, outer.as_deref());
        }
        E::ExpectedDifferentStructName { expected, found } => {
            name_unit_field(src, offset, &mut d, expected, found);
        }
        _ => {}
    }
    d
}

/// `ron` reports a missing field at the end of its struct. The scanner already knows the owner
/// (path of the struct), so point at the owner instead (for a named list element: its `name`
/// value), extend the path by the field and name the owner in cause and hint.
fn relocate_missing_field(src: &str, d: &mut Diag, field: &str, outer: Option<&str>) {
    let Some(owner) = d.node_path.clone() else {
        return;
    };
    let Some(tree) = ron_cst::Tree::parse(src) else {
        return;
    };
    let Some(node) = tree.locate(&owner) else {
        return;
    };
    let text = &src[node.start..node.end];
    let outer = outer.unwrap_or("struct");
    let who = match node.kind {
        ron_cst::RKind::Scalar if text.starts_with('"') => {
            format!("{outer} `{}`", text.trim_matches('"'))
        }
        _ => format!("`{outer}` at `{owner}`"),
    };
    let value = match model::field_newtype(field) {
        Some(nt) => format!("{nt}(<value>)"),
        None => "<value>".to_string(),
    };
    if let Some(ron_pos) = d.pos {
        d.related = Some(diag::Related {
            label: "`ron` reports the end of the struct".to_string(),
            pos: ron_pos,
        });
    }
    d.pos = Some(diag::pos_of_offset(src, node.start));
    d.node_path = Some(format!("{owner}.{field}"));
    d.cause = format!("{who} is missing the required field `{field}`.");
    d.hint = Some(format!("Add `{field}: {value},` to {who}."));
}

/// `ron` names both newtypes but not the field. The path from the scanner does, and the value
/// inside the wrong newtype can be carried over.
fn name_unit_field(src: &str, offset: usize, d: &mut Diag, expected: &str, found: &str) {
    let Some(field) = d
        .node_path
        .as_deref()
        .and_then(|p| p.rsplit('.').next())
        .filter(|f| !f.contains('['))
        .map(str::to_string)
    else {
        return;
    };
    d.cause = format!("Field `{field}` expects `{expected}(..)`, found `{found}(..)`.");
    let rest = &src[offset.min(src.len())..];
    let inner = rest
        .strip_prefix(found)
        .map(str::trim_start)
        .and_then(|r| r.strip_prefix('('))
        .and_then(|r| r.split_once(')'))
        .map(|(v, _)| v.trim())
        .filter(|v| !v.is_empty() && v.parse::<f64>().is_ok());
    d.hint = Some(match inner {
        Some(v) if expected != "Ticks" && !v.contains(['.', 'e', 'E']) => {
            format!("Write `{field}: {expected}({v}.0),`.")
        }
        Some(v) => format!("Write `{field}: {expected}({v}),`."),
        None => format!("Write `{field}: {expected}(..)` instead of `{found}(..)`."),
    });
}

/// Maps a `ron` error code to a diagnostic. `cause` is the verbatim `ron` message.
pub fn from_code(code: &E) -> Diag {
    let raw = code.to_string();
    let (phase, kind, hint): (Phase, &'static str, Option<String>) = match code {
        E::NoSuchStructField {
            expected,
            found,
            outer,
        } => {
            let mut h = String::new();
            if let Some(m) = diag::did_you_mean(found, expected.iter().copied()) {
                h.push_str(&format!("Did you mean `{m}`? "));
            }
            let outer = outer.clone().unwrap_or_default();
            h.push_str(&format!(
                "Allowed fields of `{outer}`: {}.",
                expected.join(", ")
            ));
            (Phase::Schema, "unknown-field", Some(h))
        }
        E::MissingStructField { field, outer } => {
            let outer = outer.clone().unwrap_or_default();
            let value = match model::field_newtype(field) {
                Some(nt) => format!("{nt}(<value>)"),
                None => "<value>".to_string(),
            };
            (
                Phase::Schema,
                "missing-field",
                Some(format!("Add `{field}: {value},` to `{outer}`.")),
            )
        }
        E::DuplicateStructField { field, .. } => (
            Phase::Schema,
            "duplicate-field",
            Some(format!("Remove one of the two `{field}` entries.")),
        ),
        E::NoSuchEnumVariant {
            expected, found, ..
        } => {
            let mut h = String::new();
            if let Some(m) = diag::did_you_mean(found, expected.iter().copied()) {
                h.push_str(&format!("Did you mean `{m}`? "));
            }
            h.push_str(&format!("Allowed: {}.", expected.join(", ")));
            (Phase::Schema, "unknown-variant", Some(h))
        }
        E::ExpectedDifferentStructName { expected, found } => (
            Phase::Schema,
            "unit",
            Some(format!("Write `{expected}(..)` instead of `{found}(..)`.")),
        ),
        E::InvalidValueForType { expected, .. } => (
            Phase::Schema,
            "type",
            Some(format!("Write {expected} here.")),
        ),
        E::ExpectedInteger | E::IntegerOutOfBounds => (
            Phase::Schema,
            "type",
            Some("Write a whole number here.".to_string()),
        ),
        E::ExpectedFloat
        | E::ExpectedString
        | E::ExpectedBoolean
        | E::ExpectedNamedStructLike(_) => (Phase::Schema, "type", None),
        E::ExpectedDifferentLength { .. } | E::ExpectedStructName(_) | E::Message(_) => {
            (Phase::Schema, "schema", None)
        }
        _ => (Phase::Parse, "syntax", None),
    };
    let mut d = Diag::new(phase, kind, format!("{raw}."));
    d.hint = hint;
    d.raw = Some(raw);
    d
}

pub struct RonLoc<'a> {
    pub tree: Option<ron_cst::Tree<'a>>,
}

impl<'a> RonLoc<'a> {
    pub fn new(src: &'a str) -> Self {
        RonLoc {
            tree: ron_cst::Tree::parse(src),
        }
    }
}

impl Locator for RonLoc<'_> {
    fn locate(&self, path: &str) -> Option<Pos> {
        let tree = self.tree.as_ref()?;
        let node = tree.locate(path)?;
        Some(diag::pos_of_offset(tree.src, node.start))
    }
}
