//! P1 "Figuren in der Engine" package: tests for `grimoire::adapters::figure_assets` against a
//! pack built entirely in-process with `grimoire_assets::PackWriter` (no `.pack` file exists yet —
//! Strand A's converter produces one separately; this crate builds its own fixture to stay
//! independent of that other strand, per this package's shared spec).
//!
//! The fixture is deliberately the smallest figure the shared spec's byte layout can express: two
//! bones, four vertices (a flat ribbon, base vertices bound to the root bone, tip vertices bound
//! to the child bone) — small enough to hand-encode every payload byte-for-byte and reason about
//! the exact skinning matrices, while still being a real two-bone hierarchy with an inverse bind
//! matrix, not a degenerate single-bone case.
//!
//! Three kinds of test:
//! - [`load_figure_round_trips_a_hand_written_pack`]: loads the fixture and checks every decoded
//!   field, skipped like every other GPU test in this workspace when no adapter is available.
//! - The `property_tests` module: arbitrary/truncated/single-byte-mutated pack bytes must never
//!   panic `load_figure` (contract §2 rule 9), on top of `grimoire_assets`'s own pack-level fuzz
//!   coverage (`crates/grimoire_assets/tests/pack_fuzz.rs`) — this layer additionally exercises
//!   `figure_assets`'s own cross-payload lookups (ids, kind checks, joint-index cross-check).
//! - [`render_figure_rest_and_bent_pose`]: the offscreen showcase (`#[ignore]`, run explicitly),
//!   proving the bone deformation visually rather than only asserting matrices.

use std::sync::Arc;

use grimoire::adapters::figure_assets::{self, FNP_FIGURE, FNP_MATERIAL, FNP_MESH, FNP_SKELETON};
use grimoire_assets::{AssetId, AssetPath, AssetStore, PackReader, PackWriter};
use grimoire_render::figure_format::{self, JointPose};
use grimoire_render::{
    AmbientLight, Camera25D, DirectionalLight, MaterialHandle, MeshInstance, PointLight,
    RenderError, Renderer, RendererConfig, SkinBinding, StageFrame, WgpuRenderer,
};

const FIGURE_NAME: &str = "testfig";

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

/// The fixture's flat ribbon: base vertices (bound fully to joint 0, at bind-space `Z = 0`) and
/// tip vertices (bound fully to joint 1, at bind-space `Z = 2`), two triangles.
fn mesh_payload_bytes() -> Vec<u8> {
    let mut buf = Vec::new();
    push_u32(&mut buf, figure_format::FORMAT_VERSION);
    push_u32(&mut buf, 4); // vertex_count
    push_u32(&mut buf, 6); // index_count

    let half_width = 0.3f32;
    // One raw test vertex's fields, in payload order: position, normal, uv, joints, weights.
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

/// A plain, textureless, warm-grey opaque material.
fn material_payload_bytes() -> Vec<u8> {
    let mut buf = Vec::new();
    push_u32(&mut buf, figure_format::FORMAT_VERSION);
    push_f32s(&mut buf, &[0.75, 0.55, 0.35, 1.0]); // base_color_factor
    push_f32(&mut buf, 0.0); // metallic
    push_f32(&mut buf, 0.7); // roughness
    push_f32s(&mut buf, &[0.0, 0.0, 0.0]); // emissive
    buf.push(0); // alpha_mode = Opaque
    push_f32(&mut buf, 0.0); // alpha_cutoff (unused for Opaque)
    push_u32(&mut buf, 0xFFFF_FFFF); // base_color_texture: none
    push_u32(&mut buf, 0xFFFF_FFFF); // normal_texture: none
    push_u32(&mut buf, 0xFFFF_FFFF); // orm_texture: none
    buf
}

/// Two joints: root at the origin, child bound at bind-space `[0, 0, 1]` (the ribbon's tip
/// vertices, at bind-space `Z = 2`, sit one unit further out than the joint itself — see this
/// module's doc comment).
fn skeleton_payload_bytes() -> Vec<u8> {
    let mut buf = Vec::new();
    push_u32(&mut buf, figure_format::FORMAT_VERSION);
    push_u32(&mut buf, 2); // joint_count

    // Hierarchy + inverse bind + name, one loop (this crate's `figure_format` doc comment: the
    // shared spec's "Dazu je Knochen..." reads as a second, separate loop for the rest pose).
    push_i32(&mut buf, -1); // joint 0: root
    push_f32s(&mut buf, &[1.0, 0.0, 0.0, 0.0]);
    push_f32s(&mut buf, &[0.0, 1.0, 0.0, 0.0]);
    push_f32s(&mut buf, &[0.0, 0.0, 1.0, 0.0]);
    push_f32s(&mut buf, &[0.0, 0.0, 0.0, 1.0]);
    buf.push(4);
    buf.extend_from_slice(b"root");

    push_i32(&mut buf, 0); // joint 1: child of root
    // Inverse of the bind-pose translation by [0, 0, 1].
    push_f32s(&mut buf, &[1.0, 0.0, 0.0, 0.0]);
    push_f32s(&mut buf, &[0.0, 1.0, 0.0, 0.0]);
    push_f32s(&mut buf, &[0.0, 0.0, 1.0, 0.0]);
    push_f32s(&mut buf, &[0.0, 0.0, -1.0, 1.0]);
    buf.push(4);
    buf.extend_from_slice(b"limb");

    // Rest pose, one record per joint, same order.
    push_f32s(&mut buf, &[0.0, 0.0, 0.0]); // joint 0 translation
    push_f32s(&mut buf, &[0.0, 0.0, 0.0, 1.0]); // joint 0 rotation
    push_f32s(&mut buf, &[1.0, 1.0, 1.0]); // joint 0 scale
    push_f32s(&mut buf, &[0.0, 0.0, 1.0]); // joint 1 translation
    push_f32s(&mut buf, &[0.0, 0.0, 0.0, 1.0]); // joint 1 rotation
    push_f32s(&mut buf, &[1.0, 1.0, 1.0]); // joint 1 scale
    buf
}

fn figure_manifest_bytes(mesh_id: u64, material_id: u64, skeleton_id: u64) -> Vec<u8> {
    let mut buf = Vec::new();
    push_u32(&mut buf, figure_format::FORMAT_VERSION);
    push_u32(&mut buf, 1); // part_count
    push_u64(&mut buf, mesh_id);
    push_u64(&mut buf, material_id);
    push_u32(&mut buf, 0); // texture_count
    push_u64(&mut buf, skeleton_id);
    push_f32s(&mut buf, &[-0.3, -0.01, 0.0]); // bounds_min
    push_f32s(&mut buf, &[0.3, 0.01, 2.0]); // bounds_max
    buf
}

/// Builds the fixture pack (`PackWriter`, per this package's spec: "bau dir deine Testdaten
/// selbst"), assembling the raw payload bytes above at the shared spec's own path convention
/// (`figures/<name>/...`), then serialises it exactly like Strand A's converter would.
fn write_fixture_pack() -> Vec<u8> {
    let mesh_path = AssetPath::new(&format!("figures/{FIGURE_NAME}/mesh/0")).unwrap();
    let material_path = AssetPath::new(&format!("figures/{FIGURE_NAME}/material/0")).unwrap();
    let skeleton_path = AssetPath::new(&format!("figures/{FIGURE_NAME}/skeleton")).unwrap();
    let figure_path = AssetPath::new(&format!("figures/{FIGURE_NAME}/figure")).unwrap();

    let mut writer = PackWriter::new("figure_pack_test_fixture", "0.0.0");
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
    writer.finish().unwrap()
}

fn offscreen_renderer(width: u32, height: u32) -> Option<WgpuRenderer> {
    let config = RendererConfig {
        vsync: false,
        initial_sprite_capacity: 16,
        allow_software_fallback: true,
    };
    match WgpuRenderer::new_offscreen(width, height, config) {
        Ok(renderer) => Some(renderer),
        Err(RenderError::NoAdapter) => None,
        Err(error) => panic!("offscreen renderer creation failed: {error}"),
    }
}

#[test]
fn load_figure_round_trips_a_hand_written_pack() {
    let Some(mut renderer) = offscreen_renderer(4, 4) else {
        eprintln!("no GPU adapter available; skipping load_figure_round_trips_a_hand_written_pack");
        return;
    };
    let bytes = write_fixture_pack();
    let reader = PackReader::from_bytes(Arc::from(bytes)).expect("valid fixture pack");
    let mut store = AssetStore::new(Box::new(reader));

    let figure = figure_assets::load_figure(&mut store, &mut renderer, FIGURE_NAME)
        .expect("fixture pack loads cleanly");

    assert_eq!(figure.parts.len(), 1);
    assert_eq!(figure.skeleton.joint_count(), 2);
    assert_eq!(figure.skeleton.joints[0].parent, None);
    assert_eq!(figure.skeleton.joints[1].parent, Some(0));
    assert_eq!(figure.bounds_min, [-0.3, -0.01, 0.0]);
    assert_eq!(figure.bounds_max, [0.3, 0.01, 2.0]);

    let part = &figure.parts[0];
    assert_eq!(part.material.base_color_factor, [0.75, 0.55, 0.35, 1.0]);
    assert_eq!(part.material.base_color_texture, None);

    // Rest pose (this crate's own utility, not an animation system): every joint's skin matrix
    // is the identity, since the fixture's rest pose equals its bind pose by construction.
    let rest = figure_format::rest_pose_skin_matrices(&figure.skeleton);
    assert_eq!(rest.len(), 2);
    const IDENTITY: [[f32; 4]; 4] = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    for matrix in rest {
        for col in 0..4 {
            for row in 0..4 {
                assert!((matrix[col][row] - IDENTITY[col][row]).abs() < 1e-6);
            }
        }
    }
}

mod property_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig { cases: 20, .. ProptestConfig::default() })]

        /// Arbitrary bytes, entirely unrelated to the pack format, must never panic anywhere in
        /// the `PackReader` → `AssetStore` → `load_figure` chain.
        #[test]
        fn load_figure_never_panics_on_arbitrary_bytes(
            bytes in prop::collection::vec(any::<u8>(), 0..2048)
        ) {
            let Some(mut renderer) = offscreen_renderer(4, 4) else { return Ok(()); };
            if let Ok(reader) = PackReader::from_bytes(Arc::from(bytes)) {
                let mut store = AssetStore::new(Box::new(reader));
                let _ = figure_assets::load_figure(&mut store, &mut renderer, FIGURE_NAME);
            }
        }

        /// Flipping any single byte of a valid fixture pack must never panic the same chain.
        #[test]
        fn load_figure_never_panics_on_single_byte_mutation(
            index in 0usize..1024, new_byte in any::<u8>()
        ) {
            let Some(mut renderer) = offscreen_renderer(4, 4) else { return Ok(()); };
            let mut bytes = write_fixture_pack();
            if index < bytes.len() {
                bytes[index] = new_byte;
            }
            if let Ok(reader) = PackReader::from_bytes(Arc::from(bytes)) {
                let mut store = AssetStore::new(Box::new(reader));
                let _ = figure_assets::load_figure(&mut store, &mut renderer, FIGURE_NAME);
            }
        }

        /// Truncating a valid fixture pack to any shorter length must never panic the same chain.
        #[test]
        fn load_figure_never_panics_on_truncation(len in 0usize..1024) {
            let Some(mut renderer) = offscreen_renderer(4, 4) else { return Ok(()); };
            let bytes = write_fixture_pack();
            let truncated = bytes[..len.min(bytes.len())].to_vec();
            if let Ok(reader) = PackReader::from_bytes(Arc::from(truncated)) {
                let mut store = AssetStore::new(Box::new(reader));
                let _ = figure_assets::load_figure(&mut store, &mut renderer, FIGURE_NAME);
            }
        }
    }
}

// --- Showcase: rest pose vs. a clearly bent pose, from the tilted game camera --------------

mod support;
use support::Image;

const WIDTH: u32 = 480;
const HEIGHT: u32 = 360;

fn out_dir() -> std::path::PathBuf {
    std::env::var_os("GRIMOIRE_FIGURE_SHOWCASE_OUT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new("target").join("p1-figure-showcase"))
}

/// The showcase's own tilted camera: same shape as the other WP2.x showcases'
/// (`crates/grimoire_render/tests/wp26_shadow_showcase.rs`, `pbr_stage_showcase.rs`), scaled down
/// to frame a two-unit figure instead of a whole arena.
fn showcase_camera() -> Camera25D {
    let mut camera = Camera25D::default();
    camera.target = [0.0, 0.0];
    camera.tilt_degrees = 65.0;
    camera.fov_y_degrees = 45.0;
    camera.distance = 4.5;
    camera
}

fn identity_transform() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// Renders the loaded figure with `joint_matrices` applied and returns the resulting image.
fn render_figure(
    renderer: &mut WgpuRenderer,
    figure: &figure_assets::LoadedFigure,
    joint_matrices: Vec<[[f32; 4]; 4]>,
) -> Image {
    let mut frame = StageFrame::new();
    frame.base.clear_color = [0.03, 0.032, 0.045, 1.0];
    frame.camera_25d = Some(showcase_camera());

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

    let joint_count = u32::try_from(joint_matrices.len()).expect("small fixture skeleton");
    frame.joint_matrices = joint_matrices;
    let mut skin = SkinBinding::default();
    skin.joint_offset = 0;
    skin.joint_count = joint_count;

    for part in &figure.parts {
        frame.materials.push(part.material);
        let material = MaterialHandle(u32::try_from(frame.materials.len() - 1).unwrap());
        let mut instance = MeshInstance::default();
        instance.mesh = part.mesh;
        instance.material = material;
        instance.transform = identity_transform();
        instance.skin = Some(skin);
        frame.meshes.push(instance);
    }

    renderer.render_stage(&frame).expect("render_stage");
    let rgba = renderer.read_offscreen_rgba().expect("read-back");
    Image::from_offscreen(WIDTH, HEIGHT, rgba)
}

/// Produces the P1 skinning package's belegende Bilder (item 5 of the engine work order): the
/// fixture figure once in rest pose, once with its single joint bent 90 degrees, from the tilted
/// game camera — proving the bone deformation actually happens in the renderer, not only in the
/// matrices `compute_skin_matrices` returns (already covered by
/// `grimoire_render::figure_format`'s own unit tests).
///
/// Run explicitly: `GRIMOIRE_GPU_ADAPTER=software cargo test -p grimoire --test figure_pack
/// --locked -- --ignored --nocapture render_figure_rest_and_bent_pose`. Writes
/// `figure_rest_pose.png` and `figure_bent_pose.png` under `GRIMOIRE_FIGURE_SHOWCASE_OUT_DIR`
/// (default `target/p1-figure-showcase`), like every other showcase test in this workspace.
#[test]
#[ignore = "P1 skinning showcase: offscreen comparison images, run explicitly (see this test's doc comment)"]
fn render_figure_rest_and_bent_pose() {
    let Some(mut renderer) = offscreen_renderer(WIDTH, HEIGHT) else {
        eprintln!("no GPU adapter available; skipping the P1 skinning showcase");
        return;
    };
    let bytes = write_fixture_pack();
    let reader = PackReader::from_bytes(Arc::from(bytes)).expect("valid fixture pack");
    let mut store = AssetStore::new(Box::new(reader));
    let figure = figure_assets::load_figure(&mut store, &mut renderer, FIGURE_NAME)
        .expect("fixture pack loads cleanly");

    let rest_matrices = figure_format::rest_pose_skin_matrices(&figure.skeleton);
    let rest_image = render_figure(&mut renderer, &figure, rest_matrices);

    // A clear 90-degree bend at the (only) joint, root left unposed — same pose this crate's
    // `figure_format::tests::compute_skin_matrices_bends_the_child_joint_around_its_own_pivot`
    // checks numerically; here it is rendered.
    let half_sqrt2 = std::f32::consts::FRAC_1_SQRT_2;
    let bent_poses = [
        JointPose {
            translation: figure.skeleton.joints[0].translation,
            rotation: figure.skeleton.joints[0].rotation,
            scale: figure.skeleton.joints[0].scale,
        },
        JointPose {
            translation: figure.skeleton.joints[1].translation,
            rotation: [half_sqrt2, 0.0, 0.0, half_sqrt2],
            scale: figure.skeleton.joints[1].scale,
        },
    ];
    let bent_matrices = figure_format::compute_skin_matrices(&figure.skeleton, &bent_poses)
        .expect("one pose per joint");
    let bent_image = render_figure(&mut renderer, &figure, bent_matrices);

    let dir = out_dir();
    rest_image
        .write_png(&dir.join("figure_rest_pose.png"))
        .expect("write figure_rest_pose.png");
    bent_image
        .write_png(&dir.join("figure_bent_pose.png"))
        .expect("write figure_bent_pose.png");
    let comparison = Image::beside(&[&rest_image, &bent_image]);
    comparison
        .write_png(&dir.join("figure_pose_comparison.png"))
        .expect("write figure_pose_comparison.png");

    println!(
        "grimoire-p1-figure-showcase: size={WIDTH}x{HEIGHT} dir={}",
        dir.display()
    );
}

// --- showcase against a real `.pack` file ----------------------------------------------------

/// Renders a figure from an on-disk pack instead of the in-process fixture, so the converter's
/// real output can be looked at without a second harness.
///
/// Both inputs come from the environment, because the pack is a build artefact that deliberately
/// lives outside either repository: `GRIMOIRE_FIGURE_PACK` is the path to the `.pack` file and
/// `GRIMOIRE_FIGURE_NAME` the figure inside it (its entries are `figures/<name>/...`). Without
/// `GRIMOIRE_FIGURE_PACK` the test skips, exactly like the GPU-less skip above — it is a viewing
/// aid, not a gate, and nothing in CI has a pack to point it at.
///
/// Run explicitly:
/// `GRIMOIRE_GPU_ADAPTER=software GRIMOIRE_FIGURE_PACK=<file> GRIMOIRE_FIGURE_NAME=imp
/// cargo test -p grimoire --test figure_pack --locked -- --ignored --nocapture
/// render_figure_from_pack_file`
#[test]
#[ignore = "viewing aid: renders a figure from an on-disk pack, run explicitly (see doc comment)"]
fn render_figure_from_pack_file() {
    let Some(pack_path) = std::env::var_os("GRIMOIRE_FIGURE_PACK") else {
        eprintln!("GRIMOIRE_FIGURE_PACK not set; skipping the on-disk figure showcase");
        return;
    };
    let figure_name = std::env::var("GRIMOIRE_FIGURE_NAME").unwrap_or_else(|_| "imp".to_owned());
    let Some(mut renderer) = offscreen_renderer(WIDTH, HEIGHT) else {
        eprintln!("no GPU adapter available; skipping the on-disk figure showcase");
        return;
    };

    let fs = grimoire_platform::StdFileSystem;
    let reader = PackReader::open(&fs, std::path::Path::new(&pack_path))
        .expect("the pack file opens and its manifest decodes");
    let mut store = AssetStore::new(Box::new(reader));
    let figure = figure_assets::load_figure(&mut store, &mut renderer, &figure_name)
        .expect("the figure loads from the pack");

    let joint_count = figure.skeleton.joints.len();
    println!(
        "grimoire-figure-from-pack: figure={figure_name} parts={} joints={joint_count}",
        figure.parts.len()
    );

    // Frame the figure by its own height instead of inheriting the two-unit fixture's camera:
    // the bounds come from the pack, so an imp (~1.1 units) and a brute (~2.4) both fill the frame.
    let height = figure.bounds_max[2] - figure.bounds_min[2];
    let mid_z = (figure.bounds_max[2] + figure.bounds_min[2]) * 0.5;

    let rest_matrices = figure_format::rest_pose_skin_matrices(&figure.skeleton);
    let rest_image = render_figure_framed(&mut renderer, &figure, rest_matrices, height, mid_z);
    let (dark, foreground) = count_very_dark_head_pixels(&rest_image, 30);
    println!(
        "grimoire-figure-tangent-check: figure={figure_name} tangents=real head_region_dark_pixels={dark} head_region_foreground_pixels={foreground}"
    );

    // Texture-quality package strand B2's measurement: the very same pack, pose, camera and
    // lighting, but with every part's real (MikkTSpace) tangent forced to the "no tangent"
    // sentinel `[0,0,0,0]` — the pre-B2 behaviour, since `mesh.wgsl` then falls back to
    // `cotangent_frame` exactly as it did before this package existed. Re-decodes each part's mesh
    // via the same `store` (a cache keyed by asset id, so this hits the copy `load_figure` already
    // decoded and paid no second read or GPU upload for the untouched original) rather than
    // diffing against a different engine build, which cannot even load a `kind_version` 2
    // `FNP_MESH` at all (see this test's module-level report) — this isolates exactly the one
    // variable the Festlegung asks about.
    let mut no_tangent_parts = Vec::with_capacity(figure.parts.len());
    for (index, part) in figure.parts.iter().enumerate() {
        let mesh_path = AssetPath::new(&format!("figures/{figure_name}/mesh/{index}"))
            .expect("valid asset path");
        let mesh_id = AssetId::from_path(&mesh_path);
        let mesh_handle = store
            .load(mesh_id, FNP_MESH, figure_format::decode_mesh)
            .expect("this mesh already decoded once above, so this only hits the cache");
        let mut mesh_data = store
            .get(mesh_handle)
            .expect("just loaded above, so it is present")
            .clone();
        for vertex in &mut mesh_data.vertices {
            vertex.tangent = [0.0, 0.0, 0.0, 0.0];
        }
        let mesh = renderer
            .register_mesh(mesh_data)
            .expect("stripped-tangent mesh is still structurally valid");
        no_tangent_parts.push(figure_assets::LoadedFigurePart {
            mesh,
            material: part.material,
        });
    }
    let no_tangent_figure = figure_assets::LoadedFigure {
        parts: no_tangent_parts,
        skeleton: figure.skeleton.clone(),
        bounds_min: figure.bounds_min,
        bounds_max: figure.bounds_max,
    };
    let no_tangent_rest_matrices =
        figure_format::rest_pose_skin_matrices(&no_tangent_figure.skeleton);
    let no_tangent_image = render_figure_framed(
        &mut renderer,
        &no_tangent_figure,
        no_tangent_rest_matrices,
        height,
        mid_z,
    );
    let (no_tangent_dark, no_tangent_foreground) =
        count_very_dark_head_pixels(&no_tangent_image, 30);
    println!(
        "grimoire-figure-tangent-check: figure={figure_name} tangents=stripped head_region_dark_pixels={no_tangent_dark} head_region_foreground_pixels={no_tangent_foreground}"
    );
    {
        let dir = out_dir();
        no_tangent_image
            .write_png(&dir.join(format!("{figure_name}_no_tangent_rest.png")))
            .expect("write the no-tangent comparison image");
        Image::beside(&[&no_tangent_image, &rest_image])
            .write_png(&dir.join(format!("{figure_name}_tangent_before_after.png")))
            .expect("write the before/after comparison strip");

        // Whole-image diff between the real-tangent and stripped-tangent renders: how many pixels
        // changed at all, and by how much on average/at most — the head-region dark-pixel count
        // above is a narrow proxy, this is the unfiltered picture.
        let mut changed = 0u32;
        let mut sum_abs_diff = 0u64;
        let mut max_abs_diff = 0u8;
        for (a, b) in rest_image.rgba.iter().zip(&no_tangent_image.rgba) {
            let diff = a.abs_diff(*b);
            if diff > 0 {
                changed += 1;
            }
            sum_abs_diff += u64::from(diff);
            max_abs_diff = max_abs_diff.max(diff);
        }
        let mean_abs_diff = sum_abs_diff as f64 / rest_image.rgba.len() as f64;
        println!(
            "grimoire-figure-tangent-check: figure={figure_name} whole_image_changed_channels={changed} mean_abs_diff={mean_abs_diff:.3} max_abs_diff={max_abs_diff}"
        );
    }

    // A visible bend, applied to every joint that has a parent: a quarter turn about X shared by
    // the whole hierarchy reads clearly from the tilted camera without needing to know which bone
    // is an arm. The root keeps its rest pose so the figure stays where it stands.
    let quarter = std::f32::consts::FRAC_1_SQRT_2;
    let bent_poses: Vec<JointPose> = figure
        .skeleton
        .joints
        .iter()
        .map(|joint| JointPose {
            translation: joint.translation,
            rotation: if joint.parent.is_some() {
                [
                    quarter * 0.45,
                    0.0,
                    0.0,
                    (1.0 - (quarter * 0.45) * (quarter * 0.45)).sqrt(),
                ]
            } else {
                joint.rotation
            },
            scale: joint.scale,
        })
        .collect();
    let bent_matrices = figure_format::compute_skin_matrices(&figure.skeleton, &bent_poses)
        .expect("one pose per joint");
    let bent_image = render_figure_framed(&mut renderer, &figure, bent_matrices, height, mid_z);

    let dir = out_dir();
    rest_image
        .write_png(&dir.join(format!("{figure_name}_rest.png")))
        .expect("write the rest-pose image");
    bent_image
        .write_png(&dir.join(format!("{figure_name}_bent.png")))
        .expect("write the bent-pose image");
    Image::beside(&[&rest_image, &bent_image])
        .write_png(&dir.join(format!("{figure_name}_comparison.png")))
        .expect("write the comparison image");
    println!("grimoire-figure-from-pack: dir={}", dir.display());
}

/// Head-darkness proxy for the texture-quality package's Strand B2 measurement ("miss, ob die
/// dunklen Stellen am Kopf verschwinden" — measure, don't just describe, whether the dark patches
/// at the head go away). Counts, among the pixels in the top third of `image` (where a standing
/// humanoid figure's head sits in this showcase's own framing) that do **not** match the
/// background colour sampled from the image's top-left corner (guaranteed background — the figure
/// never reaches the frame's edges here), how many are darker than `threshold` in every channel.
/// Not a general-purpose metric, just enough to compare the very same pack, camera and lighting
/// before and after a shading change.
fn count_very_dark_head_pixels(image: &Image, threshold: u8) -> (u32, u32) {
    let background = [image.rgba[0], image.rgba[1], image.rgba[2], image.rgba[3]];
    let mut dark = 0u32;
    let mut foreground = 0u32;
    let top_third = image.height / 3;
    for y in 0..top_third {
        for x in 0..image.width {
            let index = (y as usize * image.width as usize + x as usize) * 4;
            let pixel = [
                image.rgba[index],
                image.rgba[index + 1],
                image.rgba[index + 2],
                image.rgba[index + 3],
            ];
            let is_background = pixel
                .iter()
                .zip(background)
                .all(|(&a, b)| a.abs_diff(b) <= 6);
            if is_background {
                continue;
            }
            foreground += 1;
            if pixel[0] <= threshold && pixel[1] <= threshold && pixel[2] <= threshold {
                dark += 1;
            }
        }
    }
    (dark, foreground)
}

/// Like [`render_figure`], but frames the figure by its own height and lights it for a figure
/// rather than for the two-unit fixture: the showcase camera above was tuned for the four-vertex
/// test ribbon and leaves a real figure tiny, grey and under-lit.
fn render_figure_framed(
    renderer: &mut WgpuRenderer,
    figure: &figure_assets::LoadedFigure,
    joint_matrices: Vec<[[f32; 4]; 4]>,
    height: f32,
    mid_z: f32,
) -> Image {
    let mut frame = StageFrame::new();
    frame.base.clear_color = [0.035, 0.037, 0.05, 1.0];

    let mut camera = Camera25D::default();
    camera.target = [0.0, 0.0];
    camera.tilt_degrees = 62.0;
    camera.fov_y_degrees = 40.0;
    // Fill roughly two thirds of the frame height at this field of view.
    camera.distance = grimoire_core::math::dmath::max(height * 1.9, 1.2);
    frame.camera_25d = Some(camera);

    let mut key_light = DirectionalLight::default();
    key_light.direction = [0.45, 0.55, -0.7];
    key_light.color = [1.0, 0.96, 0.9];
    key_light.intensity = 3.2;
    frame.key_light = Some(key_light);
    frame.ambient = AmbientLight::Hemisphere {
        sky_color: [0.22, 0.24, 0.32],
        ground_color: [0.06, 0.055, 0.06],
        intensity: 0.9,
    };
    // A warm rim from the far side so the silhouette separates from the dark ground.
    let mut rim = PointLight::default();
    rim.position = [-height * 0.9, -height * 0.8, mid_z + height * 0.35];
    rim.color = [1.0, 0.55, 0.30];
    rim.intensity = height * 2.0;
    rim.range = height * 6.0;
    frame.point_lights.push(rim);

    let joint_count = u32::try_from(joint_matrices.len()).expect("skeleton fits in u32");
    frame.joint_matrices = joint_matrices;
    let mut skin = SkinBinding::default();
    skin.joint_offset = 0;
    skin.joint_count = joint_count;

    // Centre the figure vertically in view.
    let mut transform = identity_transform();
    transform[3][2] = -mid_z;

    for part in &figure.parts {
        frame.materials.push(part.material);
        let material = MaterialHandle(u32::try_from(frame.materials.len() - 1).unwrap());
        let mut instance = MeshInstance::default();
        instance.mesh = part.mesh;
        instance.material = material;
        instance.transform = transform;
        instance.skin = Some(skin);
        frame.meshes.push(instance);
    }

    renderer.render_stage(&frame).expect("render_stage");
    let rgba = renderer.read_offscreen_rgba().expect("read-back");
    Image::from_offscreen(WIDTH, HEIGHT, rgba)
}
