//! `sigil.update`'s per-tick interpreter: program resolution, the six stackable modifiers, and
//! the lifetime/bounds check (contract §11.6/§11.7, PRD-0004 FR-01).
//!
//! **The hot path is trig-free.** Nothing in this module calls
//! `grimoire_core::math::dmath::{sin,cos,tan,atan2,...}` on every bullet of every tick. The two
//! modifiers whose effect is inherently angular over time (`rotate`, `curve`) advance a bullet's
//! `velocity`/`angle` by a *constant* per-tick delta, and `sine_offset` advances a running
//! oscillator `(s, c)` by a constant per-tick rotation — in both cases the constant is `cos`/`sin`
//! of a value fixed by the *modifier definition* (its `rate`/`turn`/period-derived `w`), not by
//! the bullet or the tick, so it is computed once by [`RuntimeCache::ensure`] when a unit's
//! content is (re)loaded, not once per bullet and never once per tick. `sine_offset`'s own
//! oscillator still needs exactly one `dmath::sin`/`cos` pair per *bullet*, seeded from its
//! `phase` on that bullet's first tick (contract §11.3 freezes `BulletPool::spawn`'s `state =
//! [0.0; 4]`, so this cannot happen literally at spawn; it happens on the first `sigil.update` a
//! newly spawned bullet sees instead, which is the earliest point this module can reach and is
//! still "once per bullet", never "once per bullet per tick").
//!
//! `speed_curve` needs no per-bullet state at all: it multiplies the current speed by
//! `curve(age) / curve(age - 1)` every tick, a ratio recomputed fresh from `age` alone (contract
//! §8's `Tick`-derived, not wall-clock, quantity), with no trigonometry involved either way.
//!
//! State budget (`BulletMotion::state`/`BulletColumns::state`, `[f32; 4]`, contract §11.5): index
//! `1` and `2` hold `sine_offset`'s running `(s, c)`; index `3` is `0.0` until that oscillator has
//! been seeded once, then `1.0` (distinguishing "never seeded" from the legitimate, reachable
//! `(s, c) == (0.0, 0.0)` — impossible on the unit circle, but not worth relying on when a spare
//! slot is available). Index `0` is unused by this work package, reserved for WP5.2+ or a future
//! `BulletBehavior`.
//!
//! **Open point for the Product Owner (WP5.1 report):** `rotate` and `curve` are mechanically
//! identical here (both apply a constant per-tick angular delta to `velocity`, from `rate`/`turn`
//! respectively) because `docs/formats/sigil.md` §10.4 gives them the same unit (rad/tick) and no
//! further distinguishing description. A plausible alternative reading — `rotate` spins an
//! emitter's *volley-to-volley* base angle (already implemented in `crate::blocks::spiral` for
//! exactly that purpose) while `curve` bends one bullet's own in-flight path — is not implemented
//! because nothing in the wire format ties a `rotate` modifier to the emitter rather than to the
//! bullet. Recommendation: keep the current behaviour unless product testing shows the two need
//! to feel different, in which case add a compiler-side distinction rather than an interpreter
//! guess.

use grimoire_core::Vec2;
use grimoire_core::math::dmath;
use grimoire_sim::ContentManifestHash;

use crate::content::SigilLibrary;
use crate::pool::{PendingDespawn, PoolUpdateBlock};
use crate::unit::{CurveRecord, ModifierDef, ModifierKind, ProgramRecord, SigilUnit};

/// Resolves a [`crate::BulletPool`] column's `program` value into the [`ProgramRecord`] it
/// addresses, undoing the one-based shift `sigil.emit` applies (`crate::systems::EmitSystem`,
/// `BulletPool::spawn`'s doc comment): `0` is "no program", `p >= 1` addresses
/// `unit.programs()[p - 1]`. Returns the record plus its zero-based index, the key
/// [`RuntimeCache::program`] is looked up by.
pub(crate) fn resolve_program(unit: &SigilUnit, program: u16) -> Option<(&ProgramRecord, u16)> {
    if program == 0 {
        return None;
    }
    let index = program - 1;
    unit.programs()
        .get(index as usize)
        .map(|record| (record, index))
}

/// Precomputed per-tick rotation constants for one [`ModifierDef`] (`rotate`/`curve`'s angular
/// delta, `sine_offset`'s oscillator step); the identity rotation for every other modifier kind,
/// which is never read for them.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CompiledModifier {
    cos_delta: f32,
    sin_delta: f32,
}

impl CompiledModifier {
    const IDENTITY: Self = Self {
        cos_delta: 1.0,
        sin_delta: 0.0,
    };
}

/// Precomputed constants for one [`ProgramRecord`]'s modifier stack, parallel to
/// `ProgramRecord::modifiers`.
#[derive(Debug, Clone, Default)]
struct CompiledProgram {
    modifiers: Vec<CompiledModifier>,
}

/// Per-unit table of [`CompiledProgram`]s, rebuilt only when the loaded content's manifest hash
/// changes (`install`/`replace_unit`, contract §11.8) — in WP5.1's scope (no hot-swap yet) that is
/// exactly once, on the first `sigil.update` after `install`. Not simulation state: purely a
/// derived, deterministic function of the loaded [`SigilLibrary`], recomputed identically by any
/// two simulations that load the same content, so it is never hashed or snapshotted (contract
/// §11.5 already treats the `BehaviorRegistry` the same way, for the same reason).
#[derive(Debug, Default)]
pub(crate) struct RuntimeCache {
    manifest_hash: Option<ContentManifestHash>,
    programs: Vec<Vec<CompiledProgram>>,
}

impl RuntimeCache {
    /// Rebuilds the cache if `library`'s manifest hash differs from what was last compiled.
    pub(crate) fn ensure(&mut self, library: &SigilLibrary) {
        let hash = library.epoch().manifest_hash;
        if self.manifest_hash == Some(hash) {
            return;
        }
        self.programs = library
            .units()
            .iter()
            .map(|unit| unit.programs().iter().map(compile_program).collect())
            .collect();
        self.manifest_hash = Some(hash);
    }

    fn program(&self, unit_index: u16, program_index: u16) -> Option<&CompiledProgram> {
        self.programs
            .get(unit_index as usize)?
            .get(program_index as usize)
    }
}

fn compile_program(program: &ProgramRecord) -> CompiledProgram {
    let modifiers = program.modifiers.iter().map(compile_modifier).collect();
    CompiledProgram { modifiers }
}

fn compile_modifier(modifier: &ModifierDef) -> CompiledModifier {
    match modifier.kind {
        ModifierKind::ROTATE | ModifierKind::CURVE => {
            let delta = modifier.params[0];
            CompiledModifier {
                cos_delta: dmath::cos(delta),
                sin_delta: dmath::sin(delta),
            }
        }
        ModifierKind::SINE_OFFSET => {
            let period = if modifier.params[1].abs() > f32::EPSILON {
                modifier.params[1]
            } else {
                1.0
            };
            let w = dmath::TAU / period;
            CompiledModifier {
                cos_delta: dmath::cos(w),
                sin_delta: dmath::sin(w),
            }
        }
        _ => CompiledModifier::IDENTITY,
    }
}

/// Runs `sigil.update`'s per-slot work for one data-parallel block (contract §11.6/§11.7):
/// `previous_position = position`, `age += 1`, the resolved program's block motion continues at
/// its spawn-computed `velocity` while the modifier stack runs on top, then bounds/lifetime are
/// checked. Dead slots are skipped. Returns every despawn this block decided, in slot order, for
/// `sigil.resolve` to apply.
pub(crate) fn update_block(
    block: &mut PoolUpdateBlock<'_>,
    library: &SigilLibrary,
    cache: &RuntimeCache,
    bounds_min: Vec2,
    bounds_max: Vec2,
) -> Vec<PendingDespawn> {
    let start = block.slots().start;
    let mut despawns = Vec::new();
    for i in 0..block.alive.len() {
        if !block.alive[i] {
            continue;
        }
        block.previous_position[i] = block.position[i];
        block.age[i] = block.age[i].wrapping_add(1);

        let unit_index = block.unit[i];
        let Some(sigil_unit) = library.units().get(unit_index as usize) else {
            // An emitter/pool slot referencing a unit index the currently loaded library no
            // longer has (only reachable after hot-swap, WP5.5, out of scope here): skip the
            // program/bounds work rather than index out of range.
            continue;
        };

        if let Some((program, program_index)) = resolve_program(sigil_unit, block.program[i])
            && let Some(compiled) = cache.program(unit_index, program_index)
        {
            apply_modifiers(
                program,
                compiled,
                sigil_unit,
                &mut block.position[i],
                &mut block.velocity[i],
                &mut block.angle[i],
                &mut block.speed[i],
                block.age[i],
                &mut block.state[i],
            );
        }

        block.position[i] += block.velocity[i];

        debug_assert!(
            block.position[i].x.is_finite()
                && block.position[i].y.is_finite()
                && block.velocity[i].x.is_finite()
                && block.velocity[i].y.is_finite()
                && block.speed[i].is_finite()
                && block.angle[i].is_finite()
                && block.state[i].iter().all(|value| value.is_finite()),
            "non-finite bullet state at slot {} after sigil.update (unit {unit_index}, unit-index \
             {unit_index})",
            start + i,
        );

        let bullet_type = &sigil_unit.bullet_types()[block.bullet_type[i] as usize];
        if bullet_type.lifetime_ticks != 0 && block.age[i] >= bullet_type.lifetime_ticks {
            despawns.push(PendingDespawn {
                index: (start + i) as u32,
                cause: crate::pool::DespawnCause::Lifetime,
            });
            continue;
        }
        let position = block.position[i];
        if position.x < bounds_min.x
            || position.x > bounds_max.x
            || position.y < bounds_min.y
            || position.y > bounds_max.y
        {
            despawns.push(PendingDespawn {
                index: (start + i) as u32,
                cause: crate::pool::DespawnCause::Bounds,
            });
        }
    }
    despawns
}

/// Runs one bullet's modifier stack, in authored order, over its `position`/`velocity`/`angle`/
/// `speed`/`state`.
#[allow(clippy::too_many_arguments)]
fn apply_modifiers(
    program: &ProgramRecord,
    compiled: &CompiledProgram,
    unit: &SigilUnit,
    position: &mut Vec2,
    velocity: &mut Vec2,
    angle: &mut f32,
    speed: &mut f32,
    age: u32,
    state: &mut [f32; 4],
) {
    for (modifier, compiled_modifier) in program.modifiers.iter().zip(&compiled.modifiers) {
        match modifier.kind {
            ModifierKind::ACCELERATE => {
                let rate = modifier.params[0];
                let max_speed = modifier.params[1];
                let old_speed = *speed;
                let new_speed = dmath::max(0.0, dmath::min(old_speed + rate, max_speed));
                rescale(velocity, old_speed, new_speed);
                *speed = new_speed;
            }
            ModifierKind::SPEED_CURVE => {
                let Some(curve) = unit.curves().get(modifier.extra as usize) else {
                    // The decoder already rejects a `speed_curve` whose `extra` has no matching
                    // curve (`UnitError::IndexOutOfRange`, contract §11.1), so this is only
                    // reachable after a hot-swap shrinks the `Curves` section (WP5.5, out of
                    // scope): skip rather than index out of range.
                    continue;
                };
                let now = eval_curve(curve, age);
                let previous = eval_curve(curve, age.saturating_sub(1));
                let ratio = if previous.abs() > f32::EPSILON {
                    now / previous
                } else {
                    1.0
                };
                let old_speed = *speed;
                let new_speed = old_speed * ratio;
                rescale(velocity, old_speed, new_speed);
                *speed = new_speed;
            }
            ModifierKind::ROTATE | ModifierKind::CURVE => {
                *velocity = rotate(
                    *velocity,
                    compiled_modifier.cos_delta,
                    compiled_modifier.sin_delta,
                );
                *angle += modifier.params[0];
            }
            ModifierKind::SINE_OFFSET => {
                let amplitude = modifier.params[0];
                let seeded = state[3] != 0.0;
                let (s0, c0) = if seeded {
                    (state[1], state[2])
                } else {
                    let phase = modifier.params[2];
                    (dmath::sin(phase), dmath::cos(phase))
                };
                let (cos_w, sin_w) = (compiled_modifier.cos_delta, compiled_modifier.sin_delta);
                let s1 = s0 * cos_w + c0 * sin_w;
                let c1 = c0 * cos_w - s0 * sin_w;
                let delta = s1 - s0;
                let perp = velocity.normalize_or_zero().perp();
                *position += perp * (amplitude * delta);
                state[1] = s1;
                state[2] = c1;
                state[3] = 1.0;
            }
            // `mirror` only ever shapes the shot list of one volley at emit time
            // (`crate::blocks::apply_mirror`); it has no ongoing per-tick effect on a bullet
            // that already exists.
            ModifierKind::MIRROR => {}
            _ => {}
        }
    }
}

/// Rescales `velocity` from `old_speed` to `new_speed`, preserving heading. Left unchanged if
/// `old_speed` is too close to zero to recover a heading from (a bullet spawned at speed `0.0`
/// combined with `accelerate`/`speed_curve` cannot pick up a direction from acceleration alone
/// without trigonometry, which the hot path forbids; see the WP5.1 report).
fn rescale(velocity: &mut Vec2, old_speed: f32, new_speed: f32) {
    if old_speed.abs() > f32::EPSILON {
        *velocity *= new_speed / old_speed;
    }
}

/// Rotates `v` by the angle whose cosine/sine are `cos_delta`/`sin_delta` — no `dmath` call.
fn rotate(v: Vec2, cos_delta: f32, sin_delta: f32) -> Vec2 {
    Vec2::new(
        v.x * cos_delta - v.y * sin_delta,
        v.x * sin_delta + v.y * cos_delta,
    )
}

/// Evaluates a `speed_curve` keyframe curve at `age`: piecewise-linear between the two keys
/// straddling `age`, clamped to the first/last key outside the curve's range, `1.0` (neutral) for
/// an empty curve. Never divides by zero, even for two keys sharing one `at_ticks` (their later
/// value wins) or an unsorted key list (not produced by `grimoire_sigilc`, not re-checked by the
/// decoder either, so this stays defensive rather than assuming order).
fn eval_curve(curve: &CurveRecord, age: u32) -> f32 {
    let keys = &curve.keys;
    let Some(&(first_tick, first_value)) = keys.first() else {
        return 1.0;
    };
    if age <= first_tick {
        return first_value;
    }
    for window in keys.windows(2) {
        let (t0, v0) = window[0];
        let (t1, v1) = window[1];
        if age <= t1 {
            if t1 == t0 {
                return v1;
            }
            let fraction = (age - t0) as f32 / (t1 - t0) as f32;
            return v0 + (v1 - v0) * fraction;
        }
    }
    keys.last().map_or(1.0, |&(_, value)| value)
}

#[cfg(test)]
mod tests {
    use grimoire_core::hash_of;

    use super::*;
    use crate::pool::{BulletPool, BulletSpawn, DespawnCause};
    use crate::test_support::{build_content_with_program, build_program_bytes_unit};
    use crate::unit::{BlockDef, BlockKind};

    #[test]
    fn resolve_program_is_one_based_with_zero_meaning_none() {
        let unit = build_program_bytes_unit(&[
            ProgramRecord {
                block: BlockDef {
                    kind: BlockKind::RING,
                    count: 1,
                    params: [0.0; 6],
                    seed_hash: 0,
                },
                modifiers: vec![],
            },
            ProgramRecord {
                block: BlockDef {
                    kind: BlockKind::LINE,
                    count: 1,
                    params: [0.0; 6],
                    seed_hash: 0,
                },
                modifiers: vec![],
            },
        ]);
        assert!(resolve_program(&unit, 0).is_none());
        let (first, first_index) = resolve_program(&unit, 1).expect("program 1 must resolve");
        assert_eq!(first_index, 0);
        assert_eq!(first.block.kind, BlockKind::RING);
        let (second, second_index) = resolve_program(&unit, 2).expect("program 2 must resolve");
        assert_eq!(second_index, 1);
        assert_eq!(second.block.kind, BlockKind::LINE);
        assert!(resolve_program(&unit, 3).is_none());
    }

    #[test]
    fn eval_curve_interpolates_and_clamps() {
        let curve = CurveRecord {
            keys: vec![(0, 1.0), (10, 2.0), (20, 2.0)],
        };
        assert_eq!(eval_curve(&curve, 0), 1.0);
        assert!((eval_curve(&curve, 5) - 1.5).abs() < 1e-6);
        assert_eq!(eval_curve(&curve, 10), 2.0);
        assert_eq!(eval_curve(&curve, 100), 2.0);
        let empty = CurveRecord { keys: vec![] };
        assert_eq!(eval_curve(&empty, 5), 1.0);
    }

    #[test]
    fn accelerate_clamps_at_max_speed_and_rescales_velocity() {
        let modifier = ModifierDef {
            kind: ModifierKind::ACCELERATE,
            flag: 0,
            extra: 0,
            params: [1.0, 3.0, 0.0],
        };
        let program = ProgramRecord {
            block: BlockDef {
                kind: BlockKind::RING,
                count: 1,
                params: [0.0; 6],
                seed_hash: 0,
            },
            modifiers: vec![modifier],
        };
        let compiled = compile_program(&program);
        let unit = build_program_bytes_unit(std::slice::from_ref(&program));
        let mut position = Vec2::ZERO;
        let mut velocity = Vec2::new(2.0, 0.0);
        let mut angle = 0.0;
        let mut speed = 2.0;
        let mut state = [0.0; 4];
        for _ in 0..5 {
            apply_modifiers(
                &program,
                &compiled,
                &unit,
                &mut position,
                &mut velocity,
                &mut angle,
                &mut speed,
                1,
                &mut state,
            );
        }
        assert!((speed - 3.0).abs() < 1e-5, "speed must clamp at max_speed");
        assert!((velocity.x - 3.0).abs() < 1e-4);
        assert!(velocity.y.abs() < 1e-6);
    }

    #[test]
    fn rotate_advances_velocity_and_angle_by_a_constant_delta_each_call() {
        let modifier = ModifierDef {
            kind: ModifierKind::ROTATE,
            flag: 0,
            extra: 0,
            params: [dmath::FRAC_PI_2, 0.0, 0.0],
        };
        let program = ProgramRecord {
            block: BlockDef {
                kind: BlockKind::RING,
                count: 1,
                params: [0.0; 6],
                seed_hash: 0,
            },
            modifiers: vec![modifier],
        };
        let compiled = compile_program(&program);
        let unit = build_program_bytes_unit(std::slice::from_ref(&program));
        let mut position = Vec2::ZERO;
        let mut velocity = Vec2::new(1.0, 0.0);
        let mut angle = 0.0;
        let mut speed = 1.0;
        let mut state = [0.0; 4];
        apply_modifiers(
            &program,
            &compiled,
            &unit,
            &mut position,
            &mut velocity,
            &mut angle,
            &mut speed,
            1,
            &mut state,
        );
        assert!((velocity.x).abs() < 1e-4, "quarter turn: x -> ~0");
        assert!((velocity.y - 1.0).abs() < 1e-4, "quarter turn: y -> ~1");
        assert!((angle - dmath::FRAC_PI_2).abs() < 1e-6);
    }

    #[test]
    fn sine_offset_seeds_once_then_oscillates_without_reseeding() {
        let modifier = ModifierDef {
            kind: ModifierKind::SINE_OFFSET,
            flag: 0,
            extra: 0,
            params: [1.0, 4.0, 0.0],
        };
        let program = ProgramRecord {
            block: BlockDef {
                kind: BlockKind::RING,
                count: 1,
                params: [0.0; 6],
                seed_hash: 0,
            },
            modifiers: vec![modifier],
        };
        let compiled = compile_program(&program);
        let unit = build_program_bytes_unit(std::slice::from_ref(&program));
        let mut position = Vec2::ZERO;
        let mut velocity = Vec2::new(1.0, 0.0);
        let mut angle = 0.0;
        let mut speed = 1.0;
        let mut state = [0.0; 4];
        assert_eq!(state[3], 0.0, "unseeded before the first tick");
        apply_modifiers(
            &program,
            &compiled,
            &unit,
            &mut position,
            &mut velocity,
            &mut angle,
            &mut speed,
            1,
            &mut state,
        );
        assert_eq!(state[3], 1.0, "seeded after the first tick");
        let seeded_state = state;
        apply_modifiers(
            &program,
            &compiled,
            &unit,
            &mut position,
            &mut velocity,
            &mut angle,
            &mut speed,
            2,
            &mut state,
        );
        assert_ne!(state, seeded_state, "the oscillator must keep advancing");
    }

    #[test]
    fn mirror_has_no_per_tick_effect() {
        let modifier = ModifierDef {
            kind: ModifierKind::MIRROR,
            flag: 0,
            extra: 1,
            params: [0.0, 0.0, 0.0],
        };
        let program = ProgramRecord {
            block: BlockDef {
                kind: BlockKind::RING,
                count: 1,
                params: [0.0; 6],
                seed_hash: 0,
            },
            modifiers: vec![modifier],
        };
        let compiled = compile_program(&program);
        let unit = build_program_bytes_unit(std::slice::from_ref(&program));
        let mut position = Vec2::new(1.0, 2.0);
        let mut velocity = Vec2::new(1.0, 0.0);
        let mut angle = 0.3;
        let mut speed = 1.0;
        let mut state = [0.0; 4];
        apply_modifiers(
            &program,
            &compiled,
            &unit,
            &mut position,
            &mut velocity,
            &mut angle,
            &mut speed,
            1,
            &mut state,
        );
        assert_eq!(position, Vec2::new(1.0, 2.0));
        assert_eq!(velocity, Vec2::new(1.0, 0.0));
        assert_eq!(angle, 0.3);
    }

    /// The debug-only guard against NaN entering the pool (contract §11.3: "NaN darf nicht in
    /// den Pool"). Injects a `NaN` directly into a block's velocity — standing in for any future
    /// bug (a behavior, WP5.2's transforms) that might otherwise let one slip through, since no
    /// combination of finite, decoder-legal program parameters can produce one from this module's
    /// own arithmetic (every division here is guarded, every clamp keeps values finite).
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "non-finite bullet state")]
    fn nan_velocity_trips_the_debug_assertion() {
        let unit = build_program_bytes_unit(&[]);
        let content = build_content_with_program(unit);
        let mut pool = BulletPool::with_capacity(1);
        pool.spawn(
            &content,
            BulletSpawn::new(crate::UnitId(1), 0, Vec2::ZERO, 0.0, 1.0),
        )
        .expect("spawn must succeed");
        let mut blocks = pool.update_blocks_mut();
        let block = &mut blocks[0];
        block.velocity[0] = Vec2::new(f32::NAN, 0.0);
        let cache = RuntimeCache::default();
        let _ = update_block(
            block,
            content.library(),
            &cache,
            Vec2::new(-1.0e6, -1.0e6),
            Vec2::new(1.0e6, 1.0e6),
        );
    }

    #[test]
    fn lifetime_expiry_is_reported_as_a_pending_despawn() {
        let unit = build_program_bytes_unit(&[]);
        let content = build_content_with_program(unit);
        let mut pool = BulletPool::with_capacity(1);
        pool.spawn(
            &content,
            BulletSpawn::new(crate::UnitId(1), 0, Vec2::ZERO, 0.0, 1.0),
        )
        .expect("spawn must succeed");
        let mut blocks = pool.update_blocks_mut();
        let cache = RuntimeCache::default();
        let despawns = update_block(
            &mut blocks[0],
            content.library(),
            &cache,
            Vec2::new(-1.0e6, -1.0e6),
            Vec2::new(1.0e6, 1.0e6),
        );
        // `build_content_with_program`'s bullet type has `lifetime_ticks == 1`.
        assert_eq!(despawns.len(), 1);
        assert_eq!(despawns[0].cause, DespawnCause::Lifetime);
        let _ = hash_of(&pool);
    }

    #[test]
    fn leaving_bounds_is_reported_as_a_pending_despawn() {
        let unit = build_program_bytes_unit(&[]);
        let content = build_content_with_program(unit);
        let mut pool = BulletPool::with_capacity(1);
        pool.spawn(
            &content,
            BulletSpawn::new(crate::UnitId(1), 0, Vec2::new(0.0, 0.0), 0.0, 100.0),
        )
        .expect("spawn must succeed");
        let mut blocks = pool.update_blocks_mut();
        let cache = RuntimeCache::default();
        let despawns = update_block(
            &mut blocks[0],
            content.library(),
            &cache,
            Vec2::new(-1.0, -1.0),
            Vec2::new(1.0, 1.0),
        );
        assert!(
            despawns
                .iter()
                .any(|d| d.cause == DespawnCause::Bounds || d.cause == DespawnCause::Lifetime)
        );
    }
}
