//! The engine server of the debug link in the facade (contract §9.3, §9.7, §13; plan 0002 WP8.4),
//! feature `debug-link`.
//!
//! Locally every test runs over `InProcessTransport`, whose bytes pass through the same codec and
//! frame decoder as a socket's; the last test uses a real loopback socket and runs only with
//! `GRIMOIRE_SOCKET_TESTS=1`, which only CI sets. The tool side is driven from the scripts the
//! headless loops call before every frame or tick, so the engine sees each tool action at a known
//! frame or tick boundary.

use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use grimoire::adapters::debug::link::{
    DebugLink, SWAP_APPLIED, SWAP_REJECTED, SWAP_SUPERSEDED, current_epoch, engine_identity,
};
use grimoire::debug::{
    DebugTransport, ErrorCode, FrameDecoder, Hello, InProcessTransport, LogMsg, MAX_FRAME_LEN,
    MAX_HELLO_FRAME_LEN, Message, PROTOCOL_VERSION, PeerRole, Stats, SwapAck, SwapSigilUnit,
    TcpConfig, TcpServerTransport, encode_frame, socket_tests_enabled,
};
use grimoire::platform::{PlatformEvent, RawInputEvent};
use grimoire::prelude::*;
use grimoire::sigil::{
    BehaviorRegistryBuilder, Emitter, SigilConfig, SigilLibrary, SigilUnit, install,
};
use grimoire::sim::{ContentManifestHash, InputLog, Replay, ReplayHeader, SwapRecord};
use grimoire::{HeadlessReport, LoopReport};

const UNIT: &[u8] = include_bytes!("fixtures/bullet_showcase_unit_v1.bin");
/// The canonical content path `sigilc` compiled the fixture under (its unit id derives from it).
const UNIT_PATH: &str = "fixtures/bullet_showcase.sigil";
const TOKEN: [u8; 32] = [0x5a; 32];
const FRAME: Duration = Duration::from_micros(16_667);

/// Installs the showcase unit, optionally with one emitter per pattern of the unit.
struct Showcase {
    emitters: bool,
}

impl GamePlugin for Showcase {
    fn name(&self) -> &str {
        "showcase"
    }

    fn build(&mut self, sim: &mut Simulation) {
        let registry = BehaviorRegistryBuilder::new(1).build();
        let unit = SigilUnit::from_bytes(UNIT).expect("fixture unit decodes");
        let (unit_id, emitters) = (unit.id(), unit.emitter_count());
        let library = SigilLibrary::new(vec![unit], Arc::clone(&registry)).expect("library");
        install(
            sim,
            library,
            registry,
            SigilConfig::new(4096, Vec2::new(-40.0, -40.0), Vec2::new(40.0, 40.0)),
        )
        .expect("install");
        if self.emitters {
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
    }
}

/// The manifest hash of the showcase content right after installation.
fn installed_manifest() -> u64 {
    let registry = BehaviorRegistryBuilder::new(1).build();
    let library = SigilLibrary::new(vec![SigilUnit::from_bytes(UNIT).unwrap()], registry).unwrap();
    library.epoch().manifest_hash.0
}

/// The tool end of an in-process link, shared with the loop's scripts.
struct Tool {
    transport: InProcessTransport,
    seq: u32,
    received: Vec<Message>,
}

type SharedTool = Rc<RefCell<Tool>>;

fn tool_pair() -> (Box<dyn DebugTransport>, SharedTool) {
    let (engine, tool) = InProcessTransport::pair();
    let tool = Tool {
        transport: tool,
        seq: 0,
        received: Vec::new(),
    };
    (Box::new(engine), Rc::new(RefCell::new(tool)))
}

impl Tool {
    fn send(&mut self, message: &Message) -> u32 {
        self.seq += 1;
        let frame = message
            .to_frame(self.seq)
            .expect("tool messages fit their limits");
        self.transport.send(&frame).expect("send");
        self.seq
    }

    fn hello_with(&mut self, engine_version: &str, build_hash: &str, token: [u8; 32], stats: u16) {
        self.send(&Message::Hello(Hello {
            protocol_version: PROTOCOL_VERSION,
            role: PeerRole::Tool,
            engine_version: engine_version.to_owned(),
            build_hash: build_hash.to_owned(),
            token,
            stats_interval_frames: stats,
        }));
    }

    /// A `Hello` this build accepts, asking for `Stats` every `stats` frames.
    fn hello(&mut self, stats: u16) {
        let identity = engine_identity(TOKEN);
        self.hello_with(&identity.engine_version, &identity.build_hash, TOKEN, stats);
    }

    fn swap(&mut self, path: &str, bytes: &[u8]) -> u32 {
        self.send(&Message::SwapSigilUnit(SwapSigilUnit {
            unit_path: path.to_owned(),
            unit_bytes: bytes.to_vec(),
        }))
    }

    /// Everything the engine sent so far, decoded.
    fn receive(&mut self) -> &[Message] {
        let mut frames = Vec::new();
        let _ = self.transport.poll(&mut frames);
        self.received.extend(
            frames
                .iter()
                .map(|frame| Message::from_frame(frame).expect("the engine sends valid messages")),
        );
        &self.received
    }

    fn acks(&mut self) -> Vec<SwapAck> {
        self.receive()
            .iter()
            .filter_map(|message| match message {
                Message::SwapAck(ack) => Some(ack.clone()),
                _ => None,
            })
            .collect()
    }

    fn stats(&mut self) -> Vec<Stats> {
        self.receive()
            .iter()
            .filter_map(|message| match message {
                Message::Stats(stats) => Some(stats.clone()),
                _ => None,
            })
            .collect()
    }

    fn error_codes(&mut self) -> Vec<ErrorCode> {
        self.receive()
            .iter()
            .filter_map(|message| match message {
                Message::Error(error) => Some(error.code),
                _ => None,
            })
            .collect()
    }
}

fn app(emitters: bool) -> AppBuilder {
    App::new(WindowConfig::default())
        .seed(11)
        .hash_every(1)
        .plugin(Showcase { emitters })
}

fn frames_without_link(frames: u64) -> LoopReport {
    app(true).run_headless_frames(frames, FRAME).unwrap()
}

fn ticks_without_link(ticks: u64, emitters: bool) -> HeadlessReport {
    app(emitters).run_headless(ticks, &mut |_| TickInput::default())
}

/// Runs `frames` frames with `engine` as the link; `script(frame, tool)` acts before each frame.
fn frames_with_link(
    frames: u64,
    engine: Box<dyn DebugTransport>,
    tool: &SharedTool,
    mut script: impl FnMut(u64, &mut Tool),
) -> LoopReport {
    let tool = Rc::clone(tool);
    app(true)
        .debug_link(engine)
        .debug_link_token(TOKEN)
        .run_headless_frames_with_events(frames, FRAME, &mut |frame, _events| {
            script(frame, &mut tool.borrow_mut());
        })
        .expect("a debug link never ends the run")
}

/// Runs `ticks` ticks headless with `engine` as the link; `script(tick, tool)` acts in the input
/// callback of each tick, so the engine reads it before the next tick.
fn ticks_with_link(
    ticks: u64,
    emitters: bool,
    engine: Box<dyn DebugTransport>,
    tool: &SharedTool,
    mut script: impl FnMut(u64, &mut Tool),
) -> HeadlessReport {
    let tool = Rc::clone(tool);
    app(emitters)
        .debug_link(engine)
        .debug_link_token(TOKEN)
        .run_headless(ticks, &mut |tick| {
            script(tick, &mut tool.borrow_mut());
            TickInput::default()
        })
}

#[test]
fn a_tool_connects_and_receives_stats_every_requested_frames() {
    let (engine, tool) = tool_pair();
    tool.borrow_mut().hello(2);
    let report = frames_with_link(7, engine, &tool, |_, _| {});
    assert_eq!(report, frames_without_link(7), "the link changes no hash");

    let mut tool = tool.borrow_mut();
    let identity = engine_identity(TOKEN);
    assert!(matches!(
        &tool.receive()[0],
        Message::Hello(hello) if hello.role == PeerRole::Engine
            && hello.engine_version == identity.engine_version
            && hello.build_hash == identity.build_hash
            && hello.token == [0; 32]
    ));
    let stats = tool.stats();
    let frames: Vec<u64> = stats.iter().map(|stats| stats.frame).collect();
    assert_eq!(frames, [1, 3, 5], "every second finished frame");
    let manifest = installed_manifest();
    for stats in &stats {
        assert_eq!((stats.content_swaps, stats.content_manifest), (0, manifest));
        let names: Vec<&str> = stats
            .scopes
            .iter()
            .map(|scope| scope.name.as_str())
            .collect();
        assert!(
            names.contains(&"sim") && names.contains(&"frame"),
            "{names:?}"
        );
    }
}

#[test]
fn a_version_conflict_is_rejected_and_the_run_goes_on() {
    let identity = engine_identity(TOKEN);
    let other_build = "f".repeat(40);
    // Contract §13 step 7 (V-13): only two known, different build hashes are a conflict. CI builds
    // know their hash; a local build is `unknown` and connects with a warning.
    let build_conflict = (identity.build_hash != "unknown").then_some(ErrorCode::VersionMismatch);
    let cases: [(&str, &str, [u8; 32], Option<ErrorCode>); 3] = [
        (
            "0.0.0-other",
            &identity.build_hash,
            TOKEN,
            Some(ErrorCode::VersionMismatch),
        ),
        (
            &identity.engine_version,
            &other_build,
            TOKEN,
            build_conflict,
        ),
        (
            &identity.engine_version,
            &identity.build_hash,
            [0; 32],
            Some(ErrorCode::Unauthorized),
        ),
    ];
    for (engine_version, build_hash, token, expected) in cases {
        let (engine, tool) = tool_pair();
        tool.borrow_mut()
            .hello_with(engine_version, build_hash, token, 1);
        let report = frames_with_link(5, engine, &tool, |_, _| {});
        assert_eq!(report, frames_without_link(5));

        let mut tool = tool.borrow_mut();
        match expected {
            Some(code) => {
                assert_eq!(tool.error_codes(), [code]);
                assert!(!tool.transport.is_connected(), "rejected connections close");
                assert!(tool.stats().is_empty());
            }
            None => {
                assert!(matches!(tool.receive()[0], Message::Hello(_)));
                assert_eq!(tool.stats().len(), 5);
            }
        }
    }
}

#[test]
fn a_connection_ending_in_the_middle_of_a_message_changes_nothing() {
    let (engine, tool) = tool_pair();
    tool.borrow_mut().hello(0);
    let report = frames_with_link(10, engine, &tool, |frame, tool| {
        if frame == 3 {
            let swap = Message::SwapSigilUnit(SwapSigilUnit {
                unit_path: UNIT_PATH.to_owned(),
                unit_bytes: UNIT.to_vec(),
            })
            .to_frame(2)
            .unwrap();
            let mut bytes = Vec::new();
            encode_frame(&swap, &mut bytes).unwrap();
            tool.transport
                .send_bytes(&bytes[..bytes.len() / 2])
                .unwrap();
            tool.transport.disconnect();
        }
    });
    assert_eq!(report, frames_without_link(10), "no swap was applied");
    assert!(tool.borrow_mut().acks().is_empty());
}

#[test]
fn garbage_and_over_length_bytes_crash_neither_side() {
    // After the handshake: a length below the header, a length above any frame, random bytes.
    let after_handshake: [Vec<u8>; 3] = [
        3u32.to_le_bytes().to_vec(),
        (MAX_FRAME_LEN + 1).to_le_bytes().to_vec(),
        (0..64u8).map(|byte| byte.wrapping_mul(97)).collect(),
    ];
    for garbage in after_handshake {
        let (engine, tool) = tool_pair();
        tool.borrow_mut().hello(1);
        let report = frames_with_link(8, engine, &tool, |frame, tool| {
            if frame == 2 {
                tool.transport.send_bytes(&garbage).unwrap();
            }
        });
        assert_eq!(report, frames_without_link(8));
        let mut tool = tool.borrow_mut();
        let _ = tool.receive(); // the tool side decodes what arrived without panicking
        assert!(!tool.transport.is_connected(), "garbage closes the link");
    }

    // Before the handshake: a first frame longer than MAX_HELLO_FRAME_LEN.
    let (engine, tool) = tool_pair();
    let identity = engine_identity(TOKEN);
    let mut oversized = Message::Hello(Hello {
        protocol_version: PROTOCOL_VERSION,
        role: PeerRole::Tool,
        engine_version: identity.engine_version,
        build_hash: identity.build_hash,
        token: TOKEN,
        stats_interval_frames: 0,
    })
    .to_frame(1)
    .unwrap();
    oversized.payload.resize(MAX_HELLO_FRAME_LEN as usize, 0);
    tool.borrow_mut().transport.send(&oversized).unwrap();
    let report = frames_with_link(4, engine, &tool, |_, _| {});
    assert_eq!(report, frames_without_link(4));
    assert_eq!(tool.borrow_mut().error_codes(), [ErrorCode::TooLarge]);
}

#[test]
fn a_swap_takes_effect_at_the_expected_tick_boundary() {
    const SENT_AT: u64 = 20;
    let (engine, tool) = tool_pair();
    tool.borrow_mut().hello(0);
    let report = ticks_with_link(40, true, engine, &tool, |tick, tool| {
        if tick == SENT_AT {
            tool.swap(UNIT_PATH, UNIT);
        }
    });
    let baseline = ticks_without_link(40, true);

    // Sent during tick 20's input, read and applied before the step of tick 21.
    let acks = tool.borrow_mut().acks();
    assert_eq!(acks.len(), 1);
    let ack = &acks[0];
    assert_eq!((ack.status, ack.applied_tick), (SWAP_APPLIED, SENT_AT + 1));
    assert_eq!(ack.content_swaps, 1);
    assert_eq!(
        ack.content_manifest,
        installed_manifest(),
        "same bytes, same manifest"
    );
    assert_eq!(
        ack.unit_hash,
        SigilUnit::from_bytes(UNIT).unwrap().content_hash()
    );
    assert!(ack.reason.is_empty());

    assert_eq!(report.hashes.len(), baseline.hashes.len());
    for (&(tick, hash), &(baseline_tick, baseline_hash)) in
        report.hashes.iter().zip(&baseline.hashes)
    {
        assert_eq!(tick, baseline_tick);
        if tick <= SENT_AT + 1 {
            assert_eq!(hash, baseline_hash, "tick {tick} ran before the swap");
        } else {
            assert_ne!(
                hash, baseline_hash,
                "tick {tick} ran with the swapped content"
            );
        }
    }
}

#[test]
fn the_content_epoch_alone_changes_the_hash() {
    // No emitters and no bullets: a byte-identical swap changes nothing but the epoch's swap count.
    let (engine, tool) = tool_pair();
    tool.borrow_mut().hello(0);
    let report = ticks_with_link(10, false, engine, &tool, |tick, tool| {
        if tick == 4 {
            tool.swap(UNIT_PATH, UNIT);
        }
    });
    let baseline = ticks_without_link(10, false);
    let ack = tool.borrow_mut().acks()[0].clone();
    assert_eq!(
        (ack.status, ack.applied_tick, ack.content_swaps),
        (SWAP_APPLIED, 5, 1)
    );
    assert_eq!(ack.content_manifest, installed_manifest());
    assert_eq!(report.hashes[..5], baseline.hashes[..5]);
    for index in 5..10 {
        assert_ne!(
            report.hashes[index], baseline.hashes[index],
            "the epoch is in the hash"
        );
    }
}

#[test]
fn swaps_before_one_boundary_supersede_reject_and_apply_in_order() {
    let (engine, tool) = tool_pair();
    tool.borrow_mut().hello(0);
    let mut sent = Vec::new();
    let report = ticks_with_link(12, true, engine, &tool, |tick, tool| {
        if tick == 6 {
            sent.extend([
                tool.swap(UNIT_PATH, UNIT),
                tool.swap("fixtures/elsewhere.sigil", UNIT),
                tool.swap("fixtures/garbage.sigil", b"GRIMSIGL not a unit"),
                tool.swap("Not A Path", UNIT),
                tool.swap(UNIT_PATH, UNIT),
            ]);
        }
    });

    let acks = tool.borrow_mut().acks();
    let replies: Vec<(u32, u8)> = acks
        .iter()
        .map(|ack| (ack.in_reply_to, ack.status))
        .collect();
    assert_eq!(
        replies,
        [
            (sent[0], SWAP_SUPERSEDED),
            (sent[1], SWAP_REJECTED),
            (sent[2], SWAP_REJECTED),
            (sent[3], SWAP_REJECTED),
            (sent[4], SWAP_APPLIED),
        ],
        "acknowledged in the order received"
    );
    assert!(acks[1].reason.contains("asset id"), "{}", acks[1].reason);
    assert!(
        acks[2].reason.contains("not a valid unit"),
        "{}",
        acks[2].reason
    );
    assert!(acks[3].reason.contains("asset path"), "{}", acks[3].reason);
    for earlier in &acks[..4] {
        assert_eq!(
            earlier.content_swaps, 0,
            "nothing applied before the last swap"
        );
    }
    assert_eq!((acks[4].applied_tick, acks[4].content_swaps), (7, 1));

    // Exactly one swap took effect: the same state as a run with a single swap at that boundary.
    let (engine, single) = tool_pair();
    single.borrow_mut().hello(0);
    let single_report = ticks_with_link(12, true, engine, &single, |tick, tool| {
        if tick == 6 {
            tool.swap(UNIT_PATH, UNIT);
        }
    });
    assert_eq!(report, single_report);
}

#[test]
fn without_a_swap_every_loop_hash_is_the_same_with_and_without_a_link() {
    let (engine, tool) = tool_pair();
    tool.borrow_mut().hello(1);
    let with_link = frames_with_link(30, engine, &tool, |frame, tool| {
        if frame == 10 {
            // Misaddressed traffic is answered, never applied.
            tool.send(&Message::Log(LogMsg {
                level: 3,
                tick: 0,
                target: "tool".to_owned(),
                text: "wrong direction".to_owned(),
            }));
        }
    });
    assert_eq!(with_link, frames_without_link(30));
    assert_eq!(tool.borrow_mut().error_codes(), [ErrorCode::NotSupported]);
    assert_eq!(tool.borrow_mut().stats().len(), 30);
}

#[test]
fn a_recording_harness_marks_link_swaps_in_the_replay_header() {
    let (engine, tool) = tool_pair();
    tool.borrow_mut().hello(0);
    let mut link = DebugLink::new(engine, TOKEN);
    let mut sim = Simulation::new(5);
    Showcase { emitters: true }.build(&mut sim);
    let installed = current_epoch(&sim).expect("content installed");
    let mut header = ReplayHeader::for_this_build(installed.manifest_hash);
    let mut log = InputLog {
        seed: sim.seed(),
        tick_rate_hz: 60,
        frames: Vec::new(),
    };

    for tick in 0..20 {
        link.poll();
        for report in link.apply_swaps(&mut sim) {
            header.record_swap(report.into()).expect("ticks only grow");
        }
        if tick == 4 || tick == 11 {
            tool.borrow_mut().swap(UNIT_PATH, UNIT);
        }
        log.frames.push(TickInput::default());
        sim.step(TickInput::default());
    }

    let manifest = ContentManifestHash(installed_manifest());
    assert_eq!(
        header.swaps,
        [SwapRecord::new(5, manifest), SwapRecord::new(12, manifest)]
    );
    assert!(
        !header.is_golden_eligible(),
        "a swap session is never golden"
    );
    assert_eq!(current_epoch(&sim).map(|epoch| epoch.swaps), Some(2));
    let replay = Replay {
        header: Some(header),
        log,
    };
    let bytes = replay.to_bytes().expect("the header encodes");
    assert_eq!(Replay::from_bytes(&bytes).expect("and decodes"), replay);
}

fn write_message(stream: &mut TcpStream, message: &Message, seq: u32) {
    let mut bytes = Vec::new();
    encode_frame(&message.to_frame(seq).unwrap(), &mut bytes).unwrap();
    stream.write_all(&bytes).unwrap();
}

/// A blocking raw socket tool: handshake, one swap, then waits for the acknowledgement.
// The deadline only bounds how long this test tool waits for the engine; it never reaches
// simulation state (the determinism lint's concern).
#[allow(clippy::disallowed_methods)]
fn socket_tool(addr: SocketAddrV4, done: &AtomicBool) -> Vec<Message> {
    let mut stream = TcpStream::connect(addr).expect("connect to the engine");
    stream
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    let identity = engine_identity(TOKEN);
    let hello = Message::Hello(Hello {
        protocol_version: PROTOCOL_VERSION,
        role: PeerRole::Tool,
        engine_version: identity.engine_version,
        build_hash: identity.build_hash,
        token: TOKEN,
        stats_interval_frames: 0,
    });
    write_message(&mut stream, &hello, 1);
    let mut decoder = FrameDecoder::new();
    let mut received = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut buf = [0u8; 4096];
    while Instant::now() < deadline
        && !received
            .iter()
            .any(|message| matches!(message, Message::SwapAck(_)))
    {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => decoder.push(&buf[..n]),
            Err(_) => {} // read timeout: poll again
        }
        while let Ok(Some(frame)) = decoder.next_frame() {
            received.push(Message::from_frame(&frame).expect("valid engine message"));
            if received.len() == 1 {
                let swap = Message::SwapSigilUnit(SwapSigilUnit {
                    unit_path: UNIT_PATH.to_owned(),
                    unit_bytes: UNIT.to_vec(),
                });
                write_message(&mut stream, &swap, 2);
            }
        }
    }
    done.store(true, Ordering::SeqCst);
    received
}

#[test]
// The tool runs on its own thread like a separate process would; it is no simulation thread (the
// determinism lint's concern), and the engine's loop stays on the test thread.
#[allow(clippy::disallowed_methods)]
fn a_tool_swaps_a_unit_over_a_real_socket() {
    if !socket_tests_enabled() {
        eprintln!("skipping: set GRIMOIRE_SOCKET_TESTS=1");
        return;
    }
    let server = TcpServerTransport::bind(TcpConfig::new(
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0),
        TOKEN,
    ))
    .expect("bind a loopback port");
    let addr = server.local_addr();
    let done = Arc::new(AtomicBool::new(false));
    let tool_done = Arc::clone(&done);
    let tool = std::thread::spawn(move || socket_tool(addr, &tool_done));

    let report = app(true)
        .debug_link(Box::new(server))
        .debug_link_token(TOKEN)
        .exit_key(KeyCode::Escape)
        .run_headless_frames_with_events(20_000, FRAME, &mut |_, events| {
            if done.load(Ordering::SeqCst) {
                events.push(PlatformEvent::Input(RawInputEvent::Key {
                    code: KeyCode::Escape,
                    pressed: true,
                    repeat: false,
                }));
            } else {
                std::thread::sleep(Duration::from_millis(1));
            }
        })
        .expect("the run ends by the exit key");
    let received = tool.join().expect("the tool thread does not panic");

    assert!(matches!(&received[0], Message::Hello(hello) if hello.role == PeerRole::Engine));
    let ack = received
        .iter()
        .find_map(|message| match message {
            Message::SwapAck(ack) => Some(ack),
            _ => None,
        })
        .expect("the engine acknowledged the swap");
    assert_eq!((ack.status, ack.content_swaps), (SWAP_APPLIED, 1));
    assert!(ack.applied_tick <= report.final_tick);
}
