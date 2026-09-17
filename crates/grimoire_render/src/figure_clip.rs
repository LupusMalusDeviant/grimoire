//! Skeletal animation clips: the `FNP_CLIP` pack payload and stateless sampling of it
//! (engine ADR-0017, option A2; format document `docs/formats/figure-clip.md`).
//!
//! This module is the presentation half of ADR-0017's split: **the simulation owns action timing
//! in ticks as content data, clips are presentation**. Nothing here keeps a playhead, a clip
//! selection or a blend state; every function is a pure mapping from (clip, time) to a pose, so a
//! caller computing the pose in `extract_stage` from world state plus the interpolation alpha gets
//! the right pose after a rewind, after a snapshot restore and while scrubbing a replay, without
//! any presentation state to restore. [`ClipSampler`] is the one type that owns anything, and what
//! it owns is *buffers only* — no time, no clip, no weight.
//!
//! Like [`crate::figure_format`], this module is **deliberately independent of
//! `grimoire_assets`** (the engine crate map, `docs/architektur/crate-vertraege.md` §1, forbids
//! every edge from `grimoire_render` to it): [`decode_clip`] takes one pack entry's payload bytes
//! and returns a plain value or a [`FigureFormatError`]. Reading that entry out of a pack and
//! checking it against its figure's skeleton is the facade's job
//! (`grimoire::adapters::figure_assets::load_clip`).
//!
//! **Never panics** (contract §2 rule 9): every declared count is checked against a documented
//! upper bound *and* against the remaining input before it sizes an allocation, and the sampling
//! functions clamp or wrap every time value, including non-finite ones.
//!
//! **What this module is not** (ADR-0017 "Umfangsgrenze", PRD-0002 Non-Goals): no playback state
//! machine, no gameplay events, no root-motion extraction, no IK, no retargeting, no additive
//! clips and no per-limb masks. Marker names are never interpreted here; they are anchors a caller
//! looks up by name ([`ClipData::marker_time`]) to build its own [`TimeAnchor`]s.
//!
//! # Determinism
//!
//! Sampling uses only `+`, `-`, `*`, `/`, `%` and `sqrt` on `f32` — all of them exactly specified
//! by IEEE 754, none of them a transcendental function — so a sampled pose is bit-identical on
//! Windows, Linux and macOS. That is deliberate: this code sits outside the determinism set
//! (contract §3) and carries no `clippy.toml`, so nothing would stop a `sin` from creeping in
//! except the frozen pose hashes in this module's tests (ADR-0017, "Konsequenzen/Negativ").
//! Rotations interpolate with nlerp on the shorter path rather than slerp for the same reason:
//! slerp needs `acos` and `sin`, and at the largest rotation step measured in the authored clips
//! (60°) nlerp deviates by at most 0.27°.

use grimoire_core::hash::StableHasher;

use crate::figure_format::{
    Cursor, FigureFormatError, IDENTITY_MATRIX, JointPose, MAX_SKELETON_JOINTS, SkeletonData,
    check_count, mat4_mul, trs_matrix,
};

/// Magic every `FNP_CLIP` payload starts with (contract §2 rule 10: "am Anfang Magic und
/// `u32`-Version").
///
/// The older figure payloads (`FNP_MESH` and friends) carry no magic — they came from a shared
/// spec written before that rule was applied to them. A new format does not inherit that gap; see
/// `docs/formats/figure-clip.md` §1.
pub const CLIP_MAGIC: [u8; 8] = *b"FNP_CLIP";

/// `kind_version` of the `FNP_CLIP` payload this module reads and documents.
pub const CLIP_FORMAT_VERSION: u32 = 1;

/// Upper bound on a clip's `frame_count` (ADR-0017): 4096 frames are 170 s at 24 fps, and even at
/// [`MAX_SKELETON_JOINTS`] joints with every track sampled that is about 40 MiB — well under pack
/// v1's `MAX_ENTRY_LEN` of 256 MiB (`docs/formats/pack.md` §8).
pub const MAX_CLIP_FRAMES: u32 = 4096;

/// Upper bound on a clip's `marker_count` (ADR-0017).
pub const MAX_CLIP_MARKERS: u32 = 64;

/// Upper bound on a marker name's UTF-8 byte length, matching
/// [`crate::figure_format::MAX_JOINT_NAME_LEN`].
pub const MAX_CLIP_MARKER_NAME_LEN: usize = 63;

/// Upper bound on a clip's authoring rate in Hz. A rate outside `(0, MAX_CLIP_FRAME_RATE_HZ]`, or
/// a non-finite one, is rejected rather than turned into an absurd clip length.
pub const MAX_CLIP_FRAME_RATE_HZ: f32 = 1000.0;

/// Tolerance on `|q| - 1` for a stored rotation key, matching the tangent rule of
/// `FNP_MESH` version 2 (ADR-0017: "der Dekoder lehnt ... eine Normabweichung über 10⁻³ ab (nie
/// still korrigiert)"). The measured clips deviate by at most `1.4e-7`.
pub const CLIP_QUATERNION_TOLERANCE: f32 = 1e-3;

/// Flag bit 0 of the clip header: the clip loops, i.e. its last frame repeats its first and the
/// loop is `(frame_count - 1) / frame_rate_hz` seconds long (ADR-0017, "Zeitbasis").
const FLAG_LOOPING: u32 = 1;

/// Track storage byte identifying a track that holds one value for the whole clip.
const TRACK_CONSTANT: u8 = 0;
/// Track storage byte identifying a track that holds one value per frame.
const TRACK_SAMPLED: u8 = 1;

/// The identity rotation, returned by [`nlerp_shortest`] for the degenerate case of two exactly
/// opposite quaternions blended at exactly one half (the interpolated value is then the zero
/// quaternion, which has no direction to normalise).
const IDENTITY_QUATERNION: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// One joint's track of `N`-component values: either one value for the whole clip or one value per
/// frame (ADR-0017, "Ablage": constant tracks are stored once, sampled ones at the authoring rate,
/// with no key reduction, no quantisation and no resampling to the tick rate).
#[derive(Debug, Clone, PartialEq)]
enum Track<const N: usize> {
    Constant([f32; N]),
    Sampled(Vec<[f32; N]>),
}

impl<const N: usize> Track<N> {
    /// The key at `frame`, clamped to the last stored key.
    ///
    /// A [`Track::Sampled`] always holds exactly the clip's `frame_count` keys and `frame_count`
    /// is at least 1 (both checked by [`decode_clip`]), so the slice is never empty and the clamp
    /// can never index out of range.
    #[inline]
    fn key(&self, frame: usize) -> [f32; N] {
        match self {
            Track::Constant(value) => *value,
            Track::Sampled(keys) => keys[frame.min(keys.len() - 1)],
        }
    }
}

/// One joint's three tracks, in the payload's own order.
#[derive(Debug, Clone, PartialEq)]
struct JointTracks {
    translation: Track<3>,
    rotation: Track<4>,
    scale: Track<3>,
}

/// One marker of a [`ClipData`]: a frame index and a name the engine never interprets.
///
/// Markers are anchors for the caller's own time warp ([`TimeAnchor`]) and for the offline
/// converter's own cross-check against the simulation's tick anchors (ADR-0017). Only produced by
/// [`decode_clip`] (contract §2 rule 13, output-only type).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipMarker {
    /// Zero-based frame index this marker sits on, always `< ` the clip's `frame_count`.
    ///
    /// The authoring report counts Blender frames from 1; the converter subtracts one and rejects
    /// a marker outside the clip, so the payload is always zero-based (ADR-0017).
    pub frame: u32,
    /// The marker's name, at most [`MAX_CLIP_MARKER_NAME_LEN`] UTF-8 bytes.
    pub name: String,
}

/// One decoded `FNP_CLIP` payload (`figures/<figure>/clip/<clip>`): sampled and constant joint
/// tracks at the authoring rate, plus the clip's markers.
///
/// All fields are private and read through the accessors below, because the constant/sampled split
/// is a storage decision of the format rather than something a caller should branch on. Only
/// produced by [`decode_clip`].
#[derive(Debug, Clone, PartialEq)]
pub struct ClipData {
    joint_count: u32,
    frame_count: u32,
    frame_rate_hz: f32,
    skeleton_fingerprint: u64,
    looping: bool,
    joints: Vec<JointTracks>,
    markers: Vec<ClipMarker>,
}

impl ClipData {
    /// Number of joints this clip poses; always equal to the `joint_count` of the skeleton it was
    /// authored for ([`ClipData::validate_against`]).
    #[must_use]
    pub fn joint_count(&self) -> u32 {
        self.joint_count
    }

    /// Number of stored frames, at least 1 and at most [`MAX_CLIP_FRAMES`].
    #[must_use]
    pub fn frame_count(&self) -> u32 {
        self.frame_count
    }

    /// The authoring rate in Hz the clip was sampled at (24 for every clip the pilot figures
    /// ship). The clip is never resampled to the simulation's tick rate (ADR-0017).
    #[must_use]
    pub fn frame_rate_hz(&self) -> f32 {
        self.frame_rate_hz
    }

    /// Whether the clip loops, i.e. whether its last frame repeats its first.
    #[must_use]
    pub fn is_looping(&self) -> bool {
        self.looping
    }

    /// Fingerprint of the skeleton this clip was authored for, as
    /// [`skeleton_fingerprint`] computes it.
    #[must_use]
    pub fn skeleton_fingerprint(&self) -> u64 {
        self.skeleton_fingerprint
    }

    /// The clip's playable span in seconds: `(frame_count - 1) / frame_rate_hz`, the time of its
    /// last stored frame.
    ///
    /// For a looping clip this is also the loop length, because its last frame repeats its first
    /// (ADR-0017, "Zeitbasis"): sampling at `duration_seconds()` and at `0.0` gives the same pose.
    /// A one-frame clip has a duration of `0.0`.
    #[must_use]
    pub fn duration_seconds(&self) -> f32 {
        // `frame_count >= 1` and `frame_rate_hz > 0` (both checked by `decode_clip`).
        (self.frame_count - 1) as f32 / self.frame_rate_hz
    }

    /// The clip's markers, sorted by frame (never strictly: two markers may share a frame).
    #[must_use]
    pub fn markers(&self) -> &[ClipMarker] {
        &self.markers
    }

    /// The time in seconds of the first marker called `name`, or `None` if the clip has no such
    /// marker.
    ///
    /// This is the only place the engine looks at a marker name at all, and it only compares it —
    /// what a name *means* is the caller's business (ADR-0017: "Die Engine deutet Namen nicht").
    #[must_use]
    pub fn marker_time(&self, name: &str) -> Option<f32> {
        self.markers
            .iter()
            .find(|marker| marker.name == name)
            .map(|marker| marker.frame as f32 / self.frame_rate_hz)
    }

    /// Checks that this clip belongs to `skeleton`: same joint count and same
    /// [`skeleton_fingerprint`].
    ///
    /// The fingerprint catches the case the joint count alone cannot — a clip authored for a
    /// *different* rig that happens to have the same number of joints, which would otherwise
    /// produce a silently wrong pose instead of an error.
    ///
    /// # Errors
    /// [`FigureFormatError::ClipJointCountMismatch`] or
    /// [`FigureFormatError::ClipSkeletonMismatch`].
    pub fn validate_against(&self, skeleton: &SkeletonData) -> Result<(), FigureFormatError> {
        let skeleton_joints = skeleton.joint_count();
        if self.joint_count != skeleton_joints {
            return Err(FigureFormatError::ClipJointCountMismatch {
                clip: self.joint_count,
                skeleton: skeleton_joints,
            });
        }
        let expected = skeleton_fingerprint(skeleton);
        if self.skeleton_fingerprint != expected {
            return Err(FigureFormatError::ClipSkeletonMismatch {
                clip: self.skeleton_fingerprint,
                skeleton: expected,
            });
        }
        Ok(())
    }
}

/// Fingerprint of `skeleton`'s shape: [`StableHasher`] v1 over the joint count and every joint's
/// parent index (`-1` for the root), in joint order.
///
/// Deliberately *not* over the bind matrices, the rest pose or the joint names: a clip is
/// compatible with a skeleton whose hierarchy it matches, and re-exporting a figure with nudged
/// bind matrices must not invalidate its clips. `docs/formats/figure-clip.md` §4 writes the
/// algorithm out field by field so the offline converter can reproduce it without this crate.
#[must_use]
pub fn skeleton_fingerprint(skeleton: &SkeletonData) -> u64 {
    let mut hasher = StableHasher::new();
    hasher.write_u32(skeleton.joint_count());
    for joint in &skeleton.joints {
        // `-1` for the root, matching the payload encoding of `FNP_SKELETON`'s `parent` field.
        hasher.write_i32(joint.parent.map_or(-1, |parent| parent as i32));
    }
    hasher.finish()
}

/// Reads one track's storage byte and its keys, checking the declared length against the remaining
/// input *before* allocating (contract §2 rule 9).
fn decode_track<const N: usize>(
    cursor: &mut Cursor<'_>,
    frame_count: u32,
    joint: u32,
    what: &'static str,
) -> Result<Track<N>, FigureFormatError> {
    match cursor.u8()? {
        TRACK_CONSTANT => Ok(Track::Constant(read_finite(cursor, joint, 0, what)?)),
        TRACK_SAMPLED => {
            let needed = (frame_count as usize).checked_mul(N * 4).ok_or(
                FigureFormatError::UnexpectedEnd {
                    offset: cursor.position(),
                    needed: usize::MAX,
                    available: cursor.remaining(),
                },
            )?;
            if cursor.remaining() < needed {
                return Err(FigureFormatError::UnexpectedEnd {
                    offset: cursor.position(),
                    needed,
                    available: cursor.remaining(),
                });
            }
            let mut keys = Vec::with_capacity(frame_count as usize);
            for frame in 0..frame_count {
                keys.push(read_finite(cursor, joint, frame, what)?);
            }
            Ok(Track::Sampled(keys))
        }
        other => Err(FigureFormatError::InvalidClipTrackKind {
            joint,
            what,
            found: other,
        }),
    }
}

/// Reads `N` floats and rejects any non-finite one, so a sampled pose can never contain a NaN or
/// an infinity that would silently poison a skinning matrix.
fn read_finite<const N: usize>(
    cursor: &mut Cursor<'_>,
    joint: u32,
    frame: u32,
    what: &'static str,
) -> Result<[f32; N], FigureFormatError> {
    let values = cursor.f32_array::<N>()?;
    if values.iter().any(|value| !value.is_finite()) {
        return Err(FigureFormatError::NonFiniteClipValue { joint, frame, what });
    }
    Ok(values)
}

/// Checks one rotation track's keys against [`CLIP_QUATERNION_TOLERANCE`].
///
/// A sign flip between neighbouring keys is explicitly *allowed* — the measured witch clips
/// contain 22 of them — and handled by the shorter-path rule in [`nlerp_shortest`], not by
/// rewriting the stored keys.
fn check_rotation_track(track: &Track<4>, joint: u32) -> Result<(), FigureFormatError> {
    let check = |quaternion: [f32; 4], frame: u32| {
        let [x, y, z, w] = quaternion;
        let norm = (x * x + y * y + z * z + w * w).sqrt();
        if (norm - 1.0).abs() > CLIP_QUATERNION_TOLERANCE {
            return Err(FigureFormatError::InvalidClipQuaternion { joint, frame, norm });
        }
        Ok(())
    };
    match track {
        Track::Constant(value) => check(*value, 0),
        Track::Sampled(keys) => {
            for (frame, &key) in keys.iter().enumerate() {
                // `keys.len() == frame_count <= MAX_CLIP_FRAMES`, well within u32.
                check(key, u32::try_from(frame).unwrap_or(u32::MAX))?;
            }
            Ok(())
        }
    }
}

/// Decodes an `FNP_CLIP` payload (`figures/<figure>/clip/<clip>`, kind `0x8005`) into a
/// [`ClipData`]. The byte layout is written out in `docs/formats/figure-clip.md`.
///
/// Checks, in this order and always before any allocation sized by the input: the magic, the
/// version, the reserved flag bits, `joint_count` against [`MAX_SKELETON_JOINTS`], `frame_count`
/// against [`MAX_CLIP_FRAMES`], the frame rate, `marker_count` against [`MAX_CLIP_MARKERS`], then
/// per track the declared key block against the remaining input, and finally every marker's frame
/// against `frame_count` and the marker order.
///
/// # Errors
/// See [`FigureFormatError`]; never panics, for any input.
pub fn decode_clip(bytes: &[u8]) -> Result<ClipData, FigureFormatError> {
    let mut cursor = Cursor::new(bytes);

    let magic = cursor.array::<8>()?;
    if magic != CLIP_MAGIC {
        return Err(FigureFormatError::InvalidClipMagic(magic));
    }
    let version = cursor.u32()?;
    if version != CLIP_FORMAT_VERSION {
        return Err(FigureFormatError::UnsupportedVersion {
            expected: CLIP_FORMAT_VERSION,
            found: version,
        });
    }

    let flags = cursor.u32()?;
    if flags & !FLAG_LOOPING != 0 {
        return Err(FigureFormatError::InvalidClipFlags(flags));
    }
    let looping = flags & FLAG_LOOPING != 0;

    let joint_count = check_count("joint_count", cursor.u32()?, MAX_SKELETON_JOINTS)?;
    if joint_count == 0 {
        return Err(FigureFormatError::ZeroCount("joint_count"));
    }
    let frame_count = check_count("frame_count", cursor.u32()?, MAX_CLIP_FRAMES)?;
    if frame_count == 0 {
        return Err(FigureFormatError::ZeroCount("frame_count"));
    }

    let frame_rate_hz = cursor.f32()?;
    if !frame_rate_hz.is_finite() || frame_rate_hz <= 0.0 || frame_rate_hz > MAX_CLIP_FRAME_RATE_HZ
    {
        return Err(FigureFormatError::InvalidClipFrameRate(frame_rate_hz));
    }

    let skeleton_fingerprint = cursor.u64()?;
    let marker_count = check_count("marker_count", cursor.u32()?, MAX_CLIP_MARKERS)?;

    let mut joints = Vec::with_capacity(joint_count as usize);
    for joint in 0..joint_count {
        let translation = decode_track::<3>(&mut cursor, frame_count, joint, "translation")?;
        let rotation = decode_track::<4>(&mut cursor, frame_count, joint, "rotation")?;
        check_rotation_track(&rotation, joint)?;
        let scale = decode_track::<3>(&mut cursor, frame_count, joint, "scale")?;
        joints.push(JointTracks {
            translation,
            rotation,
            scale,
        });
    }

    let mut markers: Vec<ClipMarker> = Vec::with_capacity(marker_count as usize);
    for marker in 0..marker_count {
        let frame = cursor.u32()?;
        if frame >= frame_count {
            return Err(FigureFormatError::IndexOutOfRange {
                what: "clip marker frame",
                index: u64::from(frame),
                bound: u64::from(frame_count),
            });
        }
        if let Some(previous) = markers.last()
            && frame < previous.frame
        {
            return Err(FigureFormatError::ClipMarkersOutOfOrder { marker });
        }
        let name_len = usize::from(cursor.u8()?);
        if name_len > MAX_CLIP_MARKER_NAME_LEN {
            return Err(FigureFormatError::CountExceedsLimit {
                what: "clip marker name length",
                count: name_len as u64,
                limit: MAX_CLIP_MARKER_NAME_LEN as u64,
            });
        }
        let name_bytes = cursor.take(name_len)?;
        let name = String::from_utf8(name_bytes.to_vec())
            .map_err(|_| FigureFormatError::InvalidClipMarkerName(marker))?;
        markers.push(ClipMarker { frame, name });
    }

    cursor.expect_exhausted()?;

    Ok(ClipData {
        joint_count,
        frame_count,
        frame_rate_hz,
        skeleton_fingerprint,
        looping,
        joints,
        markers,
    })
}

// --- Sampling ---------------------------------------------------------------------------------

/// Where one clip time falls between two stored frames.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FramePosition {
    lower: usize,
    upper: usize,
    fraction: f32,
}

/// Maps `time` in seconds onto a position between two stored frames.
///
/// A looping clip wraps into `[0, duration_seconds())` — its last frame repeats its first, so the
/// upper frame of the last interval is the stored last frame and no modulo on the *index* is
/// needed. A non-looping clip clamps at both ends. A non-finite `time` is treated as `0.0` rather
/// than propagated into a pose: sampling is infallible by design and a NaN would otherwise reach
/// the joint palette.
fn frame_position(clip: &ClipData, time: f32) -> FramePosition {
    let last = clip.frame_count - 1;
    if last == 0 {
        return FramePosition {
            lower: 0,
            upper: 0,
            fraction: 0.0,
        };
    }
    let span = last as f32;
    let time = if time.is_finite() { time } else { 0.0 };
    let mut position = time * clip.frame_rate_hz;
    position = if clip.looping {
        // `f32::rem_euclid` is `%` (IEEE 754 remainder-truncated: exact, never rounded) plus one
        // addition, so it is bit-identical on every platform.
        position.rem_euclid(span)
    } else {
        position.clamp(0.0, span)
    };
    // `position` is now in `[0, span]`, so the truncating cast cannot saturate and `lower <= last`.
    let lower = position as u32;
    let lower = lower.min(last);
    let fraction = position - lower as f32;
    FramePosition {
        lower: lower as usize,
        upper: (lower + 1).min(last) as usize,
        fraction,
    }
}

/// Component-wise linear interpolation, with `fraction == 0.0` returning `a`'s bits unchanged.
#[inline]
fn lerp<const N: usize>(a: [f32; N], b: [f32; N], fraction: f32) -> [f32; N] {
    let mut out = a;
    for index in 0..N {
        out[index] = a[index] + (b[index] - a[index]) * fraction;
    }
    out
}

/// Normalised linear interpolation of two rotations, always along the **shorter** path: when the
/// two quaternions point into opposite hemispheres (`dot < 0`), `b` is negated first, which is the
/// same rotation but the near side of it.
///
/// This is not optional polish. The authored witch clips contain 22 sign flips between neighbouring
/// keys, almost all at the thighs (ADR-0017, "Befund"); without the flip those frames interpolate
/// the long way round and the leg visibly snaps once per loop.
///
/// Two exactly opposite quaternions at exactly `fraction == 0.5` interpolate to the zero
/// quaternion, which has no direction to normalise; that single degenerate case yields the identity
/// rotation. It cannot arise from the flip itself (after the flip the two inputs are at most a
/// right angle apart in quaternion space).
#[inline]
fn nlerp_shortest(a: [f32; 4], b: [f32; 4], fraction: f32) -> [f32; 4] {
    let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let sign = if dot < 0.0 { -1.0 } else { 1.0 };
    let mut out = [0.0f32; 4];
    for index in 0..4 {
        out[index] = a[index] + (sign * b[index] - a[index]) * fraction;
    }
    let length = (out[0] * out[0] + out[1] * out[1] + out[2] * out[2] + out[3] * out[3]).sqrt();
    if length <= 0.0 || !length.is_finite() {
        return IDENTITY_QUATERNION;
    }
    [
        out[0] / length,
        out[1] / length,
        out[2] / length,
        out[3] / length,
    ]
}

/// Samples `clip` at `time` seconds into `out`, one [`JointPose`] per joint in skeleton order.
///
/// `out` is cleared first and ends up exactly [`ClipData::joint_count`] entries long, so a caller
/// can hand the same `Vec` back every frame and never allocate again after the first call.
///
/// Sampling exactly on a stored frame returns that frame's keys **bit for bit** — no interpolation
/// and no renormalisation runs — which is what makes the frozen pose hashes in this module's tests
/// a meaningful platform check rather than a tolerance comparison.
///
/// Infallible: a time before the clip, after it, or not finite at all is wrapped or clamped (see
/// [`ClipData::duration_seconds`]), never an error and never a panic.
pub fn sample_pose_into(clip: &ClipData, time: f32, out: &mut Vec<JointPose>) {
    let position = frame_position(clip, time);
    out.clear();
    out.reserve(clip.joints.len());
    for joint in &clip.joints {
        let translation = sample_track(&joint.translation, position);
        let scale = sample_track(&joint.scale, position);
        let rotation = sample_rotation(&joint.rotation, position);
        out.push(JointPose {
            translation,
            rotation,
            scale,
        });
    }
}

/// [`sample_pose_into`] into a freshly allocated vector, for callers that sample once (tests,
/// tools) rather than every frame.
#[must_use]
pub fn sample_pose(clip: &ClipData, time: f32) -> Vec<JointPose> {
    let mut out = Vec::new();
    sample_pose_into(clip, time, &mut out);
    out
}

#[inline]
fn sample_track<const N: usize>(track: &Track<N>, position: FramePosition) -> [f32; N] {
    let lower = track.key(position.lower);
    if position.fraction == 0.0 {
        return lower;
    }
    lerp(lower, track.key(position.upper), position.fraction)
}

#[inline]
fn sample_rotation(track: &Track<4>, position: FramePosition) -> [f32; 4] {
    let lower = track.key(position.lower);
    if position.fraction == 0.0 {
        return lower;
    }
    nlerp_shortest(lower, track.key(position.upper), position.fraction)
}

/// Blends `second` into `first` with `weight`, writing the result to `out`: `weight == 0.0` yields
/// `first` unchanged, `weight == 1.0` yields `second` unchanged, and anything between interpolates
/// translation and scale linearly and rotation with nlerp on the shorter path, exactly as
/// sampling between two frames does.
///
/// Both endpoints are returned bit for bit rather than interpolated, so a crossfade that has not
/// started and one that has finished are indistinguishable from the plain sample — which is what a
/// caller needs if a crossfade is to be a pure function of the world plus `alpha` (ADR-0017) and
/// still leave the frozen pose hashes of the un-blended clip intact.
///
/// A non-finite `weight` is treated as `0.0`; a weight outside `[0, 1]` is clamped.
///
/// # Errors
/// [`FigureFormatError::ClipJointCountMismatch`] if the two poses differ in length; nothing is
/// written to `out` in that case.
pub fn blend_poses_into(
    first: &[JointPose],
    second: &[JointPose],
    weight: f32,
    out: &mut Vec<JointPose>,
) -> Result<(), FigureFormatError> {
    check_pose_lengths(first.len(), second.len())?;
    out.clear();
    out.reserve(first.len());
    out.extend_from_slice(first);
    blend_in_place(out, second, weight);
    Ok(())
}

fn check_pose_lengths(first: usize, second: usize) -> Result<(), FigureFormatError> {
    if first != second {
        return Err(FigureFormatError::ClipJointCountMismatch {
            // Both lengths come from a `ClipData::joint_count`, which is `<= MAX_SKELETON_JOINTS`.
            clip: u32::try_from(first).unwrap_or(u32::MAX),
            skeleton: u32::try_from(second).unwrap_or(u32::MAX),
        });
    }
    Ok(())
}

/// Blends `second` into `destination` in place; `destination` and `second` must already be the
/// same length (checked by every caller).
fn blend_in_place(destination: &mut [JointPose], second: &[JointPose], weight: f32) {
    let weight = if weight.is_finite() {
        weight.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if weight == 0.0 {
        return;
    }
    if weight == 1.0 {
        destination.copy_from_slice(second);
        return;
    }
    for (target, source) in destination.iter_mut().zip(second.iter()) {
        target.translation = lerp(target.translation, source.translation, weight);
        target.scale = lerp(target.scale, source.scale, weight);
        target.rotation = nlerp_shortest(target.rotation, source.rotation, weight);
    }
}

/// Samples two clips and crossfades between them in one call, allocating two temporary pose
/// buffers.
///
/// Per-frame callers should use [`ClipSampler::sample_crossfade`] instead, which keeps those two
/// buffers between calls.
///
/// # Errors
/// [`FigureFormatError::ClipJointCountMismatch`] if the two clips pose different joint counts.
pub fn crossfade_pose_into(
    first: &ClipData,
    first_time: f32,
    second: &ClipData,
    second_time: f32,
    weight: f32,
    out: &mut Vec<JointPose>,
) -> Result<(), FigureFormatError> {
    check_pose_lengths(first.joints.len(), second.joints.len())?;
    let first_pose = sample_pose(first, first_time);
    let second_pose = sample_pose(second, second_time);
    blend_poses_into(&first_pose, &second_pose, weight, out)
}

// --- Marker time warping ----------------------------------------------------------------------

/// One anchor of a piecewise-linear time warp: the time a clip reaches some pose at, and the time
/// the caller wants it reached at.
///
/// The usual pair is a clip marker and the simulation's own anchor for the same beat:
/// `TimeAnchor::new(clip.marker_time("hit").unwrap(), hit_tick_time)`. ADR-0017 puts the anchors on
/// the simulation side as content data in ticks and leaves the clip's markers as the presentation
/// side of the same beat, which is what lets a clip be re-timed without touching a state hash.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeAnchor {
    /// The time in the clip's own seconds.
    pub clip_time: f32,
    /// The time the caller wants that clip time to land on.
    pub target_time: f32,
}

impl TimeAnchor {
    /// A new anchor pairing `clip_time` with `target_time`.
    #[must_use]
    pub fn new(clip_time: f32, target_time: f32) -> Self {
        Self {
            clip_time,
            target_time,
        }
    }
}

/// Maps `target_time` back to a clip time, stretching each segment between neighbouring `anchors`
/// linearly so that every anchor's `clip_time` lands exactly on its `target_time`.
///
/// The result is continuous and strictly increasing in `target_time` (both anchor fields must be
/// strictly increasing, so every segment has a positive rate), which is what keeps a warped clip
/// from ever playing backwards. Outside the anchor range the nearest segment's rate continues, so
/// the mapping stays monotone and has no seam at the first and last anchor.
///
/// Passing exactly an anchor's `target_time` returns exactly that anchor's `clip_time`: the segment
/// search puts such a time at the start of its segment, where the interpolation factor is exactly
/// `0.0`, so no rounding happens. The test `warp_hits_every_anchor_exactly` freezes that.
///
/// **No rate limit is applied.** ADR-0017 leaves "Grenzen der Verzerrung" open (proposal: a tempo
/// factor of 0.5 to 2.0 per segment) and that bound belongs in the offline converter's cross-check
/// of anchors against markers, not here — this function must reproduce whatever the content says,
/// or a mismatch between the two would be silently clamped at runtime instead of reported at build
/// time.
///
/// # Errors
/// [`FigureFormatError::InvalidTimeAnchors`] if there are fewer than two anchors, if any field is
/// not finite, or if `clip_time` or `target_time` does not strictly increase.
pub fn warped_clip_time(
    anchors: &[TimeAnchor],
    target_time: f32,
) -> Result<f32, FigureFormatError> {
    if anchors.len() < 2 {
        return Err(FigureFormatError::InvalidTimeAnchors {
            reason: "at least two anchors are needed to define a segment",
        });
    }
    for anchor in anchors {
        if !anchor.clip_time.is_finite() || !anchor.target_time.is_finite() {
            return Err(FigureFormatError::InvalidTimeAnchors {
                reason: "every anchor time must be finite",
            });
        }
    }
    for pair in anchors.windows(2) {
        if pair[1].clip_time <= pair[0].clip_time {
            return Err(FigureFormatError::InvalidTimeAnchors {
                reason: "clip_time must strictly increase",
            });
        }
        if pair[1].target_time <= pair[0].target_time {
            return Err(FigureFormatError::InvalidTimeAnchors {
                reason: "target_time must strictly increase",
            });
        }
    }
    let target_time = if target_time.is_finite() {
        target_time
    } else {
        anchors[0].target_time
    };

    // The segment whose start is the last anchor at or before `target_time`; times before the
    // first anchor extend the first segment backwards, times after the last extend the last
    // segment forwards.
    let last_segment = anchors.len() - 2;
    let mut segment = 0;
    while segment < last_segment && target_time >= anchors[segment + 1].target_time {
        segment += 1;
    }
    let start = anchors[segment];
    let end = anchors[segment + 1];
    let fraction = (target_time - start.target_time) / (end.target_time - start.target_time);
    // Both ends are returned verbatim rather than interpolated. `start` would come out exact
    // anyway (`a + (b - a) * 0.0 == a`), but `end` would not: `a + (b - a) * 1.0` is only `b` up
    // to rounding, and the last anchor is always the *end* of its segment, never the start of the
    // next one. Without this, the one anchor a caller most cares about — the last — would be the
    // one that misses.
    if fraction == 0.0 {
        return Ok(start.clip_time);
    }
    if fraction == 1.0 {
        return Ok(end.clip_time);
    }
    Ok(start.clip_time + (end.clip_time - start.clip_time) * fraction)
}

// --- Palette ----------------------------------------------------------------------------------

/// Reusable buffers for sampling a clip into the joint matrix palette
/// [`crate::StageFrame::joint_matrices`] consumes.
///
/// Holds **buffers only** — no clip, no time, no weight, no playhead. Two calls with the same
/// arguments always produce the same output regardless of what the sampler did before, which is
/// what ADR-0017's "zustandslos" requires; the struct exists purely so a caller sampling 30 figures
/// per frame allocates nothing after the first frame.
#[derive(Debug, Clone, Default)]
pub struct ClipSampler {
    pose: Vec<JointPose>,
    scratch: Vec<JointPose>,
    world: Vec<[[f32; 4]; 4]>,
    matrices: Vec<[[f32; 4]; 4]>,
}

impl ClipSampler {
    /// A sampler with empty buffers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The pose written by the last [`ClipSampler::sample`] or
    /// [`ClipSampler::sample_crossfade`].
    #[must_use]
    pub fn pose(&self) -> &[JointPose] {
        &self.pose
    }

    /// The palette written by the last [`ClipSampler::skin_matrices`], ready to append to
    /// [`crate::StageFrame::joint_matrices`].
    #[must_use]
    pub fn matrices(&self) -> &[[[f32; 4]; 4]] {
        &self.matrices
    }

    /// Samples `clip` at `time` into this sampler's pose buffer; see [`sample_pose_into`].
    pub fn sample(&mut self, clip: &ClipData, time: f32) {
        sample_pose_into(clip, time, &mut self.pose);
    }

    /// Samples both clips and crossfades them into this sampler's pose buffer, reusing the
    /// scratch buffer for the second pose; see [`blend_poses_into`] for the endpoint rules.
    ///
    /// # Errors
    /// [`FigureFormatError::ClipJointCountMismatch`] if the two clips pose different joint counts;
    /// the pose buffer is left holding `first`'s pose in that case.
    pub fn sample_crossfade(
        &mut self,
        first: &ClipData,
        first_time: f32,
        second: &ClipData,
        second_time: f32,
        weight: f32,
    ) -> Result<(), FigureFormatError> {
        sample_pose_into(first, first_time, &mut self.pose);
        sample_pose_into(second, second_time, &mut self.scratch);
        check_pose_lengths(self.pose.len(), self.scratch.len())?;
        blend_in_place(&mut self.pose, &self.scratch, weight);
        Ok(())
    }

    /// Composes the sampled pose against `skeleton` into this sampler's palette buffer and returns
    /// it: the same `world_pose * inverse_bind` composition as
    /// [`crate::figure_format::compute_skin_matrices`], but into buffers that survive the call.
    ///
    /// A palette longer than [`crate::MAX_SKIN_JOINTS`] cannot arise here — both the clip's and the
    /// skeleton's joint count are checked against that bound when they are decoded.
    ///
    /// # Errors
    /// [`FigureFormatError::ClipJointCountMismatch`] if the sampled pose does not have exactly one
    /// entry per joint of `skeleton`.
    pub fn skin_matrices(
        &mut self,
        skeleton: &SkeletonData,
    ) -> Result<&[[[f32; 4]; 4]], FigureFormatError> {
        check_pose_lengths(self.pose.len(), skeleton.joints.len())?;
        self.world.clear();
        self.world.resize(skeleton.joints.len(), IDENTITY_MATRIX);
        for (index, joint) in skeleton.joints.iter().enumerate() {
            let local = trs_matrix(self.pose[index]);
            self.world[index] = match joint.parent {
                // `decode_skeleton` guarantees `parent < index`, so the parent is already done.
                Some(parent) => mat4_mul(&self.world[parent as usize], &local),
                None => local,
            };
        }
        self.matrices.clear();
        self.matrices.reserve(skeleton.joints.len());
        for (joint, world) in skeleton.joints.iter().zip(self.world.iter()) {
            self.matrices.push(mat4_mul(world, &joint.inverse_bind));
        }
        Ok(&self.matrices)
    }
}

#[cfg(test)]
mod tests;
