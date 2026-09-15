//! Stage planning of the schedule (engine ADR-0006, building block 2): frozen stage plans and the
//! equivalence of grouped and isolated stages.

mod support;

use std::sync::Arc;

use grimoire_core::{StableHasher, impl_stable_hash};
use grimoire_ecs::{
    Access, CommandBuffer, Executor, PermutedExecutor, Schedule, SequentialExecutor, StageMode,
    World, parallel_system_fn, system_fn,
};
use proptest::prelude::*;
use support::random_schedule::{ScheduleSpec, run_hashes, schedule_spec};

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

fn noop(_: &World, _: &mut CommandBuffer) {}

fn plan(schedule: &Schedule) -> Vec<String> {
    schedule.stages().iter().map(ToString::to_string).collect()
}

fn hash(world: &World) -> u64 {
    let mut hasher = StableHasher::new();
    world.stable_hash(&mut hasher);
    hasher.finish()
}

#[test]
fn empty_schedule_has_no_stages() {
    let schedule = Schedule::new();
    assert!(schedule.stages().is_empty());
    assert!(schedule.is_empty());
    assert_eq!(schedule.stage_mode(), StageMode::Grouped);
}

#[test]
fn exclusive_systems_form_one_stage_each() {
    let mut schedule = Schedule::new();
    schedule
        .add_system(system_fn("a", |_| {}))
        .add_system(system_fn("b", |_| {}));
    assert_eq!(
        plan(&schedule),
        [
            "exclusive [a] (exclusive system)",
            "exclusive [b] (exclusive system)",
        ]
    );
    let stage = &schedule.stages()[0];
    assert!(stage.exclusive);
    assert_eq!(stage.systems, ["a"]);
    assert_eq!(stage.reason, "exclusive system");
}

#[test]
fn disjoint_readers_share_a_stage() {
    let mut schedule = Schedule::new();
    schedule
        .add_parallel_system(parallel_system_fn("a", Access::new().read::<Pos>(), noop))
        .add_parallel_system(parallel_system_fn(
            "b",
            Access::new().read::<Pos>().read::<Vel>(),
            noop,
        ))
        .add_parallel_system(parallel_system_fn("c", Access::new(), noop));
    assert_eq!(plan(&schedule), ["parallel [a, b, c] (first stage)"]);
    assert_eq!(schedule.system_names(), ["a", "b", "c"]);
    assert_eq!(schedule.len(), 3);
}

#[test]
fn reading_a_component_written_earlier_in_the_stage_splits() {
    let mut schedule = Schedule::new();
    schedule
        .add_parallel_system(parallel_system_fn(
            "writer",
            Access::new().write::<Pos>(),
            noop,
        ))
        .add_parallel_system(parallel_system_fn(
            "reader",
            Access::new().read::<Vel>().read::<Pos>(),
            noop,
        ));
    assert_eq!(
        plan(&schedule),
        [
            "parallel [writer] (first stage)".to_owned(),
            format!(
                "parallel [reader] (reads component `{}`, written by `writer`)",
                std::any::type_name::<Pos>()
            ),
        ]
    );
}

#[test]
fn reading_a_resource_written_earlier_in_the_stage_splits() {
    let mut schedule = Schedule::new();
    schedule
        .add_parallel_system(parallel_system_fn(
            "scorer",
            Access::new().write_resource::<Score>(),
            noop,
        ))
        .add_parallel_system(parallel_system_fn(
            "display",
            Access::new().read_resource::<Score>(),
            noop,
        ));
    assert_eq!(
        plan(&schedule),
        [
            "parallel [scorer] (first stage)".to_owned(),
            format!(
                "parallel [display] (reads resource `{}`, written by `scorer`)",
                std::any::type_name::<Score>()
            ),
        ]
    );
}

#[test]
fn component_conflicts_are_reported_before_resource_conflicts() {
    let mut schedule = Schedule::new();
    schedule
        .add_parallel_system(parallel_system_fn(
            "writer",
            Access::new().write_resource::<Score>().write::<Vel>(),
            noop,
        ))
        .add_parallel_system(parallel_system_fn(
            "reader",
            Access::new().read_resource::<Score>().read::<Vel>(),
            noop,
        ));
    assert_eq!(
        schedule.stages()[1].reason,
        format!(
            "reads component `{}`, written by `writer`",
            std::any::type_name::<Vel>()
        )
    );
}

#[test]
fn write_write_and_earlier_read_later_write_share_a_stage() {
    let mut schedule = Schedule::new();
    schedule
        .add_parallel_system(parallel_system_fn(
            "reader",
            Access::new().read::<Pos>(),
            noop,
        ))
        .add_parallel_system(parallel_system_fn(
            "first",
            Access::new().write::<Pos>(),
            noop,
        ))
        .add_parallel_system(parallel_system_fn(
            "second",
            Access::new().write::<Pos>().write_resource::<Score>(),
            noop,
        ))
        .add_parallel_system(parallel_system_fn(
            "third",
            Access::new().write_resource::<Score>(),
            noop,
        ));
    assert_eq!(
        plan(&schedule),
        ["parallel [reader, first, second, third] (first stage)"]
    );
}

#[test]
fn a_structural_system_is_the_last_member_of_its_stage() {
    let mut schedule = Schedule::new();
    schedule
        .add_parallel_system(parallel_system_fn(
            "reader",
            Access::new().read::<Pos>(),
            noop,
        ))
        .add_parallel_system(parallel_system_fn(
            "spawner",
            Access::new().structural(),
            noop,
        ))
        .add_parallel_system(parallel_system_fn("idle", Access::new(), noop));
    assert_eq!(
        plan(&schedule),
        [
            "parallel [reader, spawner] (first stage)",
            "parallel [idle] (`spawner` records structural commands)",
        ]
    );
}

#[test]
fn an_exclusive_system_closes_the_open_stage() {
    let mut schedule = Schedule::new();
    schedule
        .add_parallel_system(parallel_system_fn("a", Access::new().read::<Pos>(), noop))
        .add_system(system_fn("mutate", |_| {}))
        .add_parallel_system(parallel_system_fn("b", Access::new().read::<Pos>(), noop))
        .add_parallel_system(parallel_system_fn("c", Access::new().read::<Vel>(), noop));
    assert_eq!(
        plan(&schedule),
        [
            "parallel [a] (first stage)",
            "exclusive [mutate] (exclusive system)",
            "parallel [b, c] (follows an exclusive system)",
        ]
    );
    let stages = schedule.stages();
    assert!(!stages[2].exclusive);
    assert_eq!(stages[2].systems, ["b", "c"]);
}

#[test]
fn a_component_and_a_resource_of_the_same_type_are_distinct() {
    let mut schedule = Schedule::new();
    schedule
        .add_parallel_system(parallel_system_fn(
            "component_writer",
            Access::new().write::<Pos>(),
            noop,
        ))
        .add_parallel_system(parallel_system_fn(
            "resource_reader",
            Access::new().read_resource::<Pos>(),
            noop,
        ))
        .add_parallel_system(parallel_system_fn(
            "resource_writer",
            Access::new().write_resource::<Vel>(),
            noop,
        ))
        .add_parallel_system(parallel_system_fn(
            "component_reader",
            Access::new().read::<Vel>(),
            noop,
        ));
    assert_eq!(
        plan(&schedule),
        [
            "parallel [component_writer, resource_reader, resource_writer, component_reader] (first stage)"
        ]
    );
}

#[test]
fn isolated_mode_puts_every_system_in_its_own_stage() {
    let mut schedule = Schedule::new();
    schedule
        .add_parallel_system(parallel_system_fn("a", Access::new(), noop))
        .add_parallel_system(parallel_system_fn("b", Access::new(), noop))
        .add_system(system_fn("c", |_| {}))
        .add_parallel_system(parallel_system_fn("d", Access::new(), noop))
        .set_stage_mode(StageMode::Isolated);
    assert_eq!(schedule.stage_mode(), StageMode::Isolated);
    assert_eq!(
        plan(&schedule),
        [
            "parallel [a] (first stage)",
            "parallel [b] (isolated stage mode)",
            "exclusive [c] (exclusive system)",
            "parallel [d] (follows an exclusive system)",
        ]
    );
}

#[test]
fn stages_are_rebuilt_after_every_change() {
    let mut schedule = Schedule::new();
    schedule.add_parallel_system(parallel_system_fn("a", Access::new(), noop));
    assert_eq!(plan(&schedule), ["parallel [a] (first stage)"]);
    schedule.add_parallel_system(parallel_system_fn("b", Access::new(), noop));
    assert_eq!(plan(&schedule), ["parallel [a, b] (first stage)"]);
    schedule.set_stage_mode(StageMode::Isolated);
    assert_eq!(
        plan(&schedule),
        [
            "parallel [a] (first stage)",
            "parallel [b] (isolated stage mode)"
        ]
    );
    schedule.set_stage_mode(StageMode::Grouped);
    schedule.add_system(system_fn("c", |_| {}));
    assert_eq!(
        plan(&schedule),
        [
            "parallel [a, b] (first stage)",
            "exclusive [c] (exclusive system)"
        ]
    );
}

#[test]
fn adding_systems_never_touches_a_world() {
    let mut world = World::new();
    world.spawn((Vel { x: 1.0 },));
    let before = hash(&world);
    let mut schedule = Schedule::new();
    schedule
        .add_parallel_system(parallel_system_fn(
            "a",
            Access::new()
                .read::<Pos>()
                .write::<Score>()
                .read_resource::<Vel>()
                .write_resource::<Pos>()
                .structural(),
            noop,
        ))
        .set_stage_mode(StageMode::Isolated);
    let _ = schedule.stages();
    assert_eq!(hash(&world), before);
    schedule.run(&mut world);
    assert_eq!(hash(&world), before);
}

#[test]
fn schedule_debug_lists_both_kinds_in_list_order() {
    let mut schedule = Schedule::new();
    schedule
        .add_system(system_fn("first", |_| {}))
        .add_parallel_system(parallel_system_fn("second", Access::new(), noop));
    assert_eq!(
        format!("{schedule:?}"),
        r#"Schedule { systems: ["first", "second"] }"#
    );
}

fn executors() -> Vec<Arc<dyn Executor>> {
    vec![
        Arc::new(SequentialExecutor),
        Arc::new(PermutedExecutor::new(1)),
        Arc::new(PermutedExecutor::new(2)),
        Arc::new(PermutedExecutor::new(3)),
        Arc::new(PermutedExecutor::reversed()),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Grouped stages under every test executor give the hashes of isolated stages after every
    /// tick: the stage rule preserves the sequential semantics.
    #[test]
    fn grouped_equals_isolated(spec in schedule_spec()) {
        check_spec(&spec)?;
    }
}

fn check_spec(spec: &ScheduleSpec) -> Result<(), TestCaseError> {
    let reference = run_hashes(spec, Arc::new(SequentialExecutor), StageMode::Isolated);
    for executor in executors() {
        prop_assert_eq!(&run_hashes(spec, executor, StageMode::Grouped), &reference);
    }
    Ok(())
}
