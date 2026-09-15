//! Shared in-memory model of a Sigil pattern (spike OF-4.1).
//!
//! Follows `corpus/grammar/schema-model.txt`. Both front ends produce this model: the RON front
//! end through `ron` + serde derive, the sigil 1 front end through its own serde deserializer
//! over the lossless syntax tree. The only syntax-specific part is the raw override value `R`
//! of composition (`Use`): RON keeps `Box<ron::value::RawValue>`, sigil 1 keeps the source text
//! of the value. Overrides are decoded against the target field when imports are resolved.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Sigil<R> {
    pub version: u32,
    #[serde(default)]
    pub imports: Vec<Import>,
    pub meta: Meta,
    pub bullets: Vec<Bullet>,
    pub emitters: Vec<EmitterItem<R>>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Import {
    pub path: String,
    pub alias: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    pub name: String,
    pub difficulty: Vec<String>,
    pub density: u32,
    #[serde(default)]
    pub patron: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Bullet {
    pub name: String,
    pub silhouette: String,
    pub palette: String,
    pub glow: f32,
    pub radius: Units,
    pub damage: u32,
    #[serde(default)]
    pub flags: Vec<Flag>,
    #[serde(default)]
    pub despawn_vfx: Option<String>,
    #[serde(default)]
    pub behaviour: Option<String>,
    #[serde(default)]
    pub transforms: Vec<Transform>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
pub enum Flag {
    Smashable,
    Reflectable,
    EnvActive,
    Grazeable,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub enum EmitterItem<R> {
    Emitter {
        name: String,
        #[serde(default)]
        role: Role,
        bullet: String,
        #[serde(default)]
        offset: Offset,
        #[serde(default)]
        delay: Ticks,
        repeat: Repeat,
        interval: Ticks,
        speed: UnitsPerTick,
        block: Block,
        #[serde(default)]
        modifiers: Vec<Modifier>,
    },
    Use {
        name: String,
        from: String,
        #[serde(default = "BTreeMap::new")]
        overrides: BTreeMap<String, R>,
    },
}

impl<R> EmitterItem<R> {
    pub fn name(&self) -> &str {
        match self {
            EmitterItem::Emitter { name, .. } | EmitterItem::Use { name, .. } => name,
        }
    }
}

/// A fully resolved emitter (after composition). Used for validation and model comparison.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Emitter {
    pub name: String,
    #[serde(default)]
    pub role: Role,
    pub bullet: String,
    #[serde(default)]
    pub offset: Offset,
    #[serde(default)]
    pub delay: Ticks,
    pub repeat: Repeat,
    pub interval: Ticks,
    pub speed: UnitsPerTick,
    pub block: Block,
    #[serde(default)]
    pub modifiers: Vec<Modifier>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
pub enum Role {
    #[default]
    Root,
    Sub,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Offset {
    pub x: Units,
    pub y: Units,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum Repeat {
    Times(u32),
    Forever,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub enum Block {
    Ring {
        count: u16,
        start: Deg,
    },
    Spiral {
        arms: u16,
        step: Deg,
        start: Deg,
    },
    Fan {
        count: u16,
        spread: Deg,
        center: Deg,
    },
    Aimed {
        count: u16,
        spread: Deg,
    },
    Wave {
        count: u16,
        amplitude: Units,
        wavelength: Units,
        direction: Deg,
    },
    Line {
        count: u16,
        spacing: Units,
        direction: Deg,
    },
    Scatter {
        count: u16,
        cone: Deg,
        direction: Deg,
        speed_jitter: f32,
        seed: String,
    },
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub enum Modifier {
    SpeedCurve {
        keys: Vec<Key>,
        interp: Interp,
    },
    Rotate {
        rate: DegPerTick,
    },
    Accelerate {
        rate: UnitsPerTick2,
        #[serde(default)]
        max_speed: Option<UnitsPerTick>,
    },
    Curve {
        turn: DegPerTick,
    },
    SineOffset {
        amplitude: Units,
        period: Ticks,
        #[serde(default)]
        phase: Deg,
    },
    Mirror {
        axis: Deg,
        #[serde(default = "one")]
        folds: u16,
    },
}

fn one() -> u16 {
    1
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Key {
    pub at: Ticks,
    pub mul: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum Interp {
    Linear,
    Step,
    Smooth,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub enum Transform {
    Burst {
        when: Trigger,
        bullet: String,
        speed: UnitsPerTick,
        block: Block,
    },
    ChangeType {
        when: Trigger,
        to: String,
    },
    Reverse {
        when: Trigger,
    },
    BecomeEmitter {
        when: Trigger,
        emitter: String,
    },
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub enum Trigger {
    Time(Ticks),
    Distance(Units),
    Event(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct Ticks(pub u32);
#[derive(Debug, Clone, Copy, PartialEq, Default, Deserialize, Serialize)]
pub struct Deg(pub f32);
#[derive(Debug, Clone, Copy, PartialEq, Default, Deserialize, Serialize)]
pub struct Units(pub f32);
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub struct UnitsPerTick(pub f32);
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub struct UnitsPerTick2(pub f32);
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub struct DegPerTick(pub f32);

/// Unit newtype name -> sigil 1 unit suffix.
pub fn unit_of_newtype(name: &str) -> Option<&'static str> {
    Some(match name {
        "Ticks" => "t",
        "Deg" => "deg",
        "Units" => "u",
        "UnitsPerTick" => "u/t",
        "UnitsPerTick2" => "u/t2",
        "DegPerTick" => "deg/t",
        _ => return None,
    })
}

/// sigil 1 unit suffix -> RON newtype name.
pub fn newtype_of_unit(unit: &str) -> Option<&'static str> {
    Some(match unit {
        "t" => "Ticks",
        "deg" => "Deg",
        "u" => "Units",
        "u/t" => "UnitsPerTick",
        "u/t2" => "UnitsPerTick2",
        "deg/t" => "DegPerTick",
        _ => return None,
    })
}

/// Unit newtype of a field name, where the name alone decides it (used for fix hints).
pub fn field_newtype(field: &str) -> Option<&'static str> {
    Some(match field {
        "speed" | "max_speed" => "UnitsPerTick",
        "interval" | "delay" | "period" | "at" => "Ticks",
        "radius" | "amplitude" | "wavelength" | "spacing" | "x" | "y" => "Units",
        "start" | "step" | "spread" | "center" | "cone" | "direction" | "axis" | "phase" => "Deg",
        "turn" => "DegPerTick",
        _ => return None,
    })
}

/// The fully resolved unit: imports applied, `Use` expanded, syntax-specific parts dropped.
/// Two front ends agree on a pattern iff their resolved models are equal.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Resolved {
    pub version: u32,
    /// Import aliases with the path stem (file extension removed, it differs per syntax).
    pub imports: Vec<(String, String)>,
    pub meta: Meta,
    pub bullets: Vec<Bullet>,
    pub emitters: Vec<Emitter>,
}
