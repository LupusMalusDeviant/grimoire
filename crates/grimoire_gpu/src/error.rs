//! Errors of the GPU layer.

/// Failure of a GPU operation.
#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    /// No adapter satisfies the request (including the software fallback, if allowed).
    #[error("no suitable GPU adapter found: {0}")]
    NoAdapter(String),
    /// The window surface could not be created.
    #[error("surface creation failed: {0}")]
    CreateSurface(String),
    /// The adapter refused to create a device.
    #[error("device creation failed: {0}")]
    RequestDevice(String),
    /// The surface was lost or outdated and has been reconfigured; acquire again next frame.
    #[error("surface lost or outdated (reconfigured)")]
    SurfaceLost,
    /// No frame is available right now (timeout or occluded window); skip this frame.
    #[error("surface frame unavailable (timeout or occluded)")]
    SurfaceUnavailable,
    /// The surface has no valid size yet (for example a minimised window).
    #[error("surface has zero size")]
    ZeroSize,
    /// The GPU ran out of memory.
    #[error("GPU out of memory")]
    OutOfMemory,
    /// `wgpu` reported a validation or internal error.
    #[error("GPU validation error: {0}")]
    Validation(String),
    /// Mapping or reading back a buffer failed.
    #[error("GPU read-back failed: {0}")]
    Readback(String),
}

impl From<wgpu::Error> for GpuError {
    fn from(error: wgpu::Error) -> Self {
        match error {
            wgpu::Error::OutOfMemory { .. } => Self::OutOfMemory,
            other => Self::Validation(other.to_string()),
        }
    }
}

impl From<wgpu::RequestAdapterError> for GpuError {
    fn from(error: wgpu::RequestAdapterError) -> Self {
        Self::NoAdapter(error.to_string())
    }
}

impl From<wgpu::RequestDeviceError> for GpuError {
    fn from(error: wgpu::RequestDeviceError) -> Self {
        Self::RequestDevice(error.to_string())
    }
}

impl From<wgpu::CreateSurfaceError> for GpuError {
    fn from(error: wgpu::CreateSurfaceError) -> Self {
        Self::CreateSurface(error.to_string())
    }
}
