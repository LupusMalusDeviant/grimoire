//! # grimoire_link
//!
//! Library and CLI (`grimoire-link`) of the dev link: it connects to a running engine over the
//! debug protocol (`grimoire_debug`, contract §13), reads its `Stats`, and on every save of a
//! `.sigil` file compiles that file with `grimoire_sigilc` and pushes the resulting unit as
//! `SwapSigilUnit` (Plan 0002 WP8.5, PRD-0016 FR-04: save to effect under a second).
//!
//! No window, no game: the tool is the Rust-only proof of hot reload, independent of the C#
//! tooling suite, and it is not a release artefact in P1 (PO decision P-14).
//!
//! ## Modules
//!
//! - [`tcp`]: the client side of the TCP transport (the engine side is
//!   `grimoire_debug::TcpServerTransport`; engine ADR-0008 assigns the client to this crate).
//! - [`client`]: [`client::LinkClient`], the tool side of one connection — handshake, `SwapAck`
//!   and `Stats` — over any `grimoire_debug::DebugTransport`, so tests drive it in process.
//! - [`compile`]: one `.sigil` file to unit bytes, under the canonical content path its `UnitId`
//!   derives from.
//! - [`watch`]: compile and swap once, or on every save, with the round-trip time of each swap.
//! - [`cli`]: argument parsing and the commands `stats`, `swap` and `watch`.
//!
//! ## Hot reload in three steps
//!
//! ```no_run
//! use std::collections::BTreeMap;
//! use std::path::Path;
//! use std::time::Duration;
//!
//! use grimoire_link::client::LinkClient;
//! use grimoire_link::tcp::TcpClientTransport;
//! use grimoire_link::watch::{WatchOptions, swap_once};
//!
//! let transport = TcpClientTransport::connect("127.0.0.1:47474".parse().unwrap())?;
//! let (mut client, engine) = LinkClient::connect(Box::new(transport), [0x11; 32], 0)?;
//! println!("engine {} build {}", engine.engine_version, engine.build_hash);
//!
//! let options = WatchOptions::new(Path::new("content"), Path::new("content/patterns/bolt.sigil"));
//! let outcome = swap_once(&mut client, &options, Duration::from_secs(5))?;
//! println!("applied at tick {}", outcome.ack.applied_tick);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod cli;
pub mod client;
pub mod compile;
pub mod tcp;
pub mod watch;
