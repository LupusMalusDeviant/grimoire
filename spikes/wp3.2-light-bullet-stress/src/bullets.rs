//! OF-3.3 spike data model: a synthetic pre-extraction bullet pool ([`SourceBullet`]) and the two
//! extraction targets under comparison — the real contract type [`grimoire_render::BulletInstance`]
//! (billboard impostor) and [`MeshBulletInstance`] (instanced low-poly mesh, a spike-local
//! stand-in for what a mesh-based `BulletInstance` would need to carry).
//!
//! [`MeshBulletInstance`] is deliberately **not** proposed as a replacement for
//! `BulletInstance` in code here — contract §6 makes `BulletInstance` its own only, changeable
//! only through a contract PR (WP1.7) before WP5.3. This module only measures what such a change
//! would cost; the report/ADR carries the proposal.

use grimoire_render::{BulletInstance, palette_space};

use crate::rng::Rng;

/// Pre-extraction logical bullet: the shape a facade's sigil pool would hand the render
/// extraction step (stand-in for the real WP5.3 adapter's input).
#[derive(Debug, Clone, Copy)]
pub struct SourceBullet {
    pub position: [f32; 2],
    pub radius: f32,
    pub rotation: f32,
    /// Silhouette table index (billboard variant) / mesh table index (mesh variant) — the same
    /// source field feeds both, exactly the kind of neutral visual id the facade would map
    /// (contract §6, "Kennungen").
    pub visual_id: u16,
    pub palette: u16,
    pub glow: u8,
}

/// Deterministic synthetic bullet pool scattered over a `40x40` world-unit patch centred on the
/// origin (comfortably inside the spike's camera frustum, see `src/bin/bullet_stress.rs`).
#[must_use]
pub fn synthetic_bullets(count: usize, seed: u64) -> Vec<SourceBullet> {
    let mut rng = Rng::new(seed);
    (0..count)
        .map(|_| SourceBullet {
            position: [rng.next_range(-20.0, 20.0), rng.next_range(-20.0, 20.0)],
            radius: rng.next_range(0.15, 0.25),
            rotation: rng.next_range(0.0, std::f32::consts::TAU),
            visual_id: (rng.next_u32() % 4) as u16,
            palette: (rng.next_u32() % 8) as u16,
            glow: (rng.next_u32() % 256) as u8,
        })
        .collect()
}

/// Billboard extraction: a flat copy into the real contract type — exactly the WP5.3 extraction
/// shape the render vertex/NFR budget is about. `out` is reused across repetitions (cleared, not
/// reallocated) to match the contract's "ohne Allokation je Frame" performance note.
pub fn extract_billboard(source: &[SourceBullet], out: &mut Vec<BulletInstance>) {
    out.clear();
    out.extend(source.iter().map(|bullet| BulletInstance {
        position: bullet.position,
        radius: bullet.radius,
        rotation: bullet.rotation,
        silhouette: bullet.visual_id,
        palette: bullet.palette,
        palette_space: palette_space::HOSTILE,
        glow: bullet.glow,
        flags: 0,
    }));
}

mod mesh_instance {
    // bytemuck's derive macros expand to `unsafe impl` blocks, same rationale as
    // `grimoire_render::BulletInstance` and `grimoire_render::procedural`'s mesh vertex type.
    #![allow(unsafe_code)]

    use bytemuck::{Pod, Zeroable};

    /// What an instanced low-poly-mesh bullet needs that `BulletInstance` (24 bytes, 2D position,
    /// no mesh selector) does not carry: a 3D position (the mesh has real vertical extent under
    /// the tilted camera, `BulletInstance::position` is ground-plane-only) and a mesh index in
    /// place of a 2D silhouette table index. `#[repr(C)]`, 28 bytes, no padding (verified by
    /// `crate::bullets::tests::mesh_bullet_instance_is_28_bytes_no_padding`).
    #[repr(C)]
    #[derive(Debug, Clone, Copy, PartialEq, Default, Pod, Zeroable)]
    pub struct MeshBulletInstance {
        /// World position; `z` lifts the mesh's centre above the ground plane by its own radius
        /// so it does not clip into the floor.
        pub position: [f32; 3],
        /// Radians about `+Z`, same convention as `BulletInstance::rotation`.
        pub rotation: f32,
        /// World-unit radius; the mesh itself is generated at unit radius and scaled here.
        pub scale: f32,
        /// Mesh table index — the mesh-variant equivalent of `BulletInstance::silhouette`.
        pub mesh_index: u16,
        pub palette: u16,
        /// Packed like `BulletInstance`'s tail, but as a single `u32` (low byte
        /// `palette_space`, next byte `glow`, top 16 bits unused) so the GPU vertex layout
        /// stays a plain 4-attribute, no-padding stride — see `src/bin/bullet_stress.rs`.
        pub palette_space_and_glow: u32,
    }
}
pub use mesh_instance::MeshBulletInstance;

/// Mesh extraction: same source pool as [`extract_billboard`], but into the larger
/// [`MeshBulletInstance`] — the concrete cost of the extra fields the mesh variant needs.
pub fn extract_mesh(source: &[SourceBullet], out: &mut Vec<MeshBulletInstance>) {
    out.clear();
    out.extend(source.iter().map(|bullet| MeshBulletInstance {
        position: [bullet.position[0], bullet.position[1], bullet.radius],
        rotation: bullet.rotation,
        scale: bullet.radius,
        mesh_index: bullet.visual_id,
        palette: bullet.palette,
        palette_space_and_glow: u32::from(palette_space::HOSTILE) | (u32::from(bullet.glow) << 8),
    }));
}

#[cfg(test)]
mod tests {
    use super::{MeshBulletInstance, extract_billboard, extract_mesh, synthetic_bullets};
    use grimoire_render::{BulletInstance, palette_space};

    #[test]
    fn bullet_instance_is_still_24_bytes() {
        // Not a contract test (that lives in grimoire_render itself) — a tripwire: if this ever
        // fails, the contract changed and every byte-size claim in this spike's README/ADR needs
        // re-checking.
        assert_eq!(std::mem::size_of::<BulletInstance>(), 24);
    }

    #[test]
    fn mesh_bullet_instance_is_28_bytes_no_padding() {
        assert_eq!(std::mem::size_of::<MeshBulletInstance>(), 28);
        assert_eq!(std::mem::align_of::<MeshBulletInstance>(), 4);
    }

    #[test]
    fn synthetic_bullets_is_deterministic() {
        let a = synthetic_bullets(64, 1234);
        let b = synthetic_bullets(64, 1234);
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.position, y.position);
            assert_eq!(x.visual_id, y.visual_id);
        }
    }

    #[test]
    fn billboard_extraction_preserves_count_and_rejects_no_palette_space() {
        let source = synthetic_bullets(32, 7);
        let mut out = Vec::new();
        extract_billboard(&source, &mut out);
        assert_eq!(out.len(), source.len());
        assert!(
            out.iter()
                .all(|instance| instance.palette_space == palette_space::HOSTILE)
        );
    }

    #[test]
    fn extract_billboard_reuses_the_output_allocation() {
        let source = synthetic_bullets(16, 9);
        let mut out = Vec::with_capacity(16);
        extract_billboard(&source, &mut out);
        let capacity_before = out.capacity();
        extract_billboard(&source, &mut out);
        assert_eq!(out.capacity(), capacity_before, "extraction reallocated");
    }

    #[test]
    fn mesh_extraction_lifts_the_centre_by_its_own_radius() {
        let source = synthetic_bullets(8, 3);
        let mut out = Vec::new();
        extract_mesh(&source, &mut out);
        for (bullet, instance) in source.iter().zip(out.iter()) {
            assert_eq!(instance.position[2], bullet.radius);
            assert_eq!(instance.scale, bullet.radius);
        }
    }
}
