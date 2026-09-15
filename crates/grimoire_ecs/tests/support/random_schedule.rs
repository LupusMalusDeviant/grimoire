//! Random schedules for the property test "grouped stages equal isolated stages".
//!
//! Shared by `grimoire_ecs/tests/stages.rs` and the thread-pool gate of `grimoire_exec` (via
//! `#[path]`), so it depends only on `grimoire_core`, `grimoire_ecs` and `proptest`.
//!
//! A parallel system derives everything it records from what it declared as read: it folds the
//! declared reads into a `StableHasher` and records `set`, resource and structural commands from
//! that value. A missing conflict in the stage rule therefore changes the hashes.

// Each including test binary uses a different subset of this module.
#![allow(dead_code)]

use std::sync::Arc;

use grimoire_core::{StableHash, StableHasher, impl_stable_hash};
use grimoire_ecs::{
    Access, CommandBuffer, Entity, Executor, Schedule, StageMode, With, World, parallel_system_fn,
    system_fn,
};
use proptest::collection::vec;
use proptest::prelude::*;

/// Ticks run per case; hashes are compared after each.
pub const TICKS: usize = 3;

#[derive(Clone, Debug)]
pub struct A {
    pub value: u64,
}
impl_stable_hash!(A { value });

#[derive(Clone, Debug)]
pub struct B {
    pub value: u64,
}
impl_stable_hash!(B { value });

#[derive(Clone, Debug)]
pub struct C {
    pub value: u64,
}
impl_stable_hash!(C { value });

#[derive(Clone, Debug)]
pub struct R1 {
    pub value: u64,
}
impl_stable_hash!(R1 { value });

#[derive(Clone, Debug)]
pub struct R2 {
    pub value: u64,
}
impl_stable_hash!(R2 { value });

/// Declared access and behaviour of one parallel system.
#[derive(Clone, Debug)]
pub struct ParallelSpec {
    pub reads: [bool; 3],
    pub writes: [bool; 3],
    pub resource_reads: [bool; 2],
    pub resource_writes: [bool; 2],
    pub structural: bool,
    pub salt: u64,
}

/// One entry of a random schedule.
#[derive(Clone, Debug)]
pub enum EntrySpec {
    Parallel(ParallelSpec),
    Exclusive(u64),
}

/// A random schedule over a random initial world.
#[derive(Clone, Debug)]
pub struct ScheduleSpec {
    pub entries: Vec<EntrySpec>,
    /// Component mask (bit 0 = A, 1 = B, 2 = C) per initial entity.
    pub entities: Vec<u8>,
}

const NAMES: [&str; 12] = [
    "s0", "s1", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11",
];

fn parallel_spec() -> impl Strategy<Value = ParallelSpec> {
    (
        any::<[bool; 3]>(),
        any::<[bool; 3]>(),
        any::<[bool; 2]>(),
        any::<[bool; 2]>(),
        prop::bool::weighted(0.3),
        any::<u64>(),
    )
        .prop_map(
            |(reads, writes, resource_reads, resource_writes, structural, salt)| ParallelSpec {
                reads,
                writes,
                resource_reads,
                resource_writes,
                structural,
                salt,
            },
        )
}

/// 1–8 parallel systems interleaved with 0–2 exclusive systems over 0–300 entities.
pub fn schedule_spec() -> impl Strategy<Value = ScheduleSpec> {
    (
        vec(parallel_spec(), 1..=8),
        vec((any::<prop::sample::Index>(), any::<u64>()), 0..=2),
        vec(0u8..8, 0..=300),
    )
        .prop_map(|(parallel, exclusive, entities)| {
            let mut entries: Vec<EntrySpec> =
                parallel.into_iter().map(EntrySpec::Parallel).collect();
            for (position, salt) in exclusive {
                let at = position.index(entries.len() + 1);
                entries.insert(at, EntrySpec::Exclusive(salt));
            }
            ScheduleSpec { entries, entities }
        })
}

fn initial_world(spec: &ScheduleSpec) -> World {
    let mut world = World::new();
    world.insert_resource(R1 { value: 1 });
    for (index, &mask) in spec.entities.iter().enumerate() {
        let value = index as u64;
        match mask {
            0 => world.spawn(()),
            1 => world.spawn((A { value },)),
            2 => world.spawn((B { value },)),
            3 => world.spawn((A { value }, B { value })),
            4 => world.spawn((C { value },)),
            5 => world.spawn((A { value }, C { value })),
            6 => world.spawn((B { value }, C { value })),
            _ => world.spawn((A { value }, B { value }, C { value })),
        };
    }
    world
}

fn fold_reads<T: grimoire_ecs::Component>(world: &World, hasher: &mut StableHasher) {
    for (entity, value) in world.query::<(Entity, &T)>() {
        hasher.write_u64(entity.to_bits());
        value.stable_hash(hasher);
    }
}

fn set_all<T: grimoire_ecs::Component>(
    world: &World,
    commands: &mut CommandBuffer,
    seed: u64,
    make: fn(u64) -> T,
) {
    for (row, (entity, ())) in world.query::<(Entity, With<T>)>().enumerate() {
        let mixed = seed ^ (row as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        if !mixed.is_multiple_of(3) {
            commands.set(entity, make(mixed));
        }
    }
}

fn run_parallel(spec: &ParallelSpec, world: &World, commands: &mut CommandBuffer) {
    let mut hasher = StableHasher::new();
    hasher.write_u64(spec.salt);
    if spec.reads[0] {
        fold_reads::<A>(world, &mut hasher);
    }
    if spec.reads[1] {
        fold_reads::<B>(world, &mut hasher);
    }
    if spec.reads[2] {
        fold_reads::<C>(world, &mut hasher);
    }
    if spec.resource_reads[0] {
        world
            .resource::<R1>()
            .map(|r| r.value)
            .stable_hash(&mut hasher);
    }
    if spec.resource_reads[1] {
        world
            .resource::<R2>()
            .map(|r| r.value)
            .stable_hash(&mut hasher);
    }
    // Existence and membership need no declaration.
    hasher.write_usize(world.entity_count());
    let seed = hasher.finish();

    if spec.writes[0] {
        set_all::<A>(world, commands, seed, |value| A { value });
    }
    if spec.writes[1] {
        set_all::<B>(world, commands, seed ^ 1, |value| B { value });
    }
    if spec.writes[2] {
        set_all::<C>(world, commands, seed ^ 2, |value| C { value });
    }
    if spec.resource_writes[0] {
        commands.insert_resource(R1 { value: seed });
    }
    if spec.resource_writes[1] {
        if seed.is_multiple_of(4) {
            commands.remove_resource::<R2>();
        } else {
            commands.insert_resource(R2 { value: seed });
        }
    }
    if spec.structural {
        let entities: Vec<Entity> = world.query::<Entity>().collect();
        match seed % 4 {
            0 => commands.spawn((A { value: seed }, C { value: seed >> 1 })),
            1 => commands.spawn((B { value: seed },)),
            _ => {}
        }
        if !entities.is_empty() {
            let target = entities[(seed % entities.len() as u64) as usize];
            match (seed >> 8) % 4 {
                0 => {
                    commands.despawn(target);
                }
                1 => commands.insert(target, B { value: seed }),
                2 => commands.remove::<A>(target),
                _ => commands.insert(target, C { value: seed ^ 7 }),
            }
        }
    }
}

fn access_of(spec: &ParallelSpec) -> Access {
    let mut access = Access::new();
    if spec.reads[0] {
        access = access.read::<A>();
    }
    if spec.reads[1] {
        access = access.read::<B>();
    }
    if spec.reads[2] {
        access = access.read::<C>();
    }
    if spec.writes[0] {
        access = access.write::<A>();
    }
    if spec.writes[1] {
        access = access.write::<B>();
    }
    if spec.writes[2] {
        access = access.write::<C>();
    }
    if spec.resource_reads[0] {
        access = access.read_resource::<R1>();
    }
    if spec.resource_reads[1] {
        access = access.read_resource::<R2>();
    }
    if spec.resource_writes[0] {
        access = access.write_resource::<R1>();
    }
    if spec.resource_writes[1] {
        access = access.write_resource::<R2>();
    }
    if spec.structural {
        access = access.structural();
    }
    access
}

/// Builds the schedule of `spec` in `mode`.
pub fn build_schedule(spec: &ScheduleSpec, mode: StageMode) -> Schedule {
    let mut schedule = Schedule::new();
    for (index, entry) in spec.entries.iter().enumerate() {
        let name = NAMES[index % NAMES.len()];
        match entry {
            EntrySpec::Parallel(parallel) => {
                let parallel = parallel.clone();
                schedule.add_parallel_system(parallel_system_fn(
                    name,
                    access_of(&parallel),
                    move |world, commands| run_parallel(&parallel, world, commands),
                ));
            }
            &EntrySpec::Exclusive(salt) => {
                schedule.add_system(system_fn(name, move |world: &mut World| {
                    let mut hasher = StableHasher::new();
                    world.stable_hash(&mut hasher);
                    let seed = hasher.finish() ^ salt;
                    for a in world.query_mut::<&mut A>() {
                        a.value = a.value.wrapping_mul(31).wrapping_add(seed);
                    }
                    world.insert_resource(R2 { value: seed });
                    if seed.is_multiple_of(3) {
                        world.spawn((C { value: seed },));
                    }
                }));
            }
        }
    }
    schedule.set_stage_mode(mode);
    schedule
}

/// World hash after each of [`TICKS`] runs of the schedule of `spec` with `executor` in `mode`.
pub fn run_hashes(spec: &ScheduleSpec, executor: Arc<dyn Executor>, mode: StageMode) -> Vec<u64> {
    let mut world = initial_world(spec);
    world.set_executor(executor);
    let mut schedule = build_schedule(spec, mode);
    (0..TICKS)
        .map(|_| {
            schedule.run(&mut world);
            let mut hasher = StableHasher::new();
            world.stable_hash(&mut hasher);
            hasher.finish()
        })
        .collect()
}
