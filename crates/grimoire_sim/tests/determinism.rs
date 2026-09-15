//! P0 determinism gate (PRD-0018 FR-04, plan WP5.6).
//!
//! A demo simulation with at least 2 000 moving entities, a random spawner and despawner,
//! component inserts/removes and input-driven steering runs 10 000 ticks. The tests check:
//!
//! 1. two independent runs produce identical hash sequences (every 600 ticks and the final tick);
//! 2. a snapshot at tick 4 000, restored into a freshly built simulation, continues identically;
//! 3. `replay` over the recorded (and re-encoded) `InputLog` reproduces the hash sequence;
//! 4. the final hash equals a golden constant (`scenario::GOLDEN_FINAL_HASH`) that CI reproduces
//!    on Windows, Linux and macOS, which makes every CI run a cross-platform determinism check.

mod scenario;

use std::sync::OnceLock;

use grimoire_ecs::{Entity, With};
use grimoire_sim::{InputLog, SimSnapshot, replay};
use scenario::{
    Agitated, GOLDEN_FINAL_HASH, HASH_EVERY, MAX_ENTITIES, MIN_ENTITIES, SEED, SNAPSHOT_TICK,
    TICK_RATE_HZ, TOTAL_TICKS, build_simulation, run_until, scripted_input,
};

struct Recording {
    hashes: Vec<(u64, u64)>,
    log: InputLog,
    snapshot: SimSnapshot,
    snapshot_hash: u64,
    min_entities: usize,
    max_entities: usize,
    max_agitated: usize,
    despawns_seen: bool,
}

fn record() -> Recording {
    let mut sim = build_simulation(SEED);
    let mut log = InputLog {
        seed: SEED,
        tick_rate_hz: TICK_RATE_HZ,
        frames: Vec::new(),
    };
    let mut snapshot = None;
    let mut min_entities = usize::MAX;
    let mut max_entities = 0;
    let mut max_agitated = 0;
    let mut despawns_seen = false;
    let hashes = run_until(&mut sim, TOTAL_TICKS, |sim| {
        let tick = sim.tick();
        log.frames.push(scripted_input(tick - 1));
        let world = sim.world();
        min_entities = min_entities.min(world.entity_count());
        max_entities = max_entities.max(world.entity_count());
        if tick.is_multiple_of(50) {
            max_agitated = max_agitated.max(world.query::<(With<Agitated>,)>().count());
            despawns_seen |= world
                .query::<(Entity,)>()
                .any(|(entity,)| entity.generation() > 0);
        }
        if tick == SNAPSHOT_TICK {
            snapshot = Some((sim.snapshot(), sim.state_hash()));
        }
    });
    let (snapshot, snapshot_hash) = snapshot.expect("the run passes the snapshot tick");
    Recording {
        hashes,
        log,
        snapshot,
        snapshot_hash,
        min_entities,
        max_entities,
        max_agitated,
        despawns_seen,
    }
}

fn reference() -> &'static Recording {
    static REFERENCE: OnceLock<Recording> = OnceLock::new();
    REFERENCE.get_or_init(record)
}

#[test]
fn scenario_exercises_the_engine() {
    let reference = reference();
    assert!(
        reference.min_entities >= MIN_ENTITIES,
        "{}",
        reference.min_entities
    );
    assert!(reference.max_entities <= MAX_ENTITIES);
    assert!(reference.max_entities > MIN_ENTITIES);
    assert!(reference.max_agitated > 0);
    assert!(reference.despawns_seen);
    assert_eq!(reference.log.frames.len(), TOTAL_TICKS as usize);
    let ticks: Vec<u64> = reference.hashes.iter().map(|&(tick, _)| tick).collect();
    let expected: Vec<u64> = (1..=16)
        .map(|n| n * HASH_EVERY)
        .chain([TOTAL_TICKS])
        .collect();
    assert_eq!(ticks, expected);
}

#[test]
fn double_run_gives_identical_hash_sequences() {
    let mut second = build_simulation(SEED);
    let hashes = run_until(&mut second, TOTAL_TICKS, |_| {});
    assert_eq!(hashes, reference().hashes);
}

#[test]
fn restored_snapshot_continues_identically() {
    let reference = reference();
    let mut restored = build_simulation(SEED);
    restored.restore(&reference.snapshot);
    assert_eq!(restored.tick(), SNAPSHOT_TICK);
    assert_eq!(restored.state_hash(), reference.snapshot_hash);

    let hashes = run_until(&mut restored, TOTAL_TICKS, |_| {});
    let expected: Vec<(u64, u64)> = reference
        .hashes
        .iter()
        .copied()
        .filter(|&(tick, _)| tick > SNAPSHOT_TICK)
        .collect();
    assert_eq!(hashes, expected);
}

#[test]
fn replay_of_the_recorded_log_reproduces_the_run() {
    let reference = reference();
    let decoded = InputLog::from_bytes(&reference.log.to_bytes()).expect("valid log");
    assert_eq!(decoded, reference.log);
    let mut sim = build_simulation(decoded.seed);
    assert_eq!(replay(&mut sim, &decoded, HASH_EVERY), reference.hashes);
}

#[test]
fn final_hash_matches_golden_value() {
    let hashes = &reference().hashes;
    let &(tick, hash) = hashes.last().expect("at least the final checkpoint");
    assert_eq!(tick, TOTAL_TICKS);
    assert_eq!(
        hash,
        GOLDEN_FINAL_HASH,
        "{}",
        checkpoint_report(hash, hashes)
    );
}

/// Failure text for the golden gate: the final hash, then one `tick hash` line per checkpoint.
fn checkpoint_report(final_hash: u64, hashes: &[(u64, u64)]) -> String {
    let lines: Vec<String> = hashes
        .iter()
        .map(|&(tick, hash)| format!("  {tick:>6} {hash:#018x}"))
        .collect();
    format!(
        "measured final hash: {final_hash}; checkpoints (tick, state_hash):\n{}",
        lines.join("\n")
    )
}

#[test]
fn checkpoint_report_lists_every_checkpoint_in_order() {
    let report = checkpoint_report(7, &[(600, 0xab), (10_000, 7)]);
    assert_eq!(
        report,
        "measured final hash: 7; checkpoints (tick, state_hash):\n     600 0x00000000000000ab\n   \
         10000 0x0000000000000007"
    );
}
