# Replay format (`GRIMREPL`)

A replay is the recorded input of a simulation run plus, from version 2 on, a header that names the
engine build and the content the run was recorded with. Engine contract §8.1 is the binding
specification; this page describes the bytes and walks through the checked-in fixtures. The types
live in `grimoire_sim` (`InputLog`, `Replay`, `ReplayHeader`, `SwapRecord`, `BuildHash`,
`ContentManifestHash`); `grimoire_sigil` supplies the content manifest and the swap markers.

- **Version 1** (`InputLog`): seed, tick rate and one frame per tick. `InputLog::from_bytes` reads only
  version 1.
- **Version 2** (`Replay` with `header: Some`): version 1 plus engine version, engine build hash,
  content manifest, swap markers and application metadata. `Replay::from_bytes` reads both versions;
  `Replay::to_bytes` with `header: None` writes exactly the version 1 bytes.

Plan 0002 WP1.2 specified version 2, WP1.3 implemented the codec and WP7.1 added swap recording, the
build hash in the engine workflows and this page.

## 1. Conventions

All integers are little-endian with fixed widths. Texts are UTF-8 with an explicit length prefix and
no terminator. There is no padding and no alignment.

**Frame** (`TickInput`, 48 bytes): `MAX_INPUT_SLOTS = 4` slots, each four `i16` axes followed by one
`u32` button mask (12 bytes per slot). Both versions encode frames identically. Frame `n` is the input
of tick `n`, the step that turns tick `n` into tick `n + 1`.

## 2. Version 1

| Offset | Field | Type | Rule |
|---|---|---|---|
| 0 | magic | 8 bytes | `b"GRIMREPL"` |
| 8 | version | `u32` | `1` |
| 12 | `seed` | `u64` | |
| 20 | `tick_rate_hz` | `u32` | `≠ 0` |
| 24 | `frame_count` | `u64` | followed by exactly `frame_count × 48` bytes of frames |

A version 1 replay is `32 + 48 × frame_count` bytes long.

## 3. Version 2

| Field | Type | Rule |
|---|---|---|
| magic | 8 bytes | `b"GRIMREPL"` |
| version | `u32` | `2` |
| `header_len` | `u32` | bytes from the end of this field to right before `frame_count`; at most the remaining input |
| `seed` | `u64` | |
| `tick_rate_hz` | `u32` | `≠ 0` |
| `engine_version` | `u8` length + bytes | 1 to 64 bytes from `[0-9A-Za-z.+-]` |
| `engine_build` | 20 bytes | git commit (SHA-1); all zero = unknown |
| `content_manifest` | `u64` | content manifest right after installation |
| `swap_count` | `u32` | at most 4096, then that many entries of 16 bytes: `tick: u64`, `content_manifest: u64` |
| `meta_count` | `u16` | at most 32, then that many entries: `key_len: u8`, key, `value_len: u16`, value |
| `frame_count` | `u64` | followed by exactly `frame_count × 48` bytes of frames |

The header (`seed` to the last metadata entry) is `47 + len(engine_version) + 16 × swap_count +
Σ (3 + len(key) + len(value))` bytes; that is `header_len`. The whole replay is
`16 + header_len + 8 + 48 × frame_count` bytes.

There are no optional or unknown header fields in version 2. Any extension is version 3.

## 4. Decoding without panics

Every malformed input yields a `SimError`, never a panic, and nothing is allocated from a declared
size before it has been checked:

| Condition | Error |
|---|---|
| Input ends inside a field | `UnexpectedEnd` |
| Magic is not `GRIMREPL` | `BadMagic` |
| Version other than 1 and 2 (`InputLog::from_bytes`: other than 1) | `UnsupportedVersion` |
| `tick_rate_hz = 0` | `InvalidTickRate` |
| `header_len` larger than the remaining input, or the header does not consume exactly `header_len` bytes | `HeaderLength` |
| `engine_version` longer than 64 bytes, a metadata key longer than 64 or a value longer than 1024 bytes | `FieldTooLong` |
| `swap_count` above 4096 or `meta_count` above 32, or the count times the smallest entry size (16 bytes per swap, 4 bytes per metadata entry) exceeds the remaining input | `TooManyEntries` |
| Empty `engine_version` or key, a byte outside its character set, a value that is not UTF-8 | `InvalidText` |
| Metadata keys not strictly ascending in byte order (also duplicates) | `MetadataKeyOrder` |
| Swap ticks not strictly ascending | `SwapOrder` |
| A swap tick greater than `frame_count` | `SwapOutOfRange` |
| Frame payload not exactly `frame_count × 48` bytes (also trailing bytes, also a `frame_count` whose byte size overflows) | `FrameDataLength` |

`frame_count` has no constant of its own: its upper bound is the input length, as in version 1.

## 5. Canonical form and encoding

Metadata keys are strictly ascending and swap ticks strictly ascending, so every accepted input has
exactly one encoding: `from_bytes(to_bytes(r)) == r` and `to_bytes(from_bytes(b)) == b`.
`Replay::to_bytes` checks the same rules before writing and returns `Err` instead of panicking
(`InvalidTickRate`, `FieldTooLong`, `TooManyEntries`, `InvalidText`, `SwapOrder`, `SwapOutOfRange`).

## 6. Header fields

- **`engine_version`** is `ENGINE_VERSION` (`grimoire_sim`'s `CARGO_PKG_VERSION`) when built with
  `ReplayHeader::for_this_build`. It is information: no reader rejects a replay for its value.
- **`engine_build`** is `ENGINE_BUILD`, read at compile time from `GRIMOIRE_BUILD_HASH` (40 lowercase
  hex digits; any other value is a compile error). The engine's CI, nightly and release workflows set
  it to the built commit (`github.sha`). Local builds and builds from a cargo git checkout, which is
  how a game builds the engine, leave it unset and record `BuildHash::UNKNOWN`. A game names its engine
  in the metadata instead (`app.engine_pin`, below).
- **`content_manifest`** is the content manifest hash of the session right after `install`:
  `SigilContent::epoch().manifest_hash`, computed by `grimoire_sigil` over the behaviour registry
  fingerprint and every loaded unit's id and content hash (contract §11.8). It identifies the
  simulation-relevant content; an asset pack's own content hash does not enter the replay.
- **Swap markers.** Each entry says: from tick `tick` on, the content has manifest
  `content_manifest`. The swap happened at the tick boundary right before step `tick`
  (`SwapReport::effective_tick`); a swap before the first step has tick 0, and `tick ≤ frame_count`.
  Several swaps at one boundary are one entry with the final manifest. Because the manifest after a
  swap back to the original units equals the installed one, only the marker shows that a swap
  happened at all. Unit bytes are not stored.
- **Application metadata** belongs to the application; the engine never reads it. Keys are 1 to 64
  bytes from `[a-z0-9._-]`, values UTF-8 up to 1024 bytes. The prefix `grimoire.` is reserved for the
  engine and unused in P1. Recommended keys: `app.name`, `app.version`, `app.git` (the game's commit)
  and `app.engine_pin` (the engine tag the game builds against).

Header fields never enter `state_hash`, a snapshot or `replay`. The caller decides what they mean,
for example whether a replay recorded with another `content_manifest` may run at all.

## 7. Recording a session

In P1, version 2 replays come from headless runs (a harness); the frame loop of the facade records
no input log.

```rust
let mut header = ReplayHeader::for_this_build(content.epoch().manifest_hash); // right after install
header.app_metadata.insert("app.git".into(), game_commit);
header.app_metadata.insert("app.engine_pin".into(), "v0.4.0".into());
let mut log = InputLog { seed: sim.seed(), tick_rate_hz: 60, frames: Vec::new() };
for tick in 0..ticks {
    if let Some(unit) = swap_due(tick) {
        let report = grimoire_sigil::replace_unit(&mut sim, unit)?;
        header.record_swap(report.into())?; // SwapRecord { effective_tick, manifest after the swap }
    }
    let input = next_input(tick);
    log.frames.push(input);
    sim.step(input);
}
let bytes = Replay { header: Some(header), log }.to_bytes()?;
```

`ReplayHeader::record_swap` appends in tick order and merges a swap at the tick of the last entry
into it; an earlier tick (`SwapOrder`) or a new entry beyond 4096 (`TooManyEntries`) is an error that
changes nothing.

## 8. Reproducing a run and golden masters

To replay, build the simulation with the same setup and `log.seed`, compare its content manifest with
`content_manifest`, and call `grimoire_sim::replay(&mut sim, &log, hash_every)`.

- **Without swap markers** the run reproduces every state hash, on every platform.
- **With swap markers but without the swapped units** it reproduces exactly up to the first swap tick
  and diverges after it.
- **With the units**, replacing them right before each marker's tick reproduces every state hash,
  provided each boundary had a single swap. A merged entry cannot restore the swap count of the
  content epoch, which enters the state hash.

A replay with at least one swap marker is never golden (`is_golden_eligible() == false`), and golden
master tools reject it. Golden masters compare checkpoint hashes, never replay bytes, because
`engine_version` and `engine_build` differ between builds.

`crates/grimoire_sigil/tests/replay_record.rs` records such a session and checks all three cases.

## 9. Fixtures

`crates/grimoire_sim/tests/fixtures/` holds three byte fixtures, also as a reference for readers
outside Rust. `tests/replay.rs` decodes them, re-encodes them byte for byte, and writes all three out
by hand from the tables above without the encoder.

They **pin the format, not the version of the day**: their `engine_version` is the fixed string
`0.4.0` and their build hash is fixed too, never `ENGINE_VERSION` or `ENGINE_BUILD`. Neither a
release nor `GRIMOIRE_BUILD_HASH` changes them. They are regenerated only after a deliberate format
change: `cargo test -p grimoire_sim --lib -- --ignored regenerate_fixtures`.

All three share seed `0x123456789abcdef0` (`f0 de bc 9a 78 56 34 12`), tick rate 60 (`3c 00 00 00`)
and the same frames: frame `n` has slot 0 axes `n, -n, 1, -1` and buttons `n`, slots 1 to 3 zero.
Frame 1, for example, is

```text
01 00 ff ff 01 00 ff ff  01 00 00 00              slot 0: axes 1, -1, 1, -1; buttons 1
00 × 36                                           slots 1 to 3
```

### `replay_v1.bin` (224 bytes)

```text
0x000  47 52 49 4d 52 45 50 4c                    magic "GRIMREPL"
0x008  01 00 00 00                                version 1
0x00c  f0 de bc 9a 78 56 34 12                    seed
0x014  3c 00 00 00                                tick_rate_hz 60
0x018  04 00 00 00 00 00 00 00                    frame_count 4
0x020  4 × 48 bytes                               frames 0 to 3
```

### `replay_v2_minimal.bin` (220 bytes)

```text
0x000  47 52 49 4d 52 45 50 4c                    magic "GRIMREPL"
0x008  02 00 00 00                                version 2
0x00c  34 00 00 00                                header_len 52
0x010  f0 de bc 9a 78 56 34 12                    seed
0x018  3c 00 00 00                                tick_rate_hz 60
0x01c  05 30 2e 34 2e 30                          engine_version "0.4.0"
0x022  00 × 20                                    engine_build unknown
0x036  00 00 00 00 00 00 00 00                    content_manifest EMPTY
0x03e  00 00 00 00                                swap_count 0
0x042  00 00                                      meta_count 0
0x044  03 00 00 00 00 00 00 00                    frame_count 3
0x04c  3 × 48 bytes                               frames 0 to 2
```

### `replay_v2_full.bin` (394 bytes)

```text
0x000  47 52 49 4d 52 45 50 4c                    magic "GRIMREPL"
0x008  02 00 00 00                                version 2
0x00c  82 00 00 00                                header_len 130
0x010  f0 de bc 9a 78 56 34 12                    seed
0x018  3c 00 00 00                                tick_rate_hz 60
0x01c  05 30 2e 34 2e 30                          engine_version "0.4.0"
0x022  ab × 20                                    engine_build abab…ab
0x036  77 66 55 44 33 22 11 00                    content_manifest 0x0011223344556677
0x03e  02 00 00 00                                swap_count 2
0x042  02 00 00 00 00 00 00 00                    swap 0: tick 2
0x04a  aa 00 00 00 00 00 00 00                            content_manifest 0xaa
0x052  04 00 00 00 00 00 00 00                    swap 1: tick 4
0x05a  bb 00 00 00 00 00 00 00                            content_manifest 0xbb
0x062  02 00                                      meta_count 2
0x064  08 61 70 70 2e 6e 61 6d 65                 key "app.name"
0x06d  10 00 67 72 69 6d 6f 69 72 65 2d 68 61 72
       6e 65 73 73                                value "grimoire-harness"
0x07f  0b 61 70 70 2e 76 65 72 73 69 6f 6e        key "app.version"
0x08b  05 00 30 2e 31 2e 31                       value "0.1.1"
0x092  05 00 00 00 00 00 00 00                    frame_count 5
0x09a  5 × 48 bytes                               frames 0 to 4
```

Its header is `47 + 5 + 16 × 2 + (3 + 8 + 16) + (3 + 11 + 5) = 130` bytes; the minimal header is
`47 + 5 = 52` bytes.
