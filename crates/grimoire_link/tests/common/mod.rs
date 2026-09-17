//! Shared scaffolding of the WP8.5 tests: a temporary content root with the pattern sources, the
//! engine-side plugin that installs a compiled unit and counts its bullets, and the tool thread
//! that watches the file.

// Each test binary compiles this module separately and uses a different subset of it.
#![allow(dead_code)]
// A tool runs beside the engine like a separate process would, and it measures its own round trip:
// the facade's determinism lints are about simulation code, and nothing here reaches a simulation
// except `replace_unit` at a tick boundary.
#![allow(clippy::disallowed_methods)]

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use grimoire::LoopReport;
use grimoire::platform::{PlatformEvent, RawInputEvent};
use grimoire::prelude::*;
use grimoire::sigil::{
    BehaviorRegistryBuilder, BulletPool, Emitter, SigilConfig, SigilLibrary, SigilUnit, install,
};
use grimoire_debug::{DebugTransport, InProcessTransport, TcpConfig, TcpServerTransport};
use grimoire_link::client::LinkClient;
use grimoire_link::compile::{CompiledUnit, compile_unit};
use grimoire_link::watch::{SwapOutcome, WatchOptions, watch};

/// The pattern the engine starts with.
pub const SOURCE: &str = include_str!("../patterns/bolt.sigil");
/// The edited pattern a save pushes over the link.
pub const EDITED_SOURCE: &str = include_str!("../patterns/bolt_faster.sigil");
/// Path of the watched file inside the content root; the unit id derives from it.
pub const UNIT_PATH: &str = "patterns/bolt.sigil";
/// Token the engine accepts in these tests.
pub const TOKEN: [u8; 32] = [0x7c; 32];
/// One frame at 60 Hz: one tick per frame in every run below.
pub const FRAME: Duration = Duration::from_micros(16_667);

/// A temporary content root holding `patterns/bolt.sigil`.
pub struct Content {
    pub root: PathBuf,
    pub file: PathBuf,
}

impl Content {
    /// Creates (or empties) `<cargo target tmpdir>/<name>` and writes [`SOURCE`] into it.
    pub fn new(name: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
        let _ = std::fs::remove_dir_all(&root);
        let patterns = root.join("patterns");
        std::fs::create_dir_all(&patterns).expect("content root");
        let file = patterns.join("bolt.sigil");
        std::fs::write(&file, SOURCE).expect("write the pattern");
        Self { root, file }
    }

    /// Overwrites the watched file, as saving in an editor does.
    pub fn save(&self, source: &str) {
        std::fs::write(&self.file, source).expect("save the pattern");
    }

    /// Compiles the watched file as the tool would.
    pub fn compile(&self) -> CompiledUnit {
        compile_unit(&self.root, &self.file, &BTreeMap::new()).expect("the pattern compiles")
    }

    /// Watch options for the watched file.
    pub fn options(&self) -> WatchOptions {
        let mut options = WatchOptions::new(&self.root, &self.file);
        options.poll_interval = Duration::from_millis(5);
        options
    }
}

/// What the engine's pattern looked like in the most recent frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PatternFrame {
    /// Live bullets in the pool.
    pub live: u32,
    /// Simulation tick the count was taken at.
    pub tick: u64,
}

/// Installs one compiled unit, spawns one emitter entity per emitter of the unit and reports the
/// live bullet count of every frame.
pub struct Pattern {
    unit: Vec<u8>,
    frame: Rc<Cell<PatternFrame>>,
}

impl Pattern {
    /// The plugin and the handle the test reads the last frame through.
    pub fn new(unit: Vec<u8>) -> (Self, Rc<Cell<PatternFrame>>) {
        let frame = Rc::new(Cell::new(PatternFrame::default()));
        (
            Self {
                unit,
                frame: Rc::clone(&frame),
            },
            frame,
        )
    }
}

impl GamePlugin for Pattern {
    fn name(&self) -> &str {
        "pattern"
    }

    fn build(&mut self, sim: &mut Simulation) {
        let registry = BehaviorRegistryBuilder::new(1).build();
        let unit = SigilUnit::from_bytes(&self.unit).expect("the unit decodes");
        let (unit_id, emitters) = (unit.id(), unit.emitter_count());
        let library = SigilLibrary::new(vec![unit], Arc::clone(&registry)).expect("library");
        install(
            sim,
            library,
            registry,
            SigilConfig::new(4096, Vec2::new(-20.0, -20.0), Vec2::new(20.0, 20.0)),
        )
        .expect("install");
        for emitter in 0..emitters {
            sim.world_mut().spawn((Emitter {
                unit: unit_id,
                emitter,
                origin: Vec2::ZERO,
                rotation: 0.0,
                started_at: 0,
            },));
        }
    }

    /// `extract` runs before `on_frame` in every frame and is the only read of the world here; the
    /// tick of the frame comes from `on_frame` afterwards, so the pair always belongs together.
    fn extract(&mut self, world: &World, _alpha: f32, _frame: &mut RenderFrame) {
        let mut frame = self.frame.get();
        frame.live = world.resource::<BulletPool>().map_or(0, BulletPool::len);
        self.frame.set(frame);
    }

    fn on_frame(&mut self, stats: &FrameStats) {
        let mut frame = self.frame.get();
        frame.tick = stats.sim_tick;
        self.frame.set(frame);
    }
}

/// The engine app of these tests: the pattern of `unit`, one tick per frame, a hash after every
/// tick.
pub fn app(unit: Vec<u8>) -> (AppBuilder, Rc<Cell<PatternFrame>>) {
    let (plugin, frame) = Pattern::new(unit);
    let app = App::new(WindowConfig::default())
        .seed(0x8005_1234_ABCD_0001)
        .hash_every(1)
        .plugin(plugin);
    (app, frame)
}

/// Which transport a test drives the link over.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Link {
    /// An in-process pair: the local default, no socket anywhere.
    InProcess,
    /// A real loopback socket, only with `GRIMOIRE_SOCKET_TESTS=1` (contract §13).
    Socket,
}

/// How a tool opens its end of a link: over a socket it connects, in process it takes the other
/// end of the pair.
pub type ToolOpener = Box<dyn FnOnce() -> Box<dyn DebugTransport> + Send>;

/// Creates both ends of a link of the given kind: the engine's transport for
/// `AppBuilder::debug_link`, and the tool's opener for [`Tool::spawn`].
pub fn link_ends(kind: Link) -> (Box<dyn DebugTransport>, ToolOpener) {
    match kind {
        Link::InProcess => {
            let (engine, tool) = InProcessTransport::pair();
            (
                Box::new(engine),
                Box::new(move || Box::new(tool) as Box<dyn DebugTransport>),
            )
        }
        Link::Socket => {
            let config = TcpConfig::new(
                std::net::SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 0),
                TOKEN,
            );
            let server = TcpServerTransport::bind(config).expect("bind a loopback port");
            let addr = server.local_addr();
            (
                Box::new(server),
                Box::new(move || {
                    Box::new(
                        grimoire_link::tcp::TcpClientTransport::connect(addr)
                            .expect("connect to the engine"),
                    ) as Box<dyn DebugTransport>
                }),
            )
        }
    }
}

/// The result of a tool thread: what it logged and what its swaps did.
pub struct ToolReport {
    pub log: String,
    pub swaps: Vec<SwapOutcome>,
}

/// Runs a tool on its own thread — as a separate process would — while the engine's loop runs on
/// the test thread, and sets `done` when it is finished.
pub struct Tool {
    handle: std::thread::JoinHandle<ToolReport>,
    connected: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

impl Tool {
    /// Connects, runs `work` and reports. `work` gets the client, a log to write to and the
    /// collected outcomes.
    pub fn spawn(
        open_tool: ToolOpener,
        stats_interval_frames: u16,
        work: impl FnOnce(&mut LinkClient, &mut Vec<u8>, &mut Vec<SwapOutcome>) + Send + 'static,
    ) -> Self {
        let done = Arc::new(AtomicBool::new(false));
        let connected = Arc::new(AtomicBool::new(false));
        let thread_done = Arc::clone(&done);
        let thread_connected = Arc::clone(&connected);
        let handle = std::thread::spawn(move || {
            let transport = open_tool();
            let mut log = Vec::new();
            let mut swaps = Vec::new();
            let (mut client, engine) =
                LinkClient::connect(transport, TOKEN, stats_interval_frames).expect("handshake");
            thread_connected.store(true, Ordering::SeqCst);
            use std::io::Write as _;
            let _ = writeln!(
                log,
                "connected: engine {} build {}",
                engine.engine_version, engine.build_hash
            );
            work(&mut client, &mut log, &mut swaps);
            client.disconnect();
            thread_done.store(true, Ordering::SeqCst);
            ToolReport {
                log: String::from_utf8(log).expect("utf-8"),
                swaps,
            }
        });
        Self {
            handle,
            connected,
            done,
        }
    }

    /// Whether the tool has completed the handshake and started its work.
    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    /// Whether the tool has finished.
    pub fn done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    /// Waits for the tool and returns its report.
    pub fn join(self) -> ToolReport {
        self.handle.join().expect("the tool thread does not panic")
    }
}

/// Safety net for [`run_with_tool`]: a run never takes more frames than this, so a tool that never
/// finishes fails the test instead of looping forever.
pub const MAX_FRAMES: u64 = 20_000;
/// Frames the engine keeps running after the tool is done, so the reloaded pattern is visible.
pub const TAIL_FRAMES: u64 = 30;

/// Frames between two saves, so the tool's file poll notices each of them.
pub const SAVE_GAP_FRAMES: u64 = 40;

/// Runs the engine's own main loop headlessly with the link of `app` while `tool` works beside it.
///
/// `saves` names the frame each source is written to the watched file at — what an editor's save
/// does. A save waits until the tool has completed its handshake and is watching (otherwise the
/// tool would take the edited file for its baseline and never see a change) and until
/// [`SAVE_GAP_FRAMES`] frames after the previous one. From the first save on, every frame leaves
/// the tool a moment while the loop keeps polling the link, so a swap lands as soon as the tool
/// pushed it; [`TAIL_FRAMES`] frames after the tool is done the run ends through its exit key.
pub fn run_with_tool(
    app: AppBuilder,
    tool: &Tool,
    content: &Content,
    saves: &[(u64, &str)],
    engine_transport: Box<dyn DebugTransport>,
) -> LoopReport {
    let first_save = saves.iter().map(|(frame, _)| *frame).min().unwrap_or(0);
    let mut pending = saves.to_vec();
    pending.sort_by_key(|(frame, _)| *frame);
    let mut pending = pending.into_iter().peekable();
    let mut last_save = None;
    let mut exit_at = None;
    app.debug_link(engine_transport)
        .debug_link_token(TOKEN)
        .exit_key(KeyCode::Escape)
        .run_headless_frames_with_events(MAX_FRAMES, FRAME, &mut |frame, events| {
            if frame < first_save {
                return;
            }
            if let Some(&(at, source)) = pending.peek() {
                let spaced = last_save.is_none_or(|last: u64| frame >= last + SAVE_GAP_FRAMES);
                if frame >= at && tool.connected() && spaced {
                    content.save(source);
                    last_save = Some(frame);
                    pending.next();
                }
            } else if tool.done() {
                let end = *exit_at.get_or_insert(frame + TAIL_FRAMES);
                if frame >= end {
                    events.push(PlatformEvent::Input(RawInputEvent::Key {
                        code: KeyCode::Escape,
                        pressed: true,
                        repeat: false,
                    }));
                }
                return;
            }
            // The tool connects, compiles and waits for its acknowledgement on its own thread; the
            // loop must keep polling the link meanwhile, so it only pauses a moment per frame.
            std::thread::sleep(Duration::from_millis(1));
        })
        .expect("a debug link never ends the run")
}

/// Blocks until `done()` or the deadline; panics on the deadline so a hanging tool fails the test
/// instead of timing out the whole suite.
pub fn wait_for(what: &str, done: impl Fn() -> bool, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !done() {
        assert!(
            Instant::now() < deadline,
            "{what} did not happen within {timeout:?}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// A `watch` run driven exactly like the CLI's `watch` command: it notices the save itself and
/// stops after `max_swaps` applied swaps, for [`Tool::spawn`].
pub fn watch_swaps(
    options: WatchOptions,
    max_swaps: u64,
) -> impl FnOnce(&mut LinkClient, &mut Vec<u8>, &mut Vec<SwapOutcome>) + Send + 'static {
    move |client, log, swaps| {
        // The loop ends on its own after `max_swaps` swaps; the deadline only keeps a test from
        // hanging if no save ever arrives.
        let deadline = Instant::now() + Duration::from_secs(30);
        let stop = move || Instant::now() >= deadline;
        let outcomes = watch(client, &options, Some(max_swaps), log, &stop).expect("watch");
        swaps.extend(outcomes);
    }
}

/// The path of a content root under the cargo test temp directory.
pub fn temp_root(name: &str) -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join(name)
}
