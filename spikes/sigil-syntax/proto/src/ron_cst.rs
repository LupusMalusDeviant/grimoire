//! A small lossless RON scanner. The `ron` crate has no syntax tree with spans and drops
//! comments, so everything position-related on the RON side lives here:
//! - `path_at`: byte offset -> node path (tolerant, works on broken files),
//! - `Tree::locate`: node path -> value span (valid files),
//! - `set`: lossless text edit of one value.
//! This layer is extra code the RON variant needs for diagnostics and `sigilc set`.

use crate::tree::{Seg, join_path, split_path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RK {
    Space,
    Comment,
    Ident,
    Num,
    Str,
    Char,
    Punct,
    Bad,
}

#[derive(Clone, Copy, Debug)]
pub struct RTok {
    pub kind: RK,
    pub start: usize,
    pub end: usize,
}

pub fn tokenize(src: &str) -> Vec<RTok> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let s = i;
        let c = b[i];
        let kind = if c.is_ascii_whitespace() {
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            RK::Space
        } else if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' && b[i] != b'\r' {
                i += 1;
            }
            RK::Comment
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            let mut depth = 0;
            while i < b.len() {
                if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                    depth += 1;
                    i += 2;
                } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            RK::Comment
        } else if c == b'"' {
            i += 1;
            while i < b.len() {
                match b[i] {
                    b'\\' => i += 2,
                    b'"' => {
                        i += 1;
                        break;
                    }
                    _ => i += 1,
                }
            }
            i = i.min(b.len());
            RK::Str
        } else if c == b'\'' {
            i += 1;
            while i < b.len() {
                match b[i] {
                    b'\\' => i += 2,
                    b'\'' => {
                        i += 1;
                        break;
                    }
                    _ => i += 1,
                }
            }
            i = i.min(b.len());
            RK::Char
        } else if c.is_ascii_digit()
            || ((c == b'-' || c == b'+' || c == b'.')
                && b.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            i += 1;
            while i < b.len()
                && (b[i].is_ascii_alphanumeric()
                    || b[i] == b'.'
                    || b[i] == b'_'
                    || ((b[i] == b'+' || b[i] == b'-') && matches!(b[i - 1], b'e' | b'E')))
            {
                i += 1;
            }
            RK::Num
        } else if c.is_ascii_alphabetic() || c == b'_' {
            if c == b'r' && b.get(i + 1) == Some(&b'#') {
                i += 2;
            }
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            RK::Ident
        } else {
            i += src[i..].chars().next().map_or(1, char::len_utf8);
            if b"()[]{}:,#!".contains(&c) {
                RK::Punct
            } else {
                RK::Bad
            }
        };
        out.push(RTok {
            kind,
            start: s,
            end: i,
        });
    }
    out
}

pub fn lossless(src: &str) -> bool {
    tokenize(src)
        .iter()
        .map(|t| &src[t.start..t.end])
        .collect::<String>()
        == src
}

pub fn comment_count(src: &str) -> usize {
    tokenize(src)
        .iter()
        .filter(|t| t.kind == RK::Comment)
        .count()
}

pub fn significant_tokens(src: &str) -> usize {
    tokenize(src)
        .iter()
        .filter(|t| !matches!(t.kind, RK::Space | RK::Comment))
        .count()
}

fn unquote(s: &str) -> String {
    let s = s.strip_prefix('"').unwrap_or(s);
    s.strip_suffix('"').unwrap_or(s).to_string()
}

fn significant(src: &str) -> Vec<RTok> {
    tokenize(src)
        .into_iter()
        .filter(|t| !matches!(t.kind, RK::Space | RK::Comment))
        .collect()
}

/// Index of the first token after leading `#![...]` attributes.
fn attributes_end(src: &str, toks: &[RTok]) -> usize {
    let text = |i: usize| toks.get(i).map(|t| &src[t.start..t.end]).unwrap_or("");
    let mut i = 0;
    while text(i) == "#" {
        i += 1;
        if text(i) == "!" {
            i += 1;
        }
        if text(i) != "[" {
            return i;
        }
        let mut depth = 0;
        while i < toks.len() {
            let t = text(i);
            i += 1;
            if t == "[" {
                depth += 1;
            } else if t == "]" {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
        }
    }
    i
}

/// Node path of the innermost value around `offset`. Works on files that do not parse.
pub fn path_at(src: &str, offset: usize) -> Option<String> {
    enum F {
        Paren {
            key: Option<String>,
            name: Option<String>,
        },
        Seq {
            idx: usize,
        },
        Map {
            key: Option<String>,
        },
    }
    let toks = significant(src);
    let mut st: Vec<F> = Vec::new();
    for i in attributes_end(src, &toks)..toks.len() {
        let t = toks[i];
        let tx = &src[t.start..t.end];
        // A closing bracket at the error position still belongs to the value it closes.
        if t.start > offset || (t.start == offset && matches!(tx, ")" | "]" | "}")) {
            break;
        }
        let next_colon = toks.get(i + 1).is_some_and(|n| &src[n.start..n.end] == ":");
        match (t.kind, tx) {
            (RK::Punct, "(") => st.push(F::Paren {
                key: None,
                name: None,
            }),
            (RK::Punct, "[") => st.push(F::Seq { idx: 0 }),
            (RK::Punct, "{") => st.push(F::Map { key: None }),
            (RK::Punct, ")" | "]" | "}") => {
                st.pop();
            }
            (RK::Punct, ",") => match st.last_mut() {
                Some(F::Seq { idx }) => *idx += 1,
                Some(F::Paren { key, .. }) | Some(F::Map { key }) => *key = None,
                None => {}
            },
            (RK::Ident, _) if next_colon => {
                if let Some(F::Paren { key, .. }) = st.last_mut() {
                    *key = Some(tx.to_string());
                }
            }
            (RK::Str, _) if next_colon => {
                if let Some(F::Map { key }) = st.last_mut() {
                    *key = Some(unquote(tx));
                }
            }
            (RK::Str, _) => {
                if let Some(F::Paren { key: Some(k), name }) = st.last_mut() {
                    if k == "name" {
                        *name = Some(unquote(tx));
                    }
                }
            }
            _ => {}
        }
    }
    let mut out = String::new();
    let push = |out: &mut String, k: &str| {
        if !out.is_empty() {
            out.push('.');
        }
        out.push_str(k);
    };
    for (i, f) in st.iter().enumerate() {
        match f {
            F::Paren { key: Some(k), .. } if k != "overrides" => push(&mut out, k),
            F::Seq { idx } => match st.get(i + 1) {
                Some(F::Paren { name: Some(n), .. }) => push(&mut out, n),
                _ => out.push_str(&format!("[{idx}]")),
            },
            F::Map { key: Some(k) } => push(&mut out, k),
            _ => {}
        }
    }
    (!out.is_empty()).then_some(out)
}

// ------------------------------------------------------------------------------------------
// Tree for valid files
// ------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RNode {
    pub kind: RKind,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone)]
pub enum RKind {
    Scalar,
    Named(String, Option<Body>),
    Unnamed(Body),
    Seq(Vec<RNode>),
    Map(Vec<(RNode, RNode)>),
}

#[derive(Debug, Clone)]
pub enum Body {
    Fields(Vec<RField>),
    Items(Vec<RNode>),
}

#[derive(Debug, Clone)]
pub struct RField {
    pub key: String,
    pub key_start: usize,
    pub value: RNode,
}

pub struct Tree<'a> {
    pub src: &'a str,
    pub root: RNode,
}

struct P<'a> {
    src: &'a str,
    toks: Vec<RTok>,
    i: usize,
}

impl<'a> P<'a> {
    fn peek(&self) -> Option<RTok> {
        self.toks.get(self.i).copied()
    }
    fn text(&self, t: RTok) -> &'a str {
        &self.src[t.start..t.end]
    }
    fn is(&self, s: &str) -> bool {
        self.peek()
            .is_some_and(|t| t.kind == RK::Punct && self.text(t) == s)
    }
    fn close(&mut self, s: &str) -> Option<usize> {
        let t = self.peek()?;
        if t.kind == RK::Punct && self.text(t) == s {
            self.i += 1;
            Some(t.end)
        } else {
            None
        }
    }
    fn value(&mut self) -> Option<RNode> {
        let t = self.peek()?;
        let start = t.start;
        match t.kind {
            RK::Num | RK::Str | RK::Char => {
                self.i += 1;
                Some(RNode {
                    kind: RKind::Scalar,
                    start,
                    end: t.end,
                })
            }
            RK::Ident => {
                self.i += 1;
                let name = self.text(t).to_string();
                if self.is("(") {
                    let (body, end) = self.body()?;
                    Some(RNode {
                        kind: RKind::Named(name, Some(body)),
                        start,
                        end,
                    })
                } else {
                    Some(RNode {
                        kind: RKind::Named(name, None),
                        start,
                        end: t.end,
                    })
                }
            }
            RK::Punct => match self.text(t) {
                "(" => {
                    let (body, end) = self.body()?;
                    Some(RNode {
                        kind: RKind::Unnamed(body),
                        start,
                        end,
                    })
                }
                "[" => {
                    self.i += 1;
                    let mut items = Vec::new();
                    loop {
                        if let Some(end) = self.close("]") {
                            return Some(RNode {
                                kind: RKind::Seq(items),
                                start,
                                end,
                            });
                        }
                        items.push(self.value()?);
                        if self.is(",") {
                            self.i += 1;
                        } else if !self.is("]") {
                            return None;
                        }
                    }
                }
                "{" => {
                    self.i += 1;
                    let mut entries = Vec::new();
                    loop {
                        if let Some(end) = self.close("}") {
                            return Some(RNode {
                                kind: RKind::Map(entries),
                                start,
                                end,
                            });
                        }
                        let k = self.value()?;
                        self.close(":")?;
                        let v = self.value()?;
                        entries.push((k, v));
                        if self.is(",") {
                            self.i += 1;
                        } else if !self.is("}") {
                            return None;
                        }
                    }
                }
                _ => None,
            },
            _ => None,
        }
    }
    fn body(&mut self) -> Option<(Body, usize)> {
        self.i += 1; // "("
        let fields = self.peek().is_some_and(|t| t.kind == RK::Ident)
            && self
                .toks
                .get(self.i + 1)
                .is_some_and(|t| self.text(*t) == ":");
        if fields {
            let mut fs = Vec::new();
            loop {
                if let Some(end) = self.close(")") {
                    return Some((Body::Fields(fs), end));
                }
                let k = self.peek()?;
                if k.kind != RK::Ident {
                    return None;
                }
                self.i += 1;
                self.close(":")?;
                let value = self.value()?;
                fs.push(RField {
                    key: self.text(k).to_string(),
                    key_start: k.start,
                    value,
                });
                if self.is(",") {
                    self.i += 1;
                } else if !self.is(")") {
                    return None;
                }
            }
        } else {
            let mut items = Vec::new();
            loop {
                if let Some(end) = self.close(")") {
                    return Some((Body::Items(items), end));
                }
                items.push(self.value()?);
                if self.is(",") {
                    self.i += 1;
                } else if !self.is(")") {
                    return None;
                }
            }
        }
    }
}

fn fields_of(n: &RNode) -> Option<&Vec<RField>> {
    match &n.kind {
        RKind::Named(_, Some(Body::Fields(f))) | RKind::Unnamed(Body::Fields(f)) => Some(f),
        _ => None,
    }
}

impl<'a> Tree<'a> {
    pub fn parse(src: &'a str) -> Option<Tree<'a>> {
        let toks = significant(src);
        let start = attributes_end(src, &toks);
        let mut p = P {
            src,
            toks,
            i: start,
        };
        let root = p.value()?;
        (p.i == p.toks.len()).then_some(Tree { src, root })
    }

    fn name_node<'n>(&self, n: &'n RNode) -> Option<&'n RNode> {
        fields_of(n)?
            .iter()
            .find(|f| f.key == "name")
            .map(|f| &f.value)
    }

    /// Value node at a node path. A path that ends at a named list element yields its name.
    pub fn locate(&self, path: &str) -> Option<&RNode> {
        self.walk(&self.root, &split_path(path))
    }

    fn walk<'n>(&self, n: &'n RNode, segs: &[Seg]) -> Option<&'n RNode> {
        let Some(first) = segs.first() else {
            return Some(n);
        };
        if let Some(fs) = fields_of(n) {
            let Seg::Key(k) = first else { return None };
            if let Some(f) = fs.iter().find(|f| &f.key == k) {
                return self.walk(&f.value, &segs[1..]);
            }
            let ov = fs.iter().find(|f| f.key == "overrides")?;
            if let RKind::Map(entries) = &ov.value.kind {
                let key = join_path(segs);
                return entries
                    .iter()
                    .find(|(k, _)| unquote(&self.src[k.start..k.end]) == key)
                    .map(|(_, v)| v);
            }
            return None;
        }
        if let RKind::Seq(items) = &n.kind {
            return match first {
                Seg::Index(i) => self.walk(items.get(*i)?, &segs[1..]),
                Seg::Key(k) => {
                    let item = items.iter().find(|it| {
                        self.name_node(it)
                            .is_some_and(|nn| unquote(&self.src[nn.start..nn.end]) == *k)
                    })?;
                    if segs.len() == 1 {
                        self.name_node(item)
                    } else {
                        self.walk(item, &segs[1..])
                    }
                }
            };
        }
        None
    }
}

/// Lossless edit of one value; every other byte (comments, formatting) stays.
pub fn set(src: &str, path: &str, new_text: &str) -> Result<String, String> {
    let tree = Tree::parse(src).ok_or("file does not parse with the RON scanner")?;
    let node = tree
        .locate(path)
        .ok_or_else(|| format!("path `{path}` not found"))?;
    Ok(format!(
        "{}{}{}",
        &src[..node.start],
        new_text,
        &src[node.end..]
    ))
}
