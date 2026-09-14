//! Determinism through the facade: pure headless runs and the headless frame loop.

mod common;

use std::time::Duration;

use common::{Scenario, bot_input};
use grimoire::prelude::*;
use grimoire::{HeadlessReport, LoopReport};

const MOVERS: u32 = 500;

fn headless(seed: u64, ticks: u64, input: &mut dyn FnMut(u64) -> TickInput) -> HeadlessReport {
    App::new(WindowConfig::default())
        .seed(seed)
        .hash_every(60)
        .plugin(Scenario { movers: MOVERS })
        .run_headless(ticks, input)
}

#[test]
fn identical_runs_give_identical_hash_sequences() {
    let first = headless(7, 600, &mut bot_input);
    let second = headless(7, 600, &mut bot_input);
    assert_eq!(first, second);
    assert_eq!(first.final_tick, 600);
    let ticks: Vec<u64> = first.hashes.iter().map(|&(tick, _)| tick).collect();
    assert_eq!(ticks, (1..=10).map(|i| i * 60).collect::<Vec<_>>());
    assert_eq!(first.hashes.last(), Some(&(600, first.final_hash)));
}

#[test]
fn a_different_seed_gives_a_different_final_hash() {
    let seven = headless(7, 600, &mut bot_input);
    let eight = headless(8, 600, &mut bot_input);
    assert_ne!(seven.final_hash, eight.final_hash);
}

#[test]
fn input_changes_the_final_hash() {
    let bot = headless(7, 120, &mut bot_input);
    let idle = headless(7, 120, &mut |_| TickInput::default());
    assert_ne!(bot.final_hash, idle.final_hash);
}

#[test]
fn input_source_receives_the_tick_about_to_run() {
    let mut seen = Vec::new();
    let report = headless(1, 5, &mut |tick| {
        seen.push(tick);
        TickInput::default()
    });
    assert_eq!(seen, [0, 1, 2, 3, 4]);
    assert_eq!(report.final_tick, 5);
}

#[test]
fn hash_every_zero_records_only_the_final_state() {
    let report = App::new(WindowConfig::default())
        .hash_every(0)
        .plugin(Scenario { movers: 10 })
        .run_headless(100, &mut bot_input);
    assert_eq!(report.hashes, vec![(100, report.final_hash)]);

    let empty = App::new(WindowConfig::default()).run_headless(0, &mut bot_input);
    assert_eq!(empty.final_tick, 0);
    assert_eq!(empty.hashes, vec![(0, empty.final_hash)]);
}

#[test]
fn frame_loop_simulates_the_same_states_as_the_pure_headless_run() {
    // 16 666 667 ns × 60 Hz is one tick plus 20 units, so every frame runs exactly one tick.
    let frame_delta = Duration::from_nanos(16_666_667);
    let looped: LoopReport = App::new(WindowConfig::default())
        .seed(99)
        .hash_every(30)
        .plugin(Scenario { movers: MOVERS })
        .run_headless_frames(300, frame_delta)
        .expect("headless frame loop runs");
    let pure = App::new(WindowConfig::default())
        .seed(99)
        .hash_every(30)
        .plugin(Scenario { movers: MOVERS })
        .run_headless(300, &mut |_| TickInput::default());

    assert_eq!(looped.frames, 300);
    assert_eq!(looped.final_tick, 300);
    assert_eq!(looped.final_hash, pure.final_hash);
    assert_eq!(looped.hashes, pure.hashes);
    assert_eq!(looped.dropped_time, Duration::ZERO);
}

#[test]
fn frame_loop_runs_are_reproducible() {
    let run = || {
        App::new(WindowConfig::default())
            .seed(3)
            .plugin(Scenario { movers: MOVERS })
            .run_headless_frames(500, Duration::from_micros(9_100))
            .expect("headless frame loop runs")
    };
    assert_eq!(run(), run());
}
