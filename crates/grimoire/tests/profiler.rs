//! The frame profiler of the main loop (plan 0002 WP6.3, contract §9.2, §9.7): hash neutrality,
//! the `on_profile` hook, budgets and the export of recorded frames.

mod common;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use common::Scenario;
use grimoire::adapters::debug::{ProfilerBudgets, SCOPE_GPU, SCOPE_SIM, stats_frame};
use grimoire::debug::{FrameProfile, ProfileLog};
use grimoire::prelude::*;
use grimoire::sigil::SigilContent;

/// One 60 Hz tick per frame (see `tests/headless.rs`).
const ONE_TICK: Duration = Duration::from_nanos(16_666_667);

/// Adds systems with subsystem prefixes and a parallel stage to the shared scenario.
struct Subsystems;

impl GamePlugin for Subsystems {
    fn name(&self) -> &str {
        "subsystems"
    }

    fn build(&mut self, sim: &mut Simulation) {
        sim.schedule_mut()
            .add_system(system_fn("sigil.fake_update", |world| {
                if let Some(player) = world.resource_mut::<common::Player>() {
                    player.position.x += 0.25;
                }
            }))
            .add_parallel_system(parallel_system_fn(
                "collide.fake_broadphase",
                Access::new(),
                |_world, _commands| {},
            ))
            .add_parallel_system(parallel_system_fn(
                "sigil.fake_scan",
                Access::new(),
                |_world, _commands| {},
            ));
    }
}

/// Collects every frame's profile as an exportable `Stats` value.
#[derive(Default)]
struct Recorder {
    last_frame: Option<FrameStats>,
    content: Option<grimoire::sigil::ContentEpoch>,
    log: Rc<RefCell<ProfileLog>>,
    names: Rc<RefCell<Vec<Vec<String>>>>,
}

impl GamePlugin for Recorder {
    fn name(&self) -> &str {
        "recorder"
    }

    fn extract(&mut self, world: &World, _alpha: f32, _frame: &mut RenderFrame) {
        self.content = world.resource::<SigilContent>().map(SigilContent::epoch);
    }

    fn on_frame(&mut self, stats: &FrameStats) {
        self.last_frame = Some(*stats);
    }

    fn on_profile(&mut self, profile: &FrameProfile) {
        let stats = self.last_frame.expect("on_frame runs before on_profile");
        assert_eq!(profile.frame(), stats.frame);
        self.log
            .borrow_mut()
            .push(profile.to_stats(&stats_frame(&stats, self.content)));
        self.names.borrow_mut().push(
            profile
                .scopes()
                .iter()
                .filter_map(|total| profile.scope_name(total.scope).map(str::to_owned))
                .collect(),
        );
    }
}

fn app() -> AppBuilder {
    App::new(WindowConfig::default())
        .seed(0xC0FFEE)
        .plugin(Scenario { movers: 64 })
        .plugin(Subsystems)
}

#[test]
fn the_profiler_never_changes_a_state_hash() {
    // Mixed frame times, so some frames run several ticks and some none.
    let delta = Duration::from_nanos(23_000_000);
    let profiled = app()
        .hash_every(10)
        .run_headless_frames(180, delta)
        .expect("runs");
    let plain = app()
        .hash_every(10)
        .profiler(false)
        .run_headless_frames(180, delta)
        .expect("runs");
    assert_eq!(profiled, plain);
    let headless = app()
        .hash_every(10)
        .run_headless(profiled.final_tick, &mut |_| TickInput::default());
    assert_eq!(headless.hashes, profiled.hashes);
}

#[test]
fn on_profile_reports_loop_and_subsystem_scopes_with_budgets_and_counters() {
    let recorder = Recorder::default();
    let log = Rc::clone(&recorder.log);
    let names = Rc::clone(&recorder.names);
    app()
        .plugin(recorder)
        .run_headless_frames(4, ONE_TICK)
        .expect("runs");

    let names = names.borrow();
    assert_eq!(names.len(), 4);
    assert_eq!(
        names[1],
        [
            "app", "sigil", "collide", "sim", "extract", "render", "gpu", "frame"
        ],
        "systems in first-recorded order, then the loop scopes"
    );

    let log = log.borrow();
    let frame = &log.frames()[1];
    assert_eq!(
        (frame.frame, frame.sim_tick, frame.ticks_this_frame),
        (1, 2, 1)
    );
    let scope = |name: &str| {
        frame
            .scopes
            .iter()
            .find(|scope| scope.name == name)
            .expect("scope present")
    };
    // The manual clock of the headless loop does not move within a frame.
    assert!(frame.scopes.iter().all(|scope| scope.total_ns == 0));
    assert_eq!(scope(SCOPE_SIM).budget_ns, 4_000_000);
    assert_eq!(scope("collide").budget_ns, 1_500_000);
    assert_eq!(scope("app").budget_ns, 0, "no budget for app systems");
    // The null renderer has no timestamp queries: a marked estimate, never a silent zero.
    assert!(scope(SCOPE_GPU).estimate);
    assert!(!scope(SCOPE_SIM).estimate);
    assert!(
        scope("collide").estimate,
        "shares a parallel stage with sigil"
    );
    let counters: Vec<(&str, u64)> = frame
        .counters
        .iter()
        .map(|counter| (counter.name.as_str(), counter.value))
        .collect();
    assert_eq!(
        counters,
        [
            ("bullets_drawn", 0),
            ("bullets_rejected_palette_space", 0),
            ("bullets_rejected_invalid", 0)
        ]
    );

    let mut json = Vec::new();
    log.write_json(&mut json).expect("zero durations fit JSON");
    let json = String::from_utf8(json).expect("UTF-8");
    assert!(json.starts_with(
        r#"{"schema":"grimoire.profiler.export","schema_version":1,"frames":[{"frame":0,"#
    ));
    let mut csv = Vec::new();
    log.write_csv(&mut csv).expect("writing to a Vec");
    let csv = String::from_utf8(csv).expect("UTF-8");
    // Header plus eight scopes and three counters per frame.
    assert_eq!(csv.lines().count(), 1 + 4 * 11);
}

#[test]
fn budgets_from_the_builder_reach_the_profile() {
    let recorder = Recorder::default();
    let log = Rc::clone(&recorder.log);
    app()
        .profiler_budgets(
            ProfilerBudgets::none()
                .with(SCOPE_SIM, Some(Duration::from_micros(10)))
                .with("sigil", Some(Duration::from_nanos(1))),
        )
        .plugin(recorder)
        .run_headless_frames(2, ONE_TICK)
        .expect("runs");
    let log = log.borrow();
    let budgets: Vec<(&str, u64)> = log.frames()[1]
        .scopes
        .iter()
        .map(|scope| (scope.name.as_str(), scope.budget_ns))
        .collect();
    assert_eq!(
        budgets,
        [
            ("app", 0),
            ("sigil", 1),
            ("collide", 0),
            ("sim", 10_000),
            ("extract", 0),
            ("render", 0),
            ("gpu", 0),
            ("frame", 0)
        ]
    );
}

#[test]
fn without_the_profiler_no_profile_is_delivered() {
    let recorder = Recorder::default();
    let log = Rc::clone(&recorder.log);
    let report = app()
        .profiler(false)
        .plugin(recorder)
        .run_headless_frames(10, ONE_TICK)
        .expect("runs");
    assert_eq!(report.frames, 10);
    assert!(log.borrow().is_empty());
}
