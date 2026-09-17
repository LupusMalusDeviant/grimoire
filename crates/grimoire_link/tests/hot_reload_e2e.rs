//! The DoD proof of "hot reload via dev link" (Plan 0002 WP8.5, PRD-0000 §5, PRD-0016 FR-04):
//! a real engine runs its own main loop headlessly with the debug link the facade serves, a
//! `grimoire-link` tool watches a `.sigil` file beside it, and saving that file changes the
//! pattern from the tick the engine acknowledged — nothing in the chain is stubbed.
//!
//! The engine's loop runs on the test thread; the tool runs on its own thread, as a separate
//! process would. Locally the link is an in-process pair; with `GRIMOIRE_SOCKET_TESTS=1`, which
//! only CI sets (contract §13 "Socket-Tests"), the same test runs over a real loopback socket.

mod common;

use std::time::Duration;

use common::{
    Content, EDITED_SOURCE, FRAME, Link, MAX_FRAMES, PatternFrame, Tool, UNIT_PATH, app, link_ends,
    run_with_tool, watch_swaps,
};
use grimoire::sigil::SigilUnit;
use grimoire_debug::socket_tests_enabled;

/// Frame the test saves the edited pattern at; the engine has run a while by then.
const SAVE_AT_FRAME: u64 = 30;
/// A save must reach the engine well inside this; PRD-0016 asks for under a second, which the
/// measurement session judges — this bound only keeps a stuck tool from hanging the suite.
const SWAP_BUDGET: Duration = Duration::from_secs(10);

/// Saves the edited pattern in a running engine and checks that the pattern changes exactly from
/// the tick the engine acknowledged.
fn hot_reload(kind: Link) {
    let content = Content::new(match kind {
        Link::InProcess => "hot-reload-in-process",
        Link::Socket => "hot-reload-socket",
    });
    let initial = content.compile();
    assert_eq!(initial.unit_path, UNIT_PATH);

    let (engine_transport, open_tool) = link_ends(kind);
    let tool = Tool::spawn(open_tool, 0, watch_swaps(content.options(), 1));
    let (engine, frame) = app(initial.bytes.clone());
    let report = run_with_tool(
        engine,
        &tool,
        &content,
        &[(SAVE_AT_FRAME, EDITED_SOURCE)],
        engine_transport,
    );
    let after_reload: PatternFrame = frame.get();
    let tool = tool.join();
    assert!(report.frames < MAX_FRAMES, "the run ended by its exit key");

    // The tool did what a save should do: compile the file and push it, with its own timings.
    let [swap] = tool.swaps.as_slice() else {
        panic!(
            "the tool reported {} swaps:\n{}",
            tool.swaps.len(),
            tool.log
        );
    };
    assert!(swap.applied(), "{}\n{}", swap.summary(), tool.log);
    assert_eq!(swap.unit_path, UNIT_PATH);
    assert_eq!(swap.ack.content_swaps, 1);
    let edited = SigilUnit::from_bytes(&content.compile().bytes).expect("the edited unit decodes");
    assert_eq!(swap.ack.unit_hash, edited.content_hash(), "the pushed unit");
    assert!(swap.total < SWAP_BUDGET, "{}", swap.summary());
    assert!(
        tool.log.contains("applied at tick"),
        "the round trip is logged:\n{}",
        tool.log
    );
    println!("hot reload over {kind:?}: {}", swap.summary());

    // The engine ran the new pattern from the tick it acknowledged, and not a tick earlier.
    let applied = swap.ack.applied_tick;
    assert!(
        applied >= SAVE_AT_FRAME,
        "the swap landed at tick {applied}, before the save"
    );
    assert!(
        applied < report.final_tick,
        "the run ended at tick {} before the swap at {applied} could show",
        report.final_tick
    );
    let (baseline_engine, baseline_frame) = app(initial.bytes.clone());
    let baseline = baseline_engine
        .run_headless_frames(report.frames, FRAME)
        .expect("the same run without a link");
    assert_eq!(report.hashes.len(), baseline.hashes.len());
    for (&(tick, hash), &(base_tick, base_hash)) in report.hashes.iter().zip(&baseline.hashes) {
        assert_eq!(tick, base_tick);
        if tick <= applied {
            assert_eq!(hash, base_hash, "tick {tick} ran before the swap");
        } else {
            assert_ne!(hash, base_hash, "tick {tick} ran with the reloaded pattern");
        }
    }

    // And the new pattern is visibly the edited one: three times the ring, three times as often.
    let before_reload: PatternFrame = baseline_frame.get();
    assert_eq!(after_reload.tick, before_reload.tick);
    assert!(
        after_reload.live > before_reload.live * 2,
        "after the reload {} bullets were live, before it {}",
        after_reload.live,
        before_reload.live
    );
}

#[test]
fn a_save_reloads_the_pattern_at_the_acknowledged_tick_in_process() {
    hot_reload(Link::InProcess);
}

#[test]
fn a_save_reloads_the_pattern_at_the_acknowledged_tick_over_a_socket() {
    if !socket_tests_enabled() {
        eprintln!("skipping: set GRIMOIRE_SOCKET_TESTS=1");
        return;
    }
    hot_reload(Link::Socket);
}

#[test]
fn a_source_with_diagnostics_is_reported_and_nothing_is_swapped() {
    let content = Content::new("hot-reload-broken");
    let initial = content.compile();
    let (engine_transport, open_tool) = link_ends(Link::InProcess);
    let tool = Tool::spawn(open_tool, 0, watch_swaps(content.options(), 1));

    let (engine, _frame) = app(initial.bytes.clone());
    // A broken save first: the tool must refuse it and keep watching, then push the good one.
    let broken = "sigil 1\n\nbullet bolt {\n  radius = zap\n}\n";
    let report = run_with_tool(
        engine,
        &tool,
        &content,
        &[(20, broken), (SAVE_AT_FRAME, EDITED_SOURCE)],
        engine_transport,
    );
    let tool = tool.join();

    assert!(
        tool.log.contains("not swapped"),
        "the broken save is reported:\n{}",
        tool.log
    );
    assert!(
        tool.log.contains("SIG"),
        "with the compiler's diagnostics:\n{}",
        tool.log
    );
    let [swap] = tool.swaps.as_slice() else {
        panic!("only the good save is pushed:\n{}", tool.log);
    };
    assert!(swap.applied());
    assert!(swap.ack.applied_tick >= SAVE_AT_FRAME);
    assert!(report.frames < MAX_FRAMES);
}

#[test]
fn stats_reach_the_tool_while_it_watches() {
    let content = Content::new("hot-reload-stats");
    let initial = content.compile();
    let (engine_transport, open_tool) = link_ends(Link::InProcess);
    // Stats every other frame, and stop after the first swap.
    let tool = Tool::spawn(open_tool, 2, watch_swaps(content.options(), 1));

    let (engine, _frame) = app(initial.bytes.clone());
    run_with_tool(
        engine,
        &tool,
        &content,
        &[(SAVE_AT_FRAME, EDITED_SOURCE)],
        engine_transport,
    );
    let tool = tool.join();

    assert!(
        tool.log.contains("stats: frame"),
        "the tool logged the engine's stats:\n{}",
        tool.log
    );
    assert!(
        tool.log.contains("| sim "),
        "with the profiler's scopes:\n{}",
        tool.log
    );
}
