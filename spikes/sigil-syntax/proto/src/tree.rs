//! Generic value tree with spans and node paths, and a serde deserializer over it.
//!
//! The sigil 1 front end lowers its lossless syntax tree into this tree; serde derive on the
//! shared model then does the schema pass. Every error carries the span and node path of the
//! value that caused it, which is what the diagnostics need.

use crate::diag::{self, Diag, Phase};
use crate::model;
use serde::de::{self, DeserializeSeed, Visitor};
use std::cell::Cell;
use std::fmt;
use std::rc::Rc;

/// Magic newtype name used to hand raw override text through serde.
pub const RAW_TOKEN: &str = "$sigil::raw";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub kind: NodeKind,
    /// Value span; for named items (bullet, emitter) the span of the name.
    pub span: Span,
    pub path: String,
    /// Source text of leaf values.
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub key: String,
    pub key_span: Span,
    pub value: Node,
}

#[derive(Debug, Clone)]
pub enum NodeKind {
    Int(i64),
    Float(f64),
    Qty {
        num: f64,
        int: bool,
        unit: String,
    },
    Str(String),
    Ref(String),
    Trigger {
        kind: String,
        kind_span: Span,
        arg: Box<Node>,
    },
    List(Vec<Node>),
    Record {
        tag: Option<(String, Span)>,
        label: String,
        fields: Vec<Field>,
    },
    /// Raw override value: "<byte offset>\0<source text>".
    Raw(String),
}

/// Raw override value of the sigil 1 front end (source text plus its byte offset in the file).
#[derive(Debug, Clone, PartialEq)]
pub struct SigilRaw {
    pub text: String,
    pub offset: usize,
}

impl<'de> de::Deserialize<'de> for SigilRaw {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = SigilRaw;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a raw sigil value")
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<SigilRaw, E> {
                let (o, t) = s
                    .split_once('\0')
                    .ok_or_else(|| E::custom("malformed raw value"))?;
                Ok(SigilRaw {
                    text: t.to_string(),
                    offset: o.parse().map_err(E::custom)?,
                })
            }
        }
        d.deserialize_newtype_struct(RAW_TOKEN, V)
    }
}

// ------------------------------------------------------------------------------------------
// Paths
// ------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Seg {
    Key(String),
    Index(usize),
}

pub fn split_path(path: &str) -> Vec<Seg> {
    let mut segs = Vec::new();
    for part in path.split('.') {
        let mut rest = part;
        if let Some(i) = rest.find('[') {
            if i > 0 {
                segs.push(Seg::Key(rest[..i].to_string()));
            }
            rest = &rest[i..];
            while let Some(stripped) = rest.strip_prefix('[') {
                let close = stripped.find(']').unwrap_or(stripped.len());
                if let Ok(n) = stripped[..close].parse() {
                    segs.push(Seg::Index(n));
                }
                rest = stripped.get(close + 1..).unwrap_or("");
            }
        } else if !rest.is_empty() {
            segs.push(Seg::Key(rest.to_string()));
        }
    }
    segs
}

pub fn join_path(segs: &[Seg]) -> String {
    let mut s = String::new();
    for seg in segs {
        match seg {
            Seg::Key(k) => {
                if !s.is_empty() {
                    s.push('.');
                }
                s.push_str(k);
            }
            Seg::Index(i) => s.push_str(&format!("[{i}]")),
        }
    }
    s
}

fn name_of(node: &Node) -> Option<&str> {
    if let NodeKind::Record { fields, .. } = &node.kind {
        for f in fields {
            if f.key == "name" {
                if let NodeKind::Ref(s) | NodeKind::Str(s) = &f.value.kind {
                    return Some(s);
                }
            }
        }
    }
    None
}

/// Resolves a node path (see `corpus/grammar/sigil-1.ebnf`) to a node.
pub fn locate<'a>(root: &'a Node, path: &str) -> Option<&'a Node> {
    walk(root, &split_path(path))
}

fn walk<'a>(node: &'a Node, segs: &[Seg]) -> Option<&'a Node> {
    let Some(first) = segs.first() else {
        return Some(node);
    };
    match (&node.kind, first) {
        (NodeKind::Record { fields, .. }, Seg::Key(k)) => {
            if let Some(f) = fields.iter().find(|f| &f.key == k) {
                return walk(&f.value, &segs[1..]);
            }
            // Overrides of `emitter x from a.b` are addressed with the inherited paths.
            let ov = fields.iter().find(|f| f.key == "overrides")?;
            let key = join_path(segs);
            if let NodeKind::Record { fields, .. } = &ov.value.kind {
                return fields.iter().find(|f| f.key == key).map(|f| &f.value);
            }
            None
        }
        (NodeKind::List(items), Seg::Key(k)) => {
            let item = items.iter().find(|it| name_of(it) == Some(k.as_str()))?;
            walk(item, &segs[1..])
        }
        (NodeKind::List(items), Seg::Index(i)) => walk(items.get(*i)?, &segs[1..]),
        _ => None,
    }
}

// ------------------------------------------------------------------------------------------
// Errors
// ------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum DeKind {
    Custom(String),
    InvalidType {
        found: String,
        expected: String,
    },
    UnknownField {
        found: String,
        expected: &'static [&'static str],
    },
    UnknownVariant {
        found: String,
        expected: &'static [&'static str],
    },
    MissingField(&'static str),
    DuplicateField(&'static str),
    WrongUnit {
        expected: &'static str,
        found: String,
    },
    MissingUnit {
        expected: &'static str,
        found: String,
    },
    UnexpectedUnit {
        found: String,
    },
    ReservedUnit {
        found: String,
    },
}

#[derive(Debug, Clone)]
pub struct DeError {
    pub kind: DeKind,
    pub span: Option<Span>,
    pub path: Option<String>,
    pub owner: Option<String>,
}

impl DeError {
    fn new(kind: DeKind) -> Self {
        DeError {
            kind,
            span: None,
            path: None,
            owner: None,
        }
    }
}

impl fmt::Display for DeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.kind)
    }
}

impl std::error::Error for DeError {}

impl de::Error for DeError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        DeError::new(DeKind::Custom(msg.to_string()))
    }
    fn invalid_type(unexp: de::Unexpected, exp: &dyn de::Expected) -> Self {
        DeError::new(DeKind::InvalidType {
            found: unexp.to_string(),
            expected: exp.to_string(),
        })
    }
    fn invalid_value(unexp: de::Unexpected, exp: &dyn de::Expected) -> Self {
        Self::invalid_type(unexp, exp)
    }
    fn unknown_variant(variant: &str, expected: &'static [&'static str]) -> Self {
        DeError::new(DeKind::UnknownVariant {
            found: variant.to_string(),
            expected,
        })
    }
    fn unknown_field(field: &str, expected: &'static [&'static str]) -> Self {
        DeError::new(DeKind::UnknownField {
            found: field.to_string(),
            expected,
        })
    }
    fn missing_field(field: &'static str) -> Self {
        DeError::new(DeKind::MissingField(field))
    }
    fn duplicate_field(field: &'static str) -> Self {
        DeError::new(DeKind::DuplicateField(field))
    }
}

trait Attach<T> {
    fn at(self, span: Span, path: &str, owner: Option<&str>) -> Result<T, DeError>;
}

impl<T> Attach<T> for Result<T, DeError> {
    fn at(self, span: Span, path: &str, owner: Option<&str>) -> Result<T, DeError> {
        self.map_err(|mut e| {
            if e.span.is_none() {
                e.span = Some(span);
                e.path = Some(path.to_string());
                if e.owner.is_none() {
                    e.owner = owner.map(str::to_string);
                }
            }
            e
        })
    }
}

fn child_path(parent: &str, key: &str) -> String {
    if parent.is_empty() {
        key.to_string()
    } else {
        format!("{parent}.{key}")
    }
}

// ------------------------------------------------------------------------------------------
// Deserializer
// ------------------------------------------------------------------------------------------

pub struct NodeDe<'a> {
    pub node: &'a Node,
}

impl<'a> NodeDe<'a> {
    fn number<'de, V: Visitor<'de>>(self, v: V) -> Result<V::Value, DeError> {
        if let NodeKind::Qty { .. } = &self.node.kind {
            return Err(DeError::new(DeKind::UnexpectedUnit {
                found: self.node.text.clone(),
            }))
            .at(self.node.span, &self.node.path, None);
        }
        de::Deserializer::deserialize_any(self, v)
    }
}

macro_rules! numbers {
    ($($m:ident)*) => {
        $(fn $m<V: Visitor<'de>>(self, v: V) -> Result<V::Value, DeError> { self.number(v) })*
    };
}

impl<'de, 'a> de::Deserializer<'de> for NodeDe<'a> {
    type Error = DeError;

    fn deserialize_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value, DeError> {
        let n = self.node;
        let r = match &n.kind {
            NodeKind::Int(i) => v.visit_i64(*i),
            NodeKind::Float(f) => v.visit_f64(*f),
            NodeKind::Qty { .. } => {
                let what = format!("quantity `{}`", n.text);
                Err(de::Error::invalid_type(de::Unexpected::Other(&what), &v))
            }
            NodeKind::Str(s) | NodeKind::Ref(s) | NodeKind::Raw(s) => v.visit_str(s),
            NodeKind::List(items) => v.visit_seq(SeqDe {
                items: items.iter(),
            }),
            NodeKind::Record { fields, label, .. } => v.visit_map(MapDe {
                fields: fields.iter(),
                pending: None,
                owner: n,
                label,
                last_key: Rc::new(Cell::new(n.span)),
            }),
            NodeKind::Trigger { .. } => {
                let what = format!("trigger `{}`", n.text);
                Err(de::Error::invalid_type(de::Unexpected::Other(&what), &v))
            }
        };
        r.at(n.span, &n.path, None)
    }

    numbers!(deserialize_i8 deserialize_i16 deserialize_i32 deserialize_i64 deserialize_u8
        deserialize_u16 deserialize_u32 deserialize_u64 deserialize_f32 deserialize_f64);

    fn deserialize_option<V: Visitor<'de>>(self, v: V) -> Result<V::Value, DeError> {
        v.visit_some(self)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        v: V,
    ) -> Result<V::Value, DeError> {
        let n = self.node;
        if name == RAW_TOKEN {
            return self.deserialize_any(v);
        }
        if let Some(unit) = model::unit_of_newtype(name) {
            let r = match &n.kind {
                NodeKind::Qty { num, int, unit: u } if u == unit => v.visit_newtype_struct(NumDe {
                    num: *num,
                    int: *int,
                }),
                NodeKind::Qty { unit: u, .. } if u == "beats" => {
                    Err(DeError::new(DeKind::ReservedUnit {
                        found: n.text.clone(),
                    }))
                }
                NodeKind::Qty { .. } => Err(DeError::new(DeKind::WrongUnit {
                    expected: unit,
                    found: n.text.clone(),
                })),
                NodeKind::Int(_) | NodeKind::Float(_) => Err(DeError::new(DeKind::MissingUnit {
                    expected: unit,
                    found: n.text.clone(),
                })),
                _ => return self.deserialize_any(v),
            };
            return r.at(n.span, &n.path, None);
        }
        v.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, v: V) -> Result<V::Value, DeError> {
        self.deserialize_any(v)
    }

    fn deserialize_map<V: Visitor<'de>>(self, v: V) -> Result<V::Value, DeError> {
        self.deserialize_any(v)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, DeError> {
        let n = self.node;
        match &n.kind {
            NodeKind::Record { fields, label, .. } => {
                let last_key = Rc::new(Cell::new(n.span));
                let map = MapDe {
                    fields: fields.iter(),
                    pending: None,
                    owner: n,
                    label,
                    last_key: last_key.clone(),
                };
                v.visit_map(map).map_err(|mut e| {
                    if e.span.is_none() {
                        e.span = Some(match e.kind {
                            DeKind::DuplicateField(_) => last_key.get(),
                            _ => n.span,
                        });
                        e.path = Some(n.path.clone());
                        e.owner = Some(label.clone());
                    }
                    e
                })
            }
            _ => self.deserialize_any(v),
        }
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        _variants: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, DeError> {
        let n = self.node;
        let what = diag::snake_case(name);
        let (variant, vspan, content) = match &n.kind {
            NodeKind::Record {
                tag: Some((t, ts)), ..
            } => (diag::pascal_case(t), *ts, Content::Struct(n)),
            NodeKind::Ref(r) => (diag::pascal_case(r), n.span, Content::Unit),
            NodeKind::Int(_) if name == "Repeat" => {
                ("Times".to_string(), n.span, Content::Newtype(n))
            }
            NodeKind::Trigger {
                kind,
                kind_span,
                arg,
            } => (diag::pascal_case(kind), *kind_span, Content::Newtype(arg)),
            _ => return self.deserialize_any(v),
        };
        let access = EnumDe {
            variant,
            vspan,
            content,
            path: n.path.clone(),
            what: what.clone(),
        };
        v.visit_enum(access).at(n.span, &n.path, Some(&what))
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value, DeError> {
        v.visit_unit()
    }

    serde::forward_to_deserialize_any! {
        bool char str string bytes byte_buf unit unit_struct tuple tuple_struct identifier
        i128 u128
    }
}

struct NumDe {
    num: f64,
    int: bool,
}

impl<'de> de::Deserializer<'de> for NumDe {
    type Error = DeError;
    fn deserialize_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value, DeError> {
        if self.int {
            v.visit_i64(self.num as i64)
        } else {
            v.visit_f64(self.num)
        }
    }
    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf
        option unit unit_struct newtype_struct seq tuple tuple_struct map struct enum
        identifier ignored_any
    }
}

struct IdentDe<'b>(&'b str);

impl<'de, 'b> de::Deserializer<'de> for IdentDe<'b> {
    type Error = DeError;
    fn deserialize_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value, DeError> {
        v.visit_str(self.0)
    }
    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf
        option unit unit_struct newtype_struct seq tuple tuple_struct map struct enum
        identifier ignored_any
    }
}

struct SeqDe<'a> {
    items: std::slice::Iter<'a, Node>,
}

impl<'de, 'a> de::SeqAccess<'de> for SeqDe<'a> {
    type Error = DeError;
    fn next_element_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, DeError> {
        match self.items.next() {
            Some(node) => seed.deserialize(NodeDe { node }).map(Some),
            None => Ok(None),
        }
    }
}

struct MapDe<'a> {
    fields: std::slice::Iter<'a, Field>,
    pending: Option<&'a Field>,
    owner: &'a Node,
    label: &'a str,
    last_key: Rc<Cell<Span>>,
}

impl<'de, 'a> de::MapAccess<'de> for MapDe<'a> {
    type Error = DeError;
    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, DeError> {
        let Some(f) = self.fields.next() else {
            return Ok(None);
        };
        self.last_key.set(f.key_span);
        self.pending = Some(f);
        let path = child_path(&self.owner.path, &f.key);
        seed.deserialize(IdentDe(&f.key))
            .map(Some)
            .at(f.key_span, &path, Some(self.label))
    }
    fn next_value_seed<S: DeserializeSeed<'de>>(&mut self, seed: S) -> Result<S::Value, DeError> {
        let f = self.pending.take().expect("value without key");
        seed.deserialize(NodeDe { node: &f.value })
    }
}

enum Content<'a> {
    Unit,
    Newtype(&'a Node),
    Struct(&'a Node),
}

struct EnumDe<'a> {
    variant: String,
    vspan: Span,
    content: Content<'a>,
    path: String,
    what: String,
}

impl<'de, 'a> de::EnumAccess<'de> for EnumDe<'a> {
    type Error = DeError;
    type Variant = VariantDe<'a>;
    fn variant_seed<S: DeserializeSeed<'de>>(
        self,
        seed: S,
    ) -> Result<(S::Value, VariantDe<'a>), DeError> {
        let val = seed.deserialize(IdentDe(&self.variant)).at(
            self.vspan,
            &self.path,
            Some(&self.what),
        )?;
        Ok((
            val,
            VariantDe {
                content: self.content,
                variant: self.variant,
                span: self.vspan,
                path: self.path,
            },
        ))
    }
}

struct VariantDe<'a> {
    content: Content<'a>,
    variant: String,
    span: Span,
    path: String,
}

impl<'de, 'a> de::VariantAccess<'de> for VariantDe<'a> {
    type Error = DeError;
    fn unit_variant(self) -> Result<(), DeError> {
        match self.content {
            Content::Unit => Ok(()),
            _ => Err(de::Error::custom(format!(
                "`{}` takes no value or body",
                diag::snake_case(&self.variant)
            )))
            .at(self.span, &self.path, None),
        }
    }
    fn newtype_variant_seed<S: DeserializeSeed<'de>>(self, seed: S) -> Result<S::Value, DeError> {
        match self.content {
            Content::Newtype(node) => seed.deserialize(NodeDe { node }),
            _ => Err(de::Error::custom(format!(
                "`{}` needs a value, e.g. `time 30t`",
                diag::snake_case(&self.variant)
            )))
            .at(self.span, &self.path, None),
        }
    }
    fn tuple_variant<V: Visitor<'de>>(self, _len: usize, _v: V) -> Result<V::Value, DeError> {
        Err(de::Error::custom("tuple variants are not used in sigil 1"))
            .at(self.span, &self.path, None)
    }
    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, DeError> {
        match self.content {
            Content::Struct(node) => {
                de::Deserializer::deserialize_struct(NodeDe { node }, "", fields, v)
            }
            _ => Err(de::Error::custom(format!(
                "`{}` needs a body `{{ ... }}`",
                diag::snake_case(&self.variant)
            )))
            .at(self.span, &self.path, None),
        }
    }
}

// ------------------------------------------------------------------------------------------
// Error -> diagnostic (sigil 1 wording)
// ------------------------------------------------------------------------------------------

fn unit_desc(unit: &str) -> &'static str {
    match unit {
        "t" => "a duration in ticks",
        "deg" => "an angle",
        "u" => "a distance",
        "u/t" => "a speed",
        "u/t2" => "an acceleration",
        "deg/t" => "a turn rate",
        _ => "a quantity",
    }
}

fn found_unit_desc(text: &str) -> &'static str {
    let unit = text.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '-');
    match unit {
        "t" => "tick",
        "deg" => "angle",
        "u" => "distance",
        "u/t" => "speed",
        "u/t2" => "acceleration",
        "deg/t" => "turn-rate",
        _ => "unit",
    }
}

fn number_part(text: &str) -> &str {
    let end = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .unwrap_or(text.len());
    &text[..end]
}

fn last_field(path: &str) -> String {
    match split_path(path).into_iter().rev().find_map(|s| match s {
        Seg::Key(k) => Some(k),
        Seg::Index(_) => None,
    }) {
        Some(k) => k,
        None => path.to_string(),
    }
}

pub fn to_diag(e: &DeError, src: &str) -> Diag {
    let pos = e.span.map(|s| diag::pos_of_offset(src, s.start));
    let path = e.path.clone().unwrap_or_default();
    let owner = e.owner.clone().unwrap_or_else(|| "this item".to_string());
    let field = last_field(&path);
    let d = match &e.kind {
        DeKind::UnknownField { found, expected } => {
            let allowed: Vec<&str> = expected.iter().copied().filter(|f| *f != "name").collect();
            let mut hint = String::new();
            if let Some(m) = diag::did_you_mean(found, allowed.iter().copied()) {
                hint.push_str(&format!("Did you mean `{m}`? "));
            }
            hint.push_str(&format!(
                "Allowed fields of {owner}: {}.",
                allowed.join(", ")
            ));
            Diag::new(
                Phase::Schema,
                "unknown-field",
                format!("Unknown field `{found}` in {owner}."),
            )
            .hint(hint)
            .path(path.clone())
        }
        DeKind::InvalidType { found, expected } => {
            let exp = match expected.as_str() {
                "u8" | "u16" | "u32" | "u64" | "i64" => "an integer".to_string(),
                "f32" | "f64" => "a number".to_string(),
                "a string" | "a borrowed string" => "a name".to_string(),
                other => other.to_string(),
            };
            let found_desc = found.replace("floating point", "the float");
            let hint = if exp == "an integer" && found.starts_with("floating point") {
                let num = found
                    .trim_start_matches("floating point `")
                    .trim_end_matches('`');
                let whole = num.parse::<f64>().map(|f| f.trunc() as i64).unwrap_or(0);
                format!("Use a whole number, e.g. `{field} = {whole}`.")
            } else {
                format!("Write {exp} for `{field}`.")
            };
            Diag::new(
                Phase::Schema,
                "type",
                format!("Field `{field}` expects {exp}, found {found_desc}."),
            )
            .hint(hint)
            .path(path.clone())
        }
        DeKind::WrongUnit { expected, found } => Diag::new(
            Phase::Schema,
            "unit",
            format!(
                "Field `{field}` expects {} in `{expected}`, found the {} quantity `{found}`.",
                unit_desc(expected),
                found_unit_desc(found)
            ),
        )
        .hint(format!(
            "Write `{field} = {}{expected}`.",
            number_part(found)
        ))
        .path(path.clone()),
        DeKind::MissingUnit { expected, found } => Diag::new(
            Phase::Schema,
            "unit",
            format!(
                "Field `{field}` expects {} in `{expected}`, found the plain number `{found}`.",
                unit_desc(expected)
            ),
        )
        .hint(format!("Add the unit: `{field} = {found}{expected}`."))
        .path(path.clone()),
        DeKind::UnexpectedUnit { found } => Diag::new(
            Phase::Schema,
            "unit",
            format!("Field `{field}` expects a plain number, found the quantity `{found}`."),
        )
        .hint(format!(
            "Remove the unit: `{field} = {}`.",
            number_part(found)
        ))
        .path(path.clone()),
        DeKind::ReservedUnit { found } => Diag::new(
            Phase::Schema,
            "unit",
            format!("The unit in `{found}` is reserved: beat time is not available in sigil 1."),
        )
        .hint("Use ticks (`t`) until the beat clock arrives (P2).")
        .path(path.clone()),
        DeKind::MissingField(f) => {
            let full = child_path(&path, f);
            let hint = match model::field_newtype(f).and_then(model::unit_of_newtype) {
                Some(unit) => format!("Add a line `{f} = <value>{unit}`."),
                None => format!("Add a line `{f} = <value>`."),
            };
            let mut owner_cap = owner.clone();
            if let Some(first) = owner_cap.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            Diag::new(
                Phase::Schema,
                "missing-field",
                format!("{owner_cap} is missing the required field `{f}`."),
            )
            .hint(hint)
            .path(full)
        }
        DeKind::DuplicateField(f) => Diag::new(
            Phase::Schema,
            "duplicate-field",
            format!("`{f}` is given twice in {owner}."),
        )
        .hint("Remove one of them; each field and each `block` may appear only once.")
        .path(child_path(&path, f)),
        DeKind::UnknownVariant { found, expected } => {
            let found = diag::snake_case(found);
            let allowed: Vec<String> = expected.iter().map(|v| diag::snake_case(v)).collect();
            let mut hint = String::new();
            if let Some(m) = diag::did_you_mean(&found, allowed.iter().map(String::as_str)) {
                hint.push_str(&format!("Did you mean `{m}`? "));
            }
            hint.push_str(&format!("Allowed: {}.", allowed.join(", ")));
            Diag::new(
                Phase::Schema,
                "unknown-variant",
                format!("Unknown {owner} `{found}`."),
            )
            .hint(hint)
            .path(path.clone())
        }
        DeKind::Custom(msg) => {
            Diag::new(Phase::Schema, "schema", format!("{msg}.")).path(path.clone())
        }
    };
    d.at(pos)
}
