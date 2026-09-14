//! Deterministic test scenario shared by the facade integration tests.

// Each test binary compiles this module separately and uses a different subset of it.
#![allow(dead_code)]

use grimoire::prelude::*;

/// Seconds per tick at the default 60 Hz; only a scale factor for the scenario.
pub const DT: f32 = 1.0 / 60.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Mover {
    pub position: Vec2,
    pub velocity: Vec2,
}
impl_stable_hash!(Mover { position, velocity });

/// Resource steered by input slot 0.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Player {
    pub position: Vec2,
    pub buttons: u32,
}
impl_stable_hash!(Player { position, buttons });

/// Movers bouncing inside a circle, re-aimed with per-tick RNG streams, plus an input-driven
/// player resource.
pub struct Scenario {
    pub movers: u32,
}

impl GamePlugin for Scenario {
    fn name(&self) -> &str {
        "scenario"
    }

    fn build(&mut self, sim: &mut Simulation) {
        let mut rng = derive_rng(sim.seed(), 0, 1);
        let world = sim.world_mut();
        world.insert_resource(Player::default());
        for _ in 0..self.movers {
            let position = Vec2::new(rng.range_f32(-40.0, 40.0), rng.range_f32(-40.0, 40.0));
            let velocity =
                Vec2::from_angle(rng.range_f32(0.0, dmath::TAU)) * rng.range_f32(5.0, 30.0);
            world.spawn((Mover { position, velocity },));
        }

        sim.schedule_mut()
            .add_system(system_fn("player", |world| {
                let input = world.resource::<TickInput>().copied().unwrap_or_default();
                let slot = input.slots[0];
                if let Some(player) = world.resource_mut::<Player>() {
                    player.position += Vec2::new(slot.axis(0), slot.axis(1)) * (40.0 * DT);
                    player.buttons = slot.buttons;
                }
            }))
            .add_system(system_fn("movers", |world| {
                let tick = world.resource::<Tick>().map_or(0, |tick| tick.0);
                let seed = world.resource::<SimSeed>().map_or(0, |seed| seed.0);
                let mut rng = derive_rng(seed, tick, 2);
                for mover in world.query_mut::<&mut Mover>() {
                    mover.position += mover.velocity * DT;
                    if mover.position.length_squared() > 50.0 * 50.0 {
                        mover.position = mover.position.normalize_or_zero() * 49.0;
                        mover.velocity = Vec2::from_angle(rng.range_f32(0.0, dmath::TAU))
                            * rng.range_f32(5.0, 30.0);
                    }
                }
            }));
    }
}

/// Bot input: a slowly rotating move direction and a toggling button.
pub fn bot_input(tick: u64) -> TickInput {
    let mut input = TickInput::default();
    let phase = (tick % 240) as i32;
    input.slots[0].axes[0] = i16::try_from((phase - 120) * 273).unwrap_or(0);
    input.slots[0].axes[1] = if tick % 90 < 45 { i16::MAX } else { -i16::MAX };
    input.slots[0].buttons = u32::from(tick % 30 < 10);
    input
}
