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
        ))?;
        let surface = WindowSurface::new(surface, window, &context, options.vsync)?;
        Ok((context, surface))
    }

    async fn from_instance(
        instance: wgpu::Instance,
        surface: Option<&wgpu::Surface<'static>>,
        options: ContextOptions,
        adapter_override: AdapterOverride,
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
        // WebGL2/GLES 3.0 baseline (no storage buffers, no compute), so GL 3.3-class adapters
        // qualify; resolution and buffer size follow the adapter.
        let required_limits = wgpu::Limits {
            max_buffer_size: adapter_limits.max_buffer_size,
            ..wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter_limits)
        };
        let descriptor = wgpu::DeviceDescriptor {
            label: Some("grimoire device"),
            required_features: wgpu::Features::empty(),
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
}
