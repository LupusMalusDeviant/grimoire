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
    d
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
