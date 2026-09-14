//! # grimoire_sim
//!
//! Deterministic simulation core of the Grimoire engine (game ADR-0005):
//!
//! - [`FixedTimestep`]: exact integer accumulator turning frame times into whole ticks.
//! - [`Tick`] and [`SimSeed`]: the only simulation time and the root of all randomness.
//! - [`SimRng`] and [`derive_rng`]: documented PCG32 generator with order-independent streams.
//! - [`InputFrame`], [`TickInput`], [`InputLog`]: quantised per-tick input and its binary log.
//! - [`Simulation`], [`SimSnapshot`], [`replay`]: stepping, state hashes, snapshots and replays.
//!
//! ```
//! use grimoire_ecs::system_fn;
//! use grimoire_sim::{InputLog, Simulation, Tick, TickInput, replay};
//!
//! fn build(seed: u64) -> Simulation {
//!     let mut sim = Simulation::new(seed);
//!     sim.schedule_mut().add_system(system_fn("noop", |world| {
//!         assert!(world.resource::<Tick>().is_some());
//!     }));
//!     sim
//! }
//!
//! let log = InputLog { seed: 7, tick_rate_hz: 60, frames: vec![TickInput::default(); 3] };
//! let mut recorded = build(log.seed);
//! for &input in &log.frames {
//!     recorded.step(input);
//! }
//! let hashes = replay(&mut build(log.seed), &log, 0);
//! assert_eq!(hashes, vec![(3, recorded.state_hash())]);
//! ```

mod error;
mod input;
mod rng;
mod simulation;
mod time;

pub use error::SimError;
pub use input::{InputFrame, InputLog, MAX_INPUT_SLOTS, TickInput};
pub use rng::{SimRng, derive_rng};
pub use simulation::{SimSnapshot, Simulation, replay};
pub use time::{FixedTimestep, SimSeed, StepPlan, Tick};
