//! The engine side of the debug link in the facade (contract §9.7, §13; plan 0002 WP8.4), behind
//! the feature `debug-link`.
//!
//! [`DebugLink`] owns a [`grimoire_debug::EngineLink`] and does what needs the other crates: it
//! builds the engine identity from [`ENGINE_VERSION`] and [`ENGINE_BUILD`], queues every
//! `SwapSigilUnit` and applies the queue through [`grimoire_sigil::replace_unit`] at a tick
//! boundary, answers each with a `SwapAck`, and turns the frame's profile into `Stats`. The main
//! loop drives it (see [`crate::AppBuilder::debug_link`]); a headless harness that records a replay
//! drives it itself and marks every applied swap in the replay header:
//!
//! ```
//! use grimoire::adapters::debug::link::DebugLink;
//! use grimoire::debug::InProcessTransport;
//! use grimoire::prelude::*;
//! use grimoire::sim::{ContentManifestHash, ReplayHeader};
//!
//! let (engine_side, _tool_side) = InProcessTransport::pair();
//! let mut link = DebugLink::new(Box::new(engine_side), [7; 32]);
//! let mut sim = Simulation::new(1);
//! let mut header = ReplayHeader::for_this_build(ContentManifestHash::EMPTY);
//! for _ in 0..3 {
//!     link.poll();
//!     for report in link.apply_swaps(&mut sim) {
//!         header.record_swap(report.into()).expect("ticks only grow");
//!     }
//!     sim.step(TickInput::default());
//! }
//! assert!(header.is_golden_eligible(), "no tool swapped anything");
//! ```
//!
//! Nothing here reaches the simulation except `replace_unit` at the boundary the contract names:
//! without an applied swap every state hash is the same with and without a link.

use grimoire_assets::{AssetId, AssetPath};
use grimoire_debug::{
    DebugTransport, EngineIdentity, EngineLink, FrameProfile, LinkError, LinkEvent, Message,
    SwapAck, SwapSigilUnit, TcpConfig, TcpServerTransport, TransportError,
};
use grimoire_sigil::{ContentEpoch, SigilContent, SigilUnit, SwapReport, replace_unit};
use grimoire_sim::{ENGINE_BUILD, ENGINE_VERSION, Simulation};

use super::stats_frame;
use crate::plugin::FrameStats;

/// `SwapAck.status` of a swap that was applied (contract §13).
pub const SWAP_APPLIED: u8 = 0;
/// `SwapAck.status` of a swap that was rejected; the state is unchanged (contract §13).
pub const SWAP_REJECTED: u8 = 1;
/// `SwapAck.status` of a swap replaced by a later swap of the same unit before the same tick
/// boundary (contract §13).
pub const SWAP_SUPERSEDED: u8 = 2;

/// The identity this build presents in the handshake (contract §13 "Handshake" steps 7-8):
/// [`ENGINE_VERSION`], [`ENGINE_BUILD`] as 40 hex digits or `unknown` without a known build hash
/// (§8.1), and the token a tool must present.
#[must_use]
pub fn engine_identity(token: [u8; 32]) -> EngineIdentity {
    let build_hash = if ENGINE_BUILD.is_known() {
        ENGINE_BUILD.to_hex()
    } else {
        "unknown".to_owned()
    };
    EngineIdentity::new(ENGINE_VERSION.to_owned(), build_hash, token)
}

/// A `SwapSigilUnit` waiting for the next tick boundary.
#[derive(Debug)]
struct QueuedSwap {
    seq: u32,
    swap: SwapSigilUnit,
}

/// The debug link of one run (contract §9.7): handshake, swap queue, `SwapAck` and `Stats`.
///
/// Transport errors and a full queue only end the link, never the run, and nothing panics on
/// anything a tool sends.
#[derive(Debug)]
pub struct DebugLink {
    link: EngineLink,
    events: Vec<LinkEvent>,
    queue: Vec<QueuedSwap>,
    /// Frames finished since the handshake or the last `Stats`.
    frames_since_stats: u32,
}

impl DebugLink {
    /// A link over `transport` that accepts tools presenting `token` (and this build's engine
    /// version and build hash, [`engine_identity`]).
    #[must_use]
    pub fn new(transport: Box<dyn DebugTransport>, token: [u8; 32]) -> Self {
        Self {
            link: EngineLink::new(transport, engine_identity(token)),
            events: Vec::new(),
            queue: Vec::new(),
            frames_since_stats: 0,
        }
    }

    /// Binds a [`TcpServerTransport`] from [`TcpConfig::from_env`] (`GRIMOIRE_DEBUG_ADDR`,
    /// `GRIMOIRE_DEBUG_TOKEN`) and returns a link over it; `Ok(None)` when the address variable is
    /// not set.
    ///
    /// # Errors
    /// The configuration or bind error of [`TcpConfig::from_env`] and [`TcpServerTransport::bind`].
    pub fn from_env() -> Result<Option<Self>, TransportError> {
        let Some(config) = TcpConfig::from_env()? else {
            return Ok(None);
        };
        let transport = TcpServerTransport::bind(config)?;
        log::info!("debug link listening on {}", transport.local_addr());
        Ok(Some(Self::new(Box::new(transport), config.token)))
    }

    /// Whether a tool has completed the handshake and is connected.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.link.is_connected()
    }

    /// Swaps received and not yet applied.
    #[must_use]
    pub fn pending_swaps(&self) -> usize {
        self.queue.len()
    }

    /// Frame start (contract §9.7): reads the transport, runs the handshake and the dispatch,
    /// logs connections, rejections and tool errors, and queues every `SwapSigilUnit`. Changes no
    /// simulation state.
    pub fn poll(&mut self) {
        self.link.poll(&mut self.events);
        for event in self.events.drain(..) {
            match event {
                LinkEvent::Connected(accepted) => {
                    self.frames_since_stats = 0;
                    log::info!(
                        "debug link: tool connected (engine {}, build {}, stats every {} frames)",
                        accepted.peer_hello.engine_version,
                        accepted.peer_hello.build_hash,
                        accepted.peer_hello.stats_interval_frames
                    );
                    if accepted.build_hash_unknown_warning {
                        log::warn!(
                            "debug link: a build hash is unknown on one side, so tool and engine \
                             may come from different builds"
                        );
                    }
                }
                LinkEvent::Rejected(error) => {
                    log::warn!("debug link: connection rejected: {error}");
                }
                LinkEvent::Message {
                    seq,
                    message: Message::SwapSigilUnit(swap),
                } => {
                    log::debug!(
                        "debug link: swap of {} ({} bytes) queued for the next tick boundary",
                        swap.unit_path,
                        swap.unit_bytes.len()
                    );
                    self.queue.push(QueuedSwap { seq, swap });
                }
                LinkEvent::PeerError(error) => {
                    log::warn!(
                        "debug link: tool reported {:?} for frame {}: {}",
                        error.code,
                        error.in_reply_to,
                        error.message
                    );
                }
                LinkEvent::Disconnected { error: None } => {
                    log::info!("debug link: tool disconnected");
                }
                LinkEvent::Disconnected { error: Some(error) } => {
                    log::warn!("debug link: connection closed: {error}");
                }
                other => log::debug!("debug link: ignored {other:?}"),
            }
        }
    }

    /// Tick boundary (contract §9.7, §13 "Wirkung an Grenzen"): applies the queued swaps before the
    /// next `Simulation::step` and answers each with a `SwapAck`; returns the report of every
    /// applied swap in application order. Does nothing without queued swaps.
    ///
    /// Of several swaps of the same `unit_path` only the last is applied; the earlier ones are
    /// acknowledged as superseded. The rest applies in the order received. A swap whose path is no
    /// valid asset path, whose bytes are no valid unit, whose unit id differs from the path's
    /// asset id or that [`replace_unit`] refuses is acknowledged as rejected with the reason and
    /// changes nothing. Every applied swap is logged; a recording harness marks it in the replay
    /// header with `header.record_swap(report.into())` (§8.1, §11.8).
    pub fn apply_swaps(&mut self, sim: &mut Simulation) -> Vec<SwapReport> {
        if self.queue.is_empty() {
            return Vec::new();
        }
        let queue = std::mem::take(&mut self.queue);
        let mut reports = Vec::new();
        for (index, queued) in queue.iter().enumerate() {
            let superseded = queue[index + 1..]
                .iter()
                .any(|later| later.swap.unit_path == queued.swap.unit_path);
            let mut ack = SwapAck::default();
            ack.in_reply_to = queued.seq;
            if superseded {
                ack.status = SWAP_SUPERSEDED;
                ack.reason = "superseded by a later swap of the same unit".to_owned();
                set_epoch(&mut ack, current_epoch(sim));
                log::info!(
                    "debug link: swap of {} superseded before tick {}",
                    queued.swap.unit_path,
                    sim.tick()
                );
            } else {
                match apply_one(sim, &queued.swap) {
                    Ok((report, unit_hash)) => {
                        ack.status = SWAP_APPLIED;
                        ack.applied_tick = report.effective_tick;
                        ack.unit_hash = unit_hash;
                        set_epoch(&mut ack, Some(report.epoch));
                        log::info!(
                            "debug link: swapped {} at tick {} (epoch {} / {:016x}, {} emitters restarted, {} bullets despawned)",
                            queued.swap.unit_path,
                            report.effective_tick,
                            report.epoch.swaps,
                            report.epoch.manifest_hash.0,
                            report.restarted_emitters,
                            report.despawned_bullets
                        );
                        reports.push(report);
                    }
                    Err((reason, unit_hash)) => {
                        ack.status = SWAP_REJECTED;
                        ack.unit_hash = unit_hash;
                        set_epoch(&mut ack, current_epoch(sim));
                        log::warn!(
                            "debug link: swap of {} rejected: {reason}",
                            queued.swap.unit_path
                        );
                        ack.reason = reason;
                    }
                }
            }
            self.send(&Message::SwapAck(ack));
        }
        reports
    }

    /// Frame end, after `on_frame` and `on_profile` (contract §9.7): sends `Stats` when the
    /// connected tool's `stats_interval_frames` frames have finished since the last, built from
    /// `profile` (empty without a profiler), the frame's [`FrameStats`] and the content epoch.
    pub fn frame_finished(
        &mut self,
        stats: &FrameStats,
        profile: Option<&FrameProfile>,
        content: Option<ContentEpoch>,
    ) {
        let interval = u32::from(self.link.stats_interval_frames());
        if interval == 0 {
            return;
        }
        self.frames_since_stats += 1;
        if self.frames_since_stats < interval {
            return;
        }
        self.frames_since_stats = 0;
        let frame = stats_frame(stats, content);
        let message = match profile {
            Some(profile) => profile.to_stats(&frame),
            None => FrameProfile::default().to_stats(&frame),
        };
        self.send(&Message::Stats(message));
    }

    /// Sends one engine message; contract §9.7: a message over its limits is logged and skipped,
    /// a transport error ends only the link.
    fn send(&mut self, message: &Message) {
        match self.link.send(message) {
            Ok(_) | Err(LinkError::NotConnected) => {}
            Err(LinkError::Protocol(error)) => {
                log::error!("debug link: skipped a {:?} message: {error}", message.id());
            }
            Err(error) => log::warn!("debug link: connection closed: {error}"),
        }
    }
}

/// The loaded content epoch of `sim`, if Sigil content is installed.
#[must_use]
pub fn current_epoch(sim: &Simulation) -> Option<ContentEpoch> {
    sim.world()
        .resource::<SigilContent>()
        .map(SigilContent::epoch)
}

fn set_epoch(ack: &mut SwapAck, epoch: Option<ContentEpoch>) {
    if let Some(epoch) = epoch {
        ack.content_swaps = epoch.swaps;
        ack.content_manifest = epoch.manifest_hash.0;
    }
}

/// Decodes, checks and applies one swap; on failure the reason and the unit hash, if the bytes
/// decoded.
fn apply_one(
    sim: &mut Simulation,
    swap: &SwapSigilUnit,
) -> Result<(SwapReport, u64), (String, u64)> {
    let path = AssetPath::new(&swap.unit_path)
        .map_err(|error| (format!("unit_path is not a valid asset path: {error}"), 0))?;
    let unit = SigilUnit::from_bytes(&swap.unit_bytes)
        .map_err(|error| (format!("unit_bytes are not a valid unit: {error}"), 0))?;
    let unit_hash = unit.content_hash();
    let expected = AssetId::from_path(&path);
    if unit.id().0 != expected.0 {
        return Err((
            format!(
                "the unit's id {:016x} is not the asset id {:016x} of unit_path",
                unit.id().0,
                expected.0
            ),
            unit_hash,
        ));
    }
    replace_unit(sim, unit)
        .map(|report| (report, unit_hash))
        .map_err(|error| (error.to_string(), unit_hash))
}
