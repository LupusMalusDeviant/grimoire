//! The two P0 baseline benches (Plan-0002 WP6.2 scope item 1), the WP5.1 `sigil.update`
//! micro-bench, and the calibrated regression injector (engine ADR-0010, "Vor der Umsetzung in
//! WP6.2" / calibration Nachtrag).
//!
//! The two P0 benches are reused near-verbatim from the WP6.1 noise spike (branch
//! `p1/wp6.1-bench-spike`, `spikes/bench-noise/src/lib.rs`), which measured these exact two bench
//! shapes and calibrated this exact injector against them (ADR-0010). What changes here is only
//! the crate: this is the real `grimoire_bench`, not throwaway spike code, so it carries doc
//! comments, is part of the workspace and its own contract tests, and its calibration *unit
//! counts* are **not** copied from the ADR's measurement table — engine ADR-0010's own closing
//! note ("Offen für WP6.2 selbst") says why: the per-unit slope is bench- and host-dependent, so
//! WP6.2 must calibrate fresh, in the same CI job as the target measurement, every run
//! (`scripts/calibrate_injection.sh`).
//!
//! `sim_step_600` is a small self-contained stand-in for the P0 demo scenario
//! (`grimoire_sim/tests/scenario/mod.rs` is private to that crate's test binary and not
//! reusable from here, same as in the spike): one exclusive integrate system plus a periodic
//! despawn/respawn keeps the shape (moving entities, one archetype change, a `Simulation::step`
//! call) without reproducing the golden scenario.
//!
//! `sigil_update_6k` is new in Plan 0002 WP5.1: the plan requires a micro-bench proving the
//! `sigil.update` hot path calls no `grimoire_core::math::dmath` trigonometry per bullet per tick
//! ("der heiße Pfad ist heilig ... belegst es mit einem Mikro-Bench"). It spawns
//! [`SIGIL_ENTITIES`] active bullets across programs that stack all six modifiers (`accelerate`,
//! `sine_offset`, `rotate`, `mirror`, `speed_curve`, `curve` — `mirror` only ever acts at emit
//! time, so it contributes nothing to the *ticked* cost this bench measures, but its presence in
//! the stacked program still proves the decoder/interpreter accept it there), then steps ticks
//! after the one-off emit burst has finished, so almost the entire measured cost is
//! `sigil.update`/`sigil.resolve` on an already-full pool, not `sigil.emit`.
//!
//! **What this bench proves today, and what it does not yet.** Building and running it (`cargo
//! test -p grimoire_bench`, and — per this crate's own contribution rule — Callgrind only on the
//! CI runner, never locally) shows the scenario is real and deterministic. Turning that into the
//! same kind of hard proof `ecs_query_10k`/`sim_step_600` already have — a Callgrind `Ir` count
//! wired into `.github/workflows/bench-gate.yml` with an accepted baseline — is a follow-up this
//! pull request deliberately leaves open rather than unilaterally running the separate
//! `bench-accept-baseline` acceptance ceremony (see the WP5.1 report). In the meantime, the "no
//! trigonometry per bullet per tick" claim is also checked the way `clippy.toml`'s
//! `disallowed-methods` already checks the *raw* `f32` methods for this crate family: by
//! inspection — `grimoire_sigil::runtime` (the module this bench exercises) has no `use` of
//! `grimoire_core::math::dmath`'s `sin`/`cos`/`tan`/`atan2`/... anywhere in its per-tick
//! functions, only in the once-per-content-load `RuntimeCache` builder and the once-per-bullet
//! `sine_offset` seed.

use std::hint::black_box;

use grimoire::adapters::sigil_render::extract_bullets;
use grimoire::render::BulletInstance;
use grimoire_core::impl_stable_hash;
use grimoire_core::math::Vec2;
use grimoire_ecs::{System, World, system_fn};
use grimoire_sigil::{
    BehaviorRegistryBuilder, BulletPool, BulletSpawn, Emitter, SigilConfig, SigilContent,
    SigilLibrary, SigilUnit, install,
};
use grimoire_sim::{Simulation, TickInput};

/// Entities in the ECS query benchmark (`ecs_query_10k`; task scope: "ECS query over 10k
/// entities").
pub const ECS_ENTITIES: usize = 10_000;
/// Rounds per wall-clock sample of the ECS query benchmark.
pub const ECS_WALLCLOCK_ROUNDS: u32 = 2_000;
/// Rounds per Callgrind probe of the ECS query benchmark (matches engine ADR-0010's measurement).
pub const ECS_IR_ROUNDS: u32 = 100;

/// Entities in the simplified sim-step benchmark (`sim_step_600`).
pub const SIM_ENTITIES: usize = 2_000;
/// Ticks per wall-clock sample of the sim-step benchmark.
pub const SIM_WALLCLOCK_TICKS: u32 = 600;
/// Ticks per Callgrind probe of the sim-step benchmark (matches engine ADR-0010's measurement).
pub const SIM_IR_TICKS: u32 = 600;

/// Scenario name of the ECS query benchmark (contract §15.1 `BenchResult::scenario`).
pub const ECS_SCENARIO: &str = "ecs_query_10k";
/// Scenario name of the sim-step benchmark (contract §15.1 `BenchResult::scenario`).
pub const SIM_SCENARIO: &str = "sim_step_600";

#[derive(Clone)]
struct Pos {
    x: f32,
    y: f32,
}
impl_stable_hash!(Pos { x, y });

#[derive(Clone)]
struct Vel {
    x: f32,
    y: f32,
}
impl_stable_hash!(Vel { x, y });

/// Builds the 10k-entity world for the ECS query benchmark.
#[must_use]
pub fn build_ecs_world() -> World {
    let mut world = World::new();
    for i in 0..ECS_ENTITIES {
        let f = i as f32;
        world.spawn((Pos { x: f, y: -f }, Vel { x: 1.0, y: 0.5 }));
    }
    world
}

/// Runs `rounds + extra_rounds` write-query rounds over `world`. `extra_rounds` is the nominal
/// (uncalibrated) regression injection: whole extra rounds of identical work, inside the
/// measured region.
pub fn run_ecs_rounds(world: &mut World, rounds: u32, extra_rounds: u32) {
    for _ in 0..(rounds + extra_rounds) {
        for (pos, vel) in world.query_mut::<(&mut Pos, &Vel)>() {
            pos.x += vel.x;
            pos.y += vel.y;
        }
    }
}

/// Builds the simplified sim-step benchmark: `SIM_ENTITIES` moving entities and one exclusive
/// integrate-and-bounce system, plus a periodic despawn/respawn every 50 ticks so the schedule
/// exercises at least one structural change.
#[must_use]
pub fn build_sim(seed: u64) -> Simulation {
    let mut sim = Simulation::new(seed);
    for i in 0..SIM_ENTITIES {
        let f = i as f32;
        sim.world_mut().spawn((
            Pos {
                x: f % 400.0,
                y: -(f % 400.0),
            },
            Vel { x: 0.5, y: 0.25 },
        ));
    }
    sim.schedule_mut().add_system(integrate_system());
    sim.schedule_mut().add_system(recycle_system());
    sim
}

fn integrate_system() -> impl System {
    system_fn("integrate", |world: &mut World| {
        for (pos, vel) in world.query_mut::<(&mut Pos, &Vel)>() {
            pos.x += vel.x;
            pos.y += vel.y;
            if pos.x.abs() > 512.0 {
                pos.x = -pos.x.signum() * 512.0;
            }
            if pos.y.abs() > 512.0 {
                pos.y = -pos.y.signum() * 512.0;
            }
        }
    })
}

/// Every 50th tick, despawns and respawns one entity so the world's archetype membership
/// actually changes (the one structural-change shape the real P0 scenario also exercises).
fn recycle_system() -> impl System {
    let mut tick: u32 = 0;
    system_fn("recycle", move |world: &mut World| {
        tick += 1;
        if !tick.is_multiple_of(50) {
            return;
        }
        if let Some((entity,)) = world.query::<(grimoire_ecs::Entity,)>().next() {
            world.despawn(entity);
        }
        let f = tick as f32;
        world.spawn((
            Pos {
                x: f % 400.0,
                y: -(f % 400.0),
            },
            Vel { x: 0.5, y: 0.25 },
        ));
    })
}

/// Runs `ticks` simulation steps, then `extra_ticks` more of a read-only pass over the world
/// that does not change state — the nominal (uncalibrated) regression injection.
pub fn run_sim_ticks(sim: &mut Simulation, ticks: u32, extra_ticks: u32) {
    for _ in 0..ticks {
        sim.step(TickInput::default());
    }
    if extra_ticks > 0 {
        let mut sum = 0.0f32;
        for _ in 0..extra_ticks {
            for (pos, vel) in sim.world().query::<(&Pos, &Vel)>() {
                sum += pos.x * vel.x + pos.y * vel.y;
            }
        }
        black_box(sum);
    }
}

/// Extra whole rounds/ticks for a nominal percentage regression, rounded up to the nearest unit.
/// Kept for parity with the spike and as a cheap smoke check; the calibrated injector below
/// (`run_calibration_units`) is what the production gate's calibration proof actually uses,
/// because this nominal version under-delivers its target percentage (engine ADR-0010, option 2
/// "Negativ": a nominal `+12%` measured only `+8.2%` `Ir` on `sim_step_600`).
#[must_use]
pub fn extra_units(base: u32, percent: u32) -> u32 {
    ((u64::from(base) * u64::from(percent)).div_ceil(100)) as u32
}

/// One iteration of the calibrated regression injector (engine ADR-0010, "Vor der Umsetzung in
/// WP6.2" / calibration Nachtrag): a homogeneous, arbitrarily fine-grained unit with no
/// per-round/per-tick fixed overhead, so its per-unit `Ir` cost is constant regardless of how
/// many units are injected. `scripts/calibrate_injection.sh` measures that per-unit cost on the
/// runner, in the same CI job as the target run, and solves the unit count for a target
/// percentage from the measurement — never from a hardcoded constant.
///
/// `black_box` on both the input and the output prevents the compiler from folding repeated
/// calls into a closed form (or removing the loop outright).
#[inline(never)]
fn calibration_unit(acc: f32) -> f32 {
    black_box(acc) * black_box(1.000_000_1) + black_box(0.000_000_1)
}

/// Runs `units` iterations of the calibration unit above and returns the (otherwise unused)
/// accumulator, so the loop cannot be optimized away.
pub fn run_calibration_units(units: u64) -> f32 {
    let mut acc = 1.0f32;
    for _ in 0..units {
        acc = calibration_unit(acc);
    }
    acc
}

/// Scenario name of the `sigil.update` hot-path benchmark (contract §15.1 `BenchResult::scenario`).
pub const SIGIL_SCENARIO: &str = "sigil_update_6k";

/// Volley size of the single emitter [`build_sigil_update`] installs.
const SIGIL_RING_COUNT: u16 = 50;
/// Number of volleys the emitter fires before `repeat` is exhausted; `SIGIL_RING_COUNT *
/// SIGIL_VOLLEYS * 2` bullets end up active (the `* 2` is the program's `mirror` modifier, which
/// doubles every volley's shot list at emit time — see [`build_sigil_unit`]).
const SIGIL_VOLLEYS: u32 = 60;
/// Bullets active once every volley has fired (`SIGIL_RING_COUNT * SIGIL_VOLLEYS * 2`).
pub const SIGIL_ENTITIES: u32 = SIGIL_RING_COUNT as u32 * SIGIL_VOLLEYS * 2;
/// Ticks per wall-clock sample of the `sigil.update` benchmark, after the emit burst.
pub const SIGIL_WALLCLOCK_TICKS: u32 = 300;
/// Ticks per Callgrind probe of the `sigil.update` benchmark, after the emit burst.
pub const SIGIL_IR_TICKS: u32 = 300;

const HEADER_LEN: usize = 40;
const SECTION_ENTRY_LEN: u64 = 24;

/// Hand-assembles one `SigilUnit`'s bytes (independent of `SigilUnit::to_bytes`, matching
/// `grimoire_sigil`'s own fixture convention: `src/test_support.rs`, `tests/golden.rs`,
/// `tests/interpreter.rs` — this crate cannot reach any of their `pub(crate)`/private helpers, so
/// it duplicates the small amount of encoding it needs rather than adding a public
/// fixture-building API to `grimoire_sigil` just for this bench).
///
/// One bullet type (`lifetime_ticks = 0`, unbounded — this bench measures ticked bullets staying
/// alive, not despawning), one `Curves` section (for `speed_curve`), one program stacking all six
/// modifiers on a `ring` block, and one emitter referencing it.
fn build_sigil_unit() -> SigilUnit {
    let mut bullet_type = Vec::new();
    bullet_type.extend_from_slice(&1u16.to_le_bytes());
    bullet_type.extend_from_slice(&1.0f32.to_le_bytes()); // radius
    bullet_type.extend_from_slice(&1.0f32.to_le_bytes()); // collision_radius
    bullet_type.extend_from_slice(&0u32.to_le_bytes()); // lifetime_ticks: unbounded
    bullet_type.push(0); // flags
    bullet_type.push(0); // reserved
    bullet_type.extend_from_slice(&0u16.to_le_bytes()); // silhouette
    bullet_type.extend_from_slice(&0u16.to_le_bytes()); // palette
    bullet_type.push(0); // palette_space
    bullet_type.push(0); // glow

    let mut curves = Vec::new();
    curves.extend_from_slice(&1u16.to_le_bytes()); // one curve
    curves.extend_from_slice(&3u16.to_le_bytes()); // three keys
    for &(at_ticks, mul) in &[(0u32, 1.0f32), (150, 1.4), (300, 0.8)] {
        curves.extend_from_slice(&at_ticks.to_le_bytes());
        curves.extend_from_slice(&mul.to_le_bytes());
    }

    let mut program = Vec::new();
    // BlockDef: kind 1 (ring), reserved, count, params[6], seed_hash.
    program.push(1);
    program.push(0);
    program.extend_from_slice(&SIGIL_RING_COUNT.to_le_bytes());
    for param in [0.0f32; 6] {
        program.extend_from_slice(&param.to_le_bytes());
    }
    program.extend_from_slice(&0u32.to_le_bytes());
    // Six modifiers: accelerate, sine_offset, rotate, mirror, speed_curve, curve.
    let modifiers: [(u8, u8, u16, [f32; 3]); 6] = [
        (1, 0, 0, [0.01, 5.0, 0.0]), // accelerate
        (2, 0, 0, [0.3, 8.0, 0.2]),  // sine_offset
        (3, 0, 0, [0.02, 0.0, 0.0]), // rotate
        (4, 0, 1, [0.0, 0.0, 0.0]),  // mirror
        (5, 0, 0, [0.0, 0.0, 0.0]),  // speed_curve, extra 0 -> the one curve above
        (6, 0, 0, [0.01, 0.0, 0.0]), // curve
    ];
    program.extend_from_slice(&(modifiers.len() as u16).to_le_bytes());
    for (kind, flag, extra, params) in modifiers {
        program.push(kind);
        program.push(flag);
        program.extend_from_slice(&extra.to_le_bytes());
        for param in params {
            program.extend_from_slice(&param.to_le_bytes());
        }
    }
    let mut programs = Vec::new();
    programs.extend_from_slice(&1u16.to_le_bytes());
    programs.extend_from_slice(&program);

    let mut emitter = Vec::new();
    emitter.extend_from_slice(&0u16.to_le_bytes()); // bullet_type
    emitter.extend_from_slice(&0u16.to_le_bytes()); // program 0
    emitter.push(0); // role
    emitter.push(0); // reserved
    emitter.extend_from_slice(&0u32.to_le_bytes()); // delay_ticks
    emitter.extend_from_slice(&SIGIL_VOLLEYS.to_le_bytes()); // repeat
    emitter.extend_from_slice(&1u32.to_le_bytes()); // interval_ticks
    emitter.extend_from_slice(&1.0f32.to_le_bytes()); // speed
    emitter.extend_from_slice(&0f32.to_le_bytes());
    emitter.extend_from_slice(&0f32.to_le_bytes());
    let mut emitters = Vec::new();
    emitters.extend_from_slice(&1u16.to_le_bytes());
    emitters.extend_from_slice(&emitter);

    assemble_sigil_unit(
        1,
        vec![(1, bullet_type), (2, programs), (3, emitters), (5, curves)],
    )
}

fn assemble_sigil_unit(id: u64, mut contents: Vec<(u32, Vec<u8>)>) -> SigilUnit {
    contents.sort_by_key(|&(kind, _)| kind);
    let section_count = contents.len() as u32;
    let table_len = 4u64 + u64::from(section_count) * SECTION_ENTRY_LEN;
    let mut table = Vec::new();
    table.extend_from_slice(&section_count.to_le_bytes());
    let mut body = Vec::new();
    let mut offset = table_len;
    for (kind, bytes) in &contents {
        table.extend_from_slice(&kind.to_le_bytes());
        table.extend_from_slice(&0u32.to_le_bytes());
        table.extend_from_slice(&offset.to_le_bytes());
        table.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        offset += bytes.len() as u64;
        body.extend_from_slice(bytes);
    }
    let mut payload = table;
    payload.extend_from_slice(&body);

    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&SigilUnit::MAGIC);
    out.extend_from_slice(&SigilUnit::FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    out.extend_from_slice(&payload);

    let mut hasher = grimoire_core::StableHasher::new();
    hasher.write_bytes(&out[0..24]);
    hasher.write_bytes(&out[32..out.len()]);
    let hash = hasher.finish();
    out[24..32].copy_from_slice(&hash.to_le_bytes());

    SigilUnit::from_bytes(&out).expect("hand-built bench fixture must decode")
}

/// Builds the `sigil.update` benchmark: `install`s `build_sigil_unit`'s unit with capacity for
/// every bullet the burst will spawn, and one `Emitter` entity.
#[must_use]
pub fn build_sigil_update(seed: u64) -> Simulation {
    let unit = build_sigil_unit();
    let unit_id = unit.id();
    let registry = BehaviorRegistryBuilder::new(1).build();
    let library = SigilLibrary::new(vec![unit], registry.clone()).expect("library must build");
    let mut sim = Simulation::new(seed);
    install(
        &mut sim,
        library,
        registry,
        SigilConfig::new(
            SIGIL_ENTITIES + 1,
            Vec2::new(-1.0e6, -1.0e6),
            Vec2::new(1.0e6, 1.0e6),
        ),
    )
    .expect("install must succeed");
    sim.world_mut().spawn((Emitter {
        unit: unit_id,
        emitter: 0,
        origin: Vec2::ZERO,
        rotation: 0.0,
        started_at: 0,
    },));
    sim
}

/// Runs `ticks + extra_ticks` simulation steps. Callers that want the measured region to be
/// (almost) pure `sigil.update`/`sigil.resolve` should call this only after the emitter's
/// `SIGIL_VOLLEYS` volleys have already fired (i.e. after at least that many ticks have already
/// run once, outside the measured region).
pub fn run_sigil_update_ticks(sim: &mut Simulation, ticks: u32, extra_ticks: u32) {
    for _ in 0..(ticks + extra_ticks) {
        sim.step(TickInput::default());
    }
}

/// Scenario name of the Sigil → Render extraction benchmark (contract §15.1
/// `BenchResult::scenario`, plan 0002 WP5.3).
pub const EXTRACT_SCENARIO: &str = "sigil_extract_10k";
/// Live bullets the extraction benchmark extracts every round (plan 0002 WP5.3: "Extraktions-Bench
/// ≤ 0,5 ms bei 10k").
pub const EXTRACT_BULLETS: u32 = 10_000;
/// Extractions per wall-clock sample of the extraction benchmark.
pub const EXTRACT_WALLCLOCK_ROUNDS: u32 = 200;
/// Extractions per Callgrind probe of the extraction benchmark.
pub const EXTRACT_IR_ROUNDS: u32 = 100;
/// Bullets of one bullet type in a row, like one volley of a pattern: the adapter's (unit, type)
/// cache sees realistic runs rather than a different type in every slot.
const EXTRACT_VOLLEY: u32 = 64;

/// The extraction benchmark's state: a simulation holding [`EXTRACT_BULLETS`] live, moving
/// bullets of three bullet types (one per silhouette of the bullet pass, both hostile palettes),
/// and the reused output vector the adapter appends to.
pub struct SigilExtractBench {
    sim: Simulation,
    out: Vec<BulletInstance>,
}

/// One bullet type record of `docs/formats/sigil.md` §10.3, visually a hostile bullet.
fn hostile_bullet_type(silhouette: u16, palette: u16, glow: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0.3f32.to_le_bytes()); // radius
    bytes.extend_from_slice(&0.3f32.to_le_bytes()); // collision_radius
    bytes.extend_from_slice(&0u32.to_le_bytes()); // lifetime_ticks: unbounded
    bytes.push(0); // flags
    bytes.push(0); // reserved
    bytes.extend_from_slice(&silhouette.to_le_bytes());
    bytes.extend_from_slice(&palette.to_le_bytes());
    bytes.push(1); // palette_space: hostile, the only space the bullet pass draws
    bytes.push(glow);
    bytes
}

/// Builds the extraction benchmark: [`EXTRACT_BULLETS`] bullets spawned directly into the pool
/// (no emitter, so building does not depend on pattern timing), laid out on a grid with varied
/// headings, then one simulation tick so every bullet has distinct previous and current positions
/// for the interpolation to work on.
///
/// # Panics
/// Only if the hand-built fixture stops decoding or installing, which the crate's own tests catch.
#[must_use]
pub fn build_sigil_extract(seed: u64) -> SigilExtractBench {
    let mut bullet_types = Vec::new();
    bullet_types.extend_from_slice(&3u16.to_le_bytes());
    bullet_types.extend_from_slice(&hostile_bullet_type(0, 0, 220));
    bullet_types.extend_from_slice(&hostile_bullet_type(1, 1, 128));
    bullet_types.extend_from_slice(&hostile_bullet_type(2, 0, 90));

    let mut emitters = Vec::new();
    emitters.extend_from_slice(&1u16.to_le_bytes());
    emitters.extend_from_slice(&0u16.to_le_bytes()); // bullet_type
    emitters.extend_from_slice(&0xFFFFu16.to_le_bytes()); // no program
    emitters.push(0); // role
    emitters.push(0); // reserved
    emitters.extend_from_slice(&0u32.to_le_bytes()); // delay_ticks
    emitters.extend_from_slice(&1u32.to_le_bytes()); // repeat
    emitters.extend_from_slice(&1u32.to_le_bytes()); // interval_ticks
    emitters.extend_from_slice(&0.1f32.to_le_bytes()); // speed
    emitters.extend_from_slice(&0f32.to_le_bytes()); // offset_x
    emitters.extend_from_slice(&0f32.to_le_bytes()); // offset_y

    let unit = assemble_sigil_unit(2, vec![(1, bullet_types), (3, emitters)]);
    let unit_id = unit.id();
    let registry = BehaviorRegistryBuilder::new(1).build();
    let library = SigilLibrary::new(vec![unit], registry.clone()).expect("library must build");
    let mut sim = Simulation::new(seed);
    install(
        &mut sim,
        library,
        registry,
        SigilConfig::new(
            EXTRACT_BULLETS,
            Vec2::new(-1.0e6, -1.0e6),
            Vec2::new(1.0e6, 1.0e6),
        ),
    )
    .expect("install must succeed");

    let content = sim
        .world()
        .resource::<SigilContent>()
        .expect("install adds the content")
        .clone();
    let pool = sim
        .world_mut()
        .resource_mut::<BulletPool>()
        .expect("install adds the pool");
    for i in 0..EXTRACT_BULLETS {
        let bullet_type = ((i / EXTRACT_VOLLEY) % 3) as u16;
        let position = Vec2::new((i % 100) as f32 * 0.4 - 20.0, (i / 100) as f32 * 0.4 - 20.0);
        let angle = (i % 628) as f32 * 0.01;
        pool.spawn(
            &content,
            BulletSpawn::new(unit_id, bullet_type, position, angle, 0.05),
        )
        .expect("capacity holds every bullet");
    }
    sim.step(TickInput::default());
    SigilExtractBench {
        sim,
        out: Vec::with_capacity(EXTRACT_BULLETS as usize),
    }
}

/// Runs `rounds + extra_rounds` extractions (the facade's `sigil_render::extract_bullets`, alpha
/// 0.5) into the reused output vector, clearing it before each, and returns how many instances the
/// last round extracted. `extra_rounds` is the nominal regression injection, like the other
/// benches'.
pub fn run_sigil_extract_rounds(
    bench: &mut SigilExtractBench,
    rounds: u32,
    extra_rounds: u32,
) -> u32 {
    let mut extracted = 0;
    for _ in 0..(rounds + extra_rounds) {
        bench.out.clear();
        extracted = black_box(extract_bullets(bench.sim.world(), 0.5, &mut bench.out)).extracted;
    }
    extracted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_units_rounds_up_to_a_whole_unit() {
        assert_eq!(extra_units(2_000, 5), 100);
        assert_eq!(extra_units(2_000, 12), 240);
        assert_eq!(extra_units(100, 12), 12);
    }

    #[test]
    fn run_calibration_units_is_deterministic_for_a_given_count() {
        // Not a claim about Ir (only Callgrind on the CI runner measures that); just a sanity
        // check that the same unit count always does the same arithmetic.
        assert_eq!(run_calibration_units(1_000), run_calibration_units(1_000));
    }

    #[test]
    fn run_calibration_units_zero_is_a_no_op_accumulator() {
        assert_eq!(run_calibration_units(0), 1.0f32);
    }

    #[test]
    fn ecs_benchmark_body_runs_and_touches_every_entity() {
        let mut world = build_ecs_world();
        run_ecs_rounds(&mut world, 1, 0);
        assert_eq!(world.query::<&Pos>().count(), ECS_ENTITIES);
    }

    #[test]
    fn sim_benchmark_body_runs_and_keeps_entity_count_in_range() {
        let mut sim = build_sim(42);
        run_sim_ticks(&mut sim, 60, 0);
        // The recycle system despawns and respawns one entity every 50 ticks: population stays
        // exactly SIM_ENTITIES, this is not a determinism gate, just a sanity check.
        assert_eq!(sim.world().query::<&Pos>().count(), SIM_ENTITIES);
    }

    #[test]
    fn scenario_names_are_valid_bench_result_slugs() {
        // Contract §15.1: scenario matches [a-z0-9_]{1,64}.
        for name in [ECS_SCENARIO, SIM_SCENARIO, SIGIL_SCENARIO, EXTRACT_SCENARIO] {
            assert!(!name.is_empty() && name.len() <= 64);
            assert!(
                name.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            );
        }
    }

    #[test]
    fn sigil_benchmark_body_reaches_full_population_then_keeps_it() {
        let mut sim = build_sigil_update(42);
        // Run every volley (WP5.1's `sigil.emit`), then confirm the full population is active
        // and alive before the measured region even starts.
        run_sigil_update_ticks(&mut sim, SIGIL_VOLLEYS, 0);
        assert_eq!(
            sim.world()
                .resource::<BulletPool>()
                .expect("install must have added the pool")
                .len(),
            SIGIL_ENTITIES,
            "every volley must have fired and every bullet must still be alive"
        );
        run_sigil_update_ticks(&mut sim, SIGIL_IR_TICKS, 0);
        assert_eq!(
            sim.world().resource::<BulletPool>().unwrap().len(),
            SIGIL_ENTITIES,
            "the measured region keeps every bullet alive (unbounded lifetime, generous bounds)"
        );
    }

    #[test]
    fn sigil_benchmark_is_deterministic_for_a_given_seed() {
        let mut a = build_sigil_update(7);
        let mut b = build_sigil_update(7);
        run_sigil_update_ticks(&mut a, SIGIL_VOLLEYS + SIGIL_IR_TICKS, 0);
        run_sigil_update_ticks(&mut b, SIGIL_VOLLEYS + SIGIL_IR_TICKS, 0);
        assert_eq!(a.state_hash(), b.state_hash());
    }

    #[test]
    fn sigil_extract_benchmark_extracts_every_live_bullet_each_round() {
        let mut bench = build_sigil_extract(11);
        assert_eq!(
            bench.sim.world().resource::<BulletPool>().unwrap().len(),
            EXTRACT_BULLETS
        );
        assert_eq!(run_sigil_extract_rounds(&mut bench, 2, 0), EXTRACT_BULLETS);
        assert_eq!(
            bench.out.len(),
            EXTRACT_BULLETS as usize,
            "cleared, not appended"
        );
        let silhouettes: std::collections::BTreeSet<u16> = bench
            .out
            .iter()
            .map(|instance| instance.silhouette)
            .collect();
        assert_eq!(silhouettes.len(), 3, "all three bullet types are live");
        let moved = bench
            .sim
            .world()
            .resource::<BulletPool>()
            .unwrap()
            .iter()
            .all(|bullet| bullet.position() != bullet.previous_position());
        assert!(
            moved,
            "every bullet has distinct previous and current positions"
        );
    }

    #[test]
    fn sigil_extract_benchmark_leaves_the_simulation_untouched() {
        let mut bench = build_sigil_extract(5);
        let before = bench.sim.state_hash();
        run_sigil_extract_rounds(&mut bench, 3, 0);
        assert_eq!(bench.sim.state_hash(), before);
    }
}
