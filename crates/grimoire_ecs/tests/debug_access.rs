//! Debug-build access check of parallel systems (engine ADR-0006, building block 1).
//!
//! The check exists only with `debug_assertions`; so does this test binary.
#![cfg(debug_assertions)]

use std::sync::Arc;

use grimoire_core::impl_stable_hash;
use grimoire_ecs::{
    Access, CommandBuffer, Entity, PermutedExecutor, QUERY_BLOCK_SIZE, Schedule, With, Without,
    World, parallel_system_fn, system_fn,
};

#[derive(Clone)]
struct Pos {
    x: f32,
}
impl_stable_hash!(Pos { x });

#[derive(Clone)]
struct Vel {
    x: f32,
}
impl_stable_hash!(Vel { x });

#[derive(Clone)]
struct Score {
    value: u32,
}
impl_stable_hash!(Score { value });

fn world() -> World {
    let mut world = World::new();
    for i in 0..(2 * QUERY_BLOCK_SIZE + 3) {
        let x = i as f32;
        if i % 2 == 0 {
            world.spawn((Pos { x }, Vel { x: 1.0 }));
        } else {
            world.spawn((Pos { x },));
        }
    }
    world.insert_resource(Score { value: 3 });
    world
}

/// Runs a schedule with the single parallel system `system` over [`world`].
fn run_one(
    access: Access,
    system: impl FnMut(&World, &mut CommandBuffer) + Send + 'static,
) -> World {
    let mut world = world();
    let mut schedule = Schedule::new();
    schedule.add_parallel_system(parallel_system_fn("sneaky", access, system));
    schedule.run(&mut world);
    world
}

#[test]
#[should_panic(
    expected = "system `sneaky` reads component `debug_access::Vel` without declaring it (Access::read, World::query)"
)]
fn undeclared_query_read_panics() {
    run_one(Access::new().read::<Pos>(), |world, _| {
        let _ = world.query::<(&Pos, &Vel)>().count();
    });
}

#[test]
#[should_panic(expected = "system `sneaky` reads component `debug_access::Vel`")]
fn undeclared_optional_read_panics() {
    run_one(Access::new().read::<Pos>(), |world, _| {
        let _ = world.query::<(&Pos, Option<&Vel>)>().count();
    });
}

#[test]
#[should_panic(
    expected = "system `sneaky` reads component `debug_access::Pos` without declaring it (Access::read, World::get)"
)]
fn undeclared_get_panics() {
    run_one(Access::new(), |world, _| {
        let entity = world.query::<Entity>().next();
        let _ = entity.and_then(|entity| world.get::<Pos>(entity));
    });
}

#[test]
#[should_panic(
    expected = "system `sneaky` reads component `debug_access::Vel` without declaring it (Access::read, World::par_blocks)"
)]
fn undeclared_block_query_panics() {
    run_one(Access::new().read::<Pos>(), |world, _| {
        let _ = world.par_blocks::<(&Pos, &Vel), _>(|block| block.len());
    });
}

#[test]
#[should_panic(
    expected = "system `sneaky` reads component `debug_access::Vel` without declaring it (Access::read, World::get)"
)]
fn undeclared_read_inside_a_block_panics_on_the_block_task() {
    let mut world = world();
    world.set_executor(Arc::new(PermutedExecutor::new(3)));
    let mut schedule = Schedule::new();
    schedule
        .add_parallel_system(parallel_system_fn(
            "honest",
            Access::new().read::<Pos>(),
            |world, _| {
                let _ = world.par_blocks::<&Pos, _>(|block| block.len());
            },
        ))
        .add_parallel_system(parallel_system_fn(
            "sneaky",
            Access::new().read::<Pos>(),
            |world, _| {
                let _ = world.par_blocks::<(Entity, &Pos), _>(|block| {
                    // Each block task re-enters the system's context.
                    block
                        .filter(|(entity, _)| world.get::<Vel>(*entity).is_some())
                        .count()
                });
            },
        ));
    schedule.run(&mut world);
}

#[test]
#[should_panic(
    expected = "system `sneaky` reads resource `debug_access::Score` without declaring it (Access::read_resource)"
)]
fn undeclared_resource_read_panics() {
    run_one(Access::new().read_resource::<Pos>(), |world, _| {
        let _ = world.resource::<Score>();
    });
}

#[test]
#[should_panic(expected = "system `sneaky` reads the whole world (World::stable_hash)")]
fn hashing_the_world_panics() {
    run_one(Access::new().read::<Pos>().read::<Vel>(), |world, _| {
        let mut hasher = grimoire_core::StableHasher::new();
        world.stable_hash(&mut hasher);
    });
}

#[test]
#[should_panic(expected = "system `sneaky` reads the whole world (World::snapshot)")]
fn snapshotting_the_world_panics() {
    run_one(Access::new(), |world, _| {
        let _ = world.snapshot();
    });
}

#[test]
#[should_panic(
    expected = "system `sneaky` records CommandBuffer::spawn without declaring structural commands (Access::structural)"
)]
fn undeclared_spawn_panics() {
    run_one(Access::new(), |_, commands| {
        commands.spawn((Pos { x: 0.0 },));
    });
}

#[test]
#[should_panic(
    expected = "system `sneaky` records CommandBuffer::despawn without declaring structural commands"
)]
fn undeclared_appended_despawn_panics() {
    run_one(Access::new().write::<Pos>(), |world, commands| {
        let mut local = CommandBuffer::new();
        if let Some(entity) = world.query::<Entity>().next() {
            local.despawn(entity);
        }
        commands.append(&mut local);
    });
}

#[test]
#[should_panic(expected = "system `sneaky` records CommandBuffer::insert without declaring")]
fn undeclared_insert_panics() {
    run_one(Access::new().write::<Vel>(), |world, commands| {
        if let Some(entity) = world.query::<Entity>().next() {
            commands.insert(entity, Vel { x: 2.0 });
        }
    });
}

#[test]
#[should_panic(expected = "system `sneaky` records CommandBuffer::remove without declaring")]
fn undeclared_remove_panics() {
    run_one(Access::new(), |world, commands| {
        if let Some(entity) = world.query::<Entity>().next() {
            commands.remove::<Vel>(entity);
        }
    });
}

#[test]
#[should_panic(
    expected = "system `sneaky` writes component `debug_access::Vel` without declaring it (Access::write)"
)]
fn undeclared_set_panics() {
    // `read` does not cover writing.
    run_one(Access::new().read::<Vel>(), |world, commands| {
        if let Some(entity) = world.query::<Entity>().next() {
            commands.set(entity, Vel { x: 2.0 });
        }
    });
}

#[test]
#[should_panic(expected = "system `sneaky` reads component `debug_access::Vel`")]
fn write_does_not_imply_read() {
    run_one(Access::new().write::<Vel>(), |world, _| {
        let _ = world.query::<&Vel>().count();
    });
}

#[test]
#[should_panic(
    expected = "system `sneaky` writes resource `debug_access::Score` without declaring it (Access::write_resource)"
)]
fn undeclared_resource_insert_panics() {
    run_one(Access::new().read_resource::<Score>(), |_, commands| {
        commands.insert_resource(Score { value: 1 });
    });
}

#[test]
#[should_panic(expected = "system `sneaky` writes resource `debug_access::Score`")]
fn undeclared_resource_removal_panics() {
    run_one(Access::new(), |_, commands| {
        commands.remove_resource::<Score>();
    });
}

#[test]
fn a_rejected_stage_changes_nothing() {
    let mut world = world();
    let before = world.snapshot();
    let mut schedule = Schedule::new();
    schedule.add_parallel_system(parallel_system_fn("sly", Access::new(), |_, commands| {
        commands.spawn((Pos { x: 9.0 },));
    }));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        schedule.run(&mut world);
    }));
    assert!(result.is_err());
    let mut restored = World::new();
    restored.restore(&before);
    let hash = |world: &World| {
        let mut hasher = grimoire_core::StableHasher::new();
        world.stable_hash(&mut hasher);
        hasher.finish()
    };
    assert_eq!(hash(&world), hash(&restored));
}

#[test]
fn exclusive_systems_are_not_checked() {
    let mut world = world();
    let mut schedule = Schedule::new();
    schedule.add_system(system_fn("free", |world| {
        let _ = world.query::<(&Pos, &Vel)>().count();
        let _ = world.resource::<Score>();
        let _ = world.snapshot();
        let _ = world.par_blocks::<&Vel, _>(|block| block.len());
        let mut commands = CommandBuffer::new();
        commands.spawn((Pos { x: 1.0 },));
        commands.apply(world);
    }));
    schedule.run(&mut world);
}

/// An exclusive system is not checked even when a parallel system runs its schedule on a scratch
/// world: the outer system's context must not leak into the nested schedule or its blocks.
#[test]
fn exclusive_systems_nested_in_a_parallel_system_are_not_checked() {
    let mut world = world();
    world.set_executor(Arc::new(PermutedExecutor::reversed()));
    let nested = |_: &World, _: &mut CommandBuffer| {
        let mut scratch = World::new();
        scratch.set_executor(Arc::new(PermutedExecutor::new(3)));
        scratch.insert_resource(Score { value: 7 });
        for i in 0..(QUERY_BLOCK_SIZE + 1) {
            scratch.spawn((Vel { x: i as f32 },));
        }
        let mut inner = Schedule::new();
        inner.add_system(system_fn("inner", |scratch| {
            assert_eq!(
                scratch.resource::<Score>().map(|score| score.value),
                Some(7)
            );
            let _ = scratch.query::<&Vel>().count();
            let blocks = scratch.par_blocks::<&Vel, _>(|block| block.len());
            assert_eq!(blocks.iter().sum::<usize>(), QUERY_BLOCK_SIZE + 1);
            let _ = scratch.snapshot();
        }));
        inner.run(&mut scratch);
    };
    let mut schedule = Schedule::new();
    schedule
        .add_parallel_system(parallel_system_fn("outer", Access::new(), nested))
        .add_parallel_system(parallel_system_fn("second", Access::new(), nested));
    schedule.run(&mut world);
}

#[test]
fn declared_and_undeclarable_access_passes() {
    let mut world = world();
    world.set_executor(Arc::new(PermutedExecutor::reversed()));
    let mut schedule = Schedule::new();
    schedule
        .add_parallel_system(parallel_system_fn(
            "membership",
            Access::new(),
            |world, _| {
                let with = world.query::<(Entity, With<Vel>)>().count();
                let without = world.query::<(Entity, Without<Vel>)>().count();
                assert_eq!(with + without, world.entity_count());
                let first = world.query::<Entity>().next();
                assert!(first.is_some_and(|entity| world.is_alive(entity)));
                assert_eq!(world.executor().threads(), 1);
                let blocks = world.par_blocks::<(Entity, With<Pos>), _>(|block| block.len());
                assert_eq!(blocks.iter().sum::<usize>(), world.entity_count());
            },
        ))
        .add_parallel_system(parallel_system_fn(
            "declared",
            Access::new()
                .read::<Pos>()
                .read::<Vel>()
                .write::<Vel>()
                .read_resource::<Score>()
                .write_resource::<Score>()
                .structural(),
            |world, commands| {
                let score = world.resource::<Score>().map_or(0, |score| score.value);
                let mut local = CommandBuffer::new();
                for (entity, pos, vel) in world.query::<(Entity, &Pos, Option<&Vel>)>() {
                    if vel.is_some() && world.get::<Vel>(entity).is_some() {
                        local.set(entity, Vel { x: pos.x });
                    }
                }
                commands.append(&mut local);
                commands.insert_resource(Score { value: score + 1 });
                commands.spawn((Pos { x: 0.5 },));
            },
        ));
    schedule.run(&mut world);
    assert_eq!(world.resource::<Score>().map(|score| score.value), Some(4));
}
