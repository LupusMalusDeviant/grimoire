//! # grimoire_gpu
//!
//! Ownership layer over `wgpu`: instance, adapter and device creation, surfaces for
//! [`grimoire_platform::PlatformWindow`]s, offscreen render targets and read-back helpers.
//!
//! Layer rule: `wgpu` is re-exported for `grimoire_render` only. No crate above
//! `grimoire_render` depends on this crate, so `wgpu` types never reach the facade or games.

pub use wgpu;
