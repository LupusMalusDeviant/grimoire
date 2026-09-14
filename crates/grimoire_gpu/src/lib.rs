//! # grimoire_gpu
//!
//! Ownership layer over `wgpu`: instance, adapter and device creation, surfaces for
//! [`grimoire_platform::PlatformWindow`]s, offscreen render targets and read-back helpers.
//!
//! Layer rule: `wgpu` is re-exported for `grimoire_render` only. No crate above
//! `grimoire_render` depends on this crate, so `wgpu` types never reach the facade or games
//! (engine ADR 0002, GPU encapsulation boundary).
//!
//! ## Colour space
//!
//! Every render target handed out by this crate is written through an sRGB view
//! ([`WindowSurface::view_format`], [`OFFSCREEN_FORMAT`]). Shaders therefore output *linear*
//! colour and the GPU applies the sRGB transfer function on write; blending happens in linear
//! space.

mod context;
mod error;
mod offscreen;
mod readback;
mod surface;

pub use context::{ContextOptions, GpuContext};
pub use error::GpuError;
pub use offscreen::{OFFSCREEN_FORMAT, OffscreenTarget};
pub use readback::{padded_bytes_per_row, strip_row_padding};
pub use surface::{SurfaceFrame, WindowSurface, select_present_mode, select_surface_format};
pub use wgpu;
