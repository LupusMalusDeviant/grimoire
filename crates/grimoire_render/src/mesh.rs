//! CPU-side mesh geometry and the mesh registry (plan 0002 WP2.3; contract §6, "the registry
//! itself... is WP2.3's job").
//!
//! [`MeshData`] is the CPU representation of a static triangle mesh: positions, normals, UVs and
//! indices. [`MeshRegistry`] validates a [`MeshData`] and, if it is structurally sound, stores it
//! under a deterministically assigned [`crate::MeshHandle`] — the same handle type
//! [`crate::MeshInstance::mesh`] references, frozen since WP2.2. Registration never panics: an
//! empty or structurally invalid mesh is rejected with a [`MeshError`] instead.
//!
//! This module is entirely GPU-free (no `wgpu` type appears here, per engine ADR-0002); uploading
//! a registered mesh's geometry to static GPU buffers is the `mesh_pass` module's job
//! ([`crate::WgpuRenderer::register_mesh`]).

use crate::MeshHandle;

mod vertex {
    // bytemuck's derive macros expand to `unsafe impl` blocks, like `SpriteInstance` and
    // `BulletInstance` elsewhere in this crate.
    #![allow(unsafe_code)]

    /// One mesh vertex: position, normal, texture coordinate, (P1 skinning addendum, contract §6
    /// changelog 2026-09-16) up to four bone influences, and (texture-quality package, strand B2)
    /// an optional per-vertex tangent. Layout is `#[repr(C)]`, 72 bytes, no padding, uploaded to
    /// the GPU verbatim as a vertex buffer element.
    #[repr(C)]
    #[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
    pub struct MeshVertex {
        /// Model-space position.
        pub position: [f32; 3],
        /// Surface normal, model space. Registration does not require exactly unit length
        /// (generation and, later, authoring tools may leave tiny deviations); the mesh pass
        /// re-normalises it after applying the model transform.
        pub normal: [f32; 3],
        /// Texture coordinate, used from WP2.5 onward to sample [`crate::PbrMaterial`]'s optional
        /// base colour, normal and occlusion-roughness-metallic textures (`mesh.wgsl`); WP2.3's
        /// provisional shading did not read it.
        pub uv: [f32; 2],
        /// Up to four bone indices this vertex is skinned against, indexing into the instance's
        /// bone matrix palette ([`crate::MeshInstance::skin`]). A mesh with no skeleton (every
        /// procedural mesh in this crate) uses `[0, 0, 0, 0]` with [`MeshVertex::weights`]
        /// `[1.0, 0.0, 0.0, 0.0]` — bone 0 with full weight — which the skinning vertex path
        /// treats as the identity transform when the instance's palette supplies one. Registration
        /// does not check these against a skeleton (P1 has no CPU-side skeleton type); the pack
        /// decoder (`figure_format`) checks each index against its skeleton's joint count before a
        /// mesh ever reaches [`crate::WgpuRenderer::register_mesh`].
        pub joints: [u16; 4],
        /// Skinning weight for each of [`MeshVertex::joints`], summing to `1.0` (tolerance `1e-3`,
        /// checked by [`crate::mesh::MeshData::validate`] and, before that, by the pack decoder).
        pub weights: [f32; 4],
        /// Model-space tangent (texture-quality package, strand B2, additive): `xyz` a unit vector
        /// in the surface plane, `w` the bitangent handedness (`+1.0` or `-1.0`) —
        /// `mesh.wgsl` builds `bitangent = cross(normal, tangent.xyz) * tangent.w`. The default,
        /// `[0.0, 0.0, 0.0, 0.0]` (every procedural mesh in this crate, and any `FNP_MESH` primitive
        /// without a `TEXCOORD_0`, shared spec), is the agreed "no tangent" sentinel:
        /// `mesh.wgsl`'s fragment shader falls back to its derivative-based `cotangent_frame` for
        /// any fragment whose interpolated `tangent.w` has `abs(w) < 0.5`, exactly as if this field
        /// did not exist — so growing it changes no existing mesh's rendered output (only real
        /// tangent data, which only the figure pack decoder produces so far, opts into the new
        /// path). Registration does not validate `tangent` (see
        /// [`crate::figure_format::decode_mesh`] for the pack decoder's own check); an arbitrary,
        /// invalid tangent here degrades to whatever `mesh.wgsl`'s TBN construction computes from
        /// it, never a panic.
        pub tangent: [f32; 4],
    }

    impl Default for MeshVertex {
        /// Zero position/normal/uv/tangent (tangent unchanged from `[0.0; 4]` since this field was
        /// added — the "no tangent" sentinel, see [`MeshVertex::tangent`]'s doc comment), bone 0
        /// with full weight — "no skeleton" per this struct's doc comment, *not* an all-zero weight
        /// vector (which would skin every vertex to nothing).
        fn default() -> Self {
            Self {
                position: [0.0; 3],
                normal: [0.0; 3],
                uv: [0.0; 2],
                joints: [0; 4],
                weights: [1.0, 0.0, 0.0, 0.0],
                tangent: [0.0; 4],
            }
        }
    }

    impl MeshVertex {
        /// Builds an unskinned vertex: `position`, `normal`, `uv`, and the "no skeleton" bone
        /// binding from [`MeshVertex::default`] (bone 0, full weight). Every procedural generator
        /// in [`crate::procedural`] uses this constructor, so their output is byte-for-byte the
        /// same geometry as before this struct grew skinning fields.
        #[must_use]
        pub fn new(position: [f32; 3], normal: [f32; 3], uv: [f32; 2]) -> Self {
            Self {
                position,
                normal,
                uv,
                ..Self::default()
            }
        }
    }
}
pub use vertex::MeshVertex;

/// CPU-side triangle mesh: an indexed vertex list forming a triangle list
/// (`indices.len() % 3 == 0`, every index `< vertices.len()`). Register it with
/// [`crate::WgpuRenderer::register_mesh`] to get a [`MeshHandle`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshData {
    /// Vertices, indexed by [`MeshData::indices`].
    pub vertices: Vec<MeshVertex>,
    /// Triangle indices into [`MeshData::vertices`], three per triangle.
    pub indices: Vec<u32>,
}

/// Failure registering a [`MeshData`] (plan 0002 WP2.3). Registration never panics: every
/// structural problem a mesh can have is reported through this type instead.
///
/// No longer `Eq` since the P1 skinning addendum added [`MeshError::WeightSumOutOfTolerance`]'s
/// `f32` field (`f32` has no total order, hence no `Eq`); every existing comparison only ever used
/// `PartialEq` (`assert_eq!`), so this is not a behaviour change for any caller.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MeshError {
    /// The mesh has no vertices.
    #[error("mesh has no vertices")]
    EmptyVertices,
    /// The mesh has no indices (nothing to draw).
    #[error("mesh has no indices")]
    EmptyIndices,
    /// `indices.len()` is not a multiple of 3, so the last triangle would be incomplete.
    #[error("{0} indices do not form whole triangles (count is not a multiple of 3)")]
    IndexCountNotMultipleOfThree(usize),
    /// An index addresses a vertex that does not exist.
    #[error("index {index} is out of range for {vertex_count} vertices")]
    IndexOutOfRange {
        /// The offending index.
        index: u32,
        /// Number of vertices actually present.
        vertex_count: u32,
    },
    /// A vertex position or normal has a non-finite (NaN or infinite) component.
    #[error("vertex {0} has a non-finite position or normal")]
    NonFiniteVertex(u32),
    /// A vertex's [`MeshVertex::weights`] do not sum to `1.0` within the `1e-3` tolerance (P1
    /// skinning addendum, contract §6 changelog 2026-09-16). Checked here — not only by the pack
    /// decoder (`figure_format`) — so a malformed skin binding can never reach the GPU, regardless
    /// of how the mesh was constructed.
    #[error("vertex {vertex} skinning weights sum to {sum}, not 1.0 (tolerance 1e-3)")]
    WeightSumOutOfTolerance {
        /// The offending vertex.
        vertex: u32,
        /// The actual sum of [`MeshVertex::weights`].
        sum: f32,
    },
    /// The mesh has more vertices or indices than fit into a `u32`-indexed buffer.
    #[error("mesh exceeds u32::MAX vertices or indices")]
    TooManyElements,
    /// Uploading the (structurally valid) mesh's geometry to the GPU failed, for example because
    /// the device ran out of memory. Never returned by [`MeshData::validate`] itself, only by
    /// [`crate::WgpuRenderer::register_mesh`].
    #[error("uploading mesh geometry to the GPU failed: {0}")]
    Gpu(String),
}

impl MeshData {
    /// Validates structure without ever panicking (contract §2a style): an empty vertex or index
    /// list, an index count that is not a multiple of 3, an out-of-range index and a non-finite
    /// position or normal are all reported as a [`MeshError`] instead.
    ///
    /// Does **not** require normals to be exactly unit length; see [`MeshVertex::normal`].
    ///
    /// # Errors
    /// See [`MeshError`] (every variant except [`MeshError::Gpu`], which only a GPU upload can
    /// produce).
    pub fn validate(&self) -> Result<(), MeshError> {
        if self.vertices.is_empty() {
            return Err(MeshError::EmptyVertices);
        }
        if self.indices.is_empty() {
            return Err(MeshError::EmptyIndices);
        }
        if !self.indices.len().is_multiple_of(3) {
            return Err(MeshError::IndexCountNotMultipleOfThree(self.indices.len()));
        }
        let vertex_count =
            u32::try_from(self.vertices.len()).map_err(|_| MeshError::TooManyElements)?;
        u32::try_from(self.indices.len()).map_err(|_| MeshError::TooManyElements)?;
        for &index in &self.indices {
            if index >= vertex_count {
                return Err(MeshError::IndexOutOfRange {
                    index,
                    vertex_count,
                });
            }
        }
        for (vertex_index, vertex) in self.vertices.iter().enumerate() {
            let finite = vertex.position.iter().all(|c| c.is_finite())
                && vertex.normal.iter().all(|c| c.is_finite());
            if !finite {
                // `vertex_index < self.vertices.len() <= u32::MAX`, checked above.
                #[allow(clippy::cast_possible_truncation)]
                return Err(MeshError::NonFiniteVertex(vertex_index as u32));
            }
            let weight_sum: f32 = vertex.weights.iter().sum();
            if !weight_sum.is_finite() || (weight_sum - 1.0).abs() > 1e-3 {
                #[allow(clippy::cast_possible_truncation)]
                return Err(MeshError::WeightSumOutOfTolerance {
                    vertex: vertex_index as u32,
                    sum: weight_sum,
                });
            }
        }
        Ok(())
    }
}

/// Deterministic, GPU-independent registry of validated [`MeshData`] (contract §6: "the registry
/// itself... is WP2.3's job"). Handles are assigned in registration order starting at `0`, so
/// feeding the same meshes to two registries in the same order produces identical handles.
///
/// [`crate::WgpuRenderer::register_mesh`] wraps one of these to validate before uploading its GPU
/// buffers. [`crate::NullRenderer`] does not get one of its own (it is a frozen P0 contract type,
/// contract §6; see [`crate::NullRenderer`]'s doc comment).
#[derive(Debug, Clone, Default)]
pub(crate) struct MeshRegistry {
    meshes: Vec<MeshData>,
}

impl MeshRegistry {
    /// Validates `mesh` and, if valid, stores it under the next sequential [`MeshHandle`].
    ///
    /// # Errors
    /// See [`MeshData::validate`]; the registry is left unchanged on error.
    pub(crate) fn register(&mut self, mesh: MeshData) -> Result<MeshHandle, MeshError> {
        mesh.validate()?;
        let index = u32::try_from(self.meshes.len()).map_err(|_| MeshError::TooManyElements)?;
        self.meshes.push(mesh);
        Ok(MeshHandle(index))
    }

    /// The registered mesh for `handle`, if any.
    pub(crate) fn get(&self, handle: MeshHandle) -> Option<&MeshData> {
        self.meshes.get(handle.0 as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle() -> MeshData {
        MeshData {
            vertices: vec![
                MeshVertex::new([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0]),
                MeshVertex::new([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0]),
                MeshVertex::new([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0]),
            ],
            indices: vec![0, 1, 2],
        }
    }

    #[test]
    fn mesh_vertex_is_72_bytes() {
        assert_eq!(std::mem::size_of::<MeshVertex>(), 72);
    }

    #[test]
    fn mesh_vertex_new_defaults_to_bone_zero_full_weight() {
        let vertex = MeshVertex::new([1.0, 2.0, 3.0], [0.0, 0.0, 1.0], [0.5, 0.5]);
        assert_eq!(vertex.joints, [0, 0, 0, 0]);
        assert_eq!(vertex.weights, [1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn mesh_vertex_default_matches_new_with_zeroed_geometry() {
        assert_eq!(
            MeshVertex::default(),
            MeshVertex::new([0.0; 3], [0.0; 3], [0.0; 2])
        );
    }

    #[test]
    fn weight_sum_out_of_tolerance_is_rejected_without_panic() {
        let mut mesh = triangle();
        mesh.vertices[1].weights = [0.5, 0.0, 0.0, 0.0]; // sums to 0.5, not 1.0
        assert_eq!(
            mesh.validate(),
            Err(MeshError::WeightSumOutOfTolerance {
                vertex: 1,
                sum: 0.5
            })
        );
    }

    #[test]
    fn weight_sum_within_tolerance_passes() {
        let mut mesh = triangle();
        // 1.0 + 1e-4 is within the 1e-3 tolerance.
        mesh.vertices[0].weights = [1.000_1, 0.0, 0.0, 0.0];
        assert_eq!(mesh.validate(), Ok(()));
    }

    #[test]
    fn valid_triangle_passes_validation() {
        assert_eq!(triangle().validate(), Ok(()));
    }

    #[test]
    fn empty_vertices_are_rejected_without_panic() {
        let mesh = MeshData {
            vertices: Vec::new(),
            indices: vec![0, 1, 2],
        };
        assert_eq!(mesh.validate(), Err(MeshError::EmptyVertices));
    }

    #[test]
    fn empty_indices_are_rejected_without_panic() {
        let mesh = MeshData {
            vertices: triangle().vertices,
            indices: Vec::new(),
        };
        assert_eq!(mesh.validate(), Err(MeshError::EmptyIndices));
    }

    #[test]
    fn index_count_must_be_a_multiple_of_three() {
        let mut mesh = triangle();
        mesh.indices.push(0);
        assert_eq!(
            mesh.validate(),
            Err(MeshError::IndexCountNotMultipleOfThree(4))
        );
    }

    #[test]
    fn out_of_range_index_is_rejected_without_panic() {
        let mut mesh = triangle();
        mesh.indices[2] = 99;
        assert_eq!(
            mesh.validate(),
            Err(MeshError::IndexOutOfRange {
                index: 99,
                vertex_count: 3
            })
        );
    }

    #[test]
    fn non_finite_position_is_rejected_without_panic() {
        let mut mesh = triangle();
        mesh.vertices[1].position[0] = f32::NAN;
        assert_eq!(mesh.validate(), Err(MeshError::NonFiniteVertex(1)));
    }

    #[test]
    fn non_finite_normal_is_rejected_without_panic() {
        let mut mesh = triangle();
        mesh.vertices[2].normal[1] = f32::INFINITY;
        assert_eq!(mesh.validate(), Err(MeshError::NonFiniteVertex(2)));
    }

    #[test]
    fn registry_assigns_sequential_handles_deterministically() {
        let mut registry = MeshRegistry::default();
        let a = registry.register(triangle()).expect("valid mesh");
        let b = registry.register(triangle()).expect("valid mesh");
        assert_eq!(a, MeshHandle(0));
        assert_eq!(b, MeshHandle(1));
        assert_eq!(registry.get(a), Some(&triangle()));
        assert_eq!(registry.get(b), Some(&triangle()));
    }

    #[test]
    fn registry_rejects_invalid_mesh_without_storing_it_or_advancing_handles() {
        let mut registry = MeshRegistry::default();
        let invalid = MeshData {
            vertices: Vec::new(),
            indices: Vec::new(),
        };
        assert_eq!(registry.register(invalid), Err(MeshError::EmptyVertices));
        // The next valid registration still gets handle 0: the failed attempt was never stored.
        let handle = registry.register(triangle()).expect("valid mesh");
        assert_eq!(handle, MeshHandle(0));
    }

    #[test]
    fn registry_get_of_unknown_handle_is_none_not_a_panic() {
        let registry = MeshRegistry::default();
        assert_eq!(registry.get(MeshHandle(0)), None);
        assert_eq!(registry.get(MeshHandle(u32::MAX)), None);
    }
}
