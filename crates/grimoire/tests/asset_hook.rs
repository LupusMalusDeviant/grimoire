//! The facade hook for asset registration (contract §9.2, PO decision 2026-09-17 "Fassaden-Haken
//! für Assets"): plugins register meshes and textures with the loop's renderer through
//! `GamePlugin::register_assets`.
//!
//! - Headless tests check the lifecycle (once, after every `build`, before `window_created` and
//!   the first frame), handle numbering across plugins, the error path and hash neutrality.
//! - [`a_mesh_registered_by_a_plugin_renders_offscreen`] runs the real loop over an offscreen
//!   `WgpuRenderer` (`AppBuilder::run_offscreen`) and compares a plugin that registers its mesh
//!   with one that draws the same instance without registering it. Skipped without an adapter,
//!   failing instead with `GRIMOIRE_REQUIRE_GPU_ADAPTER=1`.

mod common;

use std::cell::RefCell;
use std::io::Write as _;
use std::rc::Rc;
use std::time::Duration;

use common::Scenario;
use grimoire::prelude::*;
use grimoire::render::procedural::altar_block;
use grimoire::render::{
    AmbientLight, DirectionalLight, MaterialHandle, MeshHandle, MeshInstance, PbrMaterial,
    RenderError, StageRendererConfig, TextureColorSpace, TextureData,
};
use grimoire::{GrimoireError, HeadlessRenderAssets, OffscreenRun, PluginError, RenderAssets};

/// One 60 Hz tick per frame (see `tests/headless.rs`).
const ONE_TICK: Duration = Duration::from_nanos(16_666_667);

type Log = Rc<RefCell<Vec<String>>>;

/// Registers `meshes` meshes and one texture, logs every hook and the handles it got.
struct Registering {
    name: &'static str,
    meshes: u32,
    fail: bool,
    log: Log,
    handles: Vec<MeshHandle>,
}

impl Registering {
    fn new(name: &'static str, meshes: u32, log: &Log) -> Self {
        Self {
            name,
            meshes,
            fail: false,
            log: Rc::clone(log),
            handles: Vec::new(),
        }
    }

    fn note(&self, event: &str) {
        self.log.borrow_mut().push(format!("{}.{event}", self.name));
    }
}

impl GamePlugin for Registering {
    fn name(&self) -> &str {
        self.name
    }

    fn build(&mut self, _sim: &mut Simulation) {
        self.note("build");
    }

    fn register_assets(&mut self, assets: &mut dyn RenderAssets) -> Result<(), PluginError> {
        self.note("register_assets");
        if self.fail {
            return Err("the figure pack is missing".into());
        }
        for _ in 0..self.meshes {
            self.handles
                .push(assets.register_mesh(altar_block(1.0, 1.0, 1.0))?);
        }
        let texture = assets.register_texture(TextureData {
            width: 2,
            height: 1,
            pixels: vec![255; 8],
            color_space: TextureColorSpace::Srgb,
        })?;
        let handles: Vec<u32> = self.handles.iter().map(|handle| handle.0).collect();
        self.note(&format!("meshes {handles:?} texture {}", texture.0));
        Ok(())
    }

    fn extract_stage(&mut self, _world: &World, _alpha: f32, _stage: &mut StageFrame) {
        self.note("extract_stage");
    }

    fn on_frame(&mut self, _stats: &FrameStats) {
        self.note("on_frame");
    }

    fn shutdown(&mut self) {
        self.note("shutdown");
    }
}

#[test]
fn register_assets_runs_once_after_every_build_and_before_the_first_frame() {
    let log = Log::default();
    let report = App::new(WindowConfig::default())
        .plugin(Registering::new("a", 2, &log))
        .plugin(Registering::new("b", 1, &log))
        .run_headless_frames(2, ONE_TICK)
        .expect("runs");
    assert_eq!(report.frames, 2);
    assert_eq!(
        *log.borrow(),
        [
            "a.build",
            "b.build",
            "a.register_assets",
            "a.meshes [0, 1] texture 0",
            "b.register_assets",
            // Handles count per renderer, across plugins, like `WgpuRenderer`'s registry.
            "b.meshes [2] texture 1",
            "a.extract_stage",
            "b.extract_stage",
            "a.on_frame",
            "b.on_frame",
            "a.extract_stage",
            "b.extract_stage",
            "a.on_frame",
            "b.on_frame",
            "a.shutdown",
            "b.shutdown",
        ]
    );
}

#[test]
fn run_headless_has_no_renderer_and_never_registers_assets() {
    let log = Log::default();
    let report = App::new(WindowConfig::default())
        .plugin(Registering::new("a", 1, &log))
        .run_headless(10, &mut |_| TickInput::default());
    assert_eq!(report.final_tick, 10);
    assert_eq!(*log.borrow(), ["a.build"]);
}

#[test]
fn a_failing_registration_ends_the_run_before_the_first_frame_and_still_shuts_down() {
    let log = Log::default();
    let mut failing = Registering::new("a", 1, &log);
    failing.fail = true;
    let error = App::new(WindowConfig::default())
        .plugin(failing)
        .plugin(Registering::new("b", 1, &log))
        .run_headless_frames(5, ONE_TICK)
        .expect_err("the registration error ends the run");
    let GrimoireError::Assets { plugin, source } = &error else {
        panic!("expected GrimoireError::Assets, got {error:?}");
    };
    assert_eq!(plugin, "a");
    assert_eq!(source.to_string(), "the figure pack is missing");
    assert_eq!(
        error.to_string(),
        "plugin `a` failed to register its assets: the figure pack is missing"
    );
    assert_eq!(
        *log.borrow(),
        [
            "a.build",
            "b.build",
            "a.register_assets",
            "a.shutdown",
            "b.shutdown"
        ],
        "no further registration and no frame, but every built plugin shuts down"
    );
}

#[test]
fn an_invalid_mesh_is_an_error_the_plugin_can_return() {
    struct Invalid;
    impl GamePlugin for Invalid {
        fn name(&self) -> &str {
            "invalid"
        }
        fn register_assets(&mut self, assets: &mut dyn RenderAssets) -> Result<(), PluginError> {
            assets.register_mesh(grimoire::render::MeshData::default())?;
            Ok(())
        }
    }
    let error = App::new(WindowConfig::default())
        .plugin(Invalid)
        .run_headless_frames(1, ONE_TICK)
        .expect_err("an empty mesh is rejected");
    assert!(
        matches!(&error, GrimoireError::Assets { plugin, .. } if plugin == "invalid"),
        "{error:?}"
    );
}

#[test]
fn registered_assets_never_change_a_state_hash() {
    let log = Log::default();
    let with_assets = App::new(WindowConfig::default())
        .seed(9)
        .hash_every(20)
        .plugin(Scenario { movers: 32 })
        .plugin(Registering::new("a", 3, &log))
        .run_headless_frames(120, ONE_TICK)
        .expect("runs");
    let without = App::new(WindowConfig::default())
        .seed(9)
        .hash_every(20)
        .plugin(Scenario { movers: 32 })
        .run_headless_frames(120, ONE_TICK)
        .expect("runs");
    assert_eq!(with_assets, without);
}

#[test]
fn headless_render_assets_can_drive_a_plugin_directly() {
    let log = Log::default();
    let mut plugin = Registering::new("a", 2, &log);
    let mut assets = HeadlessRenderAssets::new();
    plugin
        .register_assets(&mut assets)
        .expect("valid assets register");
    assert_eq!(plugin.handles, [MeshHandle(0), MeshHandle(1)]);
    assert_eq!(
        (assets.meshes_registered(), assets.textures_registered()),
        (2, 1)
    );
}

// --- Offscreen ----------------------------------------------------------------------------------

/// Offscreen tests share one GPU device at a time (like `grimoire_render`'s `tests/offscreen.rs`:
/// parallel WARP devices crashed a release test process).
static GPU_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu_serial() -> std::sync::MutexGuard<'static, ()> {
    GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn adapter_required() -> bool {
    matches!(
        std::env::var("GRIMOIRE_REQUIRE_GPU_ADAPTER")
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("1" | "true")
    )
}

const WIDTH: u32 = 96;
const HEIGHT: u32 = 54;
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// Draws one large altar block in front of a tilted camera. With `register`, the block is
/// registered through the hook; without, the plugin uses the same handle without registering it.
struct Altar {
    register: bool,
    mesh: MeshHandle,
}

impl GamePlugin for Altar {
    fn name(&self) -> &str {
        "altar"
    }

    fn register_assets(&mut self, assets: &mut dyn RenderAssets) -> Result<(), PluginError> {
        if self.register {
            self.mesh = assets.register_mesh(altar_block(8.0, 6.0, 1.5))?;
        }
        Ok(())
    }

    fn extract_stage(&mut self, _world: &World, _alpha: f32, stage: &mut StageFrame) {
        let mut camera = Camera25D::default();
        camera.target = [0.0, 0.0];
        camera.tilt_degrees = 55.0;
        camera.fov_y_degrees = 50.0;
        camera.distance = 12.0;
        stage.camera_25d = Some(camera);
        stage.base.clear_color = CLEAR;
        let mut key_light = DirectionalLight::default();
        key_light.direction = [0.3, 0.4, -0.85];
        key_light.intensity = 2.0;
        stage.key_light = Some(key_light);
        stage.ambient = AmbientLight::Hemisphere {
            sky_color: [0.6, 0.6, 0.7],
            ground_color: [0.2, 0.2, 0.2],
            intensity: 0.8,
        };
        let mut material = PbrMaterial::default();
        material.base_color_factor = [0.9, 0.45, 0.2, 1.0];
        material.roughness_factor = 0.7;
        stage.materials.push(material);
        let mut instance = MeshInstance::default();
        instance.mesh = self.mesh;
        instance.material = MaterialHandle(0);
        instance.transform = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.75, 1.0],
        ];
        stage.meshes.push(instance);
    }
}

/// Runs `frames` offscreen frames and returns the captured `(frame, image)` pairs, or `None`
/// without an adapter.
fn run_altar(register: bool, frames: u64, capture_every: u64) -> Option<Vec<(u64, Vec<u8>)>> {
    let mut config = StageRendererConfig::default();
    config.base.vsync = false;
    config.base.allow_software_fallback = true;
    let mut run = OffscreenRun::new(WIDTH, HEIGHT, frames, ONE_TICK);
    run.capture_every = capture_every;
    let mut images = Vec::new();
    let result = App::new(WindowConfig::default())
        .stage_renderer_config(config)
        .plugin(Altar {
            register,
            mesh: MeshHandle(0),
        })
        .run_offscreen(run, &mut |_, _| {}, &mut |frame, rgba| {
            images.push((frame, rgba.to_vec()));
        });
    match result {
        Ok(report) => {
            assert_eq!(report.frames, frames);
            Some(images)
        }
        Err(GrimoireError::Render(RenderError::NoAdapter)) => {
            assert!(
                !adapter_required(),
                "no GPU adapter found although GRIMOIRE_REQUIRE_GPU_ADAPTER=1"
            );
            let _ = writeln!(
                std::io::stdout(),
                "\n::warning title=GPU test skipped::no GPU adapter found; grimoire asset_hook rendered nothing"
            );
            None
        }
        Err(error) => panic!("offscreen run failed: {error}"),
    }
}

/// Pixels differing from the clear colour by more than `threshold` in a colour channel.
fn drawn_pixels(rgba: &[u8], threshold: u8) -> usize {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[..3].iter().any(|&channel| channel > threshold))
        .count()
}

#[test]
fn a_mesh_registered_by_a_plugin_renders_offscreen() {
    let _serial = gpu_serial();
    let Some(registered) = run_altar(true, 3, 1) else {
        return;
    };
    let frames: Vec<u64> = registered.iter().map(|(frame, _)| *frame).collect();
    assert_eq!(
        frames,
        [0, 1, 2],
        "capture_every = 1 reads back every frame"
    );
    let pixels = (WIDTH * HEIGHT) as usize;
    for (frame, image) in &registered {
        assert_eq!(image.len(), pixels * 4);
        let drawn = drawn_pixels(image, 24);
        assert!(
            drawn > pixels / 5,
            "frame {frame}: the registered altar covers only {drawn} of {pixels} pixels"
        );
        let centre = ((HEIGHT / 2 * WIDTH + WIDTH / 2) * 4) as usize;
        assert!(
            image[centre] > image[centre + 2],
            "frame {frame}: the centre shows the orange altar, not {:?}",
            &image[centre..centre + 4]
        );
    }

    // The same instance without registration draws nothing: the hook reached the renderer that
    // renders the frame.
    let unregistered = run_altar(false, 3, 2).expect("the adapter was there a moment ago");
    let frames: Vec<u64> = unregistered.iter().map(|(frame, _)| *frame).collect();
    assert_eq!(
        frames,
        [0, 2],
        "capture_every = 2 reads back frames 0 and 2"
    );
    for (frame, image) in &unregistered {
        assert_eq!(
            drawn_pixels(image, 0),
            0,
            "frame {frame}: an unregistered mesh is not drawn"
        );
    }
}
