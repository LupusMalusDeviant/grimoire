//! Pipeline per file (parse -> schema -> validate) and the shared semantic validation.

use crate::diag::{self, Diag, Phase, Pos};
use crate::model::*;
use crate::ron_front::{self, RonLoc};
use crate::sigil;
use crate::tree::{self, Node, NodeDe, SigilRaw};
use ron::value::RawValue;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const BEHAVIOURS: &[&str] = &["orbit_parent", "seek_target_weak"];
pub const EVENTS: &[&str] = &["phase_end", "player_parry"];
pub const MAX_CASCADE_DEPTH: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    Ron,
    Sigil,
}

impl Syntax {
    pub fn of(path: &Path) -> Syntax {
        match path.extension().and_then(|e| e.to_str()) {
            Some("ron") => Syntax::Ron,
            _ => Syntax::Sigil,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Syntax::Ron => "RON",
            Syntax::Sigil => "sigil 1",
        }
    }
}

pub trait Locator {
    fn locate(&self, path: &str) -> Option<Pos>;
}

struct SigilLoc<'a> {
    root: &'a Node,
    src: &'a str,
}

impl Locator for SigilLoc<'_> {
    fn locate(&self, path: &str) -> Option<Pos> {
        tree::locate(self.root, path).map(|n| diag::pos_of_offset(self.src, n.span.start))
    }
}

/// Raw override value of composition, decoded against the target field type.
pub trait RawOverride {
    fn decode<T: DeserializeOwned>(&self, src: &str, path: &str) -> Result<T, Diag>;
}

impl RawOverride for Box<RawValue> {
    fn decode<T: DeserializeOwned>(&self, _src: &str, path: &str) -> Result<T, Diag> {
        self.into_rust::<T>().map_err(|e| {
            let mut d = ron_front::from_code(&e.code);
            d.node_path = Some(path.to_string());
            d
        })
    }
}

impl RawOverride for SigilRaw {
    fn decode<T: DeserializeOwned>(&self, src: &str, path: &str) -> Result<T, Diag> {
        let node =
            sigil::parse_value_at(src, self.offset, self.text.len(), path).ok_or_else(|| {
                Diag::new(
                    Phase::Parse,
                    "override",
                    format!("Cannot parse the override value `{}`.", self.text),
                )
                .path(path)
            })?;
        T::deserialize(NodeDe { node: &node }).map_err(|e| tree::to_diag(&e, src))
    }
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub syntax: Syntax,
    pub src: String,
    pub diags: Vec<Diag>,
    pub resolved: Option<Resolved>,
}

pub fn check_file(path: &Path) -> Outcome {
    check_file_depth(path, 0)
}

/// Like `check_file`, but resolves imports relative to `dir` (error copies of a pattern live
/// in another directory than the files they import).
pub fn check_file_in(path: &Path, dir: &Path) -> Outcome {
    let syntax = Syntax::of(path);
    let src = std::fs::read_to_string(path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    check_src(syntax, src, dir, 0)
}

fn check_file_depth(path: &Path, depth: usize) -> Outcome {
    let syntax = Syntax::of(path);
    let src = std::fs::read_to_string(path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    let dir = path.parent().unwrap_or(Path::new("."));
    check_src(syntax, src, dir, depth)
}

pub fn check_src(syntax: Syntax, src: String, dir: &Path, depth: usize) -> Outcome {
    let mut diags = Vec::new();
    let mut resolved = None;
    match syntax {
        Syntax::Sigil => {
            let (cst, parse_diags) = sigil::parse(&src);
            let fatal = parse_diags
                .iter()
                .any(|d| d.kind == "version" || d.kind == "header");
            diags.extend(parse_diags);
            if !fatal {
                let (root, lower_diags) = sigil::lower(&src, &cst);
                diags.extend(lower_diags);
                match Sigil::<SigilRaw>::deserialize(NodeDe { node: &root }) {
                    Err(e) => diags.push(tree::to_diag(&e, &src)),
                    Ok(model) => {
                        let loc = SigilLoc {
                            root: &root,
                            src: &src,
                        };
                        let (vd, res) = validate(&model, &loc, &src, dir, syntax, depth);
                        let clean = diags.is_empty() && vd.is_empty();
                        diags.extend(vd);
                        if clean {
                            resolved = res;
                        }
                    }
                }
            }
        }
        Syntax::Ron => match ron_front::parse(&src) {
            Err(d) => diags.push(d),
            Ok(model) => {
                let loc = RonLoc::new(&src);
                let (vd, res) = validate(&model, &loc, &src, dir, syntax, depth);
                let clean = vd.is_empty();
                diags.extend(vd);
                if clean {
                    resolved = res;
                }
            }
        },
    }
    Outcome {
        syntax,
        src,
        diags,
        resolved,
    }
}

// ------------------------------------------------------------------------------------------
// Validation
// ------------------------------------------------------------------------------------------

struct V<'a> {
    diags: Vec<Diag>,
    loc: &'a dyn Locator,
    syn: Syntax,
}

impl V<'_> {
    fn push(&mut self, kind: &'static str, path: &str, cause: String, hint: String) -> &mut Diag {
        let d = Diag::new(Phase::Validate, kind, cause)
            .at(self.loc.locate(path))
            .path(path)
            .hint(hint);
        self.diags.push(d);
        self.diags.last_mut().expect("just pushed")
    }
    /// Syntax-specific example text.
    fn ex(&self, ron: String, sigil: String) -> String {
        match self.syn {
            Syntax::Ron => ron,
            Syntax::Sigil => sigil,
        }
    }
    fn kind(&self, pascal: &str) -> String {
        match self.syn {
            Syntax::Ron => pascal.to_string(),
            Syntax::Sigil => diag::snake_case(pascal),
        }
    }
    fn range_f(&mut self, path: &str, what: &str, v: f32, lo: f32, hi: f32, lo_open: bool) {
        let ok = if lo_open {
            v > lo && v <= hi
        } else {
            v >= lo && v <= hi
        };
        if !ok {
            let range = if lo_open {
                format!("> {lo} and <= {hi}")
            } else {
                format!("{lo}..={hi}")
            };
            let field = path.rsplit('.').next().unwrap_or(path).to_string();
            self.push(
                "range",
                path,
                format!("{what} must be {range}, found {v}."),
                format!("Choose a value for `{field}` within {range}."),
            );
        }
    }
    fn count(&mut self, path: &str, block: &str, v: u16) {
        if !(1..=512).contains(&v) {
            let field = path.rsplit('.').next().unwrap_or("count").to_string();
            let extra = if v == 0 {
                "; a block of 0 bullets fires nothing and has no defined angle step"
            } else {
                ""
            };
            let hint = self.ex(
                format!("Use 1 to 512 bullets, e.g. `{field}: 24,`."),
                format!("Use 1 to 512 bullets, e.g. `{field} = 24`."),
            );
            let kind = self.kind(block);
            self.push(
                "range",
                path,
                format!("`{field}` of block `{kind}` must be in 1..=512, found {v}{extra}."),
                hint,
            );
        }
    }
    fn reference(
        &mut self,
        kind: &'static str,
        path: &str,
        what: &str,
        name: &str,
        known: &BTreeSet<String>,
    ) {
        if !known.contains(name) {
            let mut hint = String::new();
            if let Some(m) = diag::did_you_mean(name, known.iter().map(String::as_str)) {
                hint.push_str(&format!("Did you mean `{m}`? "));
            }
            let list: Vec<&str> = known.iter().map(String::as_str).collect();
            hint.push_str(&format!("Known {what}s: {}.", list.join(", ")));
            self.push(kind, path, format!("Unknown {what} `{name}`."), hint);
        }
    }

    fn block(&mut self, path: &str, b: &Block) {
        match b {
            Block::Ring { count, .. } => self.count(&format!("{path}.count"), "Ring", *count),
            Block::Spiral { arms, .. } => self.count(&format!("{path}.arms"), "Spiral", *arms),
            Block::Fan { count, spread, .. } => {
                self.count(&format!("{path}.count"), "Fan", *count);
                self.range_f(
                    &format!("{path}.spread"),
                    "`spread`",
                    spread.0,
                    0.0,
                    360.0,
                    false,
                );
            }
            Block::Aimed { count, spread } => {
                self.count(&format!("{path}.count"), "Aimed", *count);
                self.range_f(
                    &format!("{path}.spread"),
                    "`spread`",
                    spread.0,
                    0.0,
                    360.0,
                    false,
                );
            }
            Block::Wave {
                count,
                amplitude,
                wavelength,
                ..
            } => {
                self.count(&format!("{path}.count"), "Wave", *count);
                self.range_f(
                    &format!("{path}.amplitude"),
                    "`amplitude`",
                    amplitude.0,
                    0.0,
                    16.0,
                    true,
                );
                self.range_f(
                    &format!("{path}.wavelength"),
                    "`wavelength`",
                    wavelength.0,
                    0.0,
                    64.0,
                    true,
                );
            }
            Block::Line { count, spacing, .. } => {
                self.count(&format!("{path}.count"), "Line", *count);
                self.range_f(
                    &format!("{path}.spacing"),
                    "`spacing`",
                    spacing.0,
                    0.0,
                    16.0,
                    true,
                );
            }
            Block::Scatter {
                count,
                cone,
                speed_jitter,
                ..
            } => {
                self.count(&format!("{path}.count"), "Scatter", *count);
                self.range_f(&format!("{path}.cone"), "`cone`", cone.0, 0.0, 360.0, false);
                self.range_f(
                    &format!("{path}.speed_jitter"),
                    "`speed_jitter`",
                    *speed_jitter,
                    0.0,
                    1.0,
                    false,
                );
            }
        }
    }
}

fn apply_override<R: RawOverride>(
    e: &mut Emitter,
    key: &str,
    raw: &R,
    src: &str,
    path: &str,
) -> Result<(), Diag> {
    match key {
        "delay" => e.delay = raw.decode(src, path)?,
        "interval" => e.interval = raw.decode(src, path)?,
        "repeat" => e.repeat = raw.decode(src, path)?,
        "speed" => e.speed = raw.decode(src, path)?,
        "bullet" => e.bullet = raw.decode(src, path)?,
        "role" => e.role = raw.decode(src, path)?,
        "offset.x" => e.offset.x = raw.decode(src, path)?,
        "offset.y" => e.offset.y = raw.decode(src, path)?,
        k if k.starts_with("block.") => {
            let f = &k["block.".len()..];
            macro_rules! set {
                ($t:expr) => {
                    raw.decode(src, path).map(|v| *$t = v)
                };
            }
            use Block::*;
            let r = match (&mut e.block, f) {
                (
                    Ring { count, .. }
                    | Fan { count, .. }
                    | Aimed { count, .. }
                    | Wave { count, .. }
                    | Line { count, .. }
                    | Scatter { count, .. },
                    "count",
                ) => set!(count),
                (Spiral { arms, .. }, "arms") => set!(arms),
                (Ring { start, .. } | Spiral { start, .. }, "start") => set!(start),
                (Spiral { step, .. }, "step") => set!(step),
                (Fan { spread, .. } | Aimed { spread, .. }, "spread") => set!(spread),
                (Fan { center, .. }, "center") => set!(center),
                (Wave { amplitude, .. }, "amplitude") => set!(amplitude),
                (Wave { wavelength, .. }, "wavelength") => set!(wavelength),
                (
                    Wave { direction, .. } | Line { direction, .. } | Scatter { direction, .. },
                    "direction",
                ) => {
                    set!(direction)
                }
                (Line { spacing, .. }, "spacing") => set!(spacing),
                (Scatter { cone, .. }, "cone") => set!(cone),
                (Scatter { speed_jitter, .. }, "speed_jitter") => set!(speed_jitter),
                (Scatter { seed, .. }, "seed") => set!(seed),
                _ => {
                    return Err(Diag::new(
                        Phase::Validate,
                        "unknown-override",
                        format!("The inherited block has no field `{f}`."),
                    )
                    .path(path));
                }
            };
            r?;
        }
        _ => {
            return Err(Diag::new(
                Phase::Validate,
                "unknown-override",
                format!("`{key}` cannot be overridden."),
            )
            .hint("Overridable: delay, interval, repeat, speed, bullet, role, offset.x, offset.y, block.<field>.")
            .path(path));
        }
    }
    Ok(())
}

pub fn validate<R: RawOverride>(
    m: &Sigil<R>,
    loc: &dyn Locator,
    src: &str,
    dir: &Path,
    syn: Syntax,
    depth: usize,
) -> (Vec<Diag>, Option<Resolved>) {
    let mut v = V {
        diags: Vec::new(),
        loc,
        syn,
    };
    if m.version != 1 {
        let hint = v.ex(
            "Set `version: 1,` or migrate the file with a newer `sigilc`.".into(),
            "Change the header to `sigil 1`.".into(),
        );
        v.push(
            "version",
            "version",
            format!(
                "Unsupported Sigil version {}; this compiler reads version 1.",
                m.version
            ),
            hint,
        );
        return (v.diags, None);
    }
    if !(1..=20000).contains(&m.meta.density) {
        v.push(
            "range",
            "meta.density",
            format!("`density` must be in 1..=20000, found {}.", m.meta.density),
            "Estimate the bullets on screen at peak.".into(),
        );
    }
    if m.meta.difficulty.is_empty() {
        v.push(
            "empty",
            "meta.difficulty",
            "`difficulty` needs at least one tag.".into(),
            "Add e.g. `normal`.".into(),
        );
    }

    // Imports
    let mut imported: BTreeMap<String, Resolved> = BTreeMap::new();
    for imp in &m.imports {
        let p = dir.join(&imp.path);
        let path = format!("imports.{}.path", imp.alias);
        if depth > 4 {
            v.push(
                "import-cycle",
                &path,
                "Imports nest deeper than 4 levels (cycle?).".into(),
                "Remove the cyclic import.".into(),
            );
        } else if !p.exists() {
            v.push(
                "unknown-import",
                &path,
                format!("Import file `{}` not found.", imp.path),
                "Paths are relative to the importing file.".into(),
            );
        } else {
            let out = check_file_depth(&p, depth + 1);
            match out.resolved {
                Some(r) if out.diags.is_empty() => {
                    imported.insert(imp.alias.clone(), r);
                }
                _ => {
                    v.push(
                        "import-errors",
                        &path,
                        format!(
                            "Imported file `{}` has {} error(s).",
                            imp.path,
                            out.diags.len()
                        ),
                        "Fix the imported file first.".into(),
                    );
                }
            }
        }
    }

    // Bullets
    let mut bullet_names: BTreeSet<String> = BTreeSet::new();
    let mut first_bullet: BTreeMap<&str, usize> = BTreeMap::new();
    let mut silhouettes: BTreeMap<String, String> = BTreeMap::new();
    for unit in imported.values() {
        for b in &unit.bullets {
            bullet_names.insert(b.name.clone());
            silhouettes
                .entry(b.silhouette.clone())
                .or_insert_with(|| b.name.clone());
        }
    }
    for (i, b) in m.bullets.iter().enumerate() {
        let path = format!("bullets.{}", b.name);
        if let Some(j) = first_bullet.get(b.name.as_str()).copied() {
            let first = v.loc.locate(&format!("bullets[{j}].name"));
            let d = v.push(
                "duplicate-name",
                &format!("bullets[{i}].name"),
                format!(
                    "Duplicate bullet name `{}`; first defined at line {}.",
                    b.name,
                    first.map_or(0, |p| p.line)
                ),
                "Rename one of the bullet types; names must be unique within a unit.".into(),
            );
            if let Some(p) = first {
                d.related = Some(diag::Related {
                    label: "first definition".into(),
                    pos: p,
                });
            }
            continue;
        }
        first_bullet.insert(&b.name, i);
        bullet_names.insert(b.name.clone());
        if !b.palette.starts_with("enemy.") {
            v.push(
                "palette",
                &format!("{path}.palette"),
                format!(
                    "Palette `{}` is outside the enemy palette space `enemy.*`.",
                    b.palette
                ),
                "Enemy bullets use palettes `enemy.*` (readability rule 4).".into(),
            );
        }
        v.range_f(&format!("{path}.glow"), "`glow`", b.glow, 0.0, 1.0, false);
        v.range_f(
            &format!("{path}.radius"),
            "`radius`",
            b.radius.0,
            0.0,
            8.0,
            true,
        );
        if b.damage > 100 {
            v.push(
                "range",
                &format!("{path}.damage"),
                format!("`damage` must be in 0..=100, found {}.", b.damage),
                "Choose a smaller damage.".into(),
            );
        }
        let mut seen = BTreeSet::new();
        for (k, f) in b.flags.iter().enumerate() {
            if !seen.insert(*f) {
                v.push(
                    "duplicate-flag",
                    &format!("{path}.flags[{k}]"),
                    format!("Flag `{}` is given twice.", v.kind(&format!("{f:?}"))),
                    "Remove the duplicate flag.".into(),
                );
            }
        }
        if let Some(beh) = &b.behaviour {
            if !BEHAVIOURS.contains(&beh.as_str()) {
                let mut hint = String::new();
                if let Some(mm) = diag::did_you_mean(beh, BEHAVIOURS.iter().copied()) {
                    hint.push_str(&format!("Did you mean `{mm}`? "));
                }
                hint.push_str("New behaviours must be registered in Rust first (FR-10).");
                v.push(
                    "unknown-behaviour",
                    &format!("{path}.behaviour"),
                    format!(
                        "Unknown behaviour `{beh}`; registered behaviours: {}.",
                        BEHAVIOURS.join(", ")
                    ),
                    hint,
                );
            }
        }
        match silhouettes.get(&b.silhouette) {
            Some(other) => {
                v.push(
                    "readability",
                    &format!("{path}.silhouette"),
                    format!(
                        "Bullet types `{other}` and `{}` share the silhouette `{}`; bullet types of one unit must differ in silhouette.",
                        b.name, b.silhouette
                    ),
                    "Pick a distinct silhouette (readability rule 3).".into(),
                );
            }
            None => {
                silhouettes.insert(b.silhouette.clone(), b.name.clone());
            }
        }
    }

    // Emitters: resolve composition
    let mut emitters: Vec<Emitter> = Vec::new();
    let mut emitter_paths: Vec<usize> = Vec::new();
    for (i, item) in m.emitters.iter().enumerate() {
        match item {
            EmitterItem::Emitter {
                name,
                role,
                bullet,
                offset,
                delay,
                repeat,
                interval,
                speed,
                block,
                modifiers,
            } => {
                emitters.push(Emitter {
                    name: name.clone(),
                    role: *role,
                    bullet: bullet.clone(),
                    offset: *offset,
                    delay: *delay,
                    repeat: *repeat,
                    interval: *interval,
                    speed: *speed,
                    block: block.clone(),
                    modifiers: modifiers.clone(),
                });
                emitter_paths.push(i);
            }
            EmitterItem::Use {
                name,
                from,
                overrides,
            } => {
                let path = format!("emitters.{name}");
                let Some((alias, en)) = from.split_once('.') else {
                    v.push(
                        "unknown-reference",
                        &format!("{path}.from"),
                        format!("`from` must name `alias.emitter`, found `{from}`."),
                        "Write e.g. `ring.burst`.".into(),
                    );
                    continue;
                };
                let Some(unit) = imported.get(alias) else {
                    if !m.imports.iter().any(|imp| imp.alias == alias) {
                        v.push(
                            "unknown-import",
                            &format!("{path}.from"),
                            format!("Unknown import alias `{alias}`."),
                            "Add an import for it.".into(),
                        );
                    }
                    continue;
                };
                let Some(base) = unit.emitters.iter().find(|e| e.name == en) else {
                    let known: BTreeSet<String> =
                        unit.emitters.iter().map(|e| e.name.clone()).collect();
                    v.reference(
                        "unknown-reference",
                        &format!("{path}.from"),
                        "emitter",
                        en,
                        &known,
                    );
                    continue;
                };
                let mut e = base.clone();
                e.name = name.clone();
                for (k, raw) in overrides {
                    let opath = format!("{path}.{k}");
                    if let Err(mut d) = apply_override(&mut e, k, raw, src, &opath) {
                        if d.pos.is_none() {
                            d.pos = v.loc.locate(&opath);
                        }
                        v.diags.push(d);
                    }
                }
                emitters.push(e);
                emitter_paths.push(i);
            }
        }
    }

    let mut first_emitter: BTreeMap<String, usize> = BTreeMap::new();
    let mut unique: Vec<&Emitter> = Vec::new();
    for (k, e) in emitters.iter().enumerate() {
        let i = emitter_paths[k];
        if let Some(j) = first_emitter.get(&e.name).copied() {
            let first = v.loc.locate(&format!("emitters[{j}].name"));
            let d = v.push(
                "duplicate-name",
                &format!("emitters[{i}].name"),
                format!(
                    "Duplicate emitter name `{}`; first defined at line {}.",
                    e.name,
                    first.map_or(0, |p| p.line)
                ),
                "Rename one of the emitters; emitter names must be unique within a unit.".into(),
            );
            if let Some(p) = first {
                d.related = Some(diag::Related {
                    label: "first definition".into(),
                    pos: p,
                });
            }
            continue;
        }
        first_emitter.insert(e.name.clone(), i);
        unique.push(e);
        let path = format!("emitters.{}", e.name);
        v.reference(
            "unknown-reference",
            &format!("{path}.bullet"),
            "bullet type",
            &e.bullet,
            &bullet_names,
        );
        if e.interval.0 < 1 {
            v.push(
                "range",
                &format!("{path}.interval"),
                "`interval` must be at least 1 tick.".into(),
                "Use e.g. `1t`.".into(),
            );
        }
        v.range_f(
            &format!("{path}.speed"),
            "`speed`",
            e.speed.0,
            0.0,
            4.0,
            true,
        );
        if let Repeat::Times(n) = e.repeat {
            if !(1..=100_000).contains(&n) {
                v.push(
                    "range",
                    &format!("{path}.repeat"),
                    format!("`repeat` must be in 1..=100000, found {n}."),
                    "Use `forever` for endless emitters.".into(),
                );
            }
        }
        v.block(&format!("{path}.block"), &e.block);
        for (k, md) in e.modifiers.iter().enumerate() {
            let mp = format!("{path}.modifiers[{k}]");
            match md {
                Modifier::SpeedCurve { keys, .. } => {
                    if !(2..=16).contains(&keys.len()) {
                        v.push(
                            "range",
                            &format!("{mp}.keys"),
                            format!("A speed curve needs 2..=16 keys, found {}.", keys.len()),
                            "Add or remove keys.".into(),
                        );
                    }
                    for (q, key) in keys.iter().enumerate() {
                        if q > 0 && key.at.0 <= keys[q - 1].at.0 {
                            v.push(
                                "keys-order",
                                &format!("{mp}.keys[{q}].at"),
                                format!(
                                    "Key times must strictly increase: {} follows {}.",
                                    key.at.0,
                                    keys[q - 1].at.0
                                ),
                                "Sort the keys by `at`.".into(),
                            );
                        }
                        v.range_f(
                            &format!("{mp}.keys[{q}].mul"),
                            "`mul`",
                            key.mul,
                            0.0,
                            8.0,
                            false,
                        );
                    }
                }
                Modifier::Rotate { rate } => {
                    v.range_f(&format!("{mp}.rate"), "`rate`", rate.0, -45.0, 45.0, false)
                }
                Modifier::Curve { turn } => {
                    v.range_f(&format!("{mp}.turn"), "`turn`", turn.0, -45.0, 45.0, false)
                }
                Modifier::Accelerate { max_speed, .. } => {
                    if let Some(ms) = max_speed {
                        v.range_f(
                            &format!("{mp}.max_speed"),
                            "`max_speed`",
                            ms.0,
                            0.0,
                            4.0,
                            true,
                        );
                    }
                }
                Modifier::SineOffset {
                    amplitude, period, ..
                } => {
                    v.range_f(
                        &format!("{mp}.amplitude"),
                        "`amplitude`",
                        amplitude.0,
                        0.0,
                        8.0,
                        false,
                    );
                    if period.0 < 1 {
                        v.push(
                            "range",
                            &format!("{mp}.period"),
                            "`period` must be at least 1 tick.".into(),
                            "Use e.g. `40t`.".into(),
                        );
                    }
                }
                Modifier::Mirror { folds, .. } => {
                    if !(1..=8).contains(folds) {
                        v.push(
                            "range",
                            &format!("{mp}.folds"),
                            format!("`folds` must be in 1..=8, found {folds}."),
                            "Use 1 to 8 folds.".into(),
                        );
                    }
                }
            }
        }
    }

    // Transforms (need emitters)
    let emitter_names: BTreeSet<String> = unique.iter().map(|e| e.name.clone()).collect();
    let events: BTreeSet<String> = EVENTS.iter().map(|s| s.to_string()).collect();
    for b in &m.bullets {
        let path = format!("bullets.{}", b.name);
        for (k, t) in b.transforms.iter().enumerate() {
            let tp = format!("{path}.transforms[{k}]");
            let when = match t {
                Transform::Burst { when, .. }
                | Transform::ChangeType { when, .. }
                | Transform::Reverse { when }
                | Transform::BecomeEmitter { when, .. } => when,
            };
            match when {
                Trigger::Event(e) => {
                    v.reference("unknown-event", &format!("{tp}.when"), "event", e, &events)
                }
                Trigger::Distance(d) => v.range_f(
                    &format!("{tp}.when"),
                    "Trigger distance",
                    d.0,
                    0.0,
                    1000.0,
                    true,
                ),
                Trigger::Time(_) => {}
            }
            match t {
                Transform::Burst {
                    bullet,
                    speed,
                    block,
                    ..
                } => {
                    v.reference(
                        "unknown-reference",
                        &format!("{tp}.bullet"),
                        "bullet type",
                        bullet,
                        &bullet_names,
                    );
                    v.range_f(&format!("{tp}.speed"), "`speed`", speed.0, 0.0, 4.0, true);
                    v.block(&format!("{tp}.block"), block);
                }
                Transform::ChangeType { to, .. } => v.reference(
                    "unknown-reference",
                    &format!("{tp}.to"),
                    "bullet type",
                    to,
                    &bullet_names,
                ),
                Transform::BecomeEmitter { emitter, .. } => {
                    v.reference(
                        "unknown-reference",
                        &format!("{tp}.emitter"),
                        "emitter",
                        emitter,
                        &emitter_names,
                    );
                    if let Some(e) = unique.iter().find(|e| &e.name == emitter) {
                        if e.role != Role::Sub {
                            let hint = v.ex(
                                "Set `role: Sub,` on that emitter.".into(),
                                "Set `role = sub` on that emitter.".into(),
                            );
                            v.push(
                                "role",
                                &format!("{tp}.emitter"),
                                format!("Emitter `{emitter}` is used by `become_emitter` but is not a sub-emitter."),
                                hint,
                            );
                        }
                    }
                }
                Transform::Reverse { .. } => {}
            }
        }
    }

    // Cascade depth and cycles
    let bullets_by: BTreeMap<&str, &Bullet> =
        m.bullets.iter().map(|b| (b.name.as_str(), b)).collect();
    let emitters_by: BTreeMap<&str, &Emitter> =
        unique.iter().map(|e| (e.name.as_str(), *e)).collect();
    fn depth_of(
        b: &str,
        bullets: &BTreeMap<&str, &Bullet>,
        emitters: &BTreeMap<&str, &Emitter>,
        stack: &mut Vec<String>,
    ) -> Result<usize, String> {
        if stack.iter().any(|s| s == b) {
            return Err(stack.join(" -> ") + " -> " + b);
        }
        let Some(bullet) = bullets.get(b) else {
            return Ok(0);
        };
        stack.push(b.to_string());
        let mut max = 0;
        for t in &bullet.transforms {
            let d = match t {
                Transform::BecomeEmitter { emitter, .. } => match emitters.get(emitter.as_str()) {
                    Some(e) => 1 + depth_of(&e.bullet, bullets, emitters, stack)?,
                    None => 1,
                },
                Transform::Burst { bullet, .. } => 1 + depth_of(bullet, bullets, emitters, stack)?,
                Transform::ChangeType { to, .. } => depth_of(to, bullets, emitters, stack)?,
                Transform::Reverse { .. } => 0,
            };
            max = max.max(d);
        }
        stack.pop();
        Ok(max)
    }
    for e in unique.iter().filter(|e| e.role == Role::Root) {
        let path = format!("emitters.{}.bullet", e.name);
        match depth_of(&e.bullet, &bullets_by, &emitters_by, &mut Vec::new()) {
            Ok(d) if d > MAX_CASCADE_DEPTH => {
                v.push(
                    "cascade-depth",
                    &path,
                    format!("Cascade depth {d} starting at emitter `{}` exceeds the v1 maximum of {MAX_CASCADE_DEPTH}.", e.name),
                    "Remove a `burst` or `become_emitter` stage.".into(),
                );
            }
            Err(chain) => {
                v.push(
                    "cycle",
                    &path,
                    format!("Transform cycle: {chain}."),
                    "Break the cycle; a bullet may not turn back into an earlier type.".into(),
                );
            }
            _ => {}
        }
    }

    let resolved = Resolved {
        version: m.version,
        imports: m
            .imports
            .iter()
            .map(|i| {
                let stem = Path::new(&i.path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                (i.alias.clone(), stem)
            })
            .collect(),
        meta: m.meta.clone(),
        bullets: m.bullets.clone(),
        emitters,
    };
    (v.diags, Some(resolved))
}
