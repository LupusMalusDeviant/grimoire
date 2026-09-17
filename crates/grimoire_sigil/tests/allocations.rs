//! Contract §11.6 (Plan 0002 WP5.4): 2,000 spawns and 2,000 despawns in one tick allocate nothing.
//!
//! Three simulations hold the same 10,000 live bullets in the same 10 pool blocks and differ only
//! in their turnover per tick: 2,000 spawns and 2,000 lifetime despawns, 500 of each, and none.
//! Once every working buffer has grown to its steady size, one tick of each must perform exactly
//! the same number of heap allocations, so spawning and despawning themselves allocate nothing; the
//! remaining per-tick allocations belong to the simulation and the block runner and are bounded
//! independently of the bullet count. Lives in its own test binary because it installs a global
//! allocator.

mod support;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

use grimoire_core::Vec2;
use grimoire_sigil::{
    BehaviorRegistryBuilder, BulletPool, DespawnCause, Emitter, SigilConfig, SigilLibrary, UnitId,
    install,
};
use grimoire_sim::{Simulation, TickInput};
use support::{Sections, Timing, block, block_kind, bullet_type, emitter, program};

thread_local! {
    // Per thread, so allocations of the test harness on other threads are not counted. The
    // default `SequentialExecutor` runs every pool block on the calling thread.
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
// `turnover_does_not_change_the_allocations_of_a_tick` first checks that the wrapper counts.
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

const UNIT: u64 = 54;
const EMITTERS: u16 = 4;
const LIVE: u32 = 10_000;

/// A simulation whose four emitters each fire a ring of `per_emitter` bullets every tick for
/// `volleys` ticks (or forever); the bullets live `lifetime` ticks (`0`: unbounded).
fn simulation(per_emitter: u16, lifetime: u32, volleys: u32) -> Simulation {
    let timing = Timing {
        delay: 0,
        repeat: volleys,
        interval: 1,
    };
    let unit = Sections {
        bullet_types: vec![bullet_type(lifetime, 0, 0)],
        programs: vec![program(
            block(block_kind::RING, per_emitter, [0.0; 6]),
            &[support::modifier(
                support::modifier_kind::ACCELERATE,
                0,
                0,
                [0.001, 0.5, 0.0],
            )],
        )],
        emitters: vec![emitter(0, 0, 0, timing, 0.05)],
        ..Sections::default()
    }
    .unit(UNIT);
    let registry = BehaviorRegistryBuilder::new(1).build();
    let library = SigilLibrary::new(vec![unit], Arc::clone(&registry)).expect("library builds");
    let mut sim = Simulation::new(9);
    install(
        &mut sim,
        library,
        registry,
        SigilConfig::new(LIVE, Vec2::new(-1.0e4, -1.0e4), Vec2::new(1.0e4, 1.0e4)),
    )
    .expect("install succeeds");
    for index in 0..EMITTERS {
        sim.world_mut().spawn((Emitter {
            unit: UnitId(UNIT),
            emitter: 0,
            origin: Vec2::new(f32::from(index) * 10.0, 0.0),
            rotation: 0.1 * f32::from(index),
            started_at: 0,
        },));
    }
    sim
}

fn pool(sim: &Simulation) -> &BulletPool {
    sim.world()
        .resource::<BulletPool>()
        .expect("pool installed")
}

/// Steps `sim` 60 ticks (every buffer reaches its steady size), then counts the allocations of one
/// more tick and returns them with that tick's lifetime despawns. The population is unchanged and
/// nothing is dropped, so the tick spawned exactly as many bullets as it despawned.
fn measured_tick(sim: &mut Simulation) -> (usize, usize) {
    for _ in 0..60 {
        sim.step(TickInput::default());
    }
    assert_eq!(pool(sim).len(), LIVE, "steady population");
    assert_eq!(
        pool(sim).slot_count(),
        LIVE,
        "same pool blocks in every variant"
    );
    let before = allocations();
    sim.step(TickInput::default());
    let counted = allocations() - before;
    let despawns = pool(sim)
        .events()
        .iter()
        .filter(|event| event.cause == DespawnCause::Lifetime)
        .count();
    assert_eq!(pool(sim).len(), LIVE);
    assert_eq!(pool(sim).dropped_spawns(), 0);
    (counted, despawns)
}

#[test]
fn turnover_does_not_change_the_allocations_of_a_tick() {
    let before = allocations();
    let boxed = std::hint::black_box(Box::new(std::hint::black_box(7u64)));
    assert!(
        allocations() > before,
        "counting allocator is not installed"
    );
    drop(boxed);

    // 4 × 500 bullets per tick living 5 ticks: 2,000 spawns and 2,000 despawns per tick.
    let (full, full_despawns) = measured_tick(&mut simulation(500, 5, u32::MAX));
    assert_eq!(full_despawns, 2_000);

    // 4 × 125 bullets per tick living 20 ticks: 500 of each.
    let (quarter, quarter_despawns) = measured_tick(&mut simulation(125, 20, u32::MAX));
    assert_eq!(quarter_despawns, 500);

    // 4 × 500 bullets for 5 ticks, unbounded lifetime: no turnover at all.
    let (none, none_despawns) = measured_tick(&mut simulation(500, 0, 5));
    assert_eq!(none_despawns, 0);

    assert_eq!(
        (full, quarter),
        (none, none),
        "2,000 and 500 spawns/despawns per tick must allocate exactly what a tick without \
         turnover allocates"
    );
    // Measured when this test was written: 9 (the simulation's per-tick resources, the block list
    // and the block runner); before WP5.4 the same ticks allocated 76, 66 and 54 times.
    assert!(
        none <= 16,
        "a steady tick of 10,000 bullets in 10 blocks allocated {none} times"
    );
}
