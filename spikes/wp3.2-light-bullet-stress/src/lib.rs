//! Throwaway spike for plan 0002 WP3.2 (not part of the runtime path, lives only under
//! `spikes/`): OF-3.3 bullet representation (billboard impostor vs. instanced low-poly meshes at
//! 10k/20k instances) and light culling (CPU-froxel assignment vs. compute-shader clustering at
//! 256 lights), measured on the Linux CI runner. See `README.md` for every deviation from the
//! plan text's exact protocol.
//!
//! This crate is deliberately GPU-agnostic in its pure-logic modules ([`rng`], [`stats`],
//! [`camera`], the extraction/assignment functions in [`bullets`] and [`lights`]) so their
//! correctness can be checked with plain `cargo test`, independent of whether a GPU adapter is
//! available; only [`gpu`] and the two `src/bin/*.rs` binaries touch `grimoire_gpu`/`wgpu`.

pub mod bullets;
pub mod camera;
pub mod compute_cluster;
pub mod gpu;
pub mod lights;
pub mod rng;
pub mod stats;
