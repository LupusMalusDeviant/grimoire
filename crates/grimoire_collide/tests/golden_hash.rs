//! Golden hash of a fixed scene under a fixed set of queries (contract §14). The scene and the
//! frozen constant live in `golden_scene/mod.rs`, shared with the thread-pool hash gate in
//! `grimoire_exec/tests/hash_gate.rs`; a genuine behaviour change (not just a refactor) renews
//! `GOLDEN_QUERY_HASH` deliberately.

mod golden_scene;

use grimoire_collide::{BatchHits, SpatialGrid};
use grimoire_ecs::{Executor, PermutedExecutor, SequentialExecutor};

#[test]
fn golden_query_hash_is_stable() {
    let hash = golden_scene::sequential_query_hash();
    assert_eq!(
        hash,
        golden_scene::GOLDEN_QUERY_HASH,
        "golden hash changed: {hash:#018x} (update GOLDEN_QUERY_HASH deliberately if this is an \
         intentional behaviour change)"
    );
}

/// The data-parallel path reproduces the golden hash for the single-thread executors; real
/// thread pools follow in `grimoire_exec/tests/hash_gate.rs`.
#[test]
fn golden_query_hash_holds_for_the_data_parallel_path() {
    let executors: [&dyn Executor; 4] = [
        &SequentialExecutor,
        &PermutedExecutor::new(1),
        &PermutedExecutor::new(3),
        &PermutedExecutor::reversed(),
    ];
    for executor in executors {
        assert_eq!(
            golden_scene::parallel_query_hash(executor),
            golden_scene::GOLDEN_QUERY_HASH
        );
    }
}

/// The block scene spans several blocks on both data-parallel paths, so the thread-pool gate in
/// `grimoire_exec` really distributes work; it too has a frozen hash.
#[test]
fn golden_block_query_hash_is_stable_on_every_path() {
    assert!(golden_scene::block_items().len() > 2 * grimoire_ecs::QUERY_BLOCK_SIZE);
    assert!(golden_scene::block_queries().len() >= 64);
    let hash = golden_scene::block_sequential_query_hash();
    assert_eq!(
        hash,
        golden_scene::GOLDEN_BLOCK_QUERY_HASH,
        "block scene hash changed: {hash:#018x}"
    );
    // A scene whose queries find next to nothing would prove little about the batch assembly.
    let mut grid = SpatialGrid::new(golden_scene::block_config()).expect("valid config");
    grid.rebuild(golden_scene::block_items());
    let mut out = BatchHits::default();
    grid.overlapping_batch(
        &SequentialExecutor,
        &golden_scene::block_queries(),
        &mut out,
    );
    let total: usize = (0..out.len()).map(|index| out.hits(index).len()).sum();
    let empty = (0..out.len())
        .filter(|&index| out.hits(index).is_empty())
        .count();
    println!("block scene: {total} hits, {empty} empty queries");
    assert!(
        total > 300 && empty < 16,
        "{total} hits, {empty} empty queries"
    );
    let executors: [&dyn Executor; 4] = [
        &SequentialExecutor,
        &PermutedExecutor::new(1),
        &PermutedExecutor::new(3),
        &PermutedExecutor::reversed(),
    ];
    for executor in executors {
        assert_eq!(golden_scene::block_parallel_query_hash(executor), hash);
    }
}
