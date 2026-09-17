//! Save, compile, swap — once ([`swap_once`]) or on every save ([`watch`]) — with the round-trip
//! time of each swap (Plan 0002 WP8.5, PRD-0016: save to effect under a second).
//!
//! A save is detected by polling the file's modification time and length every
//! [`WatchOptions::poll_interval`], not through an OS watcher: no new third-party dependency
//! (contract §2 rule 4), and an editor that writes through a temporary file is noticed just as
//! reliably.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use grimoire_debug::{Message, SwapAck};

use crate::client::{LinkClient, LinkClientError};
use crate::compile::{CompileError, CompiledUnit, compile_unit};

/// Default interval between two checks of the watched file.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Default time a swap may take from sending to `SwapAck`.
pub const DEFAULT_SWAP_TIMEOUT: Duration = Duration::from_secs(5);

/// What to compile and push.
#[derive(Debug, Clone)]
pub struct WatchOptions {
    /// Content root the canonical path is relative to (`sigilc build --root`).
    pub root: PathBuf,
    /// The `.sigil` file to compile.
    pub file: PathBuf,
    /// `behaviour = <name>` to `BehaviorId`, as the game registered them (contract §11.5).
    pub behavior_ids: BTreeMap<String, u32>,
    /// Interval between two checks of the file (only [`watch`]).
    pub poll_interval: Duration,
    /// How long a swap may take from sending to its `SwapAck`.
    pub swap_timeout: Duration,
}

impl WatchOptions {
    /// Options for `file` under `root` with the defaults and no behaviour ids.
    #[must_use]
    pub fn new(root: &Path, file: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            file: file.to_path_buf(),
            behavior_ids: BTreeMap::new(),
            poll_interval: DEFAULT_POLL_INTERVAL,
            swap_timeout: DEFAULT_SWAP_TIMEOUT,
        }
    }
}

/// One completed swap: what was compiled, how long each part took and what the engine answered.
#[derive(Debug, Clone, PartialEq)]
pub struct SwapOutcome {
    /// The canonical content path the unit was compiled under.
    pub unit_path: String,
    /// Bytes of the compiled unit.
    pub unit_bytes: usize,
    /// Reading and compiling the source.
    pub compile: Duration,
    /// Sending the swap until its `SwapAck` arrived.
    pub round_trip: Duration,
    /// Compile plus round trip: what a save costs before the pattern changes.
    pub total: Duration,
    /// The engine's answer (contract §13: status, `applied_tick`, epoch, `unit_hash`, reason).
    pub ack: SwapAck,
}

impl SwapOutcome {
    /// Whether the engine applied the swap (`status == 0`, contract §13).
    #[must_use]
    pub fn applied(&self) -> bool {
        self.ack.status == 0
    }

    /// One line for a log or the console: times in milliseconds, the tick the unit took effect at
    /// and the content epoch after it.
    #[must_use]
    pub fn summary(&self) -> String {
        let status = match self.ack.status {
            0 => format!("applied at tick {}", self.ack.applied_tick),
            1 => format!("rejected ({})", self.ack.reason),
            2 => "superseded by a later swap".to_owned(),
            other => format!("status {other} ({})", self.ack.reason),
        };
        format!(
            "{}: {} bytes, compiled in {:.1} ms, {} after {:.1} ms round trip ({:.1} ms total, epoch {} / {:016x})",
            self.unit_path,
            self.unit_bytes,
            self.compile.as_secs_f64() * 1e3,
            status,
            self.round_trip.as_secs_f64() * 1e3,
            self.total.as_secs_f64() * 1e3,
            self.ack.content_swaps,
            self.ack.content_manifest
        )
    }
}

/// Everything that can stop a swap or the watch loop.
///
/// `#[non_exhaustive]`: new failure modes are additive (contract §2 rule 13).
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    /// The file could not be compiled; nothing was sent, and [`watch`] keeps watching.
    #[error(transparent)]
    Compile(#[from] CompileError),
    /// Talking to the engine failed.
    #[error(transparent)]
    Link(#[from] LinkClientError),
    /// Writing the log failed.
    #[error("could not write the log: {0}")]
    Log(String),
}

/// Compiles [`WatchOptions::file`] and pushes it, waiting for the engine's `SwapAck`.
///
/// # Errors
/// [`WatchError::Compile`] for a source with diagnostics (nothing is sent),
/// [`WatchError::Link`] if the engine cannot be reached or does not answer within `timeout`.
pub fn swap_once(
    client: &mut LinkClient,
    options: &WatchOptions,
    timeout: Duration,
) -> Result<SwapOutcome, WatchError> {
    let CompiledUnit {
        unit_path,
        bytes,
        duration: compile,
    } = compile_unit(&options.root, &options.file, &options.behavior_ids)?;
    let unit_bytes = bytes.len();
    let sent = Instant::now();
    let seq = client.swap(&unit_path, bytes)?;
    let ack = client.wait_for_ack(seq, timeout)?;
    let round_trip = sent.elapsed();
    Ok(SwapOutcome {
        unit_path,
        unit_bytes,
        compile,
        round_trip,
        total: compile + round_trip,
        ack,
    })
}

/// Compiles and pushes the file every time it changes on disk, writing one line per swap to `log`,
/// and returns every swap the engine answered, in order.
///
/// Returns when `stop()` says so, when `max_swaps` swaps have been applied or when the engine
/// closes the connection. A source with diagnostics is logged and the loop keeps watching: the
/// next save is another chance. Messages the engine sends besides the acknowledgements (`Stats`,
/// `Log`) are logged as they arrive.
///
/// # Errors
/// [`WatchError::Link`] if the connection fails, [`WatchError::Log`] if `log` cannot be written.
pub fn watch(
    client: &mut LinkClient,
    options: &WatchOptions,
    max_swaps: Option<u64>,
    log: &mut dyn Write,
    stop: &dyn Fn() -> bool,
) -> Result<Vec<SwapOutcome>, WatchError> {
    let mut fingerprint = file_fingerprint(&options.file);
    let mut outcomes = Vec::new();
    let mut swaps = 0;
    writeln!(
        log,
        "watching {} (root {}), every {} ms",
        options.file.display(),
        options.root.display(),
        options.poll_interval.as_millis()
    )
    .map_err(log_error)?;

    while !stop() && max_swaps.is_none_or(|max| swaps < max) {
        if !client.is_connected() {
            writeln!(log, "the engine closed the connection").map_err(log_error)?;
            break;
        }
        let current = file_fingerprint(&options.file);
        if current != fingerprint && current.is_some() {
            fingerprint = current;
            match swap_once(client, options, options.swap_timeout) {
                Ok(outcome) => {
                    writeln!(log, "{}", outcome.summary()).map_err(log_error)?;
                    if outcome.applied() {
                        swaps += 1;
                    }
                    outcomes.push(outcome);
                }
                Err(WatchError::Compile(error)) => {
                    writeln!(log, "not swapped: {error}").map_err(log_error)?;
                    if let CompileError::Diagnostics { diagnostics, .. } = &error {
                        for diagnostic in diagnostics {
                            writeln!(
                                log,
                                "  {}:{}:{} {} {}",
                                diagnostic.file,
                                diagnostic.line,
                                diagnostic.column,
                                diagnostic.code,
                                diagnostic.message
                            )
                            .map_err(log_error)?;
                        }
                    }
                }
                Err(error) => return Err(error),
            }
        }
        log_engine_messages(client, log)?;
        std::thread::sleep(options.poll_interval);
    }
    Ok(outcomes)
}

/// Writes one line per message the engine sent besides the acknowledgements.
pub(crate) fn log_engine_messages(
    client: &mut LinkClient,
    log: &mut dyn Write,
) -> Result<(), WatchError> {
    if let Err(error) = client.poll() {
        writeln!(log, "the connection ended: {error}").map_err(log_error)?;
        return Ok(());
    }
    for message in client.take_messages() {
        writeln!(log, "{}", describe(&message)).map_err(log_error)?;
    }
    Ok(())
}

/// One line for a message the engine sent.
pub(crate) fn describe(message: &Message) -> String {
    match message {
        Message::Stats(stats) => format!(
            "stats: frame {} tick {} ({} ticks), {:.1} fps, frame {:.2} ms, epoch {} / {:016x}{}",
            stats.frame,
            stats.sim_tick,
            stats.ticks_this_frame,
            stats.fps,
            stats.frame_time_ns as f64 / 1e6,
            stats.content_swaps,
            stats.content_manifest,
            scope_summary(stats)
        ),
        Message::Log(log) => format!(
            "engine log (level {}, tick {}) {}: {}",
            log.level, log.tick, log.target, log.text
        ),
        Message::Error(error) => format!(
            "engine error {:?} for frame {}: {}",
            error.code, error.in_reply_to, error.message
        ),
        Message::SwapAck(ack) => format!(
            "late swap acknowledgement for frame {}: status {}",
            ack.in_reply_to, ack.status
        ),
        other => format!("{other:?}"),
    }
}

/// The scopes of a `Stats` message as ` | frame 16.7 ms | sim 2.1 ms`, over budget marked `!`.
fn scope_summary(stats: &grimoire_debug::Stats) -> String {
    let mut out = String::new();
    for scope in &stats.scopes {
        let over = scope.budget_ns != 0 && scope.total_ns > scope.budget_ns;
        out.push_str(&format!(
            " | {}{}{} {:.2} ms",
            if scope.estimate { "~" } else { "" },
            scope.name,
            if over { "!" } else { "" },
            scope.total_ns as f64 / 1e6
        ));
    }
    out
}

/// Modification time and length of a file, `None` while it does not exist or cannot be read (an
/// editor replacing it through a temporary file).
fn file_fingerprint(file: &Path) -> Option<(SystemTime, u64)> {
    let metadata = std::fs::metadata(file).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

fn log_error(error: std::io::Error) -> WatchError {
    WatchError::Log(error.to_string())
}
