//! Deterministic contract tests for the player proxy (contract §9.5, feature `fixtures`).
//!
//! Builds a bare [`Simulation`] and registers only [`PlayerProxyPlugin`] on it — no `App`, no
//! renderer, no window — exactly like [`grimoire_sim::Simulation`] is used everywhere else in this
//! crate's own tests.

#![cfg(feature = "fixtures")]

use std::sync::Arc;

use grimoire::ecs::PermutedExecutor;
use grimoire::fixtures::{PlayerProxyPlugin, ProxyAim, ProxyConfig, ProxyPosition, proxy_entity};
use grimoire::prelude::*;

/// A short, fixed sequence of raw axis pairs covering both signed extremes, the centre and two
/// diagonals — the movement table from contract §9.5.
const AXIS_TABLE: [(i16, i16); 6] = [
    (0, 0),
    (32767, 0),
    (0, 32767),
    (-32768, 0),
    (0, -32768),
    (32767, 32767),
];

fn frame(axis0: i16, axis1: i16) -> InputFrame {
    InputFrame {
        axes: [axis0, axis1, 0, 0],
        buttons: 0,
    }
}

fn tick_input(slot: usize, frame_value: InputFrame) -> TickInput {
    let mut input = TickInput::default();
    input.slots[slot] = frame_value;
    input
}

/// A fresh simulation with only [`PlayerProxyPlugin`] registered.
fn build_sim(seed: u64, config: ProxyConfig) -> Simulation {
    let mut sim = Simulation::new(seed);
    PlayerProxyPlugin::new(config).build(&mut sim);
    sim
}

fn proxy_position(sim: &Simulation) -> Vec2 {
    let entity = proxy_entity(sim.world()).expect("player proxy entity must exist after build");
    sim.world()
        .get::<ProxyPosition>(entity)
        .expect("player proxy must have ProxyPosition")
        .0
}

/// Deterministic 8-phase input pattern used by the longer scripted tests: no RNG, so the sequence
/// is identical on every platform and every run by construction, not just by seed.
fn scripted_frame(tick: u64) -> InputFrame {
    const AXIS0: [i16; 8] = [32767, 16000, 0, -16000, -32768, -16000, 0, 16000];
    const AXIS1: [i16; 8] = [0, 20000, 32767, 20000, 0, -20000, -32768, -20000];
    let phase = (tick % 8) as usize;
    frame(AXIS0[phase], AXIS1[phase])
}

/// Runs `ticks` steps of [`scripted_frame`], always recorded into slot 0 regardless of which
/// slot `config` tells the proxy to read — so a `config.slot != 0` genuinely exercises "the
/// proxy reads the wrong input", not just "the same script fed through a different slot".
fn run_scripted(config: ProxyConfig, ticks: u64) -> u64 {
    let mut sim = build_sim(0xF00D, config);
    for tick in 0..ticks {
        sim.step(tick_input(0, scripted_frame(tick)));
    }
    sim.state_hash()
}

#[test]
fn movement_table_matches_the_axis_formula() {
    // `ProxyConfig` is `#[non_exhaustive]`: adjust by field assignment onto `default()` (contract
    // §2 rule 13), not by struct-literal or functional-update syntax, both forbidden outside the
    // defining crate.
    let mut config = ProxyConfig::default();
    config.start = Vec2::ZERO;
    config.speed_per_tick = 1.0;
    for &(axis0, axis1) in &AXIS_TABLE {
        let mut sim = build_sim(1, config);
        sim.step(tick_input(config.slot, frame(axis0, axis1)));

        let v = Vec2::new(
            grimoire_core::math::dmath::max(f32::from(axis0) / 32_767.0, -1.0),
            grimoire_core::math::dmath::max(f32::from(axis1) / 32_767.0, -1.0),
        );
        let normalized = if v.length_squared() > 1.0 {
            v / grimoire_core::math::dmath::sqrt(v.length_squared())
        } else {
            v
        };
        let expected = normalized * config.speed_per_tick;
        let actual = proxy_position(&sim);
        assert!(
            (actual.x - expected.x).abs() < 1e-5 && (actual.y - expected.y).abs() < 1e-5,
            "axes ({axis0}, {axis1}): expected {expected:?}, got {actual:?}"
        );
    }
}

#[test]
fn movement_clamps_to_configured_bounds() {
    let mut config = ProxyConfig::default();
    config.start = Vec2::ZERO;
    config.speed_per_tick = 5.0;
    config.bounds = grimoire::collide::Aabb {
        min: Vec2::new(-1.0, -1.0),
        max: Vec2::new(1.0, 1.0),
    };
    let mut sim = build_sim(2, config);
    sim.step(tick_input(config.slot, frame(32767, 32767)));
    let position = proxy_position(&sim);
    assert_eq!(position, Vec2::new(1.0, 1.0));
}

#[test]
fn aim_target_and_graze_probe_are_written_in_the_same_tick() {
    let mut config = ProxyConfig::default();
    config.start = Vec2::ZERO;
    config.speed_per_tick = 2.0;
    let mut sim = build_sim(3, config);
    sim.step(tick_input(config.slot, frame(32767, 0)));

    let position = proxy_position(&sim);
    assert_ne!(position, Vec2::ZERO);

    let aim = sim
        .world()
        .resource::<grimoire::sigil::AimTarget>()
        .copied()
        .expect("AimTarget resource must exist");
    assert_eq!(aim.0, Some(position));

    let probe = sim
        .world()
        .resource::<grimoire::adapters::sigil_collide::GrazeProbe>()
        .copied()
        .expect("GrazeProbe resource must exist");
    assert_eq!(probe.ring.center, position);
    assert_eq!(probe.ring.inner_radius, config.graze_inner_radius);
    assert_eq!(probe.ring.outer_radius, config.graze_outer_radius);
    assert_eq!(probe.mask, config.graze_mask);
}

#[test]
fn proxy_aim_component_records_the_raw_axes_of_the_tick() {
    // `ProxyAim` records raw axes 2/3 (the aim stick), not the movement axes 0/1 that
    // `frame`/`scripted_frame` set — a distinct pair on purpose, so this also pins the axis
    // mapping against a regression that reads the wrong pair.
    let config = ProxyConfig::default();
    let mut sim = build_sim(4, config);
    let mut input = tick_input(config.slot, frame(0, 0));
    input.slots[config.slot].axes[2] = 111;
    input.slots[config.slot].axes[3] = -222;
    sim.step(input);

    let entity = proxy_entity(sim.world()).unwrap();
    let aim = sim.world().get::<ProxyAim>(entity).unwrap();
    assert_eq!(aim.0, [111, -222]);
}

#[test]
fn snapshot_restore_then_identical_ticks_is_bit_identical() {
    let config = ProxyConfig::default();
    let mut sim = build_sim(5, config);
    for tick in 0..50u64 {
        sim.step(tick_input(config.slot, scripted_frame(tick)));
    }
    let snapshot = sim.snapshot();

    for tick in 50..80u64 {
        sim.step(tick_input(config.slot, scripted_frame(tick)));
    }
    let hash_a = sim.state_hash();

    sim.restore(&snapshot);
    for tick in 50..80u64 {
        sim.step(tick_input(config.slot, scripted_frame(tick)));
    }
    let hash_b = sim.state_hash();

    assert_eq!(hash_a, hash_b);
}

#[test]
fn hashes_agree_across_executors() {
    let config = ProxyConfig::default();
    let ticks = 120u64;

    let mut sequential = build_sim(6, config);
    for tick in 0..ticks {
        sequential.step(tick_input(config.slot, scripted_frame(tick)));
    }

    let mut permuted = build_sim(6, config);
    permuted
        .world_mut()
        .set_executor(Arc::new(PermutedExecutor::new(1)));
    for tick in 0..ticks {
        permuted.step(tick_input(config.slot, scripted_frame(tick)));
    }

    let mut reversed = build_sim(6, config);
    reversed
        .world_mut()
        .set_executor(Arc::new(PermutedExecutor::reversed()));
    for tick in 0..ticks {
        reversed.step(tick_input(config.slot, scripted_frame(tick)));
    }

    assert_eq!(sequential.state_hash(), permuted.state_hash());
    assert_eq!(sequential.state_hash(), reversed.state_hash());
}

/// Frozen end-of-run hash for 600 ticks of [`scripted_frame`] against the default
/// [`ProxyConfig`]. A change to the movement formula, the axis mapping, the default speed or
/// bounds, or anything else this system touches, changes this constant.
const GOLDEN_PROXY_HASH: u64 = 0xaeeb_f8ad_fa22_1194;

#[test]
fn golden_end_hash_over_600_scripted_ticks() {
    let hash = run_scripted(ProxyConfig::default(), 600);
    assert_eq!(
        hash, GOLDEN_PROXY_HASH,
        "state hash changed: {hash:#018x} (expected {GOLDEN_PROXY_HASH:#018x}). If this change \
         is intentional, recompute and update GOLDEN_PROXY_HASH."
    );
}

#[test]
#[should_panic(expected = "player proxy already exists")]
fn a_second_proxy_in_the_same_simulation_panics() {
    let mut sim = Simulation::new(7);
    PlayerProxyPlugin::new(ProxyConfig::default()).build(&mut sim);
    PlayerProxyPlugin::new(ProxyConfig::default()).build(&mut sim);
}

/// A replay/hash regression test (per WP1.3 integration scope): a changed speed or a changed axis
/// source (stand-in for a swapped/broken axis mapping) must change the resulting hash. If either
/// regressed silently, this test — and [`golden_end_hash_over_600_scripted_ticks`] — would start
/// failing.
#[test]
fn changed_speed_or_input_slot_changes_the_hash() {
    let baseline = run_scripted(ProxyConfig::default(), 200);

    let mut faster = ProxyConfig::default();
    faster.speed_per_tick *= 2.0;
    assert_ne!(baseline, run_scripted(faster, 200));

    // `run_scripted` always feeds `scripted_frame` into slot 0; reading a different slot is
    // observably equivalent to a broken axis/slot mapping, since slot 1 stays all-zero here.
    let mut other_slot = ProxyConfig::default();
    other_slot.slot = 1;
    assert_ne!(baseline, run_scripted(other_slot, 200));
}

// --- WP2.4: focus, extract_stage and the camera replay-hash gate ---------------------------

#[test]
fn focus_reports_the_interpolated_position_and_none_before_build() {
    let sim = Simulation::new(20);
    let plugin = PlayerProxyPlugin::default();
    assert_eq!(
        plugin.focus(sim.world(), 0.0),
        None,
        "no proxy entity exists before `build`"
    );

    let mut config = ProxyConfig::default();
    config.start = Vec2::ZERO;
    config.speed_per_tick = 4.0;
    let mut sim = build_sim(21, config);
    sim.step(tick_input(config.slot, frame(32767, 0)));

    let alpha = 0.25;
    let focus = plugin
        .focus(sim.world(), alpha)
        .expect("a focus point exists once the proxy has moved");
    let expected = Vec2::ZERO.lerp(Vec2::new(4.0, 0.0), alpha);
    assert!(
        (focus.x - expected.x).abs() < 1e-5 && (focus.y - expected.y).abs() < 1e-5,
        "expected {expected:?}, got {focus:?}"
    );
}

#[test]
fn extract_stage_draws_a_marker_at_the_same_interpolated_position_as_focus() {
    let mut config = ProxyConfig::default();
    config.start = Vec2::new(1.0, -2.0);
    config.speed_per_tick = 2.0;
    let mut sim = build_sim(22, config);
    sim.step(tick_input(config.slot, frame(0, 32767)));

    let mut plugin = PlayerProxyPlugin::default();
    let alpha = 0.6;
    let focus = plugin.focus(sim.world(), alpha).unwrap();

    let mut stage = grimoire::render::StageFrame::new();
    plugin.extract_stage(sim.world(), alpha, &mut stage);
    assert_eq!(stage.marker_sprites.len(), 1, "exactly one marker sprite");
    let marker = stage.marker_sprites[0];
    assert_eq!(marker.shape, grimoire::render::shape::CIRCLE);
    assert!(
        (marker.position[0] - focus.x).abs() < 1e-6 && (marker.position[1] - focus.y).abs() < 1e-6,
        "marker position {:?} must match focus {:?}",
        marker.position,
        focus
    );
}

/// The plan 0002 WP2.4 gate: replaying a fixed, recorded `TickInput` sequence must give identical
/// simulation hashes regardless of the camera the run is configured with — the contract's
/// determinism rule ("Die Simulation liest nie Kamera-, Zeiger-, Viewport- oder
/// Interpolationszustand") holds structurally (`run_headless` never calls `extract`/
/// `extract_stage`/the camera follow spring at all), but this test pins the observable behaviour
/// through the public `AppBuilder` API rather than only the internal wiring.
#[test]
fn replay_of_fixed_recorded_tick_inputs_is_independent_of_camera_parameters() {
    let recorded: Vec<TickInput> = (0..300u64)
        .map(|tick| tick_input(0, scripted_frame(tick)))
        .collect();

    let mut camera_a = Camera25D::default();
    camera_a.tilt_degrees = 60.0;
    camera_a.fov_y_degrees = 45.0;
    camera_a.distance = 10.0;
    camera_a.look_ahead_max = 1.0;
    camera_a.look_ahead_smoothing = 0.1;

    let mut camera_b = Camera25D::default();
    camera_b.tilt_degrees = 75.0;
    camera_b.fov_y_degrees = 90.0;
    camera_b.distance = 40.0;
    camera_b.look_ahead_max = 20.0;
    camera_b.look_ahead_smoothing = 2.0;
    assert_ne!(
        camera_a, camera_b,
        "the two configurations must actually differ for this gate to mean anything"
    );

    let run = |camera: Camera25D| {
        let recorded = recorded.clone();
        App::new(WindowConfig::default())
            .seed(0xC0FFEE)
            .camera25d(camera)
            .plugin(PlayerProxyPlugin::default())
            .run_headless(recorded.len() as u64, &mut move |tick| {
                recorded[tick as usize]
            })
    };

    let report_a = run(camera_a);
    let report_b = run(camera_b);
    assert_eq!(report_a.final_tick, report_b.final_tick);
    assert_eq!(report_a.final_hash, report_b.final_hash);
    assert_eq!(report_a.hashes, report_b.hashes);
}
