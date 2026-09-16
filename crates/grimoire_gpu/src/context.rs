//! Instance, adapter and device ownership.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use grimoire_platform::PlatformWindow;
use grimoire_platform::raw_window_handle::{DisplayHandle, HandleError, HasDisplayHandle};

use crate::error::GpuError;
use crate::surface::WindowSurface;

/// Environment variable forcing the adapter kind of every context created in the process.
///
/// `software` (or `cpu`) restricts the instance to the backend that offers the platform's CPU
/// adapter (WARP via DX12 on Windows, lavapipe via Vulkan on Linux) and accepts only an adapter of
/// type CPU, so no rendering work reaches a hardware GPU. `auto` keeps the behaviour of
/// [`ContextOptions`]. Unset or invalid values mean `auto`.
pub const ENV_GPU_ADAPTER: &str = "GRIMOIRE_GPU_ADAPTER";

#[cfg(windows)]
const SOFTWARE_BACKENDS: wgpu::Backends = wgpu::Backends::DX12;
#[cfg(all(unix, not(target_vendor = "apple")))]
const SOFTWARE_BACKENDS: wgpu::Backends = wgpu::Backends::VULKAN;
#[cfg(not(any(windows, all(unix, not(target_vendor = "apple")))))]
const SOFTWARE_BACKENDS: wgpu::Backends = wgpu::Backends::all();

/// Adapter kind forced through [`ENV_GPU_ADAPTER`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AdapterOverride {
    /// Follow [`ContextOptions`].
    #[default]
    Auto,
    /// Use a CPU adapter only; fail with [`GpuError::NoAdapter`] if there is none.
    Software,
}

impl AdapterOverride {
    /// Parses `auto`, `software` or `cpu` (case-insensitive).
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "software" | "cpu" => Some(Self::Software),
            _ => None,
        }
    }

    /// Reads [`ENV_GPU_ADAPTER`] from the process environment.
    #[must_use]
    pub fn from_env() -> Self {
        std::env::var(ENV_GPU_ADAPTER)
            .ok()
            .as_deref()
            .and_then(Self::parse)
            .unwrap_or_default()
    }

    fn restrict_backends(self, descriptor: &mut wgpu::InstanceDescriptor) {
        if self == Self::Software {
            descriptor.backends = SOFTWARE_BACKENDS;
        }
    }
}

/// Adapter selection parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextOptions {
    /// Prefer a discrete (high-performance) GPU over an integrated one.
    pub high_performance: bool,
    /// Accept a software adapter (WARP, lavapipe, ...) when no hardware adapter is found.
    pub allow_software_fallback: bool,
    /// Synchronise presentation with the display refresh (window contexts only).
    pub vsync: bool,
}

impl Default for ContextOptions {
    fn default() -> Self {
        Self {
            high_performance: true,
            allow_software_fallback: false,
            vsync: true,
        }
    }
}

/// Formats [`GpuContext::adapter_report_line`]; a free function so it is testable without a real
/// adapter (WP2.1).
fn format_adapter_line(
    name: &str,
    backend_name: &str,
    device_type: wgpu::DeviceType,
    driver_info: &str,
) -> String {
    let driver = if driver_info.is_empty() {
        "unknown"
    } else {
        driver_info
    };
    format!(
        "grimoire-gpu-adapter: name={name} backend={backend_name} device_type={device_type:?} driver={driver}"
    )
}

/// Formats [`GpuContext::capability_report_lines`]; a free function so it is testable without a
/// real adapter (WP3.1, downlevel check ahead of plan 0002 WP3.4's clustered forward+ pass).
///
/// Reports a fixed, meaningful subset of [`wgpu::DownlevelFlags`] (whether storage buffers can be
/// read in the fragment stage, written in the fragment stage, used in compute, and used in the
/// vertex stage) and the limits that bound the light/cluster storage buffers WP3.4 will need
/// (`max_storage_buffers_per_shader_stage`, `max_storage_buffer_binding_size`, the compute
/// workgroup size and invocation limits, `max_uniform_buffer_binding_size`, `max_bind_groups`,
/// `max_texture_dimension_2d`) — not every field `wgpu::Limits` defines.
fn format_capability_lines(
    features: wgpu::Features,
    downlevel: wgpu::DownlevelCapabilities,
    limits: wgpu::Limits,
) -> Vec<String> {
    vec![
        format!("grimoire-gpu-features: {features:?}"),
        format!(
            "grimoire-gpu-downlevel: shader_model={:?} fragment_storage={} fragment_writable_storage={} compute_shaders={} vertex_storage={}",
            downlevel.shader_model,
            downlevel
                .flags
                .contains(wgpu::DownlevelFlags::FRAGMENT_STORAGE),
            downlevel
                .flags
                .contains(wgpu::DownlevelFlags::FRAGMENT_WRITABLE_STORAGE),
            downlevel
                .flags
                .contains(wgpu::DownlevelFlags::COMPUTE_SHADERS),
            downlevel
                .flags
                .contains(wgpu::DownlevelFlags::VERTEX_STORAGE),
        ),
        format!(
            "grimoire-gpu-limits: max_storage_buffers_per_shader_stage={} max_storage_buffer_binding_size={} max_compute_workgroup_size={}x{}x{} max_compute_invocations_per_workgroup={} max_uniform_buffer_binding_size={} max_bind_groups={} max_texture_dimension_2d={}",
            limits.max_storage_buffers_per_shader_stage,
            limits.max_storage_buffer_binding_size,
            limits.max_compute_workgroup_size_x,
            limits.max_compute_workgroup_size_y,
            limits.max_compute_workgroup_size_z,
            limits.max_compute_invocations_per_workgroup,
            limits.max_uniform_buffer_binding_size,
            limits.max_bind_groups,
            limits.max_texture_dimension_2d,
        ),
    ]
}

/// Floor [`conservative_required_limits`] requests for `max_storage_buffer_binding_size` (P1
/// skinning addendum, contract §6 changelog 2026-09-16): comfortably above one instance's full
/// 256-bone, 16 KiB palette (engine ADR-0013's own headroom calculation) so many skinned
/// instances' palettes can be concatenated into the mesh pass's single per-frame bone buffer
/// (`grimoire_render::mesh_pass`) without hitting this floor — still tiny next to any of the three
/// measured CI target adapters (weakest: lavapipe at 128 MiB, ADR-0013). Also comfortably above
/// plan 0002 WP3.4's own worst case, the High-budget light index list at ≈3.41 MiB
/// (`grimoire_render::cluster_layout::worst_case_total_bytes`, engine ADR-0013).
const MIN_STORAGE_BUFFER_BINDING_SIZE: u64 = 16 * 1024 * 1024;

/// Floor [`conservative_required_limits`] requests for `max_storage_buffers_per_shader_stage`
/// (plan 0002 WP3.4, engine ADR-0015 "compute clustering"): the clustered forward+ fragment shader
/// binds three read-only storage buffers at once (group 4, `grimoire_render::cluster_pass` — the
/// light list, cluster table and index list, `grimoire_render::cluster_layout`), and its compute
/// counterpart (`cluster.wgsl`) binds three more (one writable) alongside a params uniform. Three
/// is this engine version's actual peak per-stage need (the skinning package's single vertex-stage
/// bone buffer, contract §6 changelog 2026-09-16, needs only one) — comfortably under every
/// measured CI target adapter's real limit for this count (weakest: macOS at 29, ADR-0013's
/// table).
const MIN_STORAGE_BUFFERS_PER_SHADER_STAGE: u32 = 3;

/// Floor [`conservative_required_limits`] requests for `max_bind_groups` (plan 0002 WP3.4): the
/// clustered forward+ pass adds a fifth bind group (group 4, the cluster/light buffers,
/// `grimoire_render::cluster_pass`) to the mesh pipeline's existing four (camera, textures, shadow
/// map, bone matrices) — comfortably under every measured CI target adapter's real limit (all
/// three measured at 8, ADR-0013's table).
const MIN_BIND_GROUPS: u32 = 5;

/// Floor [`conservative_required_limits`] requests for the compute-workgroup-shape limits (plan
/// 0002 WP3.4, engine ADR-0015): `cluster.wgsl` dispatches a one-dimensional
/// `@workgroup_size(64)` compute shader (`max_compute_invocations_per_workgroup`/
/// `max_compute_workgroup_size_x` both need to cover the 64) over
/// `ceil(cluster_layout::CLUSTER_COUNT / 64) = 54` workgroups along one dimension
/// (`max_compute_workgroups_per_dimension`) — comfortably under every measured CI target
/// adapter's real compute limits (weakest workgroup-size number, Windows/WARP, still allows
/// 1024x1024x64 with up to 1024 invocations per workgroup; `max_compute_workgroups_per_dimension`
/// is not itself in ADR-0013's measured table, but every native `wgpu` backend meets the
/// `downlevel_defaults()` figure of 65535, far past the 64 requested here).
const MIN_COMPUTE_WORKGROUP_SIZE: u32 = 64;
/// See [`MIN_COMPUTE_WORKGROUP_SIZE`]; kept as a second named constant even though it shares the
/// same value today, since it bounds a different limit (`max_compute_workgroups_per_dimension`,
/// not workgroup *size*) that could reasonably need to grow independently later (a larger froxel
/// grid, for instance).
const MIN_COMPUTE_WORKGROUPS_PER_DIMENSION: u32 = 64;

/// WebGL2/GLES 3.0-*adjacent* baseline: every field follows the strict WebGL2 downlevel defaults
/// (resolution and buffer size following the adapter) **except** a handful of floors raised for
/// features this engine actually uses in its shipping renderer — a single storage buffer binding
/// for the P1 skinning package's bone matrix palette (`grimoire_render::mesh_pass`, contract §6
/// changelog 2026-09-16), and, from plan 0002 WP3.4 (engine ADR-0015 "compute clustering"), real
/// compute shaders and up to three storage buffers per stage for the clustered forward+ pass
/// (`grimoire_render::cluster_pass`, `cluster.wgsl`, `mesh.wgsl`'s group 4). Engine ADR-0013
/// measured every one of these downlevel flags and limits as satisfied, with large margin, on all
/// three CI target adapters (Windows/WARP, Linux/lavapipe, macOS Apple-Paravirtual/Metal) — but
/// using a *separate*, elevated context ([`GpuContext::new_offscreen_with_limits`]) reserved for
/// that measurement; this function is what [`GpuContext::new_offscreen`]/
/// [`GpuContext::new_for_window`] actually request for every renderer. Before the skinning
/// addendum this function asked for zero storage buffers regardless, which made the skinning
/// pipeline fail `create_render_pipeline` validation the first time it was actually exercised;
/// before WP3.4 it asked for zero compute limits and only one storage buffer per stage, which
/// would have failed the same way the first time `ClusterPass::new` actually ran on a renderer
/// built through the normal (non-elevated) constructors.
///
/// **Deviation flagged for the PO** (originally the P1 skinning PR description, contract §6
/// "grimoire_gpu ist in P0 frei gestaltbar, solange nur grimoire_render es nutzt"; WP3.4 widens the
/// same deviation): a genuine future WebGL2/GLES 3.0 backend (P2, hypothetical — none exists
/// today) cannot provide compute shaders or more than a token number of storage buffers under any
/// circumstance, so these floors quietly retire that specific aspiration a pure WebGL2 baseline
/// previously kept open in principle. No currently supported adapter is affected (all three
/// measured targets already exceed every new floor by orders of magnitude, per ADR-0013's table);
/// restoring strict WebGL2/GLES compatibility for a future target, if one is ever built, is that
/// target's own problem — ADR-0013's own "Empfehlung" section already sketches a uniform-buffer/
/// texture-encoded fallback for exactly this case.
fn conservative_required_limits(adapter_limits: wgpu::Limits) -> wgpu::Limits {
    wgpu::Limits {
        max_buffer_size: adapter_limits.max_buffer_size,
        max_storage_buffers_per_shader_stage: adapter_limits
            .max_storage_buffers_per_shader_stage
            .min(MIN_STORAGE_BUFFERS_PER_SHADER_STAGE),
        max_storage_buffer_binding_size: adapter_limits
            .max_storage_buffer_binding_size
            .min(MIN_STORAGE_BUFFER_BINDING_SIZE),
        max_bind_groups: adapter_limits.max_bind_groups.min(MIN_BIND_GROUPS),
        max_compute_invocations_per_workgroup: adapter_limits
            .max_compute_invocations_per_workgroup
            .min(MIN_COMPUTE_WORKGROUP_SIZE),
        max_compute_workgroup_size_x: adapter_limits
            .max_compute_workgroup_size_x
            .min(MIN_COMPUTE_WORKGROUP_SIZE),
        // `cluster.wgsl`'s workgroup is one-dimensional (64x1x1): the Y/Z extents only need to
        // allow `1`, never the full `MIN_COMPUTE_WORKGROUP_SIZE`.
        max_compute_workgroup_size_y: adapter_limits.max_compute_workgroup_size_y.min(1),
        max_compute_workgroup_size_z: adapter_limits.max_compute_workgroup_size_z.min(1),
        max_compute_workgroups_per_dimension: adapter_limits
            .max_compute_workgroups_per_dimension
            .min(MIN_COMPUTE_WORKGROUPS_PER_DIMENSION),
        ..wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter_limits)
    }
}

/// `wgpu` requires `Debug` on the instance display handle; `dyn PlatformWindow` has none.
struct WindowDisplay(Arc<dyn PlatformWindow>);

impl fmt::Debug for WindowDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WindowDisplay")
    }
}

impl HasDisplayHandle for WindowDisplay {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        self.0.display_handle()
    }
}

/// An initialised GPU: instance, adapter, device and queue.
pub struct GpuContext {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    info: wgpu::AdapterInfo,
    backend_name: String,
    device_lost: Arc<AtomicBool>,
}

impl fmt::Debug for GpuContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GpuContext")
            .field("adapter", &self.info.name)
            .field("backend", &self.backend_name)
            .finish_non_exhaustive()
    }
}

impl GpuContext {
    /// Creates a context without any window, for offscreen rendering and tests.
    ///
    /// Honours [`ENV_GPU_ADAPTER`].
    ///
    /// # Errors
    /// [`GpuError::NoAdapter`] if no adapter exists, [`GpuError::RequestDevice`] if the device
    /// cannot be created.
    pub fn new_offscreen(options: ContextOptions) -> Result<Self, GpuError> {
        let adapter_override = AdapterOverride::from_env();
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        adapter_override.restrict_backends(&mut descriptor);
        let instance = wgpu::Instance::new(descriptor);
        pollster::block_on(Self::from_instance(
            instance,
            None,
            options,
            adapter_override,
            wgpu::Features::empty(),
            conservative_required_limits,
        ))
    }

    /// Creates an offscreen context like [`GpuContext::new_offscreen`], but requests
    /// `required_features` and limits built by `required_limits` (from the adapter's own limits)
    /// instead of the conservative WebGL2 baseline [`GpuContext::new_offscreen`] uses.
    ///
    /// For probing or exercising capabilities beyond the shipping baseline (WP3.1, groundwork for
    /// the light/cluster storage buffers of plan 0002 WP3.4's clustered forward+ pass); the
    /// renderer itself keeps using [`GpuContext::new_offscreen`]. Honours [`ENV_GPU_ADAPTER`].
    ///
    /// # Errors
    /// [`GpuError::NoAdapter`] if no adapter exists, [`GpuError::RequestDevice`] if the adapter
    /// refuses a device with `required_features`/the limits `required_limits` computed.
    pub fn new_offscreen_with_limits(
        options: ContextOptions,
        required_features: wgpu::Features,
        required_limits: impl FnOnce(wgpu::Limits) -> wgpu::Limits,
    ) -> Result<Self, GpuError> {
        let adapter_override = AdapterOverride::from_env();
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        adapter_override.restrict_backends(&mut descriptor);
        let instance = wgpu::Instance::new(descriptor);
        pollster::block_on(Self::from_instance(
            instance,
            None,
            options,
            adapter_override,
            required_features,
            required_limits,
        ))
    }

    /// Creates a context and a configured surface presenting to `window`.
    ///
    /// The adapter is chosen to be compatible with the window surface. If the window currently
    /// has an empty size, the surface stays unconfigured until [`WindowSurface::resize`] is called
    /// with a valid size. Honours [`ENV_GPU_ADAPTER`].
    ///
    /// # Errors
    /// [`GpuError::CreateSurface`] if the window handles are unusable, [`GpuError::NoAdapter`] if
    /// no adapter can present to the surface, [`GpuError::RequestDevice`] if the device cannot be
    /// created, [`GpuError::Validation`] or [`GpuError::OutOfMemory`] if the initial surface
    /// configuration is rejected.
    ///
    /// # Panics
    /// On macOS (Metal) `wgpu` panics if this is not called on the main thread; create window
    /// contexts from the platform event loop thread (for example in `AppHandler::init`).
    pub fn new_for_window(
        window: Arc<dyn PlatformWindow>,
        options: ContextOptions,
    ) -> Result<(Self, WindowSurface), GpuError> {
        let adapter_override = AdapterOverride::from_env();
        let mut descriptor = wgpu::InstanceDescriptor::new_with_display_handle(Box::new(
            WindowDisplay(Arc::clone(&window)),
        ));
        adapter_override.restrict_backends(&mut descriptor);
        let instance = wgpu::Instance::new(descriptor);
        // The surface owns an `Arc` of the window, so the handles outlive the surface.
        let surface = instance.create_surface(Arc::clone(&window))?;
        let context = pollster::block_on(Self::from_instance(
            instance,
            Some(&surface),
            options,
            adapter_override,
            wgpu::Features::empty(),
            conservative_required_limits,
        ))?;
        let surface = WindowSurface::new(surface, window, &context, options.vsync)?;
        Ok((context, surface))
    }

    async fn from_instance(
        instance: wgpu::Instance,
        surface: Option<&wgpu::Surface<'static>>,
        options: ContextOptions,
        adapter_override: AdapterOverride,
        required_features: wgpu::Features,
        required_limits: impl FnOnce(wgpu::Limits) -> wgpu::Limits,
    ) -> Result<Self, GpuError> {
        let power_preference = if options.high_performance {
            wgpu::PowerPreference::HighPerformance
        } else {
            wgpu::PowerPreference::LowPower
        };
        let mut request = wgpu::RequestAdapterOptions {
            power_preference,
            force_fallback_adapter: adapter_override == AdapterOverride::Software,
            compatible_surface: surface,
            apply_limit_buckets: false,
        };
        let adapter = match (adapter_override, instance.request_adapter(&request).await) {
            (AdapterOverride::Software, Ok(adapter)) => {
                let info = adapter.get_info();
                if info.device_type != wgpu::DeviceType::Cpu {
                    return Err(GpuError::NoAdapter(format!(
                        "{ENV_GPU_ADAPTER}=software, but the fallback adapter \"{}\" is a {:?} device",
                        info.name, info.device_type
                    )));
                }
                adapter
            }
            (AdapterOverride::Software, Err(error)) => return Err(error.into()),
            (AdapterOverride::Auto, Ok(adapter)) => adapter,
            (AdapterOverride::Auto, Err(hardware_error)) if options.allow_software_fallback => {
                log::warn!("no hardware GPU adapter ({hardware_error}); trying software fallback");
                request.force_fallback_adapter = true;
                instance.request_adapter(&request).await?
            }
            (AdapterOverride::Auto, Err(error)) => return Err(error.into()),
        };

        let info = adapter.get_info();
        let backend_name = format!("{:?}", info.backend);
        log::info!(
            "GPU adapter: {} ({:?}), backend {backend_name}",
            info.name,
            info.device_type
        );

        let adapter_limits = adapter.limits();
        let required_limits = required_limits(adapter_limits);
        let descriptor = wgpu::DeviceDescriptor {
            label: Some("grimoire device"),
            required_features,
            required_limits,
            ..Default::default()
        };
        let (device, queue) = adapter.request_device(&descriptor).await?;
        // The default handler panics; errors outside explicit error scopes are logged instead.
        device.on_uncaptured_error(Arc::new(|error: wgpu::Error| {
            log::error!("uncaptured wgpu error: {error}");
        }));
        let device_lost = Arc::new(AtomicBool::new(false));
        let lost_flag = Arc::clone(&device_lost);
        device.set_device_lost_callback(move |reason, message| {
            match reason {
                wgpu::DeviceLostReason::Destroyed => log::debug!("GPU device destroyed: {message}"),
                wgpu::DeviceLostReason::Unknown => log::error!("GPU device lost: {message}"),
            }
            lost_flag.store(true, Ordering::Relaxed);
        });

        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            info,
            backend_name,
            device_lost,
        })
    }

    /// Whether the device has been lost (driver reset, GPU removed, ...). A lost device never
    /// recovers; a new context has to be created.
    #[must_use]
    pub fn is_device_lost(&self) -> bool {
        self.device_lost.load(Ordering::Relaxed)
    }

    /// The `wgpu` instance.
    #[must_use]
    pub fn instance(&self) -> &wgpu::Instance {
        &self.instance
    }

    /// The selected adapter.
    #[must_use]
    pub fn adapter(&self) -> &wgpu::Adapter {
        &self.adapter
    }

    /// The logical device.
    #[must_use]
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// The command queue of [`GpuContext::device`].
    #[must_use]
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Information about the selected adapter.
    #[must_use]
    pub fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.info
    }

    /// Backend name such as `"Vulkan"`, `"Metal"`, `"Dx12"` or `"Gl"`.
    #[must_use]
    pub fn backend_name(&self) -> &str {
        &self.backend_name
    }

    /// Single greppable line identifying the selected adapter for CI logs (WP2.1, groundwork for
    /// OF-18.2): name, backend, device type (`Cpu`, `IntegratedGpu`, `DiscreteGpu`, `VirtualGpu`
    /// or `Other`) and driver info, if `wgpu` reports any. Every offscreen/GPU test binary is
    /// expected to print this once per binary (not once per test), so CI can show, per runner,
    /// exactly what each test run rendered on.
    #[must_use]
    pub fn adapter_report_line(&self) -> String {
        format_adapter_line(
            &self.info.name,
            &self.backend_name,
            self.info.device_type,
            &self.info.driver_info,
        )
    }

    /// Capability report lines for CI job summaries (WP3.1, downlevel check ahead of plan 0002
    /// WP3.4's clustered forward+ pass): the *adapter's* own features, downlevel flags and limits.
    /// These reflect what the hardware/driver can do, independent of whatever (possibly more
    /// conservative) `required_features`/limits this particular [`GpuContext`]'s device was
    /// created with — [`GpuContext::new_offscreen`]'s device, for example, is deliberately
    /// restricted to a WebGL2 baseline that supports neither storage buffers nor compute at all.
    /// Three greppable lines, expected once per test binary like
    /// [`GpuContext::adapter_report_line`].
    #[must_use]
    pub fn capability_report_lines(&self) -> Vec<String> {
        format_capability_lines(
            self.adapter.features(),
            self.adapter.get_downlevel_capabilities(),
            self.adapter.limits(),
        )
    }

    /// Runs `create` while capturing out-of-memory and validation errors, turning them into a
    /// [`GpuError`] instead of the logging uncaptured-error handler.
    ///
    /// # Errors
    /// [`GpuError::OutOfMemory`] or [`GpuError::Validation`] if `create` triggered such an error.
    pub fn capture_errors<T>(
        &self,
        create: impl FnOnce(&wgpu::Device) -> T,
    ) -> Result<T, GpuError> {
        let out_of_memory = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let value = create(&self.device);
        let validation_error = pollster::block_on(validation.pop());
        let out_of_memory_error = pollster::block_on(out_of_memory.pop());
        match out_of_memory_error.or(validation_error) {
            Some(error) => Err(error.into()),
            None => Ok(value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_adapter_overrides() {
        assert_eq!(
            AdapterOverride::parse("software"),
            Some(AdapterOverride::Software)
        );
        assert_eq!(
            AdapterOverride::parse(" CPU "),
            Some(AdapterOverride::Software)
        );
        assert_eq!(AdapterOverride::parse("Auto"), Some(AdapterOverride::Auto));
        assert_eq!(AdapterOverride::parse("gpu"), None);
        assert_eq!(AdapterOverride::parse(""), None);
    }

    #[test]
    fn software_override_restricts_the_instance_to_the_cpu_adapter_backend() {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        AdapterOverride::Software.restrict_backends(&mut descriptor);
        assert_eq!(descriptor.backends, SOFTWARE_BACKENDS);
    }

    #[test]
    fn auto_override_keeps_the_default_backends() {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        let default_backends = descriptor.backends;
        AdapterOverride::Auto.restrict_backends(&mut descriptor);
        assert_eq!(descriptor.backends, default_backends);
    }

    #[test]
    fn adapter_report_line_falls_back_to_unknown_driver() {
        let line = format_adapter_line("llvmpipe", "Vulkan", wgpu::DeviceType::Cpu, "");
        assert_eq!(
            line,
            "grimoire-gpu-adapter: name=llvmpipe backend=Vulkan device_type=Cpu driver=unknown"
        );
    }

    #[test]
    fn adapter_report_line_includes_driver_info_when_present() {
        let line = format_adapter_line(
            "Microsoft Basic Render Driver",
            "Dx12",
            wgpu::DeviceType::Cpu,
            "10.0.26200",
        );
        assert_eq!(
            line,
            "grimoire-gpu-adapter: name=Microsoft Basic Render Driver backend=Dx12 device_type=Cpu driver=10.0.26200"
        );
    }

    #[test]
    fn capability_lines_report_the_downlevel_flags_wp3_1_cares_about() {
        let downlevel = wgpu::DownlevelCapabilities {
            flags: wgpu::DownlevelFlags::FRAGMENT_STORAGE | wgpu::DownlevelFlags::COMPUTE_SHADERS,
            limits: wgpu::DownlevelLimits::default(),
            shader_model: wgpu::ShaderModel::Sm5,
        };
        let lines =
            format_capability_lines(wgpu::Features::empty(), downlevel, wgpu::Limits::default());
        assert_eq!(lines.len(), 3);
        // `wgpu::Features`'s exact `Debug` formatting is `wgpu`'s to define; only the greppable
        // prefix (what CI's report script matches on) is this crate's contract.
        assert!(lines[0].starts_with("grimoire-gpu-features: "));
        assert_eq!(
            lines[1],
            "grimoire-gpu-downlevel: shader_model=Sm5 fragment_storage=true fragment_writable_storage=false compute_shaders=true vertex_storage=false"
        );
        assert!(lines[2].starts_with("grimoire-gpu-limits: "));
    }

    #[test]
    fn capability_lines_report_the_measured_limits_wp3_1_needs_for_clustered_forward_plus() {
        let downlevel = wgpu::DownlevelCapabilities::default();
        let limits = wgpu::Limits {
            max_storage_buffers_per_shader_stage: 8,
            max_storage_buffer_binding_size: 128 << 20,
            max_compute_workgroup_size_x: 256,
            max_compute_workgroup_size_y: 256,
            max_compute_workgroup_size_z: 64,
            max_compute_invocations_per_workgroup: 256,
            max_uniform_buffer_binding_size: 64 << 10,
            max_bind_groups: 4,
            max_texture_dimension_2d: 8192,
            ..wgpu::Limits::default()
        };
        let lines = format_capability_lines(wgpu::Features::empty(), downlevel, limits);
        assert_eq!(
            lines[2],
            "grimoire-gpu-limits: max_storage_buffers_per_shader_stage=8 max_storage_buffer_binding_size=134217728 max_compute_workgroup_size=256x256x64 max_compute_invocations_per_workgroup=256 max_uniform_buffer_binding_size=65536 max_bind_groups=4 max_texture_dimension_2d=8192"
        );
    }

    #[test]
    fn conservative_required_limits_allow_compute_and_three_storage_buffers_per_stage() {
        // Plan 0002 WP3.4 (engine ADR-0015 "compute clustering"): three storage buffers per
        // stage, five bind groups and a 64-invocation, 64-workgroup compute shape are now
        // requested, for `grimoire_render::cluster_pass`'s clustered forward+ pass — widening the
        // P1 skinning addendum's single-storage-buffer, zero-compute floor (contract §6 changelog
        // 2026-09-16).
        let adapter_limits = wgpu::Limits {
            max_storage_buffers_per_shader_stage: 8,
            max_storage_buffer_binding_size: 134_217_728, // 128 MiB, ADR-0013's lavapipe figure
            max_bind_groups: 8,                           // ADR-0013's table, all three adapters
            max_compute_invocations_per_workgroup: 256,
            max_compute_workgroup_size_x: 256,
            max_compute_workgroup_size_y: 256,
            max_compute_workgroup_size_z: 64,
            max_compute_workgroups_per_dimension: 65_535,
            ..wgpu::Limits::default()
        };
        let required = conservative_required_limits(adapter_limits);
        assert_eq!(required.max_storage_buffers_per_shader_stage, 3);
        assert_eq!(
            required.max_storage_buffer_binding_size,
            MIN_STORAGE_BUFFER_BINDING_SIZE
        );
        assert_eq!(required.max_bind_groups, 5);
        assert_eq!(required.max_compute_invocations_per_workgroup, 64);
        assert_eq!(required.max_compute_workgroup_size_x, 64);
        assert_eq!(required.max_compute_workgroup_size_y, 1);
        assert_eq!(required.max_compute_workgroup_size_z, 1);
        assert_eq!(required.max_compute_workgroups_per_dimension, 64);
    }

    #[test]
    fn conservative_required_limits_never_asks_for_more_than_the_adapter_has() {
        // An adapter reporting less than a floor (storage buffers, binding size, bind groups or
        // compute limits) must never be asked for more than it actually has —
        // `request_device` would simply fail otherwise.
        let adapter_limits = wgpu::Limits {
            max_storage_buffers_per_shader_stage: 0,
            max_storage_buffer_binding_size: 4096,
            max_bind_groups: 2,
            max_compute_invocations_per_workgroup: 0,
            max_compute_workgroup_size_x: 0,
            max_compute_workgroup_size_y: 0,
            max_compute_workgroup_size_z: 0,
            max_compute_workgroups_per_dimension: 0,
            ..wgpu::Limits::default()
        };
        let required = conservative_required_limits(adapter_limits);
        assert_eq!(required.max_storage_buffers_per_shader_stage, 0);
        assert_eq!(required.max_storage_buffer_binding_size, 4096);
        assert_eq!(required.max_bind_groups, 2);
        assert_eq!(required.max_compute_invocations_per_workgroup, 0);
        assert_eq!(required.max_compute_workgroup_size_x, 0);
        assert_eq!(required.max_compute_workgroups_per_dimension, 0);
    }
}
