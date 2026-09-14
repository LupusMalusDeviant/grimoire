//! The [`Simulation`] driver, its snapshots and [`replay`].

use grimoire_core::StableHasher;
use grimoire_ecs::{Schedule, World, WorldSnapshot};

use crate::input::{InputLog, TickInput};
use crate::time::{SimSeed, Tick};

/// A deterministic simulation: one [`World`], an ordered [`Schedule`], a seed and a tick counter.
///
/// Same seed, same setup and same sequence of [`TickInput`]s give bit-identical states.
///
/// Systems must keep all state that influences the simulation inside the world (components and
/// resources). State captured in system closures is not part of [`Simulation::state_hash`] or
/// [`Simulation::snapshot`] and breaks restore and replay.
#[derive(Debug)]
pub struct Simulation {
    world: World,
    schedule: Schedule,
    seed: u64,
    tick: u64,
}

impl Simulation {
    /// Creates a simulation with an empty schedule and the resources `Tick(0)`, `SimSeed(seed)`
    /// and `TickInput::default()`, inserted in that order.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        let mut world = World::new();
        world.insert_resource(Tick(0));
        world.insert_resource(SimSeed(seed));
        world.insert_resource(TickInput::default());
        Self {
            world,
            schedule: Schedule::new(),
            seed,
            tick: 0,
        }
    }

    /// Seed of this run.
    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Number of completed steps, which is also the index of the next tick.
    #[must_use]
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// The simulated world.
    #[must_use]
    pub fn world(&self) -> &World {
        &self.world
    }

    /// Mutable access to the world, e.g. to register components and spawn initial entities.
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// The schedule run once per step.
    pub fn schedule_mut(&mut self) -> &mut Schedule {
        &mut self.schedule
    }

    /// Simulates one tick.
    ///
    /// Order: write the `Tick`, `SimSeed` and `input` resources → run the schedule → increment
    /// the tick and write `Tick` again. `Tick` and `SimSeed` are owned by the simulation, so any
    /// change a system makes to them is overwritten.
    pub fn step(&mut self, input: TickInput) {
        self.world.insert_resource(Tick(self.tick));
        self.world.insert_resource(SimSeed(self.seed));
        self.world.insert_resource(input);
        self.schedule.run(&mut self.world);
        self.tick = self.tick.wrapping_add(1);
        self.world.insert_resource(Tick(self.tick));
    }

    /// Hash of the complete simulation state.
    ///
    /// Feeds, into a fresh `StableHasher`: tick (`u64`), seed (`u64`), then
    /// [`World::stable_hash`].
    ///
    /// # Panics
    ///
    /// In debug builds, if any `f32` or `f64` in the hashed state is NaN. NaN is forbidden in
    /// simulation state (engine ADR 0004, rule 4) and hashes identically on every platform, so
    /// the golden and replay gates would otherwise pass silently. Release builds do not check.
    #[must_use]
    pub fn state_hash(&self) -> u64 {
        let mut hasher = StableHasher::new();
        hasher.write_u64(self.tick);
        hasher.write_u64(self.seed);
        self.world.stable_hash(&mut hasher);
        debug_assert!(
            !hasher.saw_nan(),
            "NaN in simulation state at tick {}",
            self.tick
        );
        hasher.finish()
    }

    /// Captures world, tick and seed. The schedule is not part of a snapshot.
    #[must_use]
    pub fn snapshot(&self) -> SimSnapshot {
        SimSnapshot {
            world: self.world.snapshot(),
            tick: self.tick,
            seed: self.seed,
        }
    }

    /// Replaces world, tick and seed with `snapshot`; the schedule is kept.
    ///
    /// Restoring into a simulation built with the same setup and schedule continues the run
    /// bit-identically.
    pub fn restore(&mut self, snapshot: &SimSnapshot) {
        self.world.restore(&snapshot.world);
        self.tick = snapshot.tick;
        self.seed = snapshot.seed;
    }
}

/// Complete state of a [`Simulation`] at one tick; see [`Simulation::snapshot`].
#[derive(Debug, Clone)]
pub struct SimSnapshot {
    world: WorldSnapshot,
    tick: u64,
    seed: u64,
}

impl SimSnapshot {
    /// Tick count at the time of the snapshot.
    #[must_use]
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Seed of the captured run.
    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }
}

/// Steps `sim` once per frame of `log` and returns `(tick, state_hash)` checkpoints.
///
/// A checkpoint is taken after every step that leaves `sim.tick()` a multiple of `hash_every`,
/// and after the last frame unless that tick was just recorded, so the final state is always the
/// last entry. `hash_every == 0` records only the final state; an empty log records the current
/// state.
///
/// `sim` must be in the state the log was recorded from: built with the same setup and schedule
/// and with seed `log.seed` (or restored to the matching snapshot). The seed is not checked here.
pub fn replay(sim: &mut Simulation, log: &InputLog, hash_every: u64) -> Vec<(u64, u64)> {
    let mut hashes = Vec::new();
    for &input in &log.frames {
        sim.step(input);
        if hash_every != 0 && sim.tick().is_multiple_of(hash_every) {
            hashes.push((sim.tick(), sim.state_hash()));
        }
    }
    let final_tick = sim.tick();
    if hashes.last().map(|&(tick, _)| tick) != Some(final_tick) {
        hashes.push((final_tick, sim.state_hash()));
    }
    hashes
}
