# Figure Clip Format — `FNP_CLIP` version 1

Hand-written format document of one skeletal animation clip of a figure, the payload of a pack v1
entry of kind `0x8005`. Decided by engine [ADR-0017](../adr/0017-skelettanimation-abtastung-in-der-praesentation.md)
(option A2), implemented in `grimoire_render`
(`crates/grimoire_render/src/figure_clip.rs`: `decode_clip` and the stateless sampling around it)
and loaded through the facade (`grimoire::adapters::figure_assets::load_clip`). This page describes
what that code does; a divergence between this page and the crate is a bug in one of the two.

The clip is **presentation data**. It never enters the simulation, never enters a state hash,
never enters a replay and never enters a golden master: which action runs and how long its phases
last are content data of the simulation in ticks, and the clip only supplies the pose (ADR-0017).
Re-timing or re-exporting a clip therefore changes nothing a determinism test would notice.

The four other figure payloads (`FNP_MESH`, `FNP_MATERIAL`, `FNP_TEXTURE_RAW`, `FNP_SKELETON`) and
the figure manifest (`FNP_FIGURE`) still have **no** document under `docs/formats/`, although
contract §2 rule 10 requires one per format. ADR-0017 flagged that gap; this page deliberately does
not inherit it, and closing it for the other five is its own piece of work.

## 1. Conventions

All integers are little-endian with fixed widths. All floats are IEEE 754 binary32 (`f32`),
little-endian. Strings are UTF-8 with an explicit length prefix and no terminator. There is no
padding and no alignment.

The payload starts with an 8-byte magic and a `u32` version, as contract §2 rule 10 requires. The
older figure payloads carry only a version and no magic — they predate that rule being applied to
them. A new format does not copy the gap.

| | |
|---|---|
| Pack kind | `0x8005` (`grimoire::adapters::figure_assets::FNP_CLIP`) |
| Pack `kind_version` | `1`, equal to the `version` field inside the payload |
| Pack path | `figures/<figure>/clip/<clip>`, so the `AssetId` follows from the path like every other entry |
| Magic | `FNP_CLIP` (`46 4e 50 5f 43 4c 49 50`) |

A clip belongs to exactly one figure's skeleton. The path convention says which, and the skeleton
fingerprint ([§4](#4-skeleton-fingerprint)) proves it.

## 2. Layout

### 2.1 Header (40 bytes)

| Offset | Field | Type | Rule | Error otherwise |
|---|---|---|---|---|
| 0 | `magic` | `[u8; 8]` | `FNP_CLIP` | `InvalidClipMagic` |
| 8 | `version` | `u32` | `1` | `UnsupportedVersion` |
| 12 | `flags` | `u32` | bit 0 = loops; every other bit `0` | `InvalidClipFlags` |
| 16 | `joint_count` | `u32` | `1` to `MAX_SKIN_JOINTS` (256) | `ZeroCount` / `CountExceedsLimit` |
| 20 | `frame_count` | `u32` | `1` to `MAX_CLIP_FRAMES` (4096) | `ZeroCount` / `CountExceedsLimit` |
| 24 | `frame_rate_hz` | `f32` | finite, `> 0`, `<= 1000` | `InvalidClipFrameRate` |
| 28 | `skeleton_fingerprint` | `u64` | [§4](#4-skeleton-fingerprint) | checked on load, not on decode |
| 36 | `marker_count` | `u32` | at most `MAX_CLIP_MARKERS` (64) | `CountExceedsLimit` |

Reserved flag bits are **rejected**, not ignored, so a later version can define one without an
older decoder silently misreading the payload.

`joint_count` must equal the `joint_count` of the figure's `FNP_SKELETON`; that cross-check happens
where both are known (the facade, [§6](#6-loading)), not in the decoder, which sees one payload.

### 2.2 Tracks

Directly after the header, for each joint `0..joint_count` **in skeleton order**, three tracks in
this order: translation, rotation, scale. Each track starts with one storage byte:

| Storage byte | Meaning | Payload |
|---|---|---|
| `0` | constant | exactly one value |
| `1` | sampled | exactly `frame_count` values, frame 0 first |
| anything else | — | `InvalidClipTrackKind` |

| Track | Value | Bytes per value |
|---|---|---|
| translation | `f32[3]` `x, y, z` | 12 |
| rotation | `f32[4]` `x, y, z, w` (quaternion, scalar **last**) | 16 |
| scale | `f32[3]` `x, y, z` | 12 |

There is exactly **one** way to read a clip: no key reduction, no quantisation, no interleaving
variant, no optional block. ADR-0017 insists on that explicitly, from the lesson of engine PR #23,
where a two-reading spec produced two self-consistent halves that could not read each other.

Storing constant tracks once is the whole compression of this format, and it is a large one for the
measured content: of 93 tracks per witch clip, 5 to 11 vary.

**Values are checked at decode time:**

- Every float must be finite. `NonFiniteClipValue` otherwise, so no NaN or infinity can reach a
  joint palette.
- Every rotation key must satisfy `| |q| - 1 | <= 1e-3`. `InvalidClipQuaternion` otherwise. The key
  is **never silently renormalised** — a rig exported with broken rotations is a converter bug, and
  a decoder that quietly fixed it would hide it. (The measured clips deviate by at most `1.4e-7`.)
- A sign flip between neighbouring rotation keys is **valid data**, not an error: `q` and `-q` are
  the same rotation, and the authored witch clips contain 22 such flips across 7 clips, almost all
  at the thighs. [§5](#5-sampling) says what the sampler does with them.

Before a sampled track is allocated, its declared size (`frame_count x value size`) is checked
against the bytes actually remaining (contract §2 rule 9), so a header claiming 4096 frames in a
20-byte payload fails with `UnexpectedEnd` and allocates nothing.

### 2.3 Markers

After the tracks, `marker_count` records:

| Field | Type | Rule | Error otherwise |
|---|---|---|---|
| `frame` | `u32` | `< frame_count`, and not lower than the previous marker's | `IndexOutOfRange` / `ClipMarkersOutOfOrder` |
| `name_len` | `u8` | at most 63 | `CountExceedsLimit` |
| `name` | `name_len` bytes | valid UTF-8 | `InvalidClipMarkerName` |

Frame indices are **zero-based**. The authoring report counts Blender frames from 1; the converter
subtracts one and rejects a marker outside the clip, so the payload never carries the authoring
base (ADR-0017).

Markers are sorted by frame, not strictly: two markers may share a frame.

The engine never interprets a marker name. `ClipData::marker_time` looks one up by name and returns
its time in seconds; what the name *means* belongs to the caller and to the converter's own check of
clip markers against the simulation's tick anchors.

### 2.4 End of payload

Nothing follows the markers. Trailing bytes are `TrailingBytes`, never ignored.

The whole payload is

```text
40 + sum over joints of (1 + t) + (1 + r) + (1 + s) + sum over markers of (5 + name_len)
```

bytes, where `t`/`s` are `12` or `12 x frame_count` and `r` is `16` or `16 x frame_count`,
depending on each track's storage byte.

## 3. Time base

Frame `k` is at `t = k / frame_rate_hz`, with frame 0 at `t = 0`. The converter removes the
one-frame offset the exported GLBs carry (their first key sits at `1/24 s`).

`duration_seconds()` is `(frame_count - 1) / frame_rate_hz`, the time of the last stored frame.

- A **looping** clip repeats its first frame as its last, so one loop is exactly
  `duration_seconds()` long and sampling at `duration_seconds()` gives the same pose as sampling at
  `0`. Times outside `[0, duration)` wrap (`rem_euclid`), in both directions.
- A **single** clip clamps at both ends instead, so its last stored frame is reachable.

The loop convention is what makes the authored clips land on whole ticks: at 24 Hz the witch's
`idle` (49 frames), `walk` (17) and `run` (13) loop in 120, 40 and 30 ticks of the 60 Hz
simulation, which an integer modulo maps without drift over an 18-minute run.

The clip is **never resampled** to the tick rate. The authoring rate stays 24 Hz and is stored, so
re-exporting at another rate needs no format change and no re-timing of anything (ADR-0017).

## 4. Skeleton fingerprint

`skeleton_fingerprint` is `StableHasher` v1 (`grimoire_core::hash`, algorithm version 1) over,
in this order:

1. `write_u32(joint_count)`
2. for each joint in skeleton order: `write_i32(parent)`, with `-1` for the root — the same value
   the `FNP_SKELETON` payload stores.

Nothing else: not the bind matrices, not the rest pose, not the joint names. A clip stays valid for
a skeleton whose *hierarchy* it matches, so re-exporting a figure with nudged bind matrices does not
invalidate its clips, while a clip authored for a different rig with the same number of joints — the
one case a joint count alone cannot catch — is rejected.

For the two-joint rig of the golden fixture (`joint_count = 2`, parents `-1` and `0`) the
fingerprint is `0x9a6c8135349d1276`.

## 5. Sampling

`grimoire_render::figure_clip` samples a clip into one `JointPose` per joint. Every function is a
pure mapping from (clip, time) to a pose; nothing holds a playhead, a clip selection or a blend
state, so the pose stays correct after a rewind, after a snapshot restore and while scrubbing a
replay (ADR-0017 option A2). `ClipSampler` owns buffers only.

- **Between frames:** translation and scale interpolate linearly, rotation with **nlerp on the
  shorter path** — when two neighbouring keys have a negative dot product, the second is negated
  first. Without that rule the 22 measured sign flips make the leg swing the long way round once
  per loop.
- **On a frame:** the stored keys are returned **bit for bit**. No interpolation runs and no
  renormalisation runs, so `q` comes back exactly as stored, hemisphere included.
- **Crossfade:** `weight` 0 returns the first pose unchanged and 1 the second, both bit for bit;
  in between, translation and scale interpolate linearly and rotation with the same shorter-path
  nlerp. A weight outside `[0, 1]` is clamped and a non-finite one is treated as 0.
- **Time outside the clip, or not finite:** wrapped, clamped or treated as 0 (see [§3](#3-time-base)).
  Sampling never fails and never panics.
- **Marker time-stretching:** `warped_clip_time(anchors, target_time)` maps a presentation time back
  to a clip time, piecewise-linearly through `TimeAnchor`s that pair a clip time (usually a marker's)
  with the time it must land on. Anchors must be at least two and strictly increasing in both
  fields, so the mapping is continuous and strictly increasing; outside the anchor range the nearest
  segment's rate continues. Every anchor's `target_time` maps back to exactly its `clip_time`.
  **No rate limit is applied here**: the bound on how far a segment may be stretched belongs in the
  converter's offline check of anchors against markers, where a mismatch is reported, not at
  runtime, where it would be silently clamped. ADR-0017 leaves that bound open (proposal: a tempo
  factor of 0.5 to 2.0 per segment).

Why nlerp and not slerp: at the largest rotation step measured in the authored clips (60°) nlerp
deviates by at most 0.27°, and at 30° by 0.03°. nlerp needs only `+`, `-`, `*`, `/` and `sqrt`,
all exactly specified by IEEE 754, so a sampled pose is bit-identical on Windows, Linux and macOS —
which is what the frozen pose hashes in `figure_clip`'s tests check, and what keeps this code
honest even though it sits outside the determinism set (contract §3) and carries no `clippy.toml`.

## 6. Loading

`grimoire::adapters::figure_assets::load_clip(store, figure, clip, skeleton)` reads
`figures/<figure>/clip/<clip>` of kind `FNP_CLIP` through an `AssetStore`, decodes it, and checks it
against `skeleton`: joint count first, then fingerprint. The kind is checked by the store before any
byte reaches the decoder, so asking for a clip at a skeleton's path fails as a kind mismatch rather
than as a decode error.

Nothing is registered with the renderer: a clip is CPU-side data that becomes a pose in
`extract_stage`, never a GPU resource. GPU-baked animation is deferred (ADR-0017).

## 7. Limits

| Constant | Value | Applies to |
|---|---|---|
| `MAX_SKIN_JOINTS` | 256 | `joint_count` (shared with the skinning palette, contract §6) |
| `MAX_CLIP_FRAMES` | 4096 | `frame_count` — 170 s at 24 Hz, and about 40 MiB even at 256 joints with every track sampled, well under pack v1's `MAX_ENTRY_LEN` of 256 MiB |
| `MAX_CLIP_MARKERS` | 64 | `marker_count` (the busiest measured clip has 3) |
| `MAX_CLIP_MARKER_NAME_LEN` | 63 | one marker name, in bytes — the same limit as a joint name |
| `MAX_CLIP_FRAME_RATE_HZ` | 1000 | `frame_rate_hz` |
| `CLIP_QUATERNION_TOLERANCE` | `1e-3` | `| |q| - 1 |` of a rotation key |

## 8. Decoding without panics

Every malformed input yields a `FigureFormatError`, never a panic (contract §2 rule 9), and nothing
is allocated from a declared size before that size has been checked against the remaining input and
against the limit above:

| Condition | Error |
|---|---|
| Input ends inside a field or a declared block | `UnexpectedEnd` |
| Magic is not `FNP_CLIP` | `InvalidClipMagic` |
| Version other than 1 | `UnsupportedVersion` |
| A reserved flag bit is set | `InvalidClipFlags` |
| `joint_count` or `frame_count` is 0 | `ZeroCount` |
| `joint_count`, `frame_count`, `marker_count` or a name length over its limit | `CountExceedsLimit` |
| Frame rate not finite, not positive, or over 1000 | `InvalidClipFrameRate` |
| A track's storage byte is neither 0 nor 1 | `InvalidClipTrackKind` |
| A key is NaN or infinite | `NonFiniteClipValue` |
| A rotation key's norm is outside the tolerance | `InvalidClipQuaternion` |
| A marker's frame is `>= frame_count` | `IndexOutOfRange` |
| A marker's frame is lower than its predecessor's | `ClipMarkersOutOfOrder` |
| A marker name is not UTF-8 | `InvalidClipMarkerName` |
| Bytes remain after the last marker | `TrailingBytes` |
| The clip does not match the skeleton it is loaded against | `ClipJointCountMismatch` / `ClipSkeletonMismatch` |

Property tests cover arbitrary bytes, arbitrary bytes behind a valid magic and version, truncations
and single-byte mutations of the golden fixture, for the decoder and for the whole pack path.

## 9. Golden fixture

`crates/grimoire_render/tests/fixtures/figure_clip_v1.bin` is the byte-wise golden fixture (contract
§2 rule 10). `figure_clip_v1.hex` next to it is its hand-written derivation: every field written out
from this page, with the fingerprint from an independent re-implementation of `StableHasher` v1 and
the float bit patterns written out by hand. `tests/figure_clip_fixture.rs` checks that the two files
agree byte for byte and that the decoder reads exactly the fields the derivation names — so this
page, the fixture and the code are checked against each other rather than against one run of the
decoder.

The fixture is 190 bytes: 2 joints, 3 frames at 24 Hz, looping, one marker, one constant and one
sampled track of each width, and a sign-flipped rotation key.

The hand-built fixtures in `figure_clip`'s own test module and in
`crates/grimoire/tests/figure_clip_pack.rs` are derived the same way, from this page; the product
owner's authored figures live outside both repositories and are deliberately not test data.

## 10. Not in version 1

Deferred by ADR-0017, each of them a later version of this same kind (stage A as long as version 1
stays readable): quantisation ("smallest three" rotations), curve keys instead of dense samples,
root-motion extraction, additive clips, and per-limb masks.
