//! Plan 0002 WP2.8 milestone showcase (M1): the merged 2.5D stage — procedural meshes, PBR
//! materials, a key light plus point lights, the key-light shadow map, and the tilted camera — all
//! in one window.
//!
//! **CI only ever builds this example, never runs it** (`.github/workflows/ci.yml`'s "Build
//! examples" step, `cargo build --workspace --examples --locked`; every example in this workspace
//! opens a window, which a CI runner cannot do): it exists to prove the merged stage compiles and
//! to give a human something to actually look at locally. Run it with
//! `cargo run -p grimoire_render --example pbr_stage`; `Escape` exits, and
//! `GRIMOIRE_EXAMPLE_MAX_FRAMES=<n>` exits after `n` frames (same convention as `instancing.rs`,
//! for a bounded local smoke run).
//!
//! `crates/grimoire_render/tests/pbr_stage_showcase.rs` renders the same kind of scene headlessly
//! (offscreen, no window) into a PNG sequence for the CI GIF-artifact job described in plan 0002
//! WP2.8; it does not reuse this file's scene directly (an example binary and a test binary are
//! separate compilation units), but keeps the same cast: floor, pillars, an altar, a few
//! "figures", torches and a key light.

use std::sync::Arc;

use grimoire_platform::{
    AppHandler, AppResult, KeyCode, PlatformContext, PlatformEvent, PlatformWindow, RawInputEvent,
    WindowConfig, run_desktop,
};
use grimoire_render::procedural::{altar_block, capsule_actor, floor_tile_grid, octagonal_pillar};
use grimoire_render::{
    AmbientLight, Camera25D, DirectionalLight, MaterialHandle, MeshHandle, MeshInstance,
    PbrMaterial, PointLight, RenderError, Renderer, RendererConfig, ShadowConfig, ShadowMode,
    StageFrame, WgpuRenderer,
};

const TITLE: &str = "Grimoire pbr_stage (WP2.8 showcase)";
const PILLAR_COUNT: u32 = 8;
const PILLAR_RADIUS: f32 = 13.0;

fn translation(t: [f32; 3]) -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [t[0], t[1], t[2], 1.0],
    ]
}

fn mesh_instance(
    mesh: MeshHandle,
    material: MaterialHandle,
    transform: [[f32; 4]; 4],
) -> MeshInstance {
    let mut instance = MeshInstance::default();
    instance.mesh = mesh;
    instance.material = material;
    instance.transform = transform;
    instance
}

fn material(base_color: [f32; 4], metallic: f32, roughness: f32) -> PbrMaterial {
    let mut material = PbrMaterial::default();
    material.base_color_factor = base_color;
    material.metallic_factor = metallic;
    material.roughness_factor = roughness;
    material
}

fn torch(position: [f32; 3]) -> PointLight {
    let mut light = PointLight::default();
    light.position = position;
    light.color = [0.85, 0.55, 0.32];
    light.intensity = 6.0;
    light.range = 15.0;
    light
}

/// The registered mesh handles the arena needs; kept together so [`Stage::rebuild_frame`] does not
/// have to re-register (and re-upload) geometry every frame. `Copy` (like the [`MeshHandle`]s it
/// holds) so [`Stage::frame`] can read it out of `self.meshes` without holding a borrow of `self`
/// across the `&mut self` call to [`Stage::rebuild_frame`].
#[derive(Clone, Copy)]
struct ArenaMeshes {
    floor: MeshHandle,
    pillar: MeshHandle,
    altar: MeshHandle,
    figure: MeshHandle,
}

struct Stage {
    window: Option<Arc<dyn PlatformWindow>>,
    renderer: Option<WgpuRenderer>,
    meshes: Option<ArenaMeshes>,
    frame: StageFrame,
    time: f32,
    frames: u64,
    max_frames: Option<u64>,
}

impl Stage {
    fn new(max_frames: Option<u64>) -> Self {
        Self {
            window: None,
            renderer: None,
            meshes: None,
            frame: StageFrame::new(),
            time: 0.0,
            frames: 0,
            max_frames,
        }
    }

    fn register_meshes(renderer: &mut WgpuRenderer) -> ArenaMeshes {
        ArenaMeshes {
            floor: renderer
                .register_mesh(floor_tile_grid(20, 2.0))
                .expect("valid mesh"),
            pillar: renderer
                .register_mesh(octagonal_pillar(0.6, 6.0))
                .expect("valid mesh"),
            altar: renderer
                .register_mesh(altar_block(3.0, 2.0, 1.2))
                .expect("valid mesh"),
            figure: renderer
                .register_mesh(capsule_actor(0.6, 1.8, 12, 3))
                .expect("valid mesh"),
        }
    }

    /// Rebuilds `self.frame` from scratch every call: simplest way to keep the merged stage
    /// (meshes, materials, lights, shadow config, camera) consistent while the tilt animates below
    /// — this example favours clarity over the per-frame allocation `StageFrame::clear` would
    /// otherwise avoid, since it is never run in CI and a human watching a window will not notice.
    fn rebuild_frame(&mut self, meshes: &ArenaMeshes) {
        self.frame = StageFrame::new();
        self.frame.base.clear_color = [0.02, 0.024, 0.036, 1.0];

        let mut camera = Camera25D::default();
        camera.target = [0.0, 2.0];
        // Slow oscillation across the shipped 60-75 degree tilt range (ADR-0014, PRD-0003 FR-03):
        // shows the projection actually changing without the camera ever yawing (it cannot,
        // `Camera25D`'s own doc comment).
        camera.tilt_degrees = 67.5 + 7.5 * (self.time * 0.3).sin();
        camera.fov_y_degrees = 50.0;
        camera.distance = 34.0;
        self.frame.camera_25d = Some(camera);

        let mut key_light = DirectionalLight::default();
        key_light.direction = [0.35, 0.5, -0.8];
        key_light.color = [0.60, 0.69, 0.85];
        key_light.intensity = 5.5;
        self.frame.key_light = Some(key_light);
        self.frame.ambient = AmbientLight::Hemisphere {
            sky_color: [0.16, 0.18, 0.24],
            ground_color: [0.06, 0.055, 0.06],
            intensity: 1.0,
        };
        self.frame.shadow_config = {
            let mut config = ShadowConfig::default();
            config.mode = ShadowMode::KeyLight;
            config
        };

        self.frame
            .materials
            .push(material([0.20, 0.21, 0.24, 1.0], 0.0, 0.85)); // 0: floor/pillars
        self.frame
            .materials
            .push(material([0.11, 0.09, 0.095, 1.0], 0.0, 0.6)); // 1: altar
        self.frame
            .materials
            .push(material([0.30, 0.27, 0.25, 1.0], 0.0, 0.7)); // 2: figures

        self.frame.meshes.push(mesh_instance(
            meshes.floor,
            MaterialHandle(0),
            translation([0.0, 0.0, 0.0]),
        ));
        self.frame.meshes.push(mesh_instance(
            meshes.altar,
            MaterialHandle(1),
            translation([0.0, 0.0, 0.6]),
        ));

        for i in 0..PILLAR_COUNT {
            let angle = i as f32 / PILLAR_COUNT as f32 * std::f32::consts::TAU;
            let (x, y) = (PILLAR_RADIUS * angle.cos(), PILLAR_RADIUS * angle.sin());
            self.frame.meshes.push(mesh_instance(
                meshes.pillar,
                MaterialHandle(0),
                translation([x, y, 3.0]),
            ));
            self.frame
                .point_lights
                .push(torch([x * 0.82, y * 0.82, 4.2]));
        }

        for &[x, y] in &[
            [-4.0, 6.0],
            [4.5, 4.0],
            [-2.5, -5.0],
            [3.0, 10.0],
            [1.0, -6.5],
        ] {
            self.frame.meshes.push(mesh_instance(
                meshes.figure,
                MaterialHandle(2),
                translation([x, y, 1.5]),
            ));
        }
    }
}

impl AppHandler for Stage {
    fn init(&mut self, ctx: &mut dyn PlatformContext) -> AppResult {
        let window = ctx.window().ok_or("the pbr_stage example needs a window")?;
        let config = RendererConfig {
            vsync: false,
            allow_software_fallback: true,
            ..RendererConfig::default()
        };
        let mut renderer = WgpuRenderer::new_for_window(Arc::clone(&window), config)?;
        let size = window.inner_size();
        renderer.resize(size.width, size.height);
        log::info!("renderer backend: {}", renderer.backend_name());
        let meshes = Stage::register_meshes(&mut renderer);
        self.meshes = Some(meshes);
        self.window = Some(window);
        self.renderer = Some(renderer);
        Ok(())
    }

    fn event(&mut self, ctx: &mut dyn PlatformContext, event: &PlatformEvent) {
        match event {
            PlatformEvent::Resized(size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size.width, size.height);
                }
            }
            PlatformEvent::Input(RawInputEvent::Key {
                code: KeyCode::Escape,
                pressed: true,
                ..
            }) => ctx.request_exit(),
            _ => {}
        }
    }

    fn frame(&mut self, ctx: &mut dyn PlatformContext) {
        self.time += 1.0 / 60.0; // A fixed visual step is fine for a showcase, unlike the sim proper.
        if let Some(meshes) = self.meshes {
            self.rebuild_frame(&meshes);
        }

        if let Some(renderer) = &mut self.renderer {
            match renderer.render_stage(&self.frame) {
                Ok(_) => {}
                Err(RenderError::SurfaceLost) => ctx.frame_not_presented(),
                Err(error) => {
                    log::error!("render failed: {error}");
                    ctx.request_exit();
                }
            }
        }

        self.frames += 1;
        if self.max_frames.is_some_and(|max| self.frames >= max) {
            ctx.request_exit();
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let max_frames = match std::env::var("GRIMOIRE_EXAMPLE_MAX_FRAMES") {
        Ok(value) => Some(value.parse::<u64>()?),
        Err(_) => None,
    };
    let config = WindowConfig {
        title: String::from(TITLE),
        ..WindowConfig::default()
    };
    run_desktop(config, Stage::new(max_frames))?;
    Ok(())
}
