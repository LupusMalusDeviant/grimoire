//! Tests for [`crate::figure_clip`] (engine ADR-0017).
//!
//! **How the fixtures were derived.** There is no checked-in `.glb` and no converter in this
//! repository, and the product owner's authored figures live outside both repositories and are not
//! test data (ADR-0017, "Datenstand"). Every clip here is therefore hand-built byte by byte from
//! the layout table in `docs/formats/figure-clip.md`, the same way
//! `crate::figure_format`'s tests build their mesh and skeleton payloads and
//! `grimoire_assets` builds `pack_v1_sigil.grimpack`. The shapes are chosen to reproduce the two
//! properties the measurement in ADR-0017 found in the real clips and that a decoder or sampler
//! can silently get wrong:
//!
//! - **the loop form** — the last frame repeats the first, so a loop is `frame_count - 1` frames
//!   long, not `frame_count`;
//! - **the quaternion sign flips** — the witch clips contain 22 of them, so the main fixture
//!   stores two of its four rotation keys in the opposite hemisphere, which makes the shorter-path
//!   rule load-bearing for the plain loop sample rather than something only a special-case test
//!   reaches.
//!
//! The skeleton the clips are authored against is the same two-joint ribbon rig
//! (`root` → `limb` at `[0, 0, 1]`) that `crate::figure_format`'s tests and
//! `grimoire/tests/figure_pack.rs` use, so a matrix computed here can be checked against a
//! hand-derived expectation rather than against another run of the same code.

use super::*;
use crate::figure_format::{JointData, MAX_JOINT_NAME_LEN, decode_skeleton};

// --- Byte helpers, the same shape as `figure_format`'s test helpers ---------------------------

fn push_f32(buf: &mut Vec<u8>, value: f32) {
    buf.extend_from_slice(&value.to_le_bytes());
}
fn push_f32s(buf: &mut Vec<u8>, values: &[f32]) {
    for &value in values {
        push_f32(buf, value);
    }
}
fn push_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_le_bytes());
}
fn push_u64(buf: &mut Vec<u8>, value: u64) {
    buf.extend_from_slice(&value.to_le_bytes());
}

/// `sin(45°) = cos(45°)`, the non-zero components of a quarter turn about X.
const HALF_TURN_COMPONENT: f32 = std::f32::consts::FRAC_1_SQRT_2;
const IDENTITY_QUAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
/// A 90° turn about X: half-angle 45°, so `x = w = sin(45°)`.
const QUARTER_TURN_X: [f32; 4] = [HALF_TURN_COMPONENT, 0.0, 0.0, HALF_TURN_COMPONENT];
/// A 60° turn about X (half-angle 30°, so `x = 0.5` and `w = cos(30°)`), with every component
/// negated: the *same* rotation, the opposite hemisphere. This is what a sign flip between
/// neighbouring keys looks like, and it is what the shorter-path rule has to undo.
const NEGATED_SIXTH_TURN_X: [f32; 4] = [-0.5, 0.0, 0.0, -0.866_025_4];
/// The identity rotation with every component negated, the sign flip that closes the fixture's
/// loop back onto its first frame.
const NEGATED_IDENTITY_QUAT: [f32; 4] = [0.0, 0.0, 0.0, -1.0];

const FIXTURE_RATE_HZ: f32 = 24.0;

/// A clip header, ready for the track blocks to be appended.
fn clip_header(
    joint_count: u32,
    frame_count: u32,
    rate: f32,
    fingerprint: u64,
    looping: bool,
    marker_count: u32,
) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&CLIP_MAGIC);
    push_u32(&mut buf, CLIP_FORMAT_VERSION);
    push_u32(&mut buf, u32::from(looping));
    push_u32(&mut buf, joint_count);
    push_u32(&mut buf, frame_count);
    push_f32(&mut buf, rate);
    push_u64(&mut buf, fingerprint);
    push_u32(&mut buf, marker_count);
    buf
}

fn push_constant_track(buf: &mut Vec<u8>, value: &[f32]) {
    buf.push(TRACK_CONSTANT);
    push_f32s(buf, value);
}

fn push_sampled_track(buf: &mut Vec<u8>, keys: &[&[f32]]) {
    buf.push(TRACK_SAMPLED);
    for key in keys {
        push_f32s(buf, key);
    }
}

fn push_marker(buf: &mut Vec<u8>, frame: u32, name: &str) {
    push_u32(buf, frame);
    // `u8::try_from` cannot fail for the short names used here.
    buf.push(u8::try_from(name.len()).expect("fixture marker names are short"));
    buf.extend_from_slice(name.as_bytes());
}

// --- The two-joint skeleton the fixtures are authored for -------------------------------------

/// `root` at the origin, `limb` bound one unit further out along Z, with `limb`'s inverse bind
/// matrix being exactly the inverse of that translation — so the rest pose equals the bind pose
/// and an un-posed figure has an identity palette.
fn ribbon_skeleton() -> SkeletonData {
    SkeletonData {
        joints: vec![
            JointData {
                parent: None,
                inverse_bind: IDENTITY_MATRIX,
                name: "root".to_string(),
                translation: [0.0, 0.0, 0.0],
                rotation: IDENTITY_QUAT,
                scale: [1.0, 1.0, 1.0],
            },
            JointData {
                parent: Some(0),
                inverse_bind: [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [0.0, 0.0, -1.0, 1.0],
                ],
                name: "limb".to_string(),
                translation: [0.0, 0.0, 1.0],
                rotation: IDENTITY_QUAT,
                scale: [1.0, 1.0, 1.0],
            },
        ],
    }
}

/// The fingerprint of [`ribbon_skeleton`], recomputed rather than frozen: freezing it here would
/// only restate [`skeleton_fingerprint`]'s own output. The frozen value lives in
/// [`skeleton_fingerprint_is_frozen`], where it is a cross-platform check.
fn ribbon_fingerprint() -> u64 {
    skeleton_fingerprint(&ribbon_skeleton())
}

// --- The fixtures -----------------------------------------------------------------------------

/// The main fixture: 2 joints, 4 frames at 24 Hz, looping (so one loop is 3 frames = 1/8 s).
///
/// Deliberately **asymmetric** in both sampled tracks. A fixture that rises and falls through the
/// same values would hash identically before and after its midpoint, and would therefore pass the
/// frozen-pose test even with the two interpolation endpoints swapped.
///
/// - joint 0 `root`: translation **sampled** — `[0, 0, 0]`, `[0, 0.5, 0]`, `[0.25, 0.125, 0]`,
///   back to `[0, 0, 0]`: the root-offset shape ADR-0017 measured in every authored clip, with a
///   net offset of zero over the loop. Rotation and scale constant.
/// - joint 1 `limb`: translation and scale constant; rotation **sampled** — identity, a quarter
///   turn about X, then a **negated** 60° turn about X (the sign flip: the same rotation as
///   `[0.5, 0, 0, 0.866]`, stored in the opposite hemisphere), then the negated identity, which
///   closes the loop back onto frame 0's rotation.
fn wave_clip_bytes() -> Vec<u8> {
    let mut buf = clip_header(2, 4, FIXTURE_RATE_HZ, ribbon_fingerprint(), true, 2);
    // Joint 0.
    push_sampled_track(
        &mut buf,
        &[
            &[0.0, 0.0, 0.0],
            &[0.0, 0.5, 0.0],
            &[0.25, 0.125, 0.0],
            &[0.0, 0.0, 0.0],
        ],
    );
    push_constant_track(&mut buf, &IDENTITY_QUAT);
    push_constant_track(&mut buf, &[1.0, 1.0, 1.0]);
    // Joint 1.
    push_constant_track(&mut buf, &[0.0, 0.0, 1.0]);
    push_sampled_track(
        &mut buf,
        &[
            &IDENTITY_QUAT,
            &QUARTER_TURN_X,
            &NEGATED_SIXTH_TURN_X,
            &NEGATED_IDENTITY_QUAT,
        ],
    );
    push_constant_track(&mut buf, &[1.0, 1.0, 1.0]);
    push_marker(&mut buf, 1, "hit");
    push_marker(&mut buf, 2, "recover");
    buf
}

fn wave_clip() -> ClipData {
    decode_clip(&wave_clip_bytes()).expect("the hand-built wave fixture decodes")
}

/// A non-looping two-frame clip, for the clamping rule.
fn once_clip_bytes() -> Vec<u8> {
    let mut buf = clip_header(1, 2, FIXTURE_RATE_HZ, 0, false, 0);
    push_sampled_track(&mut buf, &[&[0.0, 0.0, 0.0], &[2.0, 0.0, 0.0]]);
    push_constant_track(&mut buf, &IDENTITY_QUAT);
    push_constant_track(&mut buf, &[1.0, 1.0, 1.0]);
    buf
}

/// A clip that poses [`ribbon_skeleton`] in exactly its own rest pose, as one constant track per
/// channel: sampling it and composing the result must give the identity palette, the analytic case
/// ADR-0017 asks for ("Ruhepose-Clip ergibt Einheitsmatrizen").
fn rest_pose_clip_bytes() -> Vec<u8> {
    let skeleton = ribbon_skeleton();
    let mut buf = clip_header(2, 1, FIXTURE_RATE_HZ, ribbon_fingerprint(), false, 0);
    for joint in &skeleton.joints {
        push_constant_track(&mut buf, &joint.translation);
        push_constant_track(&mut buf, &joint.rotation);
        push_constant_track(&mut buf, &joint.scale);
    }
    buf
}

// --- Decoding ---------------------------------------------------------------------------------

#[test]
fn decode_clip_round_trips_the_hand_built_fixture() {
    let clip = wave_clip();
    assert_eq!(clip.joint_count(), 2);
    assert_eq!(clip.frame_count(), 4);
    assert_eq!(clip.frame_rate_hz(), FIXTURE_RATE_HZ);
    assert!(clip.is_looping());
    assert_eq!(clip.skeleton_fingerprint(), ribbon_fingerprint());
    // Three frames of loop at 24 Hz: the fourth frame repeats the first.
    assert_eq!(clip.duration_seconds(), 3.0 / 24.0);
    assert_eq!(clip.markers().len(), 2);
    assert_eq!(clip.markers()[0].frame, 1);
    assert_eq!(clip.markers()[0].name, "hit");
    assert_eq!(clip.markers()[1].name, "recover");
    assert_eq!(clip.marker_time("hit"), Some(1.0 / 24.0));
    assert_eq!(clip.marker_time("recover"), Some(2.0 / 24.0));
    assert_eq!(clip.marker_time("nope"), None);
}

#[test]
fn decode_clip_keeps_constant_tracks_as_one_value() {
    let clip = wave_clip();
    // Joint 0's rotation and scale, and joint 1's translation and scale, are constant tracks; a
    // constant track that had silently been expanded to one key per frame would still sample the
    // same, so check the stored shape itself.
    assert!(matches!(clip.joints[0].rotation, Track::Constant(_)));
    assert!(matches!(clip.joints[0].scale, Track::Constant(_)));
    assert!(matches!(clip.joints[0].translation, Track::Sampled(_)));
    assert!(matches!(clip.joints[1].translation, Track::Constant(_)));
    assert!(matches!(clip.joints[1].rotation, Track::Sampled(_)));
}

#[test]
fn decode_clip_rejects_a_foreign_magic() {
    let mut bytes = wave_clip_bytes();
    bytes[0] = b'X';
    let mut expected = CLIP_MAGIC;
    expected[0] = b'X';
    assert_eq!(
        decode_clip(&bytes),
        Err(FigureFormatError::InvalidClipMagic(expected))
    );
}

#[test]
fn decode_clip_rejects_a_foreign_version() {
    let mut bytes = wave_clip_bytes();
    bytes[8] = 7;
    assert_eq!(
        decode_clip(&bytes),
        Err(FigureFormatError::UnsupportedVersion {
            expected: CLIP_FORMAT_VERSION,
            found: 7,
        })
    );
}

#[test]
fn decode_clip_rejects_a_reserved_flag_bit() {
    let mut bytes = wave_clip_bytes();
    bytes[12..16].copy_from_slice(&0b11u32.to_le_bytes());
    assert_eq!(
        decode_clip(&bytes),
        Err(FigureFormatError::InvalidClipFlags(0b11))
    );
}

#[test]
fn decode_clip_rejects_zero_counts() {
    let mut bytes = wave_clip_bytes();
    bytes[16..20].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(
        decode_clip(&bytes),
        Err(FigureFormatError::ZeroCount("joint_count"))
    );

    let mut bytes = wave_clip_bytes();
    bytes[20..24].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(
        decode_clip(&bytes),
        Err(FigureFormatError::ZeroCount("frame_count"))
    );
}

#[test]
fn decode_clip_rejects_counts_over_the_documented_limits() {
    let mut bytes = wave_clip_bytes();
    bytes[16..20].copy_from_slice(&(MAX_SKELETON_JOINTS + 1).to_le_bytes());
    assert_eq!(
        decode_clip(&bytes),
        Err(FigureFormatError::CountExceedsLimit {
            what: "joint_count",
            count: u64::from(MAX_SKELETON_JOINTS) + 1,
            limit: u64::from(MAX_SKELETON_JOINTS),
        })
    );

    let mut bytes = wave_clip_bytes();
    bytes[20..24].copy_from_slice(&(MAX_CLIP_FRAMES + 1).to_le_bytes());
    assert_eq!(
        decode_clip(&bytes),
        Err(FigureFormatError::CountExceedsLimit {
            what: "frame_count",
            count: u64::from(MAX_CLIP_FRAMES) + 1,
            limit: u64::from(MAX_CLIP_FRAMES),
        })
    );

    let mut bytes = wave_clip_bytes();
    bytes[36..40].copy_from_slice(&(MAX_CLIP_MARKERS + 1).to_le_bytes());
    assert_eq!(
        decode_clip(&bytes),
        Err(FigureFormatError::CountExceedsLimit {
            what: "marker_count",
            count: u64::from(MAX_CLIP_MARKERS) + 1,
            limit: u64::from(MAX_CLIP_MARKERS),
        })
    );
}

#[test]
fn decode_clip_rejects_an_unusable_frame_rate() {
    for rate in [0.0f32, -24.0, f32::NAN, f32::INFINITY, 4096.0] {
        let mut bytes = wave_clip_bytes();
        bytes[24..28].copy_from_slice(&rate.to_le_bytes());
        match decode_clip(&bytes) {
            Err(FigureFormatError::InvalidClipFrameRate(found)) => {
                assert!(
                    found.is_nan() || found == rate,
                    "reported {found}, sent {rate}"
                );
            }
            other => panic!("rate {rate} should be rejected, got {other:?}"),
        }
    }
}

#[test]
fn decode_clip_rejects_a_foreign_track_storage_byte() {
    let mut bytes = wave_clip_bytes();
    // The first track's storage byte sits directly after the 40-byte header.
    bytes[40] = 9;
    assert_eq!(
        decode_clip(&bytes),
        Err(FigureFormatError::InvalidClipTrackKind {
            joint: 0,
            what: "translation",
            found: 9,
        })
    );
}

#[test]
fn decode_clip_rejects_a_non_finite_key() {
    let mut bytes = wave_clip_bytes();
    // Joint 0's translation track: storage byte at 40, then frame 0's three floats.
    bytes[41..45].copy_from_slice(&f32::NAN.to_le_bytes());
    assert_eq!(
        decode_clip(&bytes),
        Err(FigureFormatError::NonFiniteClipValue {
            joint: 0,
            frame: 0,
            what: "translation",
        })
    );
}

#[test]
fn decode_clip_rejects_a_rotation_key_that_is_not_a_unit_quaternion() {
    let mut bytes = wave_clip_bytes();
    // Joint 0's rotation is the constant track right after joint 0's 49-byte translation block.
    let offset = 40 + 1 + 4 * 3 * 4 + 1;
    bytes[offset..offset + 4].copy_from_slice(&0.5f32.to_le_bytes());
    match decode_clip(&bytes) {
        Err(FigureFormatError::InvalidClipQuaternion { joint, frame, norm }) => {
            assert_eq!((joint, frame), (0, 0));
            // The key became [0.5, 0, 0, 1], whose norm is sqrt(1.25).
            assert!((norm - 1.25f32.sqrt()).abs() < 1e-6, "norm was {norm}");
        }
        other => panic!("expected a quaternion norm error, got {other:?}"),
    }
}

#[test]
fn decode_clip_accepts_a_sign_flipped_but_unit_rotation_key() {
    // The whole point of the shorter-path rule: a flipped key is valid data, not an error.
    let clip = wave_clip();
    assert!(
        matches!(&clip.joints[1].rotation, Track::Sampled(keys) if keys[2] == NEGATED_SIXTH_TURN_X)
    );
}

#[test]
fn decode_clip_rejects_a_marker_outside_the_clip() {
    let mut buf = clip_header(1, 2, FIXTURE_RATE_HZ, 0, false, 1);
    push_constant_track(&mut buf, &[0.0, 0.0, 0.0]);
    push_constant_track(&mut buf, &IDENTITY_QUAT);
    push_constant_track(&mut buf, &[1.0, 1.0, 1.0]);
    push_marker(&mut buf, 2, "late"); // frames are 0 and 1
    assert_eq!(
        decode_clip(&buf),
        Err(FigureFormatError::IndexOutOfRange {
            what: "clip marker frame",
            index: 2,
            bound: 2,
        })
    );
}

#[test]
fn decode_clip_rejects_markers_out_of_order() {
    let mut buf = clip_header(1, 4, FIXTURE_RATE_HZ, 0, false, 2);
    push_constant_track(&mut buf, &[0.0, 0.0, 0.0]);
    push_constant_track(&mut buf, &IDENTITY_QUAT);
    push_constant_track(&mut buf, &[1.0, 1.0, 1.0]);
    push_marker(&mut buf, 3, "late");
    push_marker(&mut buf, 1, "early");
    assert_eq!(
        decode_clip(&buf),
        Err(FigureFormatError::ClipMarkersOutOfOrder { marker: 1 })
    );
}

#[test]
fn decode_clip_accepts_two_markers_on_one_frame() {
    let mut buf = clip_header(1, 4, FIXTURE_RATE_HZ, 0, false, 2);
    push_constant_track(&mut buf, &[0.0, 0.0, 0.0]);
    push_constant_track(&mut buf, &IDENTITY_QUAT);
    push_constant_track(&mut buf, &[1.0, 1.0, 1.0]);
    push_marker(&mut buf, 2, "hit");
    push_marker(&mut buf, 2, "sound");
    let clip = decode_clip(&buf).expect("markers may share a frame");
    assert_eq!(clip.markers().len(), 2);
    assert_eq!(clip.marker_time("sound"), Some(2.0 / 24.0));
}

#[test]
fn decode_clip_rejects_a_marker_name_over_the_documented_limit() {
    let mut buf = clip_header(1, 2, FIXTURE_RATE_HZ, 0, false, 1);
    push_constant_track(&mut buf, &[0.0, 0.0, 0.0]);
    push_constant_track(&mut buf, &IDENTITY_QUAT);
    push_constant_track(&mut buf, &[1.0, 1.0, 1.0]);
    push_u32(&mut buf, 0);
    buf.push(255);
    assert_eq!(
        decode_clip(&buf),
        Err(FigureFormatError::CountExceedsLimit {
            what: "clip marker name length",
            count: 255,
            limit: MAX_CLIP_MARKER_NAME_LEN as u64,
        })
    );
    // The clip and the joint-name limit are deliberately the same number.
    assert_eq!(MAX_CLIP_MARKER_NAME_LEN, MAX_JOINT_NAME_LEN);
}

#[test]
fn decode_clip_rejects_a_marker_name_that_is_not_utf8() {
    let mut buf = clip_header(1, 2, FIXTURE_RATE_HZ, 0, false, 1);
    push_constant_track(&mut buf, &[0.0, 0.0, 0.0]);
    push_constant_track(&mut buf, &IDENTITY_QUAT);
    push_constant_track(&mut buf, &[1.0, 1.0, 1.0]);
    push_u32(&mut buf, 0);
    buf.push(2);
    buf.extend_from_slice(&[0xFF, 0xFE]);
    assert_eq!(
        decode_clip(&buf),
        Err(FigureFormatError::InvalidClipMarkerName(0))
    );
}

#[test]
fn decode_clip_rejects_trailing_bytes() {
    let mut bytes = wave_clip_bytes();
    bytes.push(0);
    assert_eq!(
        decode_clip(&bytes),
        Err(FigureFormatError::TrailingBytes(1))
    );
}

#[test]
fn decode_clip_rejects_a_frame_count_it_has_no_bytes_for_before_allocating() {
    // A header that declares the maximum frame count but carries only one key: the decoder must
    // notice from the remaining input, not from a failed allocation of 4096 keys.
    let mut buf = clip_header(1, MAX_CLIP_FRAMES, FIXTURE_RATE_HZ, 0, false, 0);
    buf.push(TRACK_SAMPLED);
    push_f32s(&mut buf, &[0.0, 0.0, 0.0]);
    assert_eq!(
        decode_clip(&buf),
        Err(FigureFormatError::UnexpectedEnd {
            offset: 41,
            needed: MAX_CLIP_FRAMES as usize * 12,
            available: 12,
        })
    );
}

#[test]
fn decode_clip_rejects_every_truncation_without_panicking() {
    let bytes = wave_clip_bytes();
    for len in 0..bytes.len() {
        assert!(
            decode_clip(&bytes[..len]).is_err(),
            "a clip truncated to {len} bytes must not decode"
        );
    }
    assert!(decode_clip(&bytes).is_ok());
}

// --- Skeleton fingerprint ---------------------------------------------------------------------

#[test]
fn skeleton_fingerprint_is_frozen() {
    // Frozen so a change to the algorithm is caught here rather than by every clip in the wild
    // failing to load, and so Windows, Linux and macOS are checked against one number.
    assert_eq!(ribbon_fingerprint(), 0x9a6c_8135_349d_1276);
}

#[test]
fn skeleton_fingerprint_separates_different_hierarchies() {
    let mut flat = ribbon_skeleton();
    flat.joints[1].parent = None; // same joint count, different hierarchy
    assert_ne!(skeleton_fingerprint(&flat), ribbon_fingerprint());
}

#[test]
fn skeleton_fingerprint_ignores_bind_matrices_and_names() {
    let mut nudged = ribbon_skeleton();
    nudged.joints[1].inverse_bind[3][2] = -1.001;
    nudged.joints[1].name = "arm".to_string();
    nudged.joints[1].translation = [0.0, 0.0, 1.001];
    assert_eq!(skeleton_fingerprint(&nudged), ribbon_fingerprint());
}

#[test]
fn validate_against_accepts_its_own_skeleton_and_rejects_others() {
    let clip = wave_clip();
    let skeleton = ribbon_skeleton();
    assert_eq!(clip.validate_against(&skeleton), Ok(()));

    let mut flat = ribbon_skeleton();
    flat.joints[1].parent = None;
    assert_eq!(
        clip.validate_against(&flat),
        Err(FigureFormatError::ClipSkeletonMismatch {
            clip: ribbon_fingerprint(),
            skeleton: skeleton_fingerprint(&flat),
        })
    );

    let one_joint = SkeletonData {
        joints: vec![ribbon_skeleton().joints.remove(0)],
    };
    assert_eq!(
        clip.validate_against(&one_joint),
        Err(FigureFormatError::ClipJointCountMismatch {
            clip: 2,
            skeleton: 1,
        })
    );
}

// --- Sampling ---------------------------------------------------------------------------------

/// Hashes a sampled pose with `StableHasher` v1, the same algorithm every golden hash in this
/// workspace uses, so the frozen values below compare bit for bit across operating systems.
fn hash_poses(poses: &[JointPose]) -> u64 {
    let mut hasher = StableHasher::new();
    hasher.write_usize(poses.len());
    for pose in poses {
        for value in pose.translation {
            hasher.write_f32(value);
        }
        for value in pose.rotation {
            hasher.write_f32(value);
        }
        for value in pose.scale {
            hasher.write_f32(value);
        }
    }
    hasher.finish()
}

#[test]
fn sampling_on_a_frame_returns_the_stored_keys_bit_for_bit() {
    let clip = wave_clip();
    // Frame 1 of the wave fixture: root lifted to y = 0.5, limb a quarter turn about X.
    let pose = sample_pose(&clip, 1.0 / 24.0);
    assert_eq!(pose[0].translation, [0.0, 0.5, 0.0]);
    assert_eq!(pose[0].rotation, IDENTITY_QUAT);
    assert_eq!(pose[0].scale, [1.0, 1.0, 1.0]);
    assert_eq!(pose[1].rotation, QUARTER_TURN_X);
    assert_eq!(pose[1].translation, [0.0, 0.0, 1.0]);

    // Frame 2, including its sign-flipped rotation key: a stored key comes back exactly as
    // stored, hemisphere included, and is never renormalised on the way out.
    let pose = sample_pose(&clip, 2.0 / 24.0);
    assert_eq!(pose[0].translation, [0.25, 0.125, 0.0]);
    assert_eq!(pose[1].rotation, NEGATED_SIXTH_TURN_X);

    // The last stored frame of a *looping* clip is never sampled directly: `duration_seconds()`
    // wraps back onto frame 0, which is the same pose by the loop convention.
    assert_eq!(
        hash_poses(&sample_pose(&clip, clip.duration_seconds())),
        hash_poses(&sample_pose(&clip, 0.0))
    );

    // A non-looping clip clamps instead, so its last stored key *is* reachable.
    let once = decode_clip(&once_clip_bytes()).expect("the once fixture decodes");
    assert_eq!(
        sample_pose(&once, 1.0 / 24.0)[0].translation,
        [2.0, 0.0, 0.0]
    );
}

#[test]
fn sampling_a_looping_clip_wraps_and_a_single_clip_clamps() {
    let clip = wave_clip();
    let at_zero = sample_pose(&clip, 0.0);
    // One whole loop later, and five loops later, the pose is identical, with no drift.
    assert_eq!(
        hash_poses(&sample_pose(&clip, clip.duration_seconds())),
        hash_poses(&at_zero)
    );
    assert_eq!(
        hash_poses(&sample_pose(&clip, 5.0 * clip.duration_seconds())),
        hash_poses(&at_zero)
    );
    // Negative times wrap forwards, not into a clamp.
    assert_eq!(
        hash_poses(&sample_pose(&clip, -clip.duration_seconds())),
        hash_poses(&at_zero)
    );

    let once = decode_clip(&once_clip_bytes()).expect("the once fixture decodes");
    assert_eq!(sample_pose(&once, -10.0)[0].translation, [0.0, 0.0, 0.0]);
    assert_eq!(sample_pose(&once, 10.0)[0].translation, [2.0, 0.0, 0.0]);
}

#[test]
fn sampling_a_non_finite_time_yields_the_first_frame_instead_of_a_nan_pose() {
    let clip = wave_clip();
    let first = hash_poses(&sample_pose(&clip, 0.0));
    for time in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(hash_poses(&sample_pose(&clip, time)), first);
    }
}

#[test]
fn sampling_between_frames_interpolates_translation_linearly() {
    let clip = wave_clip();
    // Exactly halfway between frame 0 (y = 0) and frame 1 (y = 0.5).
    let pose = sample_pose(&clip, 0.5 / 24.0);
    assert_eq!(pose[0].translation, [0.0, 0.25, 0.0]);
}

#[test]
fn sampling_takes_the_shorter_path_over_a_sign_flip() {
    let clip = wave_clip();
    // Halfway between frame 1 (90 degrees about X) and frame 2 (60 degrees about X, stored
    // negated). The shorter path flips frame 2's sign first and lands on 75 degrees, exactly
    // between the two; without the flip the interpolation goes the long way round and ends up
    // past a half turn.
    let pose = sample_pose(&clip, 1.5 / 24.0);
    let [x, y, z, w] = pose[1].rotation;
    assert_eq!([y, z], [0.0, 0.0], "the fixture only ever turns about X");
    assert!(
        x > 0.0,
        "the shorter path stays on the near hemisphere: {x}"
    );
    // w is the cosine of half the angle: 75 degrees gives cos(37.5 degrees) = 0.7934.
    assert!(
        (w - 0.793_353_3).abs() < 1e-5,
        "expected a 75 degree turn, got w = {w}"
    );

    // The same numbers, computed from the raw keys, with and without the rule.
    let shorter = nlerp_shortest(QUARTER_TURN_X, NEGATED_SIXTH_TURN_X, 0.5);
    assert_eq!(pose[1].rotation, shorter);
    let longer = nlerp_without_the_rule(QUARTER_TURN_X, NEGATED_SIXTH_TURN_X, 0.5);
    assert_ne!(shorter, longer);
    assert!(longer[3] < 0.0, "long way round gave w = {}", longer[3]);
}

/// What [`nlerp_shortest`] would do *without* the shorter-path rule, so the test above compares
/// two numbers rather than asserting one looks plausible.
fn nlerp_without_the_rule(a: [f32; 4], b: [f32; 4], fraction: f32) -> [f32; 4] {
    let mut out = [0.0f32; 4];
    for index in 0..4 {
        out[index] = a[index] + (b[index] - a[index]) * fraction;
    }
    let length = (out[0] * out[0] + out[1] * out[1] + out[2] * out[2] + out[3] * out[3]).sqrt();
    [
        out[0] / length,
        out[1] / length,
        out[2] / length,
        out[3] / length,
    ]
}

#[test]
fn nlerp_of_two_exactly_opposite_rotations_at_one_half_is_the_identity_not_a_nan() {
    let flipped = nlerp_shortest([1.0, 0.0, 0.0, 0.0], [-1.0, 0.0, 0.0, 0.0], 0.5);
    // The rule flips the second input first, so this case does *not* degenerate.
    assert_eq!(flipped, [1.0, 0.0, 0.0, 0.0]);
    // Construct the degenerate case directly: two inputs that cancel with a non-negative dot.
    let degenerate = nlerp_shortest([0.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 0.0], 0.5);
    assert_eq!(degenerate, IDENTITY_QUATERNION);
}

#[test]
fn golden_sampled_poses_are_frozen() {
    let clip = wave_clip();
    // On a frame, between frames, across the sign flip, at the loop seam, several loops later,
    // and before the clip starts.
    let times = [
        0.0,
        0.5 / 24.0,
        1.0 / 24.0,
        1.5 / 24.0,
        2.0 / 24.0,
        2.5 / 24.0,
        3.0 / 24.0,
        10.5 / 24.0,
        -0.5 / 24.0,
    ];
    let hashes: Vec<u64> = times
        .iter()
        .map(|&time| hash_poses(&sample_pose(&clip, time)))
        .collect();
    assert_eq!(
        hashes,
        vec![
            0x1a81_069c_843e_3ee8,
            0xd52b_e904_3cdd_9fea,
            0x5acd_812e_0ed4_f21f,
            0x3a0d_5a12_7277_ab61,
            0xf53d_52b9_5de6_ba3f,
            0xc6d9_792f_c94c_9b9e,
            0x1a81_069c_843e_3ee8,
            0x3a0d_5a12_7277_ab61,
            0xc6d9_792f_c94c_9b9e,
        ]
    );
}

#[test]
fn golden_crossfade_poses_are_frozen() {
    let clip = wave_clip();
    let once = decode_clip(&rest_pose_clip_bytes()).expect("the rest-pose fixture decodes");
    let hashes: Vec<u64> = [0.0f32, 0.25, 0.5, 0.75, 1.0]
        .iter()
        .map(|&weight| {
            let mut out = Vec::new();
            crossfade_pose_into(&clip, 1.5 / 24.0, &once, 0.0, weight, &mut out)
                .expect("both fixtures pose two joints");
            hash_poses(&out)
        })
        .collect();
    assert_eq!(
        hashes,
        vec![
            0x3a0d_5a12_7277_ab61,
            0x5650_1fe7_6229_d867,
            0x9d9d_669b_3b41_3d0d,
            0x8465_66fb_7e14_a556,
            0x1a81_069c_843e_3ee8,
        ]
    );
}

// --- Crossfade --------------------------------------------------------------------------------

#[test]
fn crossfade_endpoints_are_the_plain_samples_bit_for_bit() {
    let first = wave_clip();
    let second = decode_clip(&rest_pose_clip_bytes()).expect("the rest-pose fixture decodes");
    let mut out = Vec::new();

    crossfade_pose_into(&first, 1.5 / 24.0, &second, 0.0, 0.0, &mut out).unwrap();
    assert_eq!(out, sample_pose(&first, 1.5 / 24.0));

    crossfade_pose_into(&first, 1.5 / 24.0, &second, 0.0, 1.0, &mut out).unwrap();
    assert_eq!(out, sample_pose(&second, 0.0));
}

#[test]
fn crossfade_clamps_a_weight_outside_the_unit_range_and_ignores_a_non_finite_one() {
    let first = wave_clip();
    let second = decode_clip(&rest_pose_clip_bytes()).expect("the rest-pose fixture decodes");
    let mut out = Vec::new();

    crossfade_pose_into(&first, 0.0, &second, 0.0, 4.0, &mut out).unwrap();
    assert_eq!(out, sample_pose(&second, 0.0));
    crossfade_pose_into(&first, 0.0, &second, 0.0, -4.0, &mut out).unwrap();
    assert_eq!(out, sample_pose(&first, 0.0));
    crossfade_pose_into(&first, 0.0, &second, 0.0, f32::NAN, &mut out).unwrap();
    assert_eq!(out, sample_pose(&first, 0.0));
}

#[test]
fn crossfade_rejects_clips_with_different_joint_counts() {
    let first = wave_clip();
    let one_joint = decode_clip(&once_clip_bytes()).expect("the once fixture decodes");
    let mut out = Vec::new();
    assert_eq!(
        crossfade_pose_into(&first, 0.0, &one_joint, 0.0, 0.5, &mut out),
        Err(FigureFormatError::ClipJointCountMismatch {
            clip: 2,
            skeleton: 1,
        })
    );
}

#[test]
fn crossfade_halfway_between_a_pose_and_itself_is_that_pose() {
    let clip = wave_clip();
    let mut out = Vec::new();
    crossfade_pose_into(&clip, 1.0 / 24.0, &clip, 1.0 / 24.0, 0.5, &mut out).unwrap();
    let plain = sample_pose(&clip, 1.0 / 24.0);
    for (blended, expected) in out.iter().zip(plain.iter()) {
        assert_eq!(blended.translation, expected.translation);
        assert_eq!(blended.scale, expected.scale);
        // The rotation goes through a normalisation, so it may differ in the last bit.
        for axis in 0..4 {
            assert!((blended.rotation[axis] - expected.rotation[axis]).abs() < 1e-6);
        }
    }
}

// --- Time warping -----------------------------------------------------------------------------

fn anchors() -> Vec<TimeAnchor> {
    // A clip whose "hit" sits at 1/24 s, stretched so the hit lands at 0.2 s and the clip ends at
    // 0.5 s: the first segment plays at roughly a fifth speed, the second faster.
    vec![
        TimeAnchor::new(0.0, 0.0),
        TimeAnchor::new(1.0 / 24.0, 0.2),
        TimeAnchor::new(2.0 / 24.0, 0.5),
    ]
}

#[test]
fn warp_hits_every_anchor_exactly() {
    let anchors = anchors();
    for anchor in &anchors {
        assert_eq!(
            warped_clip_time(&anchors, anchor.target_time),
            Ok(anchor.clip_time),
            "anchor at target {} must map back exactly",
            anchor.target_time
        );
    }
}

#[test]
fn warp_puts_a_clip_marker_on_the_requested_time() {
    let clip = wave_clip();
    let hit = clip
        .marker_time("hit")
        .expect("the fixture has a hit marker");
    let anchors = [
        TimeAnchor::new(0.0, 0.0),
        TimeAnchor::new(hit, 0.35),
        TimeAnchor::new(clip.duration_seconds(), 0.5),
    ];
    // Sampling the warped clip at the requested time gives exactly the marked frame's keys.
    let warped = warped_clip_time(&anchors, 0.35).expect("valid anchors");
    assert_eq!(warped, hit);
    assert_eq!(sample_pose(&clip, warped)[1].rotation, QUARTER_TURN_X);
}

#[test]
fn warp_is_monotone_and_continuous() {
    let anchors = anchors();
    let mut previous = f32::NEG_INFINITY;
    for step in -20..=120 {
        let target = step as f32 * 0.005;
        let clip_time = warped_clip_time(&anchors, target).expect("valid anchors");
        assert!(
            clip_time > previous,
            "warp went backwards at target {target}: {clip_time} after {previous}"
        );
        previous = clip_time;
    }
}

#[test]
fn warp_extrapolates_outside_the_anchor_range_at_the_nearest_segment_rate() {
    let anchors = anchors();
    // The first segment maps 0.2 s of target onto 1/24 s of clip.
    let before = warped_clip_time(&anchors, -0.2).expect("valid anchors");
    assert!((before + 1.0 / 24.0).abs() < 1e-6, "got {before}");
    // The last segment maps 0.3 s of target onto 1/24 s of clip.
    let after = warped_clip_time(&anchors, 0.8).expect("valid anchors");
    assert!((after - 3.0 / 24.0).abs() < 1e-6, "got {after}");
}

#[test]
fn warp_rejects_anchors_it_cannot_build_a_monotone_mapping_from() {
    let single = [TimeAnchor::new(0.0, 0.0)];
    assert!(matches!(
        warped_clip_time(&single, 0.0),
        Err(FigureFormatError::InvalidTimeAnchors { .. })
    ));
    assert!(matches!(
        warped_clip_time(&[], 0.0),
        Err(FigureFormatError::InvalidTimeAnchors { .. })
    ));

    let backwards = [TimeAnchor::new(0.0, 0.0), TimeAnchor::new(-1.0, 1.0)];
    assert_eq!(
        warped_clip_time(&backwards, 0.5),
        Err(FigureFormatError::InvalidTimeAnchors {
            reason: "clip_time must strictly increase",
        })
    );

    let stalled = [TimeAnchor::new(0.0, 0.0), TimeAnchor::new(1.0, 0.0)];
    assert_eq!(
        warped_clip_time(&stalled, 0.5),
        Err(FigureFormatError::InvalidTimeAnchors {
            reason: "target_time must strictly increase",
        })
    );

    let infinite = [
        TimeAnchor::new(0.0, 0.0),
        TimeAnchor::new(f32::INFINITY, 1.0),
    ];
    assert_eq!(
        warped_clip_time(&infinite, 0.5),
        Err(FigureFormatError::InvalidTimeAnchors {
            reason: "every anchor time must be finite",
        })
    );
}

#[test]
fn warp_of_a_non_finite_target_falls_back_to_the_first_anchor() {
    let anchors = anchors();
    assert_eq!(warped_clip_time(&anchors, f32::NAN), Ok(0.0));
}

// --- Palette ----------------------------------------------------------------------------------

#[test]
fn a_rest_pose_clip_composes_to_the_identity_palette() {
    let skeleton = ribbon_skeleton();
    let clip = decode_clip(&rest_pose_clip_bytes()).expect("the rest-pose fixture decodes");
    let mut sampler = ClipSampler::new();
    sampler.sample(&clip, 0.0);
    let matrices = sampler.skin_matrices(&skeleton).expect("matching counts");
    assert_eq!(matrices.len(), 2);
    for matrix in matrices {
        for column in 0..4 {
            for row in 0..4 {
                assert!(
                    (matrix[column][row] - IDENTITY_MATRIX[column][row]).abs() < 1e-6,
                    "matrix {matrix:?} is not the identity"
                );
            }
        }
    }
}

#[test]
fn the_palette_matches_compute_skin_matrices_for_the_same_pose() {
    let skeleton = ribbon_skeleton();
    let clip = wave_clip();
    let mut sampler = ClipSampler::new();
    sampler.sample(&clip, 1.0 / 24.0);
    let expected = crate::figure_format::compute_skin_matrices(&skeleton, sampler.pose())
        .expect("matching counts");
    let matrices = sampler.skin_matrices(&skeleton).expect("matching counts");
    assert_eq!(matrices, expected.as_slice());
}

#[test]
fn the_palette_bends_the_limb_around_its_own_pivot() {
    let skeleton = ribbon_skeleton();
    let clip = wave_clip();
    let mut sampler = ClipSampler::new();
    // Frame 1: the limb is a quarter turn about X, the root lifted by 0.5 in Y.
    sampler.sample(&clip, 1.0 / 24.0);
    let matrices = sampler
        .skin_matrices(&skeleton)
        .expect("matching counts")
        .to_vec();
    // A vertex at the limb's own pivot (bind space [0, 0, 1]) only follows the root's lift.
    let pivot = apply(matrices[1], [0.0, 0.0, 1.0]);
    assert_point_close(pivot, [0.0, 0.5, 1.0], 1e-5);
    // A vertex one unit beyond the pivot swings out along -Y, then follows the root's lift.
    let tip = apply(matrices[1], [0.0, 0.0, 2.0]);
    assert_point_close(tip, [0.0, -0.5, 1.0], 1e-4);
}

fn apply(matrix: [[f32; 4]; 4], point: [f32; 3]) -> [f32; 3] {
    let [x, y, z] = point;
    [
        matrix[0][0] * x + matrix[1][0] * y + matrix[2][0] * z + matrix[3][0],
        matrix[0][1] * x + matrix[1][1] * y + matrix[2][1] * z + matrix[3][1],
        matrix[0][2] * x + matrix[1][2] * y + matrix[2][2] * z + matrix[3][2],
    ]
}

fn assert_point_close(actual: [f32; 3], expected: [f32; 3], tolerance: f32) {
    for axis in 0..3 {
        assert!(
            (actual[axis] - expected[axis]).abs() < tolerance,
            "point mismatch: actual {actual:?}, expected {expected:?}"
        );
    }
}

#[test]
fn a_reused_sampler_produces_the_same_bytes_as_a_fresh_one() {
    // The point of `ClipSampler` is that it owns buffers, not state: whatever it sampled before
    // must not change what it samples now.
    let clip = wave_clip();
    let skeleton = ribbon_skeleton();
    let mut reused = ClipSampler::new();
    reused.sample(&clip, 1.5 / 24.0);
    let _ = reused.skin_matrices(&skeleton).unwrap();
    reused
        .sample_crossfade(&clip, 0.0, &clip, 1.0 / 24.0, 0.3)
        .unwrap();
    let _ = reused.skin_matrices(&skeleton).unwrap();
    reused.sample(&clip, 0.5 / 24.0);
    let reused_matrices = reused.skin_matrices(&skeleton).unwrap().to_vec();

    let mut fresh = ClipSampler::new();
    fresh.sample(&clip, 0.5 / 24.0);
    let fresh_matrices = fresh.skin_matrices(&skeleton).unwrap().to_vec();
    assert_eq!(reused_matrices, fresh_matrices);
    assert_eq!(hash_poses(reused.pose()), hash_poses(fresh.pose()));
}

#[test]
fn the_sampler_rejects_a_skeleton_the_pose_does_not_fit() {
    let clip = wave_clip();
    let mut sampler = ClipSampler::new();
    sampler.sample(&clip, 0.0);
    let one_joint = SkeletonData {
        joints: vec![ribbon_skeleton().joints.remove(0)],
    };
    assert_eq!(
        sampler.skin_matrices(&one_joint).map(<[_]>::to_vec),
        Err(FigureFormatError::ClipJointCountMismatch {
            clip: 2,
            skeleton: 1,
        })
    );
}

#[test]
fn the_sampler_crossfade_matches_the_free_function() {
    let clip = wave_clip();
    let rest = decode_clip(&rest_pose_clip_bytes()).expect("the rest-pose fixture decodes");
    let mut sampler = ClipSampler::new();
    sampler
        .sample_crossfade(&clip, 1.5 / 24.0, &rest, 0.0, 0.4)
        .unwrap();
    let mut free = Vec::new();
    crossfade_pose_into(&clip, 1.5 / 24.0, &rest, 0.0, 0.4, &mut free).unwrap();
    assert_eq!(sampler.pose(), free.as_slice());
}

// --- Decoding a skeleton payload and fingerprinting it ----------------------------------------

#[test]
fn a_decoded_skeleton_fingerprints_the_same_as_a_hand_built_one() {
    // Guards the facade path: the fingerprint a converter computes from the payload bytes and the
    // one the engine computes from the decoded skeleton must agree.
    let mut buf = Vec::new();
    buf.extend_from_slice(&1u32.to_le_bytes()); // FORMAT_VERSION
    buf.extend_from_slice(&2u32.to_le_bytes()); // joint_count
    for (parent, name, bind_z) in [(-1i32, "root", 0.0f32), (0, "limb", -1.0)] {
        buf.extend_from_slice(&parent.to_le_bytes());
        push_f32s(&mut buf, &[1.0, 0.0, 0.0, 0.0]);
        push_f32s(&mut buf, &[0.0, 1.0, 0.0, 0.0]);
        push_f32s(&mut buf, &[0.0, 0.0, 1.0, 0.0]);
        push_f32s(&mut buf, &[0.0, 0.0, bind_z, 1.0]);
        buf.push(u8::try_from(name.len()).unwrap());
        buf.extend_from_slice(name.as_bytes());
    }
    push_f32s(&mut buf, &[0.0, 0.0, 0.0]);
    push_f32s(&mut buf, &IDENTITY_QUAT);
    push_f32s(&mut buf, &[1.0, 1.0, 1.0]);
    push_f32s(&mut buf, &[0.0, 0.0, 1.0]);
    push_f32s(&mut buf, &IDENTITY_QUAT);
    push_f32s(&mut buf, &[1.0, 1.0, 1.0]);

    let decoded = decode_skeleton(&buf).expect("valid skeleton payload");
    assert_eq!(skeleton_fingerprint(&decoded), ribbon_fingerprint());
}

// --- Property tests (contract §2 rule 9) -------------------------------------------------------

mod property_tests {
    use super::*;
    use proptest::prelude::*;

    /// Decoding must never panic, and whatever decodes must then sample, crossfade and compose
    /// without panicking either — a clip that decodes is a clip the renderer will use.
    fn decode_and_use(bytes: &[u8]) {
        let Ok(clip) = decode_clip(bytes) else {
            return;
        };
        let mut sampler = ClipSampler::new();
        for time in [
            0.0f32,
            0.5,
            -1.0,
            1e9,
            f32::NAN,
            f32::INFINITY,
            clip.duration_seconds(),
        ] {
            sampler.sample(&clip, time);
            assert_eq!(sampler.pose().len(), clip.joint_count() as usize);
            for pose in sampler.pose() {
                // Rotations are always finite and unit, whatever the stored keys are: they are
                // bounded by the norm check at decode time, and `nlerp_shortest` falls back to
                // the identity if a normalisation ever degenerates. Translation and scale are
                // only as finite as the content: two keys at the f32 extremes can overflow when
                // interpolated, which no authored clip does but arbitrary bytes can.
                for value in pose.rotation {
                    assert!(
                        value.is_finite(),
                        "a decoded clip sampled to a non-finite rotation"
                    );
                }
            }
            let _ = sampler.sample_crossfade(&clip, time, &clip, -time, 0.5);
        }
    }

    proptest! {
        /// Arbitrary bytes, entirely unrelated to this format, must never panic the decoder.
        #[test]
        fn decode_clip_never_panics_on_arbitrary_bytes(
            bytes in prop::collection::vec(any::<u8>(), 0..1024)
        ) {
            decode_and_use(&bytes);
        }

        /// Arbitrary bytes behind a valid magic and version get much further into the decoder
        /// than random noise ever would, which is where the count and limit checks live.
        #[test]
        fn decode_clip_never_panics_on_arbitrary_bytes_behind_a_valid_header(
            bytes in prop::collection::vec(any::<u8>(), 0..1024)
        ) {
            let mut input = Vec::with_capacity(bytes.len() + 12);
            input.extend_from_slice(&CLIP_MAGIC);
            input.extend_from_slice(&CLIP_FORMAT_VERSION.to_le_bytes());
            input.extend_from_slice(&bytes);
            decode_and_use(&input);
        }

        /// Flipping any single byte of the valid fixture must never panic the decoder.
        #[test]
        fn decode_clip_never_panics_on_single_byte_mutation(
            index in 0usize..256, new_byte in any::<u8>()
        ) {
            let mut bytes = wave_clip_bytes();
            if index < bytes.len() {
                bytes[index] = new_byte;
            }
            decode_and_use(&bytes);
        }

        /// Truncating the valid fixture to any shorter length must never panic the decoder.
        #[test]
        fn decode_clip_never_panics_on_truncation(len in 0usize..256) {
            let bytes = wave_clip_bytes();
            decode_and_use(&bytes[..len.min(bytes.len())]);
        }

        /// Sampling a valid clip at any time whatsoever yields a finite pose of the right length.
        #[test]
        fn sampling_any_time_yields_a_finite_pose(time in any::<f32>()) {
            let clip = wave_clip();
            let pose = sample_pose(&clip, time);
            prop_assert_eq!(pose.len(), 2);
            for joint in &pose {
                for value in joint.rotation {
                    prop_assert!(value.is_finite());
                }
            }
            // A sampled rotation is always a unit quaternion, whatever the time.
            let [x, y, z, w] = pose[1].rotation;
            let norm = (x * x + y * y + z * z + w * w).sqrt();
            prop_assert!((norm - 1.0).abs() < 1e-5, "norm was {}", norm);
        }

        /// The warp maps every anchor back exactly, whatever the anchors are.
        #[test]
        fn warp_hits_arbitrary_anchors_exactly(
            first in -100.0f32..100.0, clip_gap in 0.01f32..10.0,
            target_start in -100.0f32..100.0, target_gap in 0.01f32..10.0,
        ) {
            let anchors = [
                TimeAnchor::new(first, target_start),
                TimeAnchor::new(first + clip_gap, target_start + target_gap),
                TimeAnchor::new(first + 2.0 * clip_gap, target_start + 3.0 * target_gap),
            ];
            for anchor in &anchors {
                prop_assert_eq!(
                    warped_clip_time(&anchors, anchor.target_time),
                    Ok(anchor.clip_time)
                );
            }
        }
    }
}
