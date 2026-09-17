# grimoire_link

Library and CLI `grimoire-link`: the dev link between an editor's save and a running engine
(Plan-0002 WP8.5, PRD-0016 FR-04). It connects over the debug protocol (engine contract §13),
reads the engine's `Stats`, and on every save of a `.sigil` file compiles that file with
`grimoire_sigilc` and pushes the unit as `SwapSigilUnit`, logging how long compile and round trip
took.

No window, no GPU, no game: this is the Rust-only proof of hot reload, independent of the C#
tooling suite, and it is not a release artefact in P1 (PO decision P-14). Outside the determinism
set (engine ADR-0008, contract §1, §3): it carries no `clippy.toml`, may depend on any runtime
crate and reads the wall clock for its own timings. Nothing depends on it.

## What lives here

| Module | Purpose |
|---|---|
| `src/tcp.rs` | `TcpClientTransport`: the client side of the debug link's TCP transport, loopback only (the engine side is `grimoire_debug::TcpServerTransport`; engine ADR-0008 assigns the client here). |
| `src/client.rs` | `LinkClient`: one connection's tool side — handshake, sending swaps, waiting for `SwapAck`, queueing `Stats` and `Log`. Written against `grimoire_debug::DebugTransport`, so tests drive it in process. |
| `src/compile.rs` | `compile_unit`: one `.sigil` file to unit bytes under the canonical content path its `UnitId` derives from — the same compiler and the same path rules `sigilc build` uses. |
| `src/watch.rs` | `swap_once` and `watch`: compile and push once, or on every save, with compile time, round-trip time and the engine's answer per swap. |
| `src/cli.rs` | Argument parsing and the three commands; `src/bin/grimoire-link.rs` only forwards `argv`. |

## Commands

```text
grimoire-link [--addr <127.0.0.1:port>] [--token <64 hex digits>] <command>

  stats [--count <n>] [--interval <frames>]
  swap  --root <dir> [--behaviors <file>] <file.sigil>
  watch --root <dir> [--behaviors <file>] [--poll-ms <ms>] [--swaps <n>] [--stats <frames>] <file.sigil>
```

The address and token default to `GRIMOIRE_DEBUG_ADDR` and `GRIMOIRE_DEBUG_TOKEN`, the variables
the engine itself reads (contract §13), so a tool started in the same shell as the engine needs
neither flag. `--root` is the content root the unit path is relative to, exactly as for
`sigilc build --root`; `--behaviors` is the same `{"<name>": <id>}` manifest `sigilc` takes
(contract §11.5).

Exit codes follow `sigilc`: `0` when everything asked for succeeded, `1` when the tool ran but the
work did not succeed (a source with diagnostics, a swap the engine rejected, a closed connection),
`2` when it could not run as asked (usage error, no engine, unreadable manifest). No input ends in
a panic (contract §2 rule 9).

A watch session looks like this:

```text
$ grimoire-link --addr 127.0.0.1:47474 --token $TOKEN watch --root content --stats 60 content/patterns/curtain.sigil
connected to 127.0.0.1:47474: engine 0.4.0 build 4f2c...
watching content/patterns/curtain.sigil (root content), every 100 ms
patterns/curtain.sigil: 322 bytes, compiled in 3.8 ms, applied at tick 612 after 11.2 ms round trip (15.0 ms total, epoch 1 / 65d5942cb2ee3772)
stats: frame 640 tick 640 (1 ticks), 60.0 fps, frame 16.55 ms, epoch 1 / 65d5942cb2ee3772 | sim 1.90 ms | frame 16.40 ms
```

A rejected swap prints the engine's reason (`status 1`), a source with diagnostics prints them and
keeps the session watching: the next save is another chance.

## Tests

| Test | What it proves |
|---|---|
| `tests/hot_reload_e2e.rs` | The DoD proof "hot reload via dev link": a real engine runs the facade's main loop headlessly, this tool watches a `.sigil` file beside it, and a save changes the pattern exactly from the tick the engine acknowledged — locally over an in-process transport, in CI additionally over a real loopback socket (`GRIMOIRE_SOCKET_TESTS=1`, contract §13). Also: a broken save is reported and swaps nothing, and `Stats` reach the tool while it watches. |
| `tests/hot_swap_showcase.rs` | The M3 showcase: the `sigil_curtain` scene rendered offscreen while this tool swaps its pattern. The small run checks the picture really changes; the `#[ignore]`d sequence writes the PNGs the CI job `showcase-gif` turns into the before/after GIF. |
| `src/cli.rs`, `src/tcp.rs` unit tests | Usage errors, token parsing and the loopback-only rule. |

The patterns under `tests/patterns/` are compiled by these tests; the WP4.4 identity gate
(`.github/scripts/build-sigil-units.sh`, group `link`) compiles them on all three operating
systems and compares the bytes.
