//! # grimoire_sim
//!
//! Deterministic simulation core of the Grimoire engine (game ADR-0005):
//!
//! - [`FixedTimestep`]: exact integer accumulator turning frame times into whole ticks.
//! - [`Tick`] and [`SimSeed`]: the only simulation time and the root of all randomness.
//! - [`SimRng`], [`derive_rng`] and [`derive_block_rng`]: documented PCG32 generator with
//!   order-independent streams per system and per data-parallel block.
//! - [`InputFrame`], [`TickInput`], [`InputLog`]: quantised per-tick input and its binary log.
//! - [`Simulation`], [`SimSnapshot`], [`replay`]: stepping, state hashes, snapshots and replays.
//! - [`Replay`], [`ReplayHeader`]: the additive version-2 replay format on top of [`InputLog`].
//! - [`stream`]: bit layout of random-stream numbers shared by every engine crate.
//!
//! [`Simulation::step`] runs the schedule stage by stage through the world's executor
//! (engine ADR-0006); every state hash is independent of the executor and its thread count.
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

mod binio;
mod error;
mod input;
mod replay;
mod rng;
mod simulation;
pub mod stream;
mod time;

pub use error::SimError;
pub use input::{InputFrame, InputLog, MAX_INPUT_SLOTS, TickInput};
pub use replay::{
    BuildHash, ContentManifestHash, ENGINE_BUILD, ENGINE_VERSION, MAX_APP_KEY_BYTES,
    MAX_APP_METADATA, MAX_APP_VALUE_BYTES, MAX_ENGINE_VERSION_BYTES, MAX_SWAP_RECORDS, Replay,
    ReplayHeader, SwapRecord,
};
pub use rng::{SimRng, derive_block_rng, derive_rng};
pub use simulation::{SimSnapshot, Simulation, replay};
pub use time::{FixedTimestep, SimSeed, StepPlan, Tick};
