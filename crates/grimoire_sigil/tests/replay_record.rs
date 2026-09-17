//! WP7.1: recording a Sigil session as a replay v2 (contract §8.1, §11.8 "Replay v2").
//!
//! A recording session writes the content manifest right after `install` into the header, pushes
//! every tick's input into the log and hands every `SwapReport` to `ReplayHeader::record_swap`.
//! These tests record such a session with hot swaps, encode and decode it, and check what the
//! header promises: without the swapped units the run reproduces exactly up to the first swap tick
//! and no further; with them, applying each recorded swap right before its tick reproduces every
//! state hash. Units are hand-built from `docs/formats/sigil.md` §10 (`tests/support`).

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;

use grimoire_core::Vec2;
use grimoire_sigil::{
    BehaviorRegistryBuilder, Emitter, SigilConfig, SigilContent, SigilLibrary, SigilUnit, UnitId,
    install, replace_unit,
};
use grimoire_sim::{
    ContentManifestHash, InputFrame, InputLog, Replay, ReplayHeader, Simulation, SwapRecord,
    TickInput, replay,
};
use support::{Sections, Timing, When, block, block_kind, bullet_type, emitter, program};

const SWAPPED: u64 = 21;
const BYSTANDER: u64 = 22;
const SEED: u64 = 5;
const TICK_RATE_HZ: u32 = 60;
const TICKS: u64 = 60;

fn every(interval: u32) -> Timing {
    Timing {
        delay: 0,
        repeat: u32::MAX,
        interval,
    }
}

/// Version 1 of unit 21: a 4-shot ring every 10 ticks.
fn v1() -> SigilUnit {
    Sections {
        bullet_types: vec![bullet_type(0, 0, 0)],
        programs: vec![program(block(block_kind::RING, 4, [0.0; 6]), &[])],
        emitters: vec![emitter(0, 0, 0, every(10), 0.5)],
        ..Sections::default()
    }
    .unit(SWAPPED)
}

/// Version 2 of unit 21: a 3-shot fan every 7 ticks whose bullets reverse at age 3.
fn v2() -> SigilUnit {
    Sections {
        bullet_types: vec![bullet_type(0, 0, 0), bullet_type(0, 0, 1)],
        programs: vec![
            program(block(block_kind::RING, 4, [0.0; 6]), &[]),
            program(
                block(block_kind::FAN, 3, [0.6, 0.0, 0.0, 0.0, 0.0, 0.0]),
                &[],
            ),
        ],
        emitters: vec![emitter(1, 1, 0, every(7), 0.9)],
        scripts: vec![support::script(1, None, &[support::reverse(When::Time(3))])],
        ..Sections::default()
    }
    .unit(SWAPPED)
}

/// Unit 22: a 2-shot ring every 5 ticks that no swap touches.
fn bystander() -> SigilUnit {
    Sections {
        bullet_types: vec![bullet_type(0, 0, 0)],
        programs: vec![program(block(block_kind::RING, 2, [0.0; 6]), &[])],
        emitters: vec![emitter(0, 0, 0, every(5), 0.3)],
        ..Sections::default()
    }
    .unit(BYSTANDER)
}

/// The session's setup: both units installed, one emitter each, seed [`SEED`].
fn session() -> Simulation {
    let registry = BehaviorRegistryBuilder::new(1).build();
    let library =
        SigilLibrary::new(vec![v1(), bystander()], Arc::clone(&registry)).expect("library builds");
    let mut sim = Simulation::new(SEED);
    install(
        &mut sim,
        library,
        registry,
        SigilConfig::new(4_096, Vec2::new(-1.0e4, -1.0e4), Vec2::new(1.0e4, 1.0e4)),
    )
    .expect("install succeeds");
    for unit in [SWAPPED, BYSTANDER] {
        sim.world_mut().spawn((Emitter {
            unit: UnitId(unit),
            emitter: 0,
            origin: Vec2::ZERO,
            rotation: 0.0,
            started_at: 0,
        },));
    }
    sim
}

fn manifest(sim: &Simulation) -> ContentManifestHash {
    sim.world()
        .resource::<SigilContent>()
        .expect("content installed")
        .epoch()
        .manifest_hash
}

/// Input of `tick`: Sigil reads none of it, but the log must carry real frames.
fn input(tick: u64) -> TickInput {
    let mut input = TickInput::default();
    input.slots[0] = InputFrame {
        axes: [tick as i16, 1, -1, 0],
        buttons: (tick % 3) as u32,
    };
    input
}

/// A recorded session: the replay, the state hash after every tick, and for every swap the units
/// it installed, keyed by the tick it took effect at.
struct Recording {
    replay: Replay,
    hashes: Vec<u64>,
    swapped_units: BTreeMap<u64, Vec<SigilUnit>>,
}

/// Records [`TICKS`] ticks of [`session`], swapping unit 21 to each unit of `swaps[t]` right
/// before tick `t`, the way a harness or the debug link would.
fn record(swaps: &BTreeMap<u64, Vec<fn() -> SigilUnit>>) -> Recording {
    let mut sim = session();
    let mut header = ReplayHeader::for_this_build(manifest(&sim));
    header
        .app_metadata
        .insert("app.engine_pin".to_string(), "v0.4.0".to_string());
    header.app_metadata.insert(
        "app.git".to_string(),
        "0123456789abcdef0123456789abcdef01234567".to_string(),
    );
    let mut log = InputLog {
        seed: sim.seed(),
        tick_rate_hz: TICK_RATE_HZ,
        frames: Vec::new(),
    };
    let mut hashes = Vec::new();
    let mut swapped_units = BTreeMap::new();
    for tick in 0..TICKS {
        for build in swaps.get(&tick).into_iter().flatten() {
            let report = replace_unit(&mut sim, build()).expect("the swap succeeds");
            assert_eq!(report.effective_tick, tick);
            header
                .record_swap(report.into())
                .expect("swaps arrive in tick order");
            swapped_units
                .entry(tick)
                .or_insert_with(Vec::new)
                .push(build());
        }
        log.frames.push(input(tick));
        sim.step(input(tick));
        hashes.push(sim.state_hash());
    }
    Recording {
        replay: Replay {
            header: Some(header),
            log,
        },
        hashes,
        swapped_units,
    }
}

#[test]
fn a_recorded_session_marks_its_swaps_and_reproduces_exactly_up_to_the_first_swap() {
    // Two swaps at one boundary (to v2 and straight back to v1), one more later.
    let swaps: BTreeMap<u64, Vec<fn() -> SigilUnit>> =
        BTreeMap::from([(20, vec![v2 as fn() -> SigilUnit, v1]), (40, vec![v2])]);
    let recording = record(&swaps);
    let bytes = recording.replay.to_bytes().expect("the recording encodes");
    let decoded = Replay::from_bytes(&bytes).expect("the recording decodes");
    assert_eq!(decoded, recording.replay);

    let header = decoded.header.as_ref().expect("version 2");
    let fresh = session();
    let installed = manifest(&fresh);
    assert_eq!(
        header.content_manifest, installed,
        "the header names the content right after install"
    );
    // The double swap at tick 20 leaves one entry with the final content. That content is v1
    // again, so the manifest alone would not show that anything happened; the marker does.
    let mut with_v2 = session();
    let v2_manifest = replace_unit(&mut with_v2, v2())
        .expect("the swap succeeds")
        .epoch
        .manifest_hash;
    assert_ne!(v2_manifest, installed);
    assert_eq!(
        header.swaps,
        vec![
            SwapRecord::new(20, installed),
            SwapRecord::new(40, v2_manifest),
        ]
    );
    assert!(!header.is_golden_eligible());
    assert_eq!(header.app_metadata.len(), 2);
    assert_eq!(header.app_metadata["app.engine_pin"], "v0.4.0");

    // Without the units: exactly the recorded hashes up to the first swap tick, then divergence.
    let first_swap = header.swaps[0].tick as usize;
    let mut replayed = session();
    let prefix = InputLog {
        seed: decoded.log.seed,
        tick_rate_hz: decoded.log.tick_rate_hz,
        frames: decoded.log.frames[..first_swap].to_vec(),
    };
    let checkpoints = replay(&mut replayed, &prefix, 1);
    let expected: Vec<(u64, u64)> = recording.hashes[..first_swap]
        .iter()
        .enumerate()
        .map(|(index, &hash)| (index as u64 + 1, hash))
        .collect();
    assert_eq!(checkpoints, expected);
    replayed.step(decoded.log.frames[first_swap]);
    assert_ne!(
        replayed.state_hash(),
        recording.hashes[first_swap],
        "past the swap tick the run no longer matches without the swap"
    );
}

#[test]
fn replaying_the_recorded_swaps_before_their_ticks_reproduces_every_state_hash() {
    let swaps: BTreeMap<u64, Vec<fn() -> SigilUnit>> = BTreeMap::from([
        (0, vec![v2 as fn() -> SigilUnit]),
        (25, vec![v1]),
        (59, vec![v2]),
    ]);
    let recording = record(&swaps);
    let decoded =
        Replay::from_bytes(&recording.replay.to_bytes().expect("encodes")).expect("decodes");
    let header = decoded.header.expect("version 2");
    assert_eq!(
        header
            .swaps
            .iter()
            .map(|swap| swap.tick)
            .collect::<Vec<_>>(),
        vec![0, 25, 59]
    );

    let mut sim = session();
    assert_eq!(manifest(&sim), header.content_manifest);
    let mut pending = header.swaps.iter().peekable();
    for (tick, frame) in decoded.log.frames.iter().enumerate() {
        let tick = tick as u64;
        if let Some(swap) = pending.next_if(|swap| swap.tick == tick) {
            for unit in &recording.swapped_units[&tick] {
                replace_unit(&mut sim, unit.clone()).expect("the swap succeeds");
            }
            assert_eq!(
                manifest(&sim),
                swap.content_manifest,
                "tick {tick}: the swapped content matches the marker"
            );
        }
        sim.step(*frame);
        assert_eq!(
            sim.state_hash(),
            recording.hashes[tick as usize],
            "tick {tick}"
        );
    }
    assert!(pending.next().is_none());
}
