//! Data-parallel queries in fixed blocks (engine ADR-0006, building block 4).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use grimoire_core::{StableHasher, impl_stable_hash};
use grimoire_ecs::{
    Entity, Executor, PermutedExecutor, QUERY_BLOCK_SIZE, SequentialExecutor, Without, World,
};
use proptest::prelude::*;

const B: usize = QUERY_BLOCK_SIZE;

#[derive(Clone, Debug, PartialEq)]
struct P {
    x: f32,
}
impl_stable_hash!(P { x });

#[derive(Clone, Debug, PartialEq)]
struct V {
    x: f32,
}
impl_stable_hash!(V { x });

#[derive(Clone, Debug, PartialEq)]
struct T {
    n: u32,
}
impl_stable_hash!(T { n });

fn hash(world: &World) -> u64 {
    let mut hasher = StableHasher::new();
    world.stable_hash(&mut hasher);
    hasher.finish()
}

fn executors() -> Vec<Arc<dyn Executor>> {
    vec![
        Arc::new(SequentialExecutor),
        Arc::new(PermutedExecutor::new(11)),
        Arc::new(PermutedExecutor::reversed()),
    ]
}

/// Row counts around the block size.
fn row_count() -> impl Strategy<Value = usize> {
    prop::sample::select(vec![0, 1, B - 1, B, B + 1, 3 * B + 7])
}

/// Builds a world with up to three archetypes of the given row counts, then despawns every
/// `despawn_every`-th entity (swap-removes) when it is non-zero.
fn world_with(rows: &[usize], despawn_every: usize) -> World {
    let mut world = World::new();
    let mut spawned = Vec::new();
    for (archetype, &count) in rows.iter().enumerate() {
        for row in 0..count {
            let x = (archetype * 10_000 + row) as f32 * 0.25;
            let entity = match archetype {
                0 => world.spawn((P { x }, V { x: 1.0 + x * 0.001 })),
                1 => world.spawn((P { x }, V { x: -0.5 }, T { n: row as u32 })),
                _ => world.spawn((P { x }, T { n: 7 })),
            };
            spawned.push(entity);
        }
    }
    if despawn_every > 0 {
        for entity in spawned.into_iter().step_by(despawn_every) {
            world.despawn(entity);
        }
    }
    world
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn blocks_follow_the_block_rule(
        rows in prop::collection::vec(row_count(), 1..=3),
        despawn_every in prop::sample::select(vec![0usize, 2, 7]),
    ) {
        let world = world_with(&rows, despawn_every);
        let order: Vec<Entity> = world.query::<Entity>().collect();
        for executor in executors() {
            let mut world = world_with(&rows, despawn_every);
            world.set_executor(executor);
            let blocks = world.par_blocks::<Entity, _>(|block| {
                let (index, len) = (block.index(), block.len());
                (index, len, block.collect::<Vec<_>>())
            });
            let mut joined = Vec::new();
            for (position, (index, len, entities)) in blocks.into_iter().enumerate() {
                prop_assert_eq!(index, position);
                prop_assert!((1..=B).contains(&len));
                prop_assert_eq!(len, entities.len());
                // Entities of one block are adjacent rows of one archetype.
                let first = entities[0];
                let kind = |entity: Entity| {
                    (world.get::<T>(entity).is_some(), world.get::<V>(entity).is_some())
                };
                let one_archetype = entities.iter().all(|&entity| kind(entity) == kind(first));
                prop_assert!(one_archetype);
                joined.extend(entities);
            }
            prop_assert_eq!(&joined, &order);
        }
    }

    #[test]
    fn mutable_blocks_equal_query_mut(
        rows in prop::collection::vec(row_count(), 1..=3),
        despawn_every in prop::sample::select(vec![0usize, 3]),
    ) {
        let mut model = world_with(&rows, despawn_every);
        for (p, v) in model.query_mut::<(&mut P, &V)>() {
            p.x += v.x;
        }
        for (tag, p) in model.query_mut::<(Option<&mut T>, &P)>() {
            if let Some(tag) = tag {
                tag.n = tag.n.wrapping_add(p.x as u32);
            }
        }
        for (entity, p, ()) in model.query_mut::<(Entity, &mut P, Without<T>)>() {
            p.x -= entity.index() as f32;
        }
        for executor in executors() {
            let mut world = world_with(&rows, despawn_every);
            world.set_executor(executor);
            world.par_blocks_mut::<(&mut P, &V), _>(|block| {
                for (p, v) in block {
                    p.x += v.x;
                }
            });
            world.par_blocks_mut::<(Option<&mut T>, &P), _>(|block| {
                for (tag, p) in block {
                    if let Some(tag) = tag {
                        tag.n = tag.n.wrapping_add(p.x as u32);
                    }
                }
            });
            world.par_blocks_mut::<(Entity, &mut P, Without<T>), _>(|block| {
                for (entity, p, ()) in block {
                    p.x -= entity.index() as f32;
                }
            });
            prop_assert_eq!(hash(&world), hash(&model));
        }
    }

    #[test]
    fn block_reductions_equal_the_manual_chunked_fold(
        rows in prop::collection::vec(row_count(), 1..=3),
    ) {
        let world = world_with(&rows, 0);
        // Manual fold: per archetype in chunks of B rows, each chunk folded from 0.0, then the
        // chunk sums folded in order.
        let values: Vec<(bool, bool, f32)> = world
            .query::<(&P, Option<&V>, Option<&T>)>()
            .map(|(p, v, t)| (v.is_some(), t.is_some(), p.x))
            .collect();
        let mut manual = 0.0_f32;
        let mut start = 0;
        while start < values.len() {
            let kind = (values[start].0, values[start].1);
            let mut end = start;
            while end < values.len() && (values[end].0, values[end].1) == kind && end - start < B {
                end += 1;
            }
            let chunk = values[start..end].iter().fold(0.0_f32, |sum, value| sum + value.2 * 0.1);
            manual += chunk;
            start = end;
        }
        for executor in executors() {
            let mut world = world_with(&rows, 0);
            world.set_executor(executor);
            let sums = world.par_blocks::<&P, _>(|block| {
                block.fold(0.0_f32, |sum, p| sum + p.x * 0.1)
            });
            let folded = sums.into_iter().fold(0.0_f32, |sum, block| sum + block);
            prop_assert_eq!(folded.to_bits(), manual.to_bits());
        }
    }
}

#[test]
fn empty_and_unmatched_queries_produce_no_blocks() {
    let mut world = World::new();
    assert!(world.par_blocks::<&P, _>(|block| block.len()).is_empty());
    world.spawn((V { x: 1.0 },));
    assert!(world.par_blocks::<&P, _>(|block| block.len()).is_empty());
    assert!(
        world
            .par_blocks_mut::<&mut P, _>(|block| block.len())
            .is_empty()
    );
}

#[test]
fn a_block_reports_its_size_while_iterating() {
    let world = world_with(&[B + 5], 0);
    let sizes = world.par_blocks::<&P, _>(|mut block| {
        let before = block.size_hint();
        let _ = block.next();
        (block.len(), block.is_empty(), before, block.size_hint())
    });
    assert_eq!(
        sizes,
        [
            (B, false, (B, Some(B)), (B - 1, Some(B - 1))),
            (5, false, (5, Some(5)), (4, Some(4))),
        ]
    );
}

#[test]
#[should_panic(expected = "requests mutable and shared access to component")]
fn aliasing_block_queries_panic_like_query_mut() {
    let mut world = world_with(&[3], 0);
    world.par_blocks_mut::<(&mut P, &P), _>(|block| block.len());
}

#[test]
#[should_panic(expected = "requests duplicate mutable access to component")]
fn duplicate_mutable_block_queries_panic_like_query_mut() {
    let mut world = world_with(&[3], 0);
    world.par_blocks_mut::<(&mut P, Option<&mut P>), _>(|block| block.len());
}

/// Executor that counts its `run` calls and forwards to [`SequentialExecutor`].
#[derive(Debug, Default)]
struct CountingExecutor {
    calls: AtomicUsize,
}

impl Executor for CountingExecutor {
    fn threads(&self) -> usize {
        1
    }

    fn run(&self, tasks: &mut [&mut (dyn FnMut() + Send)]) {
        self.calls.fetch_add(1, Ordering::Relaxed);
        SequentialExecutor.run(tasks);
    }
}

#[test]
fn queries_with_at_most_one_block_never_call_the_executor() {
    let counter = Arc::new(CountingExecutor::default());
    let mut world = world_with(&[B], 0);
    world.set_executor(Arc::clone(&counter) as Arc<dyn Executor>);
    assert_eq!(world.par_blocks::<&P, _>(|block| block.len()), [B]);
    world.par_blocks_mut::<&mut P, _>(|block| block.len());
    assert_eq!(counter.calls.load(Ordering::Relaxed), 0);

    world.spawn((P { x: 0.0 }, V { x: 0.0 }));
    world.par_blocks::<&P, _>(|block| block.len());
    assert_eq!(counter.calls.load(Ordering::Relaxed), 1);
}

#[test]
fn the_panic_of_the_lowest_block_index_wins() {
    for executor in executors() {
        let mut world = world_with(&[3 * B + 7], 0);
        world.set_executor(executor);
        let finished = AtomicUsize::new(0);
        let payload = catch_unwind(AssertUnwindSafe(|| {
            world.par_blocks::<&P, _>(|block| {
                if block.index() >= 1 && block.index() <= 2 {
                    panic!("block {}", block.index());
                }
                finished.fetch_add(1, Ordering::Relaxed);
            })
        }))
        .expect_err("two blocks panic");
        let text = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .unwrap_or_default();
        assert_eq!(text, "block 1");
        // The blocks that did not panic all ran before the panic was resumed.
        assert_eq!(finished.load(Ordering::Relaxed), 2);
    }
}
