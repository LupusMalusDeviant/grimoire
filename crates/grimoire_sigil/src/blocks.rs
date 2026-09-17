//! Emit-time shot placement: the seven blocks and the `mirror` modifier (contract §11.4,
//! `docs/formats/sigil.md` §10.4; PRD-0004 FR-01).
//!
//! Everything here runs once per *volley* (one `sigil.emit` firing of one emitter), never once
//! per bullet per tick, so calling `grimoire_core::math::dmath` freely here does not touch the
//! hot path the contract protects (§11.6: "Richtungs- und Rotationskonstanten kommen aus der Unit
//! ... oder werden einmalig beim Spawn berechnet"). [`crate::runtime`] is the module that must
//! stay trig-free, not this one.
//!
//! A [`Shot`] is the emit-time output of one block/mirror pass: an absolute heading, a speed and
//! a lateral position offset from the emitter's own spawn point. `sigil.emit` (`crate::systems`)
//! turns each into a [`crate::BulletSpawn`].

use grimoire_core::Vec2;
use grimoire_core::math::dmath;
use grimoire_sim::SimRng;

use crate::unit::{BlockDef, BlockKind, ModifierDef, ModifierKind, ProgramRecord};

/// One placed shot: heading (radians), speed and a position offset from the emitter's spawn
/// point, before the per-bullet modifier stack's spawn-time contribution (`crate::runtime`) is
/// added on top.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Shot {
    pub(crate) angle: f32,
    pub(crate) speed: f32,
    pub(crate) offset: Vec2,
}

/// Generates the shots of one block, given the emitter's `base_speed`/`base_angle`
/// (`Emitter::rotation` plus the unit's per-emitter `speed`), the current aim point (`Aimed`
/// only) and, for `spiral`, which volley this is since the emitter started (contract §11.4:
/// emitters are stateless, so the volley index — not any stored rotation — is what makes a spiral
/// wind up over repeated firings; see `crate::systems::volley_index`).
///
/// `rng` is drawn from only by `scatter` (contract §11.4's "Streuung zieht aus
/// `derive_block_rng`"); every other kind ignores it and therefore never advances it, so an
/// emitter with a non-scatter program consumes no randomness at all.
///
/// A `count`/`arms` of `0` cannot arise from a compiled unit (`docs/formats/sigil.md` `SIG0017`
/// forbids it at the source level) but is not re-checked by the decoder (contract §11.1 does not
/// list it among the decoder's re-checks); this function treats it as `1` rather than dividing by
/// zero, matching contract §2 rule 9's "malformed input is an error, never a panic" even for a
/// hand-built or fuzzed-but-otherwise-valid `SigilUnit`.
pub(crate) fn generate_shots(
    block: &BlockDef,
    base_speed: f32,
    base_angle: f32,
    aim: Option<Vec2>,
    origin: Vec2,
    volley_index: u32,
    rng: &mut SimRng,
) -> Vec<Shot> {
    let count = u32::from(block.count.max(1));
    match block.kind {
        BlockKind::RING => ring(block, count, base_speed, base_angle),
        BlockKind::SPIRAL => spiral(block, count, base_speed, base_angle, volley_index),
        BlockKind::FAN => fan(block, count, base_speed, base_angle + block.params[1]),
        BlockKind::AIMED => {
            let direction = match aim {
                Some(target) => (target - origin).angle(),
                None => base_angle,
            };
            fan(block, count, base_speed, direction)
        }
        BlockKind::WAVE => wave(block, count, base_speed),
        BlockKind::LINE => line(block, count, base_speed),
        BlockKind::SCATTER => scatter(block, count, base_speed, base_angle, rng),
        // The decoder only ever hands this function a `SigilUnit` it has already validated
        // (`kind` in `1..=BlockKind::MAX`); an unreachable default keeps this exhaustive without
        // inventing an eighth block kind here.
        _ => Vec::new(),
    }
}

/// Generates one volley of `program`: its block's shots (see [`generate_shots`]), then every
/// `mirror` modifier of its stack applied in authored order. Shared by `sigil.emit` (an emitter's
/// volley) and `sigil.update` (the volley of a `burst` or `become_emitter` transform), so both
/// place shots identically; only the generator `rng` they pass differs (contract §11.7).
pub(crate) fn program_shots(
    program: &ProgramRecord,
    base_speed: f32,
    base_angle: f32,
    aim: Option<Vec2>,
    origin: Vec2,
    volley_index: u32,
    rng: &mut SimRng,
) -> Vec<Shot> {
    let mut shots = generate_shots(
        &program.block,
        base_speed,
        base_angle,
        aim,
        origin,
        volley_index,
        rng,
    );
    for modifier in &program.modifiers {
        if modifier.kind == ModifierKind::MIRROR {
            shots = apply_mirror(&shots, modifier, base_angle);
        }
    }
    shots
}

const TAU: f32 = dmath::TAU;

fn ring(block: &BlockDef, count: u32, base_speed: f32, base_angle: f32) -> Vec<Shot> {
    let start = block.params[0];
    let step = TAU / count as f32;
    (0..count)
        .map(|i| Shot {
            angle: base_angle + start + step * i as f32,
            speed: base_speed,
            offset: Vec2::ZERO,
        })
        .collect()
}

/// `spiral`'s `params[0]` is the per-volley rotation `step`, `params[1]` the initial `start`
/// offset; `count` is `arms`. The base rotation of volley `n` is `start + step * n`, a pure
/// function of the volley index alone (contract §11.4: no per-emitter state), so two emitters
/// firing the same spiral program stay perfectly in sync regardless of scheduling.
fn spiral(
    block: &BlockDef,
    arms: u32,
    base_speed: f32,
    base_angle: f32,
    volley_index: u32,
) -> Vec<Shot> {
    let step = block.params[0];
    let start = block.params[1];
    let rotation_for_volley = start + step * volley_index as f32;
    let arm_step = TAU / arms as f32;
    (0..arms)
        .map(|i| Shot {
            angle: base_angle + rotation_for_volley + arm_step * i as f32,
            speed: base_speed,
            offset: Vec2::ZERO,
        })
        .collect()
}

/// Shared by `fan` (`center` around `base_angle`) and `aimed` (`center` is the resolved aim
/// direction, `docs/formats/sigil.md` §10.4: "aimed ... spread (rad)" with no separate `center`
/// param of its own).
fn fan(block: &BlockDef, count: u32, base_speed: f32, center: f32) -> Vec<Shot> {
    let spread = block.params[0];
    if count == 1 {
        return vec![Shot {
            angle: center,
            speed: base_speed,
            offset: Vec2::ZERO,
        }];
    }
    let step = spread / (count - 1) as f32;
    let half = spread * 0.5;
    (0..count)
        .map(|i| Shot {
            angle: center - half + step * i as f32,
            speed: base_speed,
            offset: Vec2::ZERO,
        })
        .collect()
}

/// `wave`'s baked `params[4..6]` unit vector (`docs/formats/sigil.md` §10.4) is used as the shots'
/// absolute heading directly — the compiler already spent its one allowed `dmath::cos`/`sin` call
/// baking it, so this function does not call `dmath` for direction at all, only for the per-shot
/// sine lateral offset (`params[0]` amplitude, `params[1]` wavelength), which is still emit-time,
/// not per-tick.
fn wave(block: &BlockDef, count: u32, base_speed: f32) -> Vec<Shot> {
    let amplitude = block.params[0];
    let wavelength = if block.params[1].abs() > f32::EPSILON {
        block.params[1]
    } else {
        1.0
    };
    let angle = block.params[2];
    let direction = Vec2::new(block.params[4], block.params[5]);
    let perp = direction.perp();
    let mid = (count - 1) as f32 * 0.5;
    (0..count)
        .map(|i| {
            let t = i as f32 - mid;
            let lateral = amplitude * dmath::sin(TAU * t / wavelength);
            Shot {
                angle,
                speed: base_speed,
                offset: perp * lateral,
            }
        })
        .collect()
}

/// `line`'s baked `params[4..6]` unit vector, used the same way as `wave`'s but for a fixed
/// `spacing` (`params[0]`) instead of a sine lateral offset: no `dmath` call at all.
fn line(block: &BlockDef, count: u32, base_speed: f32) -> Vec<Shot> {
    let spacing = block.params[0];
    let angle = block.params[1];
    let direction = Vec2::new(block.params[4], block.params[5]);
    let perp = direction.perp();
    let mid = (count - 1) as f32 * 0.5;
    (0..count)
        .map(|i| Shot {
            angle,
            speed: base_speed,
            offset: perp * (spacing * (i as f32 - mid)),
        })
        .collect()
}

/// `scatter`'s only randomised block (contract §11.4): draws exactly two `SimRng::next_f32`s per
/// shot — one for the angular jitter within `cone`, one for the speed jitter — regardless of
/// whether `cone`/`speed_jitter` are `0`, so the stream position after a scatter block never
/// depends on its parameters, only on its shot count.
fn scatter(
    block: &BlockDef,
    count: u32,
    base_speed: f32,
    base_angle: f32,
    rng: &mut SimRng,
) -> Vec<Shot> {
    let cone = block.params[0];
    let base = base_angle + block.params[1];
    let speed_jitter = block.params[2].clamp(0.0, 1.0);
    (0..count)
        .map(|_| {
            let angle = base + (rng.next_f32() - 0.5) * cone;
            let speed_mul = 1.0 + (rng.next_f32() - 0.5) * 2.0 * speed_jitter;
            Shot {
                angle,
                speed: base_speed * speed_mul,
                offset: Vec2::ZERO,
            }
        })
        .collect()
}

/// Applies a `mirror` modifier to an already-generated shot list (contract §11.4's stackable
/// modifiers; `docs/formats/sigil.md` §10.4's `mirror`), appending reflected copies of every
/// original shot rather than recursively re-mirroring already-mirrored ones.
///
/// `modifier.extra` (`mirror.folds`) is the number of evenly-spaced mirror axes starting at
/// `base_angle + modifier.params[0]`, `π/folds` apart; the source-level compiler rejects `folds ==
/// 0` as a division-by-zero (`SIG0017`), so, like [`generate_shots`]'s block `count`, a wire-level
/// `0` is defensively treated as `1` instead of panicking. This mirrors around a `folds`-way
/// rotationally-spaced set of axes rather than the alternative (recursive kaleidoscope) reading;
/// see the WP5.1 report for this as an open point for the Product Owner.
pub(crate) fn apply_mirror(shots: &[Shot], modifier: &ModifierDef, base_angle: f32) -> Vec<Shot> {
    let folds = u32::from(modifier.extra.max(1));
    let axis0 = base_angle + modifier.params[0];
    let mut all = Vec::with_capacity(shots.len() * (1 + folds as usize));
    all.extend_from_slice(shots);
    for k in 0..folds {
        let axis = axis0 + (dmath::PI / folds as f32) * k as f32;
        let normal = Vec2::from_angle(axis);
        for shot in shots {
            all.push(Shot {
                angle: 2.0 * axis - shot.angle,
                speed: shot.speed,
                offset: reflect(shot.offset, normal),
            });
        }
    }
    all
}

/// Reflects `v` across the line through the origin in direction `axis` (a unit vector).
fn reflect(v: Vec2, axis: Vec2) -> Vec2 {
    axis * (2.0 * v.dot(axis)) - v
}

#[cfg(test)]
mod tests {
    use grimoire_sim::derive_rng;

    use super::*;

    fn block(kind: u8, count: u16, params: [f32; 6]) -> BlockDef {
        BlockDef {
            kind,
            count,
            params,
            seed_hash: 0,
        }
    }

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn ring_spreads_shots_evenly_over_a_full_turn() {
        let b = block(BlockKind::RING, 4, [0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let mut rng = derive_rng(1, 0, 1);
        let shots = generate_shots(&b, 2.0, 0.0, None, Vec2::ZERO, 0, &mut rng);
        assert_eq!(shots.len(), 4);
        let angles: Vec<f32> = shots.iter().map(|s| s.angle).collect();
        for (i, &angle) in angles.iter().enumerate() {
            assert!(approx_eq(angle, dmath::TAU / 4.0 * i as f32));
        }
        assert!(shots.iter().all(|s| s.speed == 2.0));
    }

    #[test]
    fn spiral_advances_by_step_times_volley_index() {
        let b = block(BlockKind::SPIRAL, 1, [0.3, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let mut rng = derive_rng(1, 0, 1);
        let volley5 = generate_shots(&b, 1.0, 0.0, None, Vec2::ZERO, 5, &mut rng);
        assert!(approx_eq(volley5[0].angle, 0.3 * 5.0));
    }

    #[test]
    fn fan_centers_the_spread_and_handles_a_single_shot() {
        let b = block(
            BlockKind::FAN,
            3,
            [dmath::FRAC_PI_2, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        let mut rng = derive_rng(1, 0, 1);
        let shots = generate_shots(&b, 1.0, 0.0, None, Vec2::ZERO, 0, &mut rng);
        assert_eq!(shots.len(), 3);
        assert!(approx_eq(shots[0].angle, -dmath::FRAC_PI_2 / 2.0));
        assert!(approx_eq(shots[1].angle, 0.0));
        assert!(approx_eq(shots[2].angle, dmath::FRAC_PI_2 / 2.0));

        let single = block(BlockKind::FAN, 1, [1.0, 0.5, 0.0, 0.0, 0.0, 0.0]);
        let solo = generate_shots(&single, 1.0, 0.0, None, Vec2::ZERO, 0, &mut rng);
        assert_eq!(solo.len(), 1);
        assert!(approx_eq(solo[0].angle, 0.5));
    }

    #[test]
    fn aimed_points_at_the_target_and_falls_back_to_rotation() {
        let b = block(BlockKind::AIMED, 1, [0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let mut rng = derive_rng(1, 0, 1);
        let aimed = generate_shots(
            &b,
            1.0,
            0.7,
            Some(Vec2::new(1.0, 0.0)),
            Vec2::ZERO,
            0,
            &mut rng,
        );
        assert!(approx_eq(aimed[0].angle, 0.0));

        let unaimed = generate_shots(&b, 1.0, 0.7, None, Vec2::ZERO, 0, &mut rng);
        assert!(approx_eq(unaimed[0].angle, 0.7));
    }

    #[test]
    fn line_spaces_shots_symmetrically_with_no_trig_needed() {
        // direction along +Y (already "baked"), so the perpendicular spacing runs along X.
        let b = block(BlockKind::LINE, 3, [2.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
        let mut rng = derive_rng(1, 0, 1);
        let shots = generate_shots(&b, 1.0, 0.0, None, Vec2::ZERO, 0, &mut rng);
        assert_eq!(shots.len(), 3);
        // `perp()` rotates +Y by +90 deg to -X, so ascending shot index runs -X -> +X.
        assert!(approx_eq(shots[0].offset.x, 2.0));
        assert!(approx_eq(shots[1].offset.x, 0.0));
        assert!(approx_eq(shots[2].offset.x, -2.0));
        assert!(shots.iter().all(|s| s.offset.y.abs() < 1e-6));
    }

    #[test]
    fn scatter_draws_exactly_two_f32s_per_shot_regardless_of_parameters() {
        let degenerate = block(BlockKind::SCATTER, 5, [0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let mut a = derive_rng(1, 0, 1);
        let _ = generate_shots(&degenerate, 1.0, 0.0, None, Vec2::ZERO, 0, &mut a);
        let mut b = derive_rng(1, 0, 1);
        for _ in 0..10 {
            b.next_f32();
        }
        assert_eq!(
            a, b,
            "must consume exactly 2 draws per shot, cone/jitter 0 or not"
        );
    }

    #[test]
    fn scatter_is_deterministic_for_a_given_rng_state() {
        let b = block(BlockKind::SCATTER, 8, [0.5, 0.0, 0.3, 0.0, 0.0, 0.0]);
        let mut rng1 = derive_rng(42, 3, 7);
        let mut rng2 = derive_rng(42, 3, 7);
        let shots1 = generate_shots(&b, 1.0, 0.0, None, Vec2::ZERO, 0, &mut rng1);
        let shots2 = generate_shots(&b, 1.0, 0.0, None, Vec2::ZERO, 0, &mut rng2);
        assert_eq!(shots1, shots2);
    }

    #[test]
    fn mirror_doubles_shots_and_reflects_angle_across_the_axis() {
        let shots = vec![Shot {
            angle: 0.2,
            speed: 1.0,
            offset: Vec2::new(1.0, 0.0),
        }];
        let modifier = ModifierDef {
            kind: crate::unit::ModifierKind::MIRROR,
            flag: 0,
            extra: 1,
            params: [0.0, 0.0, 0.0],
        };
        let mirrored = apply_mirror(&shots, &modifier, 0.0);
        assert_eq!(mirrored.len(), 2);
        assert!(approx_eq(mirrored[0].angle, 0.2));
        assert!(approx_eq(mirrored[1].angle, -0.2));
    }

    #[test]
    fn mirror_folds_zero_behaves_like_folds_one_instead_of_panicking() {
        let shots = vec![Shot {
            angle: 0.1,
            speed: 1.0,
            offset: Vec2::ZERO,
        }];
        let modifier = ModifierDef {
            kind: crate::unit::ModifierKind::MIRROR,
            flag: 0,
            extra: 0,
            params: [0.0, 0.0, 0.0],
        };
        let mirrored = apply_mirror(&shots, &modifier, 0.0);
        assert_eq!(mirrored.len(), 2);
    }
}
