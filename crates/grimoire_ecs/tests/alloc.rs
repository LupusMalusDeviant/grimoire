//! Contract check (section 7): iterating queries over 10,000 entities performs no heap
//! allocation per entity; schedule ticks and data-parallel blocks stay within their allocation
//! bounds. Lives in its own test binary because it installs a global allocator.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;

use grimoire_core::impl_stable_hash;
use grimoire_ecs::{Access, Entity, Schedule, World, parallel_system_fn, system_fn};

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

#[derive(Clone)]
struct Tag {
    id: u32,
}
impl_stable_hash!(Tag { id });

const ENTITIES: usize = 10_000;

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
fn queries_do_not_allocate_per_entity() {
    let mut world = World::new();
    for i in 0..ENTITIES {
        let f = i as f32;
        if i % 2 == 0 {
            world.spawn((Pos { x: f, y: -f }, Vel { x: 1.0, y: 0.5 }));
        } else {
            world.spawn((Pos { x: f, y: -f }, Vel { x: 1.0, y: 0.5 }, Tag { id: 1 }));
        }
    }

    let before = allocations();
    let mut written = 0usize;
    for (pos, vel) in world.query_mut::<(&mut Pos, &Vel)>() {
        pos.x += vel.x;
        pos.y += vel.y;
        written += 1;
    }
    let mut read = 0usize;
    let mut sum = 0.0f32;
    for (entity, pos, vel) in world.query::<(Entity, &Pos, &Vel)>() {
        sum += pos.x * vel.x + pos.y * vel.y;
        read += usize::from(entity.generation() == 0);
    }
    let mut tagged = 0usize;
    for (pos, tag) in world.query_mut::<(&mut Pos, Option<&mut Tag>)>() {
        if let Some(tag) = tag {
            tag.id += 1;
            pos.x -= 1.0;
            tagged += 1;
        }
    }
    let during = allocations() - before;

    black_box(sum);
    assert_eq!((written, read, tagged), (ENTITIES, ENTITIES, ENTITIES / 2));
    assert_eq!(during, 0, "query iteration allocated {during} times");
}

fn populated() -> World {
    let mut world = World::new();
    for i in 0..ENTITIES {
        let f = i as f32;
        world.spawn((Pos { x: f, y: -f }, Vel { x: 1.0, y: 0.5 }));
    }
    world
}

#[test]
fn a_tick_of_exclusive_systems_does_not_allocate() {
    let mut world = populated();
    let mut schedule = Schedule::new();
    schedule
        .add_system(system_fn("integrate", |world| {
            for (pos, vel) in world.query_mut::<(&mut Pos, &Vel)>() {
                pos.x += vel.x;
            }
        }))
        .add_system(system_fn("drift", |world| {
            for pos in world.query_mut::<&mut Pos>() {
                pos.y -= 0.25;
            }
        }));
    schedule.run(&mut world);

    let before = allocations();
    schedule.run(&mut world);
    let during = allocations() - before;
    assert_eq!(during, 0, "an exclusive-only tick allocated {during} times");
}

#[test]
fn a_parallel_stage_without_commands_allocates_at_most_twice() {
    let mut world = populated();
    let mut schedule = Schedule::new();
    for name in ["first", "second", "third"] {
        schedule.add_parallel_system(parallel_system_fn(
            name,
            Access::new().read::<Pos>().read::<Vel>(),
            |world, _commands| {
                let mut sum = 0.0f32;
                for (pos, vel) in world.query::<(&Pos, &Vel)>() {
                    sum += pos.x * vel.x;
                }
                black_box(sum);
            },
        ));
    }
    assert_eq!(schedule.stages().len(), 1);
    schedule.run(&mut world);

    let before = allocations();
    schedule.run(&mut world);
    let during = allocations() - before;
    assert!(during <= 2, "a three-system stage allocated {during} times");
}

#[test]
fn blocks_allocate_per_block_not_per_entity() {
    let mut world = populated();
    let before = allocations();
    let rows = world.par_blocks_mut::<(&mut Pos, &Vel), _>(|block| {
        let len = block.len();
        for (pos, vel) in block {
            pos.x += vel.x;
        }
        len
    });
    let during = allocations() - before;
    assert_eq!(rows.iter().sum::<usize>(), ENTITIES);
    assert!(during <= 32, "par_blocks_mut allocated {during} times");
}
