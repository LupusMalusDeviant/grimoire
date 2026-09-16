//! WP5.1 end-to-end interpreter gate: [`install`] wired into a real [`Simulation`], firing real
//! emitters through all seven blocks and six modifiers, checked against the three things the plan
//! demands from the first commit (Plan 0002 WP5.1):
//!
//! - `snapshot_restore_is_bit_identical_with_active_bullets`: snapshot -> restore -> N ticks is
//!   bit-identical while bullets are alive.
//! - `golden_hash_*`: one frozen `state_hash` per test unit (two units here, `MOVEMENT` and
//!   `SCATTER_AIM_CURVE`, covering all seven blocks and six modifiers between them — see each
//!   unit's doc comment for which).
//! - `debug_assertion_fires_before_nan_reaches_the_pool` (in `crate::runtime`'s own unit tests,
//!   not here, since only `pub(crate)` code can inject a `NaN` directly): documented here as the
//!   third leg of the gate for anyone looking for it from the outside.
//!
//! Units are hand-assembled bytes, independent of [`SigilUnit::to_bytes`], following this crate's
//! existing fixture convention (`tests/golden.rs`, `src/test_support.rs`) — only through the
//! public API, since an integration test cannot reach `pub(crate)` items.

use std::sync::Arc;

use grimoire_core::{StableHasher, Vec2};
use grimoire_sigil::{
    AimTarget, BehaviorRegistryBuilder, BulletPool, Emitter, SigilConfig, SigilLibrary, SigilUnit,
    install,
};
use grimoire_sim::{Simulation, TickInput};

const HEADER_LEN: usize = 40;
const SECTION_ENTRY_LEN: u64 = 24;

/// One `BulletTypes` record (contract §11.2, `docs/formats/sigil.md` §10.3): a plain, unbounded
/// (`lifetime_ticks == 0`) bullet type, since these tests care about bullets that stay alive.
fn bullet_type_bytes() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&1u16.to_le_bytes()); // count
    out.extend_from_slice(&1.0f32.to_le_bytes()); // radius
    out.extend_from_slice(&1.0f32.to_le_bytes()); // collision_radius
    out.extend_from_slice(&0u32.to_le_bytes()); // lifetime_ticks: unbounded
    out.push(0); // flags
    out.push(0); // reserved
    out.extend_from_slice(&0u16.to_le_bytes()); // silhouette
    out.extend_from_slice(&0u16.to_le_bytes()); // palette
    out.push(0); // palette_space
    out.push(0); // glow
    out
}

/// One `BlockDef` (`docs/formats/sigil.md` §10.4): `kind`, `count`/`arms`, six `params`, no
/// `scatter.seed` (`seed_hash: 0`).
fn block_bytes(kind: u8, count: u16, params: [f32; 6]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(kind);
    out.push(0); // reserved
    out.extend_from_slice(&count.to_le_bytes());
    for param in params {
        out.extend_from_slice(&param.to_le_bytes());
    }
    out.extend_from_slice(&0u32.to_le_bytes()); // seed_hash
    out
}

/// One `ModifierDef` (`docs/formats/sigil.md` §10.4).
fn modifier_bytes(kind: u8, flag: u8, extra: u16, params: [f32; 3]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(kind);
    out.push(flag);
    out.extend_from_slice(&extra.to_le_bytes());
    for param in params {
        out.extend_from_slice(&param.to_le_bytes());
    }
    out
}

/// One `Programs`-section entry: `block_bytes` followed by every modifier in `modifiers`.
fn program_bytes(block: Vec<u8>, modifiers: &[Vec<u8>]) -> Vec<u8> {
    let mut out = block;
    out.extend_from_slice(&(modifiers.len() as u16).to_le_bytes());
    for modifier in modifiers {
        out.extend_from_slice(modifier);
    }
    out
}

/// Wraps `count` program entries into the `Programs` section content (kind 2).
fn programs_section(programs: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(programs.len() as u16).to_le_bytes());
    for program in programs {
        out.extend_from_slice(program);
    }
    out
}

/// One `EmitterRecord` (`docs/formats/sigil.md` §10.5): bullet type `0`, `program` (or
/// `0xFFFF` for none), primary role, firing every `interval` ticks forever from tick `0`.
fn emitter_record_bytes(program: u16, speed: f32, interval: u32) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // bullet_type
    out.extend_from_slice(&program.to_le_bytes());
    out.push(0); // role: primary
    out.push(0); // reserved
    out.extend_from_slice(&0u32.to_le_bytes()); // delay_ticks
    out.extend_from_slice(&u32::MAX.to_le_bytes()); // repeat: forever
    out.extend_from_slice(&interval.to_le_bytes());
    out.extend_from_slice(&speed.to_le_bytes());
    out.extend_from_slice(&0f32.to_le_bytes()); // offset_x
    out.extend_from_slice(&0f32.to_le_bytes()); // offset_y
    out
}

fn emitters_section(records: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(records.len() as u16).to_le_bytes());
    for record in records {
        out.extend_from_slice(record);
    }
    out
}

/// One `Curves`-section entry (`docs/formats/sigil.md` §10.6): a keyframe list.
fn curve_bytes(keys: &[(u32, f32)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(keys.len() as u16).to_le_bytes());
    for &(at_ticks, mul) in keys {
        out.extend_from_slice(&at_ticks.to_le_bytes());
        out.extend_from_slice(&mul.to_le_bytes());
    }
    out
}

fn curves_section(curves: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(curves.len() as u16).to_le_bytes());
    for curve in curves {
        out.extend_from_slice(curve);
    }
    out
}

/// Assembles a complete unit's bytes (header, section table, `content_hash`) from already-encoded
/// section contents, exactly like `src/test_support.rs`'s own `assemble_unit` (duplicated here:
/// an integration test cannot reach that `pub(crate)` helper).
fn assemble_unit(id: u64, mut contents: Vec<(u32, Vec<u8>)>) -> SigilUnit {
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

    let mut hasher = StableHasher::new();
    hasher.write_bytes(&out[0..24]);
    hasher.write_bytes(&out[32..out.len()]);
    let hash = hasher.finish();
    out[24..32].copy_from_slice(&hash.to_le_bytes());

    SigilUnit::from_bytes(&out).expect("hand-built fixture must decode")
}

/// Block kind tags (`docs/formats/sigil.md` §10.4); this crate keeps them `pub(crate)`-only, so
/// an integration test names them again.
mod block_kind {
    pub const RING: u8 = 1;
    pub const SPIRAL: u8 = 2;
    pub const FAN: u8 = 3;
    pub const AIMED: u8 = 4;
    pub const WAVE: u8 = 5;
    pub const LINE: u8 = 6;
    pub const SCATTER: u8 = 7;
}

/// Modifier kind tags (`docs/formats/sigil.md` §10.4).
mod modifier_kind {
    pub const ACCELERATE: u8 = 1;
    pub const SINE_OFFSET: u8 = 2;
    pub const ROTATE: u8 = 3;
    pub const MIRROR: u8 = 4;
    pub const SPEED_CURVE: u8 = 5;
    pub const CURVE: u8 = 6;
}

/// Unit `1`: `ring`+`accelerate`, `spiral`+`rotate`, `wave`+`curve`, `line` (no modifier). Covers
/// three of the seven blocks and three of the six modifiers, all deterministic (no `scatter`, so
/// no randomness).
fn movement_unit() -> SigilUnit {
    let programs = programs_section(&[
        program_bytes(
            block_bytes(block_kind::RING, 6, [0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            &[modifier_bytes(
                modifier_kind::ACCELERATE,
                0,
                0,
                [0.02, 4.0, 0.0],
            )],
        ),
        program_bytes(
            block_bytes(block_kind::SPIRAL, 3, [0.15, 0.0, 0.0, 0.0, 0.0, 0.0]),
            &[modifier_bytes(
                modifier_kind::ROTATE,
                0,
                0,
                [0.05, 0.0, 0.0],
            )],
        ),
        program_bytes(
            block_bytes(block_kind::WAVE, 5, [0.3, 4.0, 0.0, 0.0, 1.0, 0.0]),
            &[modifier_bytes(modifier_kind::CURVE, 0, 0, [0.02, 0.0, 0.0])],
        ),
        program_bytes(
            block_bytes(block_kind::LINE, 4, [1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
            &[],
        ),
    ]);
    let emitters = emitters_section(&[
        emitter_record_bytes(0, 1.0, 3),
        emitter_record_bytes(1, 1.2, 4),
        emitter_record_bytes(2, 0.8, 5),
        emitter_record_bytes(3, 1.0, 6),
    ]);
    assemble_unit(
        1,
        vec![(1, bullet_type_bytes()), (2, programs), (3, emitters)],
    )
}

/// Unit `2`: `scatter`+`sine_offset` (randomised), `aimed` (reads `AimTarget`), `fan`+`mirror`+
/// `speed_curve` (a `Curves` section). Covers the remaining four of the seven blocks and the
/// remaining three of the six modifiers.
fn scatter_aim_curve_unit() -> SigilUnit {
    let curves = curves_section(&[curve_bytes(&[(0, 1.0), (20, 2.0), (40, 0.5)])]);
    let programs = programs_section(&[
        program_bytes(
            block_bytes(block_kind::SCATTER, 5, [0.6, 0.0, 0.4, 0.0, 0.0, 0.0]),
            &[modifier_bytes(
                modifier_kind::SINE_OFFSET,
                0,
                0,
                [0.4, 12.0, 0.3],
            )],
        ),
        program_bytes(
            block_bytes(block_kind::AIMED, 3, [0.3, 0.0, 0.0, 0.0, 0.0, 0.0]),
            &[],
        ),
        program_bytes(
            block_bytes(block_kind::FAN, 4, [1.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            &[
                modifier_bytes(modifier_kind::MIRROR, 0, 1, [0.0, 0.0, 0.0]),
                modifier_bytes(modifier_kind::SPEED_CURVE, 0, 0, [0.0, 0.0, 0.0]),
            ],
        ),
    ]);
    let emitters = emitters_section(&[
        emitter_record_bytes(0, 1.0, 4),
        emitter_record_bytes(1, 1.5, 5),
        emitter_record_bytes(2, 1.0, 4),
    ]);
    assemble_unit(
        2,
        vec![
            (1, bullet_type_bytes()),
            (2, programs),
            (3, emitters),
            (5, curves),
        ],
    )
}

/// Builds a simulation with `unit` installed, one `Emitter` entity per emitter the unit defines,
/// and a fixed `AimTarget` (`aimed` needs one to be exercised meaningfully).
fn setup(seed: u64, unit: SigilUnit) -> Simulation {
    let unit_id = unit.id();
    let emitter_count = unit.emitter_count();
    let registry = BehaviorRegistryBuilder::new(1).build();
    let library = SigilLibrary::new(vec![unit], Arc::clone(&registry)).expect("library must build");
    let mut sim = Simulation::new(seed);
    install(
        &mut sim,
        library,
        registry,
        SigilConfig::new(4096, Vec2::new(-1.0e4, -1.0e4), Vec2::new(1.0e4, 1.0e4)),
    )
    .expect("install must succeed");
    sim.world_mut()
        .insert_resource(AimTarget(Some(Vec2::new(40.0, 25.0))));
    for index in 0..emitter_count {
        sim.world_mut().spawn((Emitter {
            unit: unit_id,
            emitter: index,
            origin: Vec2::new(f32::from(index) * 3.0, 0.0),
            rotation: 0.1 * f32::from(index),
            started_at: 0,
        },));
    }
    sim
}

/// Runs `ticks` steps with the default (all-neutral) input.
fn run(sim: &mut Simulation, ticks: u32) {
    for _ in 0..ticks {
        sim.step(TickInput::default());
    }
}

#[test]
fn snapshot_restore_is_bit_identical_with_active_bullets() {
    let unit = movement_unit();
    let unit_id = unit.id();
    let registry = BehaviorRegistryBuilder::new(1).build();
    let library = SigilLibrary::new(vec![unit], Arc::clone(&registry)).expect("library must build");
    let mut sim = Simulation::new(7);
    install(
        &mut sim,
        library,
        registry,
        SigilConfig::new(4096, Vec2::new(-1.0e4, -1.0e4), Vec2::new(1.0e4, 1.0e4)),
    )
    .expect("install must succeed");
    for index in 0..4u16 {
        sim.world_mut().spawn((Emitter {
            unit: unit_id,
            emitter: index,
            origin: Vec2::new(f32::from(index) * 3.0, 0.0),
            rotation: 0.1 * f32::from(index),
            started_at: 0,
        },));
    }

    run(&mut sim, 15);
    let active = sim
        .world()
        .resource::<BulletPool>()
        .expect("pool must be installed")
        .len();
    assert!(
        active > 0,
        "the gate requires active bullets at snapshot time, found none"
    );
    let snapshot = sim.snapshot();

    let mut continued = sim;
    run(&mut continued, 20);
    let continued_hash = continued.state_hash();

    // A fresh simulation, same setup and schedule (contract §8.2's restore contract), never
    // stepped itself: `restore` must make it continue exactly like `continued` from here.
    let registry = BehaviorRegistryBuilder::new(1).build();
    let library = SigilLibrary::new(vec![movement_unit()], Arc::clone(&registry))
        .expect("library must build");
    let mut restored = Simulation::new(999); // deliberately wrong seed: restore must overwrite it
    install(
        &mut restored,
        library,
        registry,
        SigilConfig::new(4096, Vec2::new(-1.0e4, -1.0e4), Vec2::new(1.0e4, 1.0e4)),
    )
    .expect("install must succeed");
    restored.restore(&snapshot);
    run(&mut restored, 20);
    let restored_hash = restored.state_hash();

    assert_eq!(
        continued_hash, restored_hash,
        "snapshot -> restore -> N ticks must be bit-identical with active bullets"
    );
}

/// Frozen `state_hash` after 30 ticks of [`movement_unit`] (seed `1001`). If this fails after an
/// *intentional* interpreter change, recompute it (the assertion prints the actual value) and
/// update this constant, exactly like `src/pool.rs`'s `GOLDEN_POOL_HASH`.
const GOLDEN_MOVEMENT_HASH: u64 = 0x20f5_6209_31a9_5ff9;

#[test]
fn golden_hash_movement_unit() {
    let mut sim = setup(1001, movement_unit());
    run(&mut sim, 30);
    let hash = sim.state_hash();
    assert_eq!(
        hash, GOLDEN_MOVEMENT_HASH,
        "movement-unit state hash changed; if intentional, update GOLDEN_MOVEMENT_HASH to {hash:#018x}"
    );
}

/// Frozen `state_hash` after 30 ticks of [`scatter_aim_curve_unit`] (seed `2002`), exercising
/// `derive_block_rng`, `AimTarget` and `speed_curve` together.
const GOLDEN_SCATTER_AIM_CURVE_HASH: u64 = 0x3b5d_0744_4ca8_0b7c;

#[test]
fn golden_hash_scatter_aim_curve_unit() {
    let mut sim = setup(2002, scatter_aim_curve_unit());
    run(&mut sim, 30);
    let hash = sim.state_hash();
    assert_eq!(
        hash, GOLDEN_SCATTER_AIM_CURVE_HASH,
        "scatter/aim/curve-unit state hash changed; if intentional, update \
         GOLDEN_SCATTER_AIM_CURVE_HASH to {hash:#018x}"
    );
}
