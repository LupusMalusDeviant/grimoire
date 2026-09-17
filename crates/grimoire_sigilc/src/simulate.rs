//! `sigilc simulate` (Plan 0002 WP5.6; `docs/formats/sigil.md` §13.8): runs a compiled unit through
//! the runtime interpreter (`grimoire_sigil::install` on a `grimoire_sim::Simulation`, the code the
//! engine runs) and records every tick as the JSON preview document the Sigil editor (WP10.4) and
//! agents read.
//!
//! The run is a pure function of the unit bytes and [`SimulateOptions`]: one thread, the default
//! sequential executor, no clock (contract §3). Every primary emitter of the unit (not `role = sub`)
//! starts at tick `0` at the origin with rotation `0`; the aim target is fixed, scripted along a
//! [`TargetPath`] or absent; named events are raised at given ticks. Behaviours are game code the
//! compiler cannot run: every behaviour the unit binds is replaced by a stand-in that keeps the
//! bullet unchanged, and the document lists them.

use std::collections::BTreeMap;
use std::sync::Arc;

use grimoire_core::Vec2;
use grimoire_sigil::{
    AimTarget, BehaviorId, BehaviorInput, BehaviorOutcome, BehaviorRegistryBuilder, BulletFlags,
    BulletMotion, BulletPool, DespawnCause, Emitter, EventId, EventRequest, SigilConfig,
    SigilLibrary, SigilUnit, install,
};
use grimoire_sim::{SimRng, Simulation, TickInput};
use serde::{Deserialize, Serialize};

/// Most ticks one simulation may run (ten minutes at 60 ticks per second).
pub const MAX_SIMULATE_TICKS: u32 = 36_000;
/// Pool capacity when none is given.
pub const DEFAULT_CAPACITY: u32 = 65_536;
/// Half the edge length of the default simulation bounds, in units (`-1000..=1000` on both axes).
pub const DEFAULT_BOUNDS: f32 = 1_000.0;

/// Value of the target path document's `schema` field.
pub const TARGET_PATH_SCHEMA: &str = "grimoire.sigilc.target_path";
/// The only `schema_version` the target path decoder reads.
pub const TARGET_PATH_SCHEMA_VERSION: u32 = 1;
/// Largest target path document accepted, in bytes.
pub const MAX_TARGET_PATH_BYTES: usize = 1024 * 1024;
/// Most points one target path may have.
pub const MAX_TARGET_PATH_POINTS: usize = 65_536;

/// Largest tick a target path point may name: every integer up to it is exact in JSON (contract
/// §2 rule 11).
const MAX_JSON_INTEGER: u64 = (1 << 53) - 1;

/// Why a target path document was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TargetPathError {
    /// The document is larger than [`MAX_TARGET_PATH_BYTES`].
    #[error("the target path is {len} bytes, more than the limit of {max}")]
    TooLarge {
        /// Actual size in bytes.
        len: usize,
        /// The limit.
        max: usize,
    },
    /// The document is not JSON of the documented shape.
    #[error("the target path is malformed: {0}")]
    Malformed(String),
    /// `schema` or `schema_version` names another document.
    #[error(
        "the target path has schema `{schema}` version {schema_version}, expected `grimoire.sigilc.target_path` version 1"
    )]
    WrongSchema {
        /// The `schema` found.
        schema: String,
        /// The `schema_version` found.
        schema_version: u32,
    },
    /// No points, or more than [`MAX_TARGET_PATH_POINTS`].
    #[error("the target path has {count} points; it needs 1 to {max}")]
    PointCount {
        /// Points found.
        count: usize,
        /// The limit.
        max: usize,
    },
    /// A point's ticks are not strictly ascending, or a tick is beyond 2^53 - 1.
    #[error(
        "target path point {index} has tick {tick}, which is not after the previous point or beyond 2^53 - 1"
    )]
    TickOrder {
        /// Index of the offending point.
        index: usize,
        /// Its tick.
        tick: u64,
    },
    /// A coordinate is not a finite `f32`.
    #[error("target path point {index} has a coordinate that is not a finite 32-bit float")]
    NonFinite {
        /// Index of the offending point.
        index: usize,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetPathDocument {
    schema: String,
    schema_version: u32,
    points: Vec<TargetPathPoint>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetPathPoint {
    tick: u64,
    x: f64,
    y: f64,
}

/// A scripted aim target: positions at strictly ascending ticks, linearly interpolated in between
/// and held before the first and after the last point.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetPath {
    points: Vec<(u64, Vec2)>,
}

impl TargetPath {
    /// The aim target during `tick`.
    #[must_use]
    pub fn at(&self, tick: u64) -> Vec2 {
        // Non-empty and strictly ascending: guaranteed by `parse_target_path`.
        let (first_tick, first) = self.points[0];
        if tick <= first_tick {
            return first;
        }
        let (last_tick, last) = self.points[self.points.len() - 1];
        if tick >= last_tick {
            return last;
        }
        let next = self
            .points
            .partition_point(|&(point_tick, _)| point_tick <= tick);
        let (from_tick, from) = self.points[next - 1];
        let (to_tick, to) = self.points[next];
        let fraction = (tick - from_tick) as f32 / (to_tick - from_tick) as f32;
        from + (to - from) * fraction
    }
}

/// Decodes a target path document (`docs/formats/sigil.md` §13.8).
///
/// # Errors
/// A [`TargetPathError`] for input that is too large, malformed, of another schema, without points
/// or with too many, out of tick order, or with a coordinate that is not a finite `f32`. Never
/// panics (contract §2 rule 9).
pub fn parse_target_path(json: &str) -> Result<TargetPath, TargetPathError> {
    if json.len() > MAX_TARGET_PATH_BYTES {
        return Err(TargetPathError::TooLarge {
            len: json.len(),
            max: MAX_TARGET_PATH_BYTES,
        });
    }
    let document: TargetPathDocument = serde_json::from_str(json)
        .map_err(|error| TargetPathError::Malformed(error.to_string()))?;
    if document.schema != TARGET_PATH_SCHEMA
        || document.schema_version != TARGET_PATH_SCHEMA_VERSION
    {
        return Err(TargetPathError::WrongSchema {
            schema: document.schema,
            schema_version: document.schema_version,
        });
    }
    let count = document.points.len();
    if count == 0 || count > MAX_TARGET_PATH_POINTS {
        return Err(TargetPathError::PointCount {
            count,
            max: MAX_TARGET_PATH_POINTS,
        });
    }
    let mut points: Vec<(u64, Vec2)> = Vec::with_capacity(count);
    for (index, point) in document.points.into_iter().enumerate() {
        let after_previous = points.last().is_none_or(|&(tick, _)| point.tick > tick);
        if !after_previous || point.tick > MAX_JSON_INTEGER {
            return Err(TargetPathError::TickOrder {
                index,
                tick: point.tick,
            });
        }
        let (x, y) = (point.x as f32, point.y as f32);
        if !x.is_finite() || !y.is_finite() {
            return Err(TargetPathError::NonFinite { index });
        }
        points.push((point.tick, Vec2::new(x, y)));
    }
    Ok(TargetPath { points })
}

/// Where `Aimed` blocks aim during a simulation.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// No target: `Aimed` uses the emitter's rotation (contract §11.4).
    None,
    /// One fixed position.
    Fixed(Vec2),
    /// A scripted path.
    Path(TargetPath),
}

impl Target {
    fn at(&self, tick: u64) -> Option<Vec2> {
        match self {
            Target::None => None,
            Target::Fixed(position) => Some(*position),
            Target::Path(path) => Some(path.at(tick)),
        }
    }
}

/// One event raised during a simulation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledEvent {
    /// Tick in which the event is raised (its `event` triggers fire in this tick).
    pub tick: u64,
    /// Event name, as written after `event` in Sigil source.
    pub name: String,
}

/// Everything a simulation run depends on besides the unit.
#[derive(Debug, Clone, PartialEq)]
pub struct SimulateOptions {
    /// Ticks to run, `1..=MAX_SIMULATE_TICKS`.
    pub ticks: u32,
    /// Simulation seed (`scatter` and behaviour randomness).
    pub seed: u64,
    /// Aim target.
    pub target: Target,
    /// Events to raise, in any order.
    pub events: Vec<ScheduledEvent>,
    /// Bullet pool capacity, `1..=BulletPool::MAX_CAPACITY`.
    pub capacity: u32,
    /// Lower simulation bound.
    pub bounds_min: Vec2,
    /// Upper simulation bound.
    pub bounds_max: Vec2,
}

impl SimulateOptions {
    /// `ticks` ticks with seed `0`, no target, no events, the default capacity and bounds.
    #[must_use]
    pub fn new(ticks: u32) -> Self {
        Self {
            ticks,
            seed: 0,
            target: Target::None,
            events: Vec::new(),
            capacity: DEFAULT_CAPACITY,
            bounds_min: Vec2::splat(-DEFAULT_BOUNDS),
            bounds_max: Vec2::splat(DEFAULT_BOUNDS),
        }
    }
}

/// Why a simulation could not run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SimulateError {
    /// The unit bytes do not decode.
    #[error("the unit does not decode: {0}")]
    Unit(String),
    /// The options are out of range.
    #[error("{0}")]
    Options(String),
    /// Installing the unit into the simulation failed.
    #[error("the unit cannot be installed: {0}")]
    Install(String),
}

/// Column order of one bullet row of a [`Frame`].
pub const BULLET_FIELDS: [&str; 9] = [
    "slot",
    "generation",
    "type",
    "x",
    "y",
    "vx",
    "vy",
    "age",
    "cascade",
];
/// Column order of one despawn row of a [`Frame`].
pub const DESPAWN_FIELDS: [&str; 6] = ["slot", "generation", "type", "x", "y", "cause"];

/// One live bullet after a tick: slot, generation, bullet type, position, velocity, age, cascade
/// depth (serialised as a JSON array in [`BULLET_FIELDS`] order).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BulletRow(
    pub u32,
    pub u32,
    pub u16,
    pub f32,
    pub f32,
    pub f32,
    pub f32,
    pub u32,
    pub u8,
);

/// One bullet despawned in a tick: slot, generation, bullet type, position, cause (serialised as a
/// JSON array in [`DESPAWN_FIELDS`] order; the cause is the `DespawnCause` tag, `0` lifetime, `1`
/// bounds, `2` transform, `3` behaviour, `4` clear, `5` swap, `6` external).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct DespawnRow(pub u32, pub u32, pub u16, pub f32, pub f32, pub u8);

/// The state after one simulated tick.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Frame {
    /// The simulated tick, `0` for the first.
    pub tick: u64,
    /// `Simulation::state_hash` after the tick, 16 lowercase hex digits.
    pub state_hash: String,
    /// The aim target during the tick, `null` without one.
    pub target: Option<[f32; 2]>,
    /// Names of the events raised in the tick, in the order given.
    pub events: Vec<String>,
    /// Live bullets after the tick.
    pub live: u32,
    /// Spawns dropped since the start (full pool or cascade cap).
    pub dropped_spawns: u64,
    /// Every live bullet, ascending by slot.
    pub bullets: Vec<BulletRow>,
    /// Every bullet despawned in the tick, in despawn order.
    pub despawned: Vec<DespawnRow>,
}

/// A bullet type of the simulated unit, for drawing it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BulletTypeJson {
    /// Visual catalogue row of the silhouette.
    pub silhouette: u16,
    /// Visual catalogue row of the palette.
    pub palette: u16,
    /// Palette space (`1`: hostile).
    pub palette_space: u8,
    /// Glow, `0`–`255`.
    pub glow: u8,
    /// Visible radius, in units.
    pub radius: f32,
    /// Hit radius, in units.
    pub collision_radius: f32,
    /// Lifetime in ticks, `0` for unbounded.
    pub lifetime_ticks: u32,
    /// Flag names, in bit order.
    pub flags: Vec<&'static str>,
}

/// A behaviour the simulation replaced by its keep-the-bullet stand-in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StubbedBehavior {
    /// Name from the behaviour manifest, `null` if the manifest does not name the id.
    pub name: Option<String>,
    /// The `BehaviorId`.
    pub id: u32,
}

/// The recorded run.
#[derive(Debug, Clone, PartialEq)]
pub struct Simulated {
    /// Indices of the emitters that ran (the primary ones), ascending.
    pub emitters: Vec<u16>,
    /// The unit's bullet types, by index.
    pub bullet_types: Vec<BulletTypeJson>,
    /// The behaviours replaced by stand-ins, ascending by id.
    pub stubbed_behaviors: Vec<StubbedBehavior>,
    /// One frame per tick.
    pub frames: Vec<Frame>,
    /// `Simulation::state_hash` after the last tick, 16 lowercase hex digits.
    pub final_state_hash: String,
}

/// The stand-in for every behaviour: the compiler cannot run game code, so the bullet is kept as
/// its program moved it.
fn keep_bullet(_: &BehaviorInput<'_>, _: &mut BulletMotion, _: &mut SimRng) -> BehaviorOutcome {
    BehaviorOutcome::Keep
}

/// Runs `unit_bytes` for `options.ticks` ticks and records every tick.
///
/// `behavior_names` maps behaviour names to ids (the behaviour manifest); it only names the
/// stubbed behaviours in the result.
///
/// # Errors
/// [`SimulateError::Unit`] if the bytes do not decode, [`SimulateError::Options`] for ticks,
/// capacity or bounds out of range, [`SimulateError::Install`] if the runtime refuses the unit.
pub fn simulate(
    unit_bytes: &[u8],
    behavior_names: &BTreeMap<String, u32>,
    options: &SimulateOptions,
) -> Result<Simulated, SimulateError> {
    if options.ticks == 0 || options.ticks > MAX_SIMULATE_TICKS {
        return Err(SimulateError::Options(format!(
            "ticks must be 1 to {MAX_SIMULATE_TICKS}, got {}",
            options.ticks
        )));
    }
    if options.capacity == 0 || options.capacity > BulletPool::MAX_CAPACITY {
        return Err(SimulateError::Options(format!(
            "capacity must be 1 to {}, got {}",
            BulletPool::MAX_CAPACITY,
            options.capacity
        )));
    }
    let (min, max) = (options.bounds_min, options.bounds_max);
    if !(min.x.is_finite() && min.y.is_finite() && max.x.is_finite() && max.y.is_finite())
        || min.x >= max.x
        || min.y >= max.y
    {
        return Err(SimulateError::Options(
            "bounds must be finite with min below max on both axes".to_string(),
        ));
    }
    let roles = primary_emitters(unit_bytes)
        .ok_or_else(|| SimulateError::Unit("the Emitters section cannot be read".to_string()))?;
    let unit = SigilUnit::from_bytes(unit_bytes)
        .map_err(|error| SimulateError::Unit(error.to_string()))?;
    let unit_id = unit.id();

    let mut builder = BehaviorRegistryBuilder::new(0);
    let mut stubbed_behaviors = Vec::new();
    let mut referenced: Vec<BehaviorId> = unit.behavior_refs().to_vec();
    referenced.sort_unstable();
    referenced.dedup();
    for id in referenced {
        builder
            .register(id, "sigilc.simulate.stub", keep_bullet)
            .map_err(|error| SimulateError::Install(error.to_string()))?;
        stubbed_behaviors.push(StubbedBehavior {
            name: behavior_names
                .iter()
                .find(|&(_, &named)| named == id.0)
                .map(|(name, _)| name.clone()),
            id: id.0,
        });
    }
    let registry = builder.build();
    let bullet_types = unit.bullet_types().iter().map(bullet_type_json).collect();
    let library = SigilLibrary::new(vec![unit], Arc::clone(&registry))
        .map_err(|error| SimulateError::Install(error.to_string()))?;

    let mut sim = Simulation::new(options.seed);
    install(
        &mut sim,
        library,
        registry,
        SigilConfig::new(options.capacity, min, max),
    )
    .map_err(|error| SimulateError::Install(error.to_string()))?;
    let emitters: Vec<u16> = roles
        .iter()
        .enumerate()
        .filter(|&(_, &primary)| primary)
        .map(|(index, _)| index as u16)
        .collect();
    for &emitter in &emitters {
        sim.world_mut().spawn((Emitter {
            unit: unit_id,
            emitter,
            origin: Vec2::ZERO,
            rotation: 0.0,
            started_at: 0,
        },));
    }

    let mut events_by_tick: BTreeMap<u64, Vec<&str>> = BTreeMap::new();
    for event in &options.events {
        events_by_tick
            .entry(event.tick)
            .or_default()
            .push(event.name.as_str());
    }

    let mut frames = Vec::with_capacity(options.ticks as usize);
    for tick in 0..u64::from(options.ticks) {
        let target = options.target.at(tick);
        if let Some(aim) = sim.world_mut().resource_mut::<AimTarget>() {
            aim.0 = target;
        }
        let raised = events_by_tick.get(&tick).cloned().unwrap_or_default();
        for name in &raised {
            sim.world_mut().spawn((EventRequest {
                event: EventId::from_name(name),
            },));
        }
        sim.step(TickInput::default());
        frames.push(frame(&sim, tick, target, &raised));
    }
    let final_state_hash = hex(sim.state_hash());
    Ok(Simulated {
        emitters,
        bullet_types,
        stubbed_behaviors,
        frames,
        final_state_hash,
    })
}

fn frame(sim: &Simulation, tick: u64, target: Option<Vec2>, raised: &[&str]) -> Frame {
    let empty = BulletPool::default();
    let pool = sim.world().resource::<BulletPool>().unwrap_or(&empty);
    let columns = pool.columns();
    let bullets = pool
        .iter()
        .map(|bullet| {
            let id = bullet.id();
            let slot = id.index() as usize;
            let (position, velocity) = (bullet.position(), bullet.velocity());
            BulletRow(
                id.index(),
                id.generation(),
                bullet.bullet_type(),
                position.x,
                position.y,
                velocity.x,
                velocity.y,
                bullet.age(),
                columns.cascade[slot],
            )
        })
        .collect();
    let despawned = pool
        .events()
        .iter()
        .map(|event| {
            DespawnRow(
                event.id.index(),
                event.id.generation(),
                event.bullet_type,
                event.position.x,
                event.position.y,
                cause_tag(event.cause),
            )
        })
        .collect();
    Frame {
        tick,
        state_hash: hex(sim.state_hash()),
        target: target.map(|position| [position.x, position.y]),
        events: raised.iter().map(|name| (*name).to_string()).collect(),
        live: pool.len(),
        dropped_spawns: pool.dropped_spawns(),
        bullets,
        despawned,
    }
}

/// The `DespawnCause` tag (`#[repr(u8)]`, contract §11.3).
fn cause_tag(cause: DespawnCause) -> u8 {
    match cause {
        DespawnCause::Lifetime => 0,
        DespawnCause::Bounds => 1,
        DespawnCause::Transform => 2,
        DespawnCause::Behavior => 3,
        DespawnCause::Clear => 4,
        DespawnCause::Swap => 5,
        _ => 6,
    }
}

fn bullet_type_json(bullet_type: &grimoire_sigil::BulletType) -> BulletTypeJson {
    let names = [
        (BulletFlags::SMASHABLE, "smashable"),
        (BulletFlags::REFLECTABLE, "reflectable"),
        (BulletFlags::ENV_ACTIVE, "env_active"),
        (BulletFlags::GRAZEABLE, "grazeable"),
    ];
    BulletTypeJson {
        silhouette: bullet_type.visual.silhouette,
        palette: bullet_type.visual.palette,
        palette_space: bullet_type.visual.palette_space,
        glow: bullet_type.visual.glow,
        radius: bullet_type.radius,
        collision_radius: bullet_type.collision_radius,
        lifetime_ticks: bullet_type.lifetime_ticks,
        flags: names
            .iter()
            .filter(|(flag, _)| bullet_type.flags.contains(*flag))
            .map(|&(_, name)| name)
            .collect(),
    }
}

fn hex(value: u64) -> String {
    format!("{value:016x}")
}

/// Whether each emitter of `unit_bytes` is primary (`role` `0`), read from the `Emitters` section
/// (`docs/formats/sigil.md` §10.2 and §10.5). The runtime keeps roles private; this compiler writes
/// that layout, so it reads it back here. `None` if the bytes do not have that shape.
fn primary_emitters(unit_bytes: &[u8]) -> Option<Vec<bool>> {
    const HEADER_LEN: usize = 40;
    const ENTRY_LEN: usize = 24;
    const EMITTERS: u32 = 3;
    const RECORD_LEN: usize = 30;
    let read_u32 = |at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(
            unit_bytes.get(at..at + 4)?.try_into().ok()?,
        ))
    };
    let read_u64 = |at: usize| -> Option<u64> {
        Some(u64::from_le_bytes(
            unit_bytes.get(at..at + 8)?.try_into().ok()?,
        ))
    };
    let section_count = read_u32(HEADER_LEN)? as usize;
    let entry = (0..section_count)
        .map(|index| HEADER_LEN + 4 + index * ENTRY_LEN)
        .find(|&entry| read_u32(entry) == Some(EMITTERS))?;
    let offset = usize::try_from(read_u64(entry + 8)?).ok()?;
    let start = HEADER_LEN.checked_add(offset)?;
    let count = u16::from_le_bytes(unit_bytes.get(start..start + 2)?.try_into().ok()?);
    (0..usize::from(count))
        .map(|index| {
            let role = *unit_bytes.get(start + 2 + index * RECORD_LEN + 4)?;
            Some(role == 0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(points: &str) -> String {
        format!(
            r#"{{"schema":"grimoire.sigilc.target_path","schema_version":1,"points":[{points}]}}"#
        )
    }

    #[test]
    fn target_path_interpolates_and_holds_its_ends() {
        let path = parse_target_path(&document(
            r#"{"tick":10,"x":0.0,"y":0.0},{"tick":20,"x":10.0,"y":-5.0},{"tick":30,"x":10.0,"y":5.0}"#,
        ))
        .expect("valid path");
        assert_eq!(path.at(0), Vec2::new(0.0, 0.0));
        assert_eq!(path.at(10), Vec2::new(0.0, 0.0));
        assert_eq!(path.at(15), Vec2::new(5.0, -2.5));
        assert_eq!(path.at(20), Vec2::new(10.0, -5.0));
        assert_eq!(path.at(25), Vec2::new(10.0, 0.0));
        assert_eq!(path.at(1_000), Vec2::new(10.0, 5.0));
    }

    #[test]
    fn target_path_rejects_every_documented_problem() {
        let cases: Vec<(String, &str)> = vec![
            ("not json".to_string(), "malformed"),
            (document(r#"{"tick":1,"x":0.0}"#), "malformed"),
            (document(r#"{"tick":1,"x":0.0,"y":0.0,"z":1}"#), "malformed"),
            (document(r#"{"tick":-1,"x":0.0,"y":0.0}"#), "malformed"),
            (
                r#"{"schema":"other","schema_version":1,"points":[]}"#.to_string(),
                "schema",
            ),
            (document(""), "count"),
            (
                document(r#"{"tick":5,"x":0.0,"y":0.0},{"tick":5,"x":1.0,"y":0.0}"#),
                "order",
            ),
            (
                document(r#"{"tick":9007199254740992,"x":0.0,"y":0.0}"#),
                "order",
            ),
            (document(r#"{"tick":1,"x":1e39,"y":0.0}"#), "finite"),
        ];
        for (json, kind) in cases {
            let error = parse_target_path(&json).expect_err(&json);
            let matches = match kind {
                "malformed" => matches!(error, TargetPathError::Malformed(_)),
                "schema" => matches!(error, TargetPathError::WrongSchema { .. }),
                "count" => matches!(error, TargetPathError::PointCount { .. }),
                "order" => matches!(error, TargetPathError::TickOrder { .. }),
                _ => matches!(error, TargetPathError::NonFinite { .. }),
            };
            assert!(matches, "{json}: {error:?}");
        }
        let too_large = " ".repeat(MAX_TARGET_PATH_BYTES + 1);
        assert!(matches!(
            parse_target_path(&too_large),
            Err(TargetPathError::TooLarge { .. })
        ));
    }

    proptest::proptest! {
        #[test]
        fn target_path_decoder_never_panics(json in ".{0,256}") {
            let _ = parse_target_path(&json);
        }
    }
}
