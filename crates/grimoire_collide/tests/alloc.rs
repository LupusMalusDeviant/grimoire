//! Contract check (§14, "Leistung"): `overlapping` and `graze_ring` allocate nothing once `out`
//! has enough capacity, `rebuild` allocates nothing once warmed up, and `overlapping_batch`
//! allocates per call in proportion to its block count, never per hit. Lives in its own test
//! binary because it installs a global allocator.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;

use grimoire_collide::{
    BatchHits, Circle, ColliderKey, CollisionQuery, GrazeRing, GridConfig, GridItem, LayerMask,
    Shape, ShapeQuery, SpatialGrid,
};
use grimoire_core::Vec2;
use grimoire_ecs::{PermutedExecutor, SequentialExecutor};

thread_local! {
    // Per thread, so allocations of the test harness on other threads are not counted.
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

fn record_allocation() {
    // `try_with` because the allocator may run while thread-locals are being torn down.
    let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
}

fn allocations() -> usize {
    ALLOCATIONS.with(Cell::get)
}

/// Forwards to [`System`] and counts allocations of the current thread.
struct CountingAllocator;

// SAFETY: every method forwards its arguments unchanged to `System`, which upholds the
// `GlobalAlloc` contract. Counting only touches a const-initialised thread-local `Cell` without a
// destructor, which never allocates and therefore cannot recurse into the allocator.
// `counter_observes_allocations` checks that the wrapper is installed and counts.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        // SAFETY: the caller's guarantees for `layout` are passed through unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        // SAFETY: the caller's guarantees for `layout` are passed through unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record_allocation();
        // SAFETY: `ptr` was allocated by `System` through this wrapper with `layout`.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` was allocated by `System` through this wrapper with `layout`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

const ITEMS: u32 = 5_000;

fn config() -> GridConfig {
    GridConfig::new(Vec2::new(-64.0, -64.0), 4.0, 32, 32)
}

/// `ITEMS` small circles on a lattice over the grid; `shift` moves all of them, so two item sets
/// with different shifts occupy different cells.
fn items(shift: f32) -> Vec<GridItem> {
    (0..ITEMS)
        .map(|index| GridItem {
            key: ColliderKey::pool(index, 0),
            shape: Shape::Circle(Circle {
                center: Vec2::new(
                    (index % 100) as f32 * 1.2 - 60.0 + shift,
                    (index / 100) as f32 * 1.2 - 60.0 + shift,
                ),
                radius: 0.3,
            }),
            layers: LayerMask::layer((index % 2) as u8),
        })
        .collect()
}

fn circle(x: f32, y: f32, radius: f32) -> Shape {
    Shape::Circle(Circle {
        center: Vec2::new(x, y),
        radius,
    })
}

#[test]
fn counter_observes_allocations() {
    let before = allocations();
    let boxed = black_box(Box::new(black_box(7u64)));
    assert!(
        allocations() > before,
        "counting allocator is not installed"
    );
    drop(boxed);
}

#[test]
fn queries_do_not_allocate_once_out_has_capacity() {
    let mut grid = SpatialGrid::new(config()).expect("valid config");
    grid.rebuild(items(0.0));
    let shape = circle(-30.0, -30.0, 6.0);
    let ring = GrazeRing {
        center: Vec2::new(-30.0, -30.0),
        inner_radius: 1.0,
        outer_radius: 8.0,
    };
    let mut out = Vec::new();
    grid.overlapping(&shape, LayerMask::ALL, &mut out);
    grid.graze_ring(&ring, LayerMask::ALL, &mut out);
    assert!(!out.is_empty(), "the probes must find something");

    let before = allocations();
    for _ in 0..10 {
        grid.overlapping(&shape, LayerMask::ALL, &mut out);
        black_box(out.len());
        grid.graze_ring(&ring, LayerMask::ALL, &mut out);
        black_box(out.len());
    }
    assert_eq!(allocations() - before, 0, "queries allocated");
}

#[test]
fn rebuild_does_not_allocate_once_warmed_up() {
    let first = items(0.0);
    let second = items(2.5);
    let mut grid = SpatialGrid::new(config()).expect("valid config");
    grid.rebuild(first.iter().copied());
    grid.rebuild(second.iter().copied());

    let before = allocations();
    for _ in 0..5 {
        grid.rebuild(first.iter().copied());
        grid.rebuild(second.iter().copied());
    }
    assert_eq!(allocations() - before, 0, "a warmed-up rebuild allocated");
    assert_eq!(grid.len(), ITEMS as usize);
}

#[test]
fn rebuild_par_allocation_does_not_grow_with_the_item_count() {
    // Warm up with the larger set, then compare a small and a large rebuild: the per-call
    // allocations of the block bookkeeping must not depend on how many items there are.
    let executor = PermutedExecutor::new(1);
    let large = items(0.0);
    let small: Vec<GridItem> = large.iter().take(1_500).copied().collect();
    let mut grid = SpatialGrid::new(config()).expect("valid config");
    grid.rebuild_par(&executor, large.iter().copied());

    let before = allocations();
    grid.rebuild_par(&executor, small.iter().copied());
    let small_allocations = allocations() - before;
    let before = allocations();
    grid.rebuild_par(&executor, large.iter().copied());
    let large_allocations = allocations() - before;
    assert!(
        large_allocations <= small_allocations + 16,
        "rebuild_par allocated {large_allocations} times for {ITEMS} items, {small_allocations} for 1,500"
    );
}

#[test]
fn overlapping_batch_allocates_per_block_never_per_hit() {
    let mut grid = SpatialGrid::new(config()).expect("valid config");
    grid.rebuild(items(0.0));
    // 100 queries in either case; one set finds almost nothing, the other hundreds per query.
    let sparse: Vec<ShapeQuery> = (0..100)
        .map(|index| ShapeQuery {
            shape: circle(200.0 + index as f32, 200.0, 0.1),
            mask: LayerMask::ALL,
        })
        .collect();
    let dense: Vec<ShapeQuery> = (0..100)
        .map(|index| ShapeQuery {
            shape: circle(-40.0 + (index % 10) as f32, -40.0, 12.0),
            mask: LayerMask::ALL,
        })
        .collect();

    for executor in [
        &SequentialExecutor as &dyn grimoire_ecs::Executor,
        &PermutedExecutor::new(2),
    ] {
        let mut out = BatchHits::default();
        grid.overlapping_batch(executor, &dense, &mut out);
        let dense_hits: usize = (0..out.len()).map(|i| out.hits(i).len()).sum();
        assert!(dense_hits > 10_000, "{dense_hits} hits in the dense batch");

        let before = allocations();
        grid.overlapping_batch(executor, &sparse, &mut out);
        let sparse_allocations = allocations() - before;
        let before = allocations();
        grid.overlapping_batch(executor, &dense, &mut out);
        let dense_allocations = allocations() - before;
        assert_eq!(
            dense_allocations, sparse_allocations,
            "a warmed-up batch allocated more for {dense_hits} hits than for almost none"
        );
        assert_eq!(out.len(), dense.len());
    }
}
