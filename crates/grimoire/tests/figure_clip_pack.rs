//! Engine ADR-0017: loading an `FNP_CLIP` entry out of a pack through the facade and drawing the
//! pose it samples, end to end.
//!
//! `tests/figure_pack.rs` already covers the static half of this path (mesh, material, skeleton,
//! figure manifest). This file adds the animated half and answers the one question the unit tests
//! in `grimoire_render::figure_clip` cannot: does a sampled pose actually reach the GPU, or does it
//! stop at a `Vec` of matrices nobody draws?
//!
//! **No new reference image.** ADR-0017 asked for an offscreen scene with reference images per
//! driver family; this file instead renders the *same* figure twice from the same camera, once at a
//! clip time whose pose is the rest pose and once at a clip time whose pose is a 90-degree bend,
//! and asserts the two images differ far beyond the render-comparison tolerance of OF-18.2
//! (mean ≤ 3.0, max ≤ 60). That is the "Gegenfall" the ADR's own test list asks for — proof the
//! scene would notice a pose that was not applied — without freezing a Windows and a Linux PNG for
//! a scene whose only new logic is arithmetic the unit tests already pin to frozen hashes. The
//! pixels themselves stay covered by the existing skinning scenes.
//!
//! **Fixtures are hand-built** (see `grimoire_render::figure_clip`'s test module for the full
//! derivation): a two-joint ribbon rig and two clips written byte by byte from the layout table in
//! `docs/formats/figure-clip.md`. The product owner's authored figures live outside both
//! repositories and are deliberately not test data.

use std::sync::Arc;

use grimoire::adapters::figure_assets::{
    self, FNP_CLIP, FNP_FIGURE, FNP_MATERIAL, FNP_MESH, FNP_SKELETON,
};
use grimoire_assets::{AssetId, AssetPath, AssetStore, PackReader, PackWriter};
use grimoire_render::figure_clip::{self, ClipSampler};
use grimoire_render::figure_format;
use grimoire_render::{
    AmbientLight, Camera25D, DirectionalLight, MaterialHandle, MeshInstance, RenderError, Renderer,
    RendererConfig, SkinBinding, StageFrame, WgpuRenderer,
};

const FIGURE_NAME: &str = "clipfig";
const BEND_CLIP: &str = "bend";
const REST_CLIP: &str = "rest";
const RATE_HZ: f32 = 24.0;

// --- Byte helpers ------------------------------------------------------------------------------

fn push_f32(buf: &mut Vec<u8>, value: f32) {
    buf.extend_from_slice(&value.to_le_bytes());
}
fn push_f32s(buf: &mut Vec<u8>, values: &[f32]) {
    for &value in values {
        push_f32(buf, value);
    }
}
fn push_u16(buf: &mut Vec<u8>, value: u16) {
    buf.extend_from_slice(&value.to_le_bytes());
}
fn push_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_le_bytes());
}
fn push_u64(buf: &mut Vec<u8>, value: u64) {
    buf.extend_from_slice(&value.to_le_bytes());
}
fn push_i32(buf: &mut Vec<u8>, value: i32) {
    buf.extend_from_slice(&value.to_le_bytes());
}

const IDENTITY_QUAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
/// A 90-degree turn about X, the bend the clip's second frame holds.
fn quarter_turn_x() -> [f32; 4] {
    let half = std::f32::consts::FRAC_1_SQRT_2;
    [half, 0.0, 0.0, half]
}

// --- Figure payloads (the ribbon of `tests/figure_pack.rs`, wide enough to see bend) ------------

fn mesh_payload_bytes() -> Vec<u8> {
    let mut buf = Vec::new();
    push_u32(&mut buf, figure_format::FORMAT_VERSION);
    push_u32(&mut buf, 4); // vertex_count
    push_u32(&mut buf, 6); // index_count
    let half_width = 0.35f32;
    type RawVertex = ([f32; 3], [f32; 3], [f32; 2], [u16; 4], [f32; 4]);
    let vertices: [RawVertex; 4] = [
        (
            [-half_width, 0.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0],
            [0, 0, 0, 0],
            [1.0, 0.0, 0.0, 0.0],
        ),
        (
            [half_width, 0.0, 0.0],
            [0.0, -1.0, 0.0],
            [1.0, 0.0],
            [0, 0, 0, 0],
            [1.0, 0.0, 0.0, 0.0],
        ),
        (
            [-half_width, 0.0, 2.0],
            [0.0, -1.0, 0.0],
            [0.0, 1.0],
            [1, 0, 0, 0],
            [1.0, 0.0, 0.0, 0.0],
        ),
        (
            [half_width, 0.0, 2.0],
            [0.0, -1.0, 0.0],
            [1.0, 1.0],
            [1, 0, 0, 0],
            [1.0, 0.0, 0.0, 0.0],
        ),
    ];
    for (position, normal, uv, joints, weights) in vertices {
        push_f32s(&mut buf, &position);
        push_f32s(&mut buf, &normal);
        push_f32s(&mut buf, &uv);
        for joint in joints {
            push_u16(&mut buf, joint);
        }
        push_f32s(&mut buf, &weights);
    }
    for index in [0u32, 1, 2, 1, 3, 2] {
        push_u32(&mut buf, index);
    }
    buf
}

fn material_payload_bytes() -> Vec<u8> {
    let mut buf = Vec::new();
    push_u32(&mut buf, figure_format::FORMAT_VERSION);
    push_f32s(&mut buf, &[0.75, 0.55, 0.35, 1.0]);
    push_f32(&mut buf, 0.0);
    push_f32(&mut buf, 0.7);
    push_f32s(&mut buf, &[0.0, 0.0, 0.0]);
    buf.push(0); // opaque
    push_f32(&mut buf, 0.0);
    push_u32(&mut buf, 0xFFFF_FFFF);
    push_u32(&mut buf, 0xFFFF_FFFF);
    push_u32(&mut buf, 0xFFFF_FFFF);
    buf
}

/// `root` at the origin, `limb` bound at `[0, 0, 1]`, its inverse bind matrix the exact inverse of
/// that translation.
fn skeleton_payload_bytes() -> Vec<u8> {
    let mut buf = Vec::new();
    push_u32(&mut buf, figure_format::FORMAT_VERSION);
    push_u32(&mut buf, 2);

    push_i32(&mut buf, -1);
    push_f32s(&mut buf, &[1.0, 0.0, 0.0, 0.0]);
    push_f32s(&mut buf, &[0.0, 1.0, 0.0, 0.0]);
    push_f32s(&mut buf, &[0.0, 0.0, 1.0, 0.0]);
    push_f32s(&mut buf, &[0.0, 0.0, 0.0, 1.0]);
    buf.push(4);
    buf.extend_from_slice(b"root");

    push_i32(&mut buf, 0);
    push_f32s(&mut buf, &[1.0, 0.0, 0.0, 0.0]);
    push_f32s(&mut buf, &[0.0, 1.0, 0.0, 0.0]);
    push_f32s(&mut buf, &[0.0, 0.0, 1.0, 0.0]);
    push_f32s(&mut buf, &[0.0, 0.0, -1.0, 1.0]);
    buf.push(4);
    buf.extend_from_slice(b"limb");

    push_f32s(&mut buf, &[0.0, 0.0, 0.0]);
    push_f32s(&mut buf, &IDENTITY_QUAT);
    push_f32s(&mut buf, &[1.0, 1.0, 1.0]);
    push_f32s(&mut buf, &[0.0, 0.0, 1.0]);
    push_f32s(&mut buf, &IDENTITY_QUAT);
    push_f32s(&mut buf, &[1.0, 1.0, 1.0]);
    buf
}

fn figure_manifest_bytes(mesh_id: u64, material_id: u64, skeleton_id: u64) -> Vec<u8> {
    let mut buf = Vec::new();
    push_u32(&mut buf, figure_format::FORMAT_VERSION);
    push_u32(&mut buf, 1);
    push_u64(&mut buf, mesh_id);
    push_u64(&mut buf, material_id);
    push_u32(&mut buf, 0); // texture_count
    push_u64(&mut buf, skeleton_id);
    push_f32s(&mut buf, &[-0.35, -1.0, 0.0]);
    push_f32s(&mut buf, &[0.35, 1.0, 2.0]);
    buf
}

// --- Clip payloads -----------------------------------------------------------------------------

/// The skeleton fingerprint of the payload above, computed through the decoder so the fixture and
/// the engine agree by construction rather than by a copied number.
fn fixture_fingerprint() -> u64 {
    let skeleton = figure_format::decode_skeleton(&skeleton_payload_bytes())
        .expect("the fixture skeleton decodes");
    figure_clip::skeleton_fingerprint(&skeleton)
}

fn clip_header(frame_count: u32, looping: bool, fingerprint: u64) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&figure_clip::CLIP_MAGIC);
    push_u32(&mut buf, figure_clip::CLIP_FORMAT_VERSION);
    push_u32(&mut buf, u32::from(looping));
    push_u32(&mut buf, 2); // joint_count
    push_u32(&mut buf, frame_count);
    push_f32(&mut buf, RATE_HZ);
    push_u64(&mut buf, fingerprint);
    push_u32(&mut buf, 0); // marker_count
    buf
}

fn push_constant_track(buf: &mut Vec<u8>, value: &[f32]) {
    buf.push(0);
    push_f32s(buf, value);
}

fn push_sampled_track(buf: &mut Vec<u8>, keys: &[&[f32]]) {
    buf.push(1);
    for key in keys {
        push_f32s(buf, key);
    }
}

/// Two frames: frame 0 is exactly the skeleton's rest pose, frame 1 bends `limb` by 90 degrees
/// about X. Not looping, so sampling past the end clamps to the bend.
fn bend_clip_bytes(fingerprint: u64) -> Vec<u8> {
    let mut buf = clip_header(2, false, fingerprint);
    // Joint 0: rest throughout.
    push_constant_track(&mut buf, &[0.0, 0.0, 0.0]);
    push_constant_track(&mut buf, &IDENTITY_QUAT);
    push_constant_track(&mut buf, &[1.0, 1.0, 1.0]);
    // Joint 1: rest, then bent.
    push_constant_track(&mut buf, &[0.0, 0.0, 1.0]);
    push_sampled_track(&mut buf, &[&IDENTITY_QUAT, &quarter_turn_x()]);
    push_constant_track(&mut buf, &[1.0, 1.0, 1.0]);
    buf
}

/// One frame, the skeleton's rest pose: every skinning matrix must come out as the identity.
fn rest_clip_bytes(fingerprint: u64) -> Vec<u8> {
    let mut buf = clip_header(1, false, fingerprint);
    push_constant_track(&mut buf, &[0.0, 0.0, 0.0]);
    push_constant_track(&mut buf, &IDENTITY_QUAT);
    push_constant_track(&mut buf, &[1.0, 1.0, 1.0]);
    push_constant_track(&mut buf, &[0.0, 0.0, 1.0]);
    push_constant_track(&mut buf, &IDENTITY_QUAT);
    push_constant_track(&mut buf, &[1.0, 1.0, 1.0]);
    buf
}

// --- The pack ----------------------------------------------------------------------------------

/// Builds the fixture pack. `clip_fingerprint` is a parameter so one test can write a clip that
/// claims a *different* rig and prove the facade rejects it.
fn write_fixture_pack(clip_fingerprint: u64) -> Vec<u8> {
    let mesh_path = AssetPath::new(&format!("figures/{FIGURE_NAME}/mesh/0")).unwrap();
    let material_path = AssetPath::new(&format!("figures/{FIGURE_NAME}/material/0")).unwrap();
    let skeleton_path = AssetPath::new(&format!("figures/{FIGURE_NAME}/skeleton")).unwrap();
    let figure_path = AssetPath::new(&format!("figures/{FIGURE_NAME}/figure")).unwrap();
    let bend_path = AssetPath::new(&format!("figures/{FIGURE_NAME}/clip/{BEND_CLIP}")).unwrap();
    let rest_path = AssetPath::new(&format!("figures/{FIGURE_NAME}/clip/{REST_CLIP}")).unwrap();

    let mut writer = PackWriter::new("figure_clip_test_fixture", "0.0.0");
    let mesh_id = writer
        .add(&mesh_path, FNP_MESH, 1, &mesh_payload_bytes())
        .unwrap();
    let material_id = writer
        .add(&material_path, FNP_MATERIAL, 1, &material_payload_bytes())
        .unwrap();
    let skeleton_id = writer
        .add(&skeleton_path, FNP_SKELETON, 1, &skeleton_payload_bytes())
        .unwrap();
    writer
        .add(
            &figure_path,
            FNP_FIGURE,
            1,
            &figure_manifest_bytes(mesh_id.0, material_id.0, skeleton_id.0),
        )
        .unwrap();
    writer
        .add(
            &bend_path,
            FNP_CLIP,
            figure_clip::CLIP_FORMAT_VERSION,
            &bend_clip_bytes(clip_fingerprint),
        )
        .unwrap();
    writer
        .add(
            &rest_path,
            FNP_CLIP,
            figure_clip::CLIP_FORMAT_VERSION,
            &rest_clip_bytes(clip_fingerprint),
        )
        .unwrap();
    writer.finish().unwrap()
}

fn fixture_store(clip_fingerprint: u64) -> AssetStore {
    let reader = PackReader::from_bytes(Arc::from(write_fixture_pack(clip_fingerprint)))
        .expect("valid pack");
    AssetStore::new(Box::new(reader))
}

// --- Loading -----------------------------------------------------------------------------------

#[test]
fn load_clip_round_trips_a_hand_written_pack_entry() {
    let skeleton = figure_format::decode_skeleton(&skeleton_payload_bytes()).unwrap();
    let mut store = fixture_store(fixture_fingerprint());
    let clip =
        figure_assets::load_clip(&mut store, FIGURE_NAME, BEND_CLIP, &skeleton).expect("loads");
    assert_eq!(clip.joint_count(), 2);
    assert_eq!(clip.frame_count(), 2);
    assert_eq!(clip.frame_rate_hz(), RATE_HZ);
    assert!(!clip.is_looping());
    assert_eq!(clip.markers(), &[]);
    assert_eq!(clip.skeleton_fingerprint(), fixture_fingerprint());
}

#[test]
fn load_clip_rejects_a_clip_authored_for_another_rig() {
    // Same joint count, same byte length, different hierarchy: only the fingerprint separates
    // them, and without it the figure would pose into nonsense instead of failing.
    let skeleton = figure_format::decode_skeleton(&skeleton_payload_bytes()).unwrap();
    let mut store = fixture_store(fixture_fingerprint() ^ 1);
    let error = figure_assets::load_clip(&mut store, FIGURE_NAME, BEND_CLIP, &skeleton)
        .expect_err("a clip for another rig must not load");
    let message = error.to_string();
    assert!(
        message.contains("authored for skeleton"),
        "expected a fingerprint mismatch, got: {message}"
    );
}

#[test]
fn load_clip_reports_a_missing_clip_instead_of_panicking() {
    let skeleton = figure_format::decode_skeleton(&skeleton_payload_bytes()).unwrap();
    let mut store = fixture_store(fixture_fingerprint());
    assert!(figure_assets::load_clip(&mut store, FIGURE_NAME, "nope", &skeleton).is_err());
}

#[test]
fn load_clip_reports_a_kind_mismatch_instead_of_decoding_the_wrong_entry() {
    // The skeleton entry exists at a path a clip could be asked for; the store checks the kind
    // before it ever hands bytes to `decode_clip`.
    let mut store = fixture_store(fixture_fingerprint());
    let skeleton_id = AssetId::from_path(
        &AssetPath::new(&format!("figures/{FIGURE_NAME}/skeleton")).expect("valid path"),
    );
    let error = store
        .load(skeleton_id, FNP_CLIP, figure_clip::decode_clip)
        .expect_err("a skeleton entry is not a clip");
    let message = error.to_string();
    assert!(
        message.contains("kind"),
        "expected a kind mismatch, got: {message}"
    );
}

#[test]
fn a_rest_pose_clip_loaded_from_a_pack_composes_to_the_identity_palette() {
    let mut store = fixture_store(fixture_fingerprint());
    let skeleton = figure_format::decode_skeleton(&skeleton_payload_bytes()).unwrap();
    let clip =
        figure_assets::load_clip(&mut store, FIGURE_NAME, REST_CLIP, &skeleton).expect("loads");
    let mut sampler = ClipSampler::new();
    sampler.sample(&clip, 0.0);
    let matrices = sampler.skin_matrices(&skeleton).expect("matching counts");
    let rest = figure_format::rest_pose_skin_matrices(&skeleton);
    assert_eq!(matrices, rest.as_slice());
}

/// The whole facade path as a game would walk it: a plugin loads figure and clip inside
/// `register_assets` (contract §9.10) and keeps the sampled palette, with no GPU and no animation
/// system anywhere in the simulation.
#[test]
fn a_plugin_loads_a_figure_and_its_clip_through_the_asset_hook() {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    use grimoire::prelude::*;
    use grimoire::{PluginError, RenderAssets};

    type Palette = Rc<RefCell<Vec<[[f32; 4]; 4]>>>;

    struct ClipPlugin {
        palette: Palette,
    }

    impl GamePlugin for ClipPlugin {
        fn name(&self) -> &str {
            "clipfigure"
        }

        fn register_assets(&mut self, assets: &mut dyn RenderAssets) -> Result<(), PluginError> {
            let mut store = fixture_store(fixture_fingerprint());
            let figure = figure_assets::load_figure_into(&mut store, assets, FIGURE_NAME)?;
            let clip =
                figure_assets::load_clip(&mut store, FIGURE_NAME, BEND_CLIP, &figure.skeleton)?;
            let mut sampler = ClipSampler::new();
            // The bend clip's last frame; a non-looping clip clamps there.
            sampler.sample(&clip, 1.0 / RATE_HZ);
            *self.palette.borrow_mut() = sampler.skin_matrices(&figure.skeleton)?.to_vec();
            Ok(())
        }
    }

    let palette = Palette::default();
    App::new(WindowConfig::default())
        .plugin(ClipPlugin {
            palette: Rc::clone(&palette),
        })
        .run_headless_frames(1, Duration::from_millis(16))
        .expect("the fixture figure and its clip register");

    let palette = palette.borrow();
    assert_eq!(palette.len(), 2);
    // The root was never posed, so its skinning matrix is still the identity; the limb's is not.
    let identity = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    for column in 0..4 {
        for row in 0..4 {
            assert!((palette[0][column][row] - identity[column][row]).abs() < 1e-6);
        }
    }
    assert_ne!(palette[1], identity);
}

// --- Offscreen: does the sampled pose reach the GPU? -------------------------------------------

static GPU_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu_serial() -> std::sync::MutexGuard<'static, ()> {
    GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;

fn offscreen_renderer() -> Option<WgpuRenderer> {
    let config = RendererConfig {
        vsync: false,
        initial_sprite_capacity: 16,
        allow_software_fallback: true,
    };
    match WgpuRenderer::new_offscreen(WIDTH, HEIGHT, config) {
        Ok(renderer) => Some(renderer),
        Err(RenderError::NoAdapter) => None,
        Err(error) => panic!("offscreen renderer creation failed: {error}"),
    }
}

fn render_with_palette(
    renderer: &mut WgpuRenderer,
    figure: &figure_assets::LoadedFigure,
    palette: &[[[f32; 4]; 4]],
) -> Vec<u8> {
    let mut frame = StageFrame::new();
    frame.base.clear_color = [0.03, 0.032, 0.045, 1.0];
    let mut camera = Camera25D::default();
    camera.target = [0.0, 0.0];
    camera.tilt_degrees = 65.0;
    camera.fov_y_degrees = 45.0;
    camera.distance = 4.5;
    frame.camera_25d = Some(camera);

    let mut key_light = DirectionalLight::default();
    key_light.direction = [0.4, 0.6, -0.7];
    key_light.color = [0.85, 0.85, 0.9];
    key_light.intensity = 4.5;
    frame.key_light = Some(key_light);
    frame.ambient = AmbientLight::Hemisphere {
        sky_color: [0.18, 0.19, 0.24],
        ground_color: [0.05, 0.05, 0.06],
        intensity: 1.2,
    };

    frame.joint_matrices = palette.to_vec();
    let mut skin = SkinBinding::default();
    skin.joint_offset = 0;
    skin.joint_count = u32::try_from(palette.len()).expect("two joints");

    for part in &figure.parts {
        frame.materials.push(part.material);
        let material = MaterialHandle(u32::try_from(frame.materials.len() - 1).unwrap());
        let mut instance = MeshInstance::default();
        instance.mesh = part.mesh;
        instance.material = material;
        instance.transform = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        instance.skin = Some(skin);
        frame.meshes.push(instance);
    }

    renderer.render_stage(&frame).expect("render_stage");
    renderer.read_offscreen_rgba().expect("read-back")
}

/// Mean and maximum absolute per-channel difference of two RGBA8 images of the same size, the two
/// numbers the render-comparison rules of OF-18.2 judge a scene by.
fn image_difference(left: &[u8], right: &[u8]) -> (f64, u8) {
    assert_eq!(left.len(), right.len());
    let mut total = 0u64;
    let mut worst = 0u8;
    for (&a, &b) in left.iter().zip(right.iter()) {
        let delta = a.abs_diff(b);
        total += u64::from(delta);
        worst = worst.max(delta);
    }
    (total as f64 / left.len() as f64, worst)
}

/// A pose sampled from a clip reaches the GPU and changes the picture.
///
/// Renders the same figure, same camera, same lights, twice: once with the palette sampled at the
/// clip's rest frame, once at its bent frame. The two images must differ by more than the render
/// comparison tolerance allows (mean ≤ 3.0, max ≤ 60, OF-18.2) — if a future change made
/// `joint_matrices` stop reaching the skinned vertex path, both renders would be the rest pose and
/// the difference would be zero. As a control, rendering the rest palette twice must be bit-equal.
#[test]
fn a_clip_sampled_pose_changes_what_the_renderer_draws() {
    let _serial = gpu_serial();
    let Some(mut renderer) = offscreen_renderer() else {
        eprintln!(
            "no GPU adapter available; skipping a_clip_sampled_pose_changes_what_the_renderer_draws"
        );
        return;
    };
    let mut store = fixture_store(fixture_fingerprint());
    let figure = figure_assets::load_figure(&mut store, &mut renderer, FIGURE_NAME)
        .expect("fixture pack loads cleanly");
    let clip = figure_assets::load_clip(&mut store, FIGURE_NAME, BEND_CLIP, &figure.skeleton)
        .expect("the bend clip loads");

    let mut sampler = ClipSampler::new();
    sampler.sample(&clip, 0.0);
    let rest = sampler.skin_matrices(&figure.skeleton).unwrap().to_vec();
    sampler.sample(&clip, 1.0 / RATE_HZ);
    let bent = sampler.skin_matrices(&figure.skeleton).unwrap().to_vec();
    assert_ne!(rest, bent, "the two clip times must pose differently");

    let rest_image = render_with_palette(&mut renderer, &figure, &rest);
    let rest_again = render_with_palette(&mut renderer, &figure, &rest);
    let bent_image = render_with_palette(&mut renderer, &figure, &bent);

    // Control: the same palette twice is the same picture, so the difference below is the pose and
    // nothing else.
    assert_eq!(
        rest_image, rest_again,
        "the same palette must render bit-equal"
    );

    let (mean, worst) = image_difference(&rest_image, &bent_image);
    assert!(
        mean > 3.0 && worst > 60,
        "a bent pose must change the picture beyond the render tolerance, but the difference was \
         mean {mean:.3}, max {worst}"
    );
}

// --- Property tests (contract §2 rule 9) -------------------------------------------------------

mod property_tests {
    use super::*;
    use proptest::prelude::*;

    fn skeleton() -> figure_format::SkeletonData {
        figure_format::decode_skeleton(&skeleton_payload_bytes()).unwrap()
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 32, .. ProptestConfig::default() })]

        /// Arbitrary bytes through `PackReader` → `AssetStore` → `load_clip` must never panic.
        #[test]
        fn load_clip_never_panics_on_arbitrary_bytes(
            bytes in prop::collection::vec(any::<u8>(), 0..2048)
        ) {
            if let Ok(reader) = PackReader::from_bytes(Arc::from(bytes)) {
                let mut store = AssetStore::new(Box::new(reader));
                let _ = figure_assets::load_clip(&mut store, FIGURE_NAME, BEND_CLIP, &skeleton());
            }
        }

        /// Flipping any single byte of the valid fixture pack must never panic the same chain.
        #[test]
        fn load_clip_never_panics_on_single_byte_mutation(
            index in 0usize..2048, new_byte in any::<u8>()
        ) {
            let mut bytes = write_fixture_pack(fixture_fingerprint());
            if index < bytes.len() {
                bytes[index] = new_byte;
            }
            if let Ok(reader) = PackReader::from_bytes(Arc::from(bytes)) {
                let mut store = AssetStore::new(Box::new(reader));
                let _ = figure_assets::load_clip(&mut store, FIGURE_NAME, BEND_CLIP, &skeleton());
            }
        }

        /// Truncating the valid fixture pack to any shorter length must never panic it either.
        #[test]
        fn load_clip_never_panics_on_truncation(len in 0usize..2048) {
            let bytes = write_fixture_pack(fixture_fingerprint());
            let truncated = bytes[..len.min(bytes.len())].to_vec();
            if let Ok(reader) = PackReader::from_bytes(Arc::from(truncated)) {
                let mut store = AssetStore::new(Box::new(reader));
                let _ = figure_assets::load_clip(&mut store, FIGURE_NAME, BEND_CLIP, &skeleton());
            }
        }
    }
}
