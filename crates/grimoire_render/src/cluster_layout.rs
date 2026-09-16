//! Light and cluster data layout for the clustered forward+ pass (plan 0002 WP3.1, groundwork for
//! WP3.4).
//!
//! **Status:** this module is WP3.1 preparation, not yet part of the render contract (contract
//! §6 explicitly defers the light budget and its storage layout to WP3.4: *"Ein Zähl-Budget (Low
//! 32/High 256, PRD-0003 FR-11) gehört nach Plan 0002 WP3.4 zum Clustered-Forward+-Pass und ist
//! bewusst nicht Teil dieses Vertrags"*). Nothing here is wired into [`crate::StageFrame`],
//! [`crate::StageStats`] or [`crate::Renderer`]; WP3.4 chooses how (and whether unchanged) these
//! types reach the render vertex/fragment/compute pipeline, subject to a contract PR (§2b) before
//! it starts. Downlevel evidence for the storage-buffer access this layout assumes lives in
//! `grimoire_gpu`'s `tests/downlevel.rs` and the crate's `capability_report_lines`.
//!
//! # The three buffers
//!
//! A froxel grid of [`CLUSTER_GRID_X`] x [`CLUSTER_GRID_Y`] x [`CLUSTER_GRID_Z`]
//! ([`CLUSTER_COUNT`] clusters total) with a per-quality light budget
//! ([`LIGHT_BUDGET_LOW`]/[`LIGHT_BUDGET_HIGH`], PRD-0003 FR-11) needs three storage buffers, the
//! standard clustered-forward-plus shape (light array, per-cluster range, flat index list):
//!
//! 1. **Light list** ([`GpuPointLight`] array, length = the active light budget): the lights
//!    themselves, uploaded once per frame in an arbitrary but stable order.
//! 2. **Cluster table** ([`GpuClusterLightRange`] array, length [`CLUSTER_COUNT`], fixed
//!    regardless of light budget): for each cluster, the `(offset, count)` slice of the index list
//!    below that lists which lights affect it.
//! 3. **Light index list** (`u32` array): a flat list of indices into the light list, sliced per
//!    cluster by the cluster table. Its size depends on actual light/cluster overlap, which is
//!    unknown ahead of the culling pass WP3.4 implements; [`light_index_list_worst_case_len`]
//!    gives the safe upper bound this layout must budget for (every light touching every
//!    cluster), not the typical case.
//!
//! [`worst_case_total_bytes`] sums all three for a given light budget and is the number to compare
//! against a measured `max_storage_buffer_binding_size` — each buffer is bound separately, so in
//! principle only the *largest single buffer* (the index list) must fit under that limit, but the
//! combined figure is the conservative, easier-to-communicate number and is what the WP3.1 PR
//! discussion (and the ADR under `docs/adr/`) compares against the weakest measured adapter.

/// Froxels along the screen-space X axis.
pub const CLUSTER_GRID_X: u32 = 16;
/// Froxels along the screen-space Y axis.
pub const CLUSTER_GRID_Y: u32 = 9;
/// Froxels along the view-depth (Z) axis.
pub const CLUSTER_GRID_Z: u32 = 24;
/// Total froxel count: [`CLUSTER_GRID_X`] x [`CLUSTER_GRID_Y`] x [`CLUSTER_GRID_Z`] = 3456.
pub const CLUSTER_COUNT: usize = (CLUSTER_GRID_X * CLUSTER_GRID_Y * CLUSTER_GRID_Z) as usize;

/// Light budget of the "Low" quality preset (PRD-0003 FR-11).
pub const LIGHT_BUDGET_LOW: usize = 32;
/// Light budget of the "High" quality preset (PRD-0003 FR-11).
pub const LIGHT_BUDGET_HIGH: usize = 256;

mod gpu_types {
    // bytemuck's derive macros expand to `unsafe impl` blocks (same rationale as
    // `crate::instance`'s module-local allow for `SpriteInstance`).
    #![allow(unsafe_code)]

    /// One point light as the clustered forward+ pass's light-list storage buffer stores it.
    ///
    /// `#[repr(C, align(16))]`, 32 bytes, no padding: two 16-byte std430 array elements
    /// (`position`+`range`, `color`+`intensity`), so an array of these needs no interior padding
    /// on the GPU side either — std430 gives a `vec3` a base alignment of 16 (rounded up to
    /// `vec4`), and packing the trailing scalar into that same 16 bytes is the standard way to
    /// avoid wasting it. The explicit `align(16)` documents that requirement in the Rust type
    /// itself rather than leaving it an unstated assumption about call-site buffer offsets.
    ///
    /// Contract note: this is a preparatory type (module doc comment); it does not replace
    /// [`crate::PointLight`], the render-contract type extraction produces
    /// (contract §6) — a future WP3.4 upload step converts one into the other.
    #[repr(C, align(16))]
    #[derive(Debug, Clone, Copy, PartialEq, Default, bytemuck::Pod, bytemuck::Zeroable)]
    pub struct GpuPointLight {
        /// World-space position.
        pub position: [f32; 3],
        /// Cutoff distance in world units, matching [`crate::PointLight::range`].
        pub range: f32,
        /// Linear RGB, matching [`crate::PointLight::color`].
        pub color: [f32; 3],
        /// Matching [`crate::PointLight::intensity`].
        pub intensity: f32,
    }

    /// One cluster's slice of the light index list: `[offset, offset + count)`.
    ///
    /// `#[repr(C)]`, 8 bytes, no padding: two plain `u32`s have a std430 base alignment of 4, so
    /// this needs no extra alignment beyond Rust's default for the type.
    #[repr(C)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, bytemuck::Pod, bytemuck::Zeroable)]
    pub struct GpuClusterLightRange {
        /// Start index into the light index list.
        pub offset: u32,
        /// Number of entries in the light index list belonging to this cluster.
        pub count: u32,
    }
}

pub use gpu_types::{GpuClusterLightRange, GpuPointLight};

/// Worst-case byte size of the light list for `light_budget` active lights (every light the
/// budget allows, uploaded in full).
#[must_use]
pub const fn light_list_bytes(light_budget: usize) -> usize {
    light_budget * size_of::<GpuPointLight>()
}

/// Byte size of the cluster table: fixed at [`CLUSTER_COUNT`] entries regardless of light budget.
#[must_use]
pub const fn cluster_table_bytes() -> usize {
    CLUSTER_COUNT * size_of::<GpuClusterLightRange>()
}

/// Worst-case light index list length: every one of `light_budget` lights listed in every one of
/// [`CLUSTER_COUNT`] clusters. Real scenes overlap far less (a light's range only reaches nearby
/// clusters), but the buffer must be sized for the worst case up front — a culling pass that
/// undercounts silently drops lights instead of failing loudly.
#[must_use]
pub const fn light_index_list_worst_case_len(light_budget: usize) -> usize {
    CLUSTER_COUNT * light_budget
}

/// Worst-case byte size of the light index list (see [`light_index_list_worst_case_len`]); each
/// entry is one `u32` index into the light list.
#[must_use]
pub const fn light_index_list_worst_case_bytes(light_budget: usize) -> usize {
    light_index_list_worst_case_len(light_budget) * size_of::<u32>()
}

/// Sum of [`light_list_bytes`], [`cluster_table_bytes`] and
/// [`light_index_list_worst_case_bytes`] for `light_budget` — the conservative total to compare
/// against a measured `max_storage_buffer_binding_size` (see the module doc comment for why the
/// combined figure, rather than the largest single buffer, is what this crate's WP3.1 discussion
/// quotes).
#[must_use]
pub const fn worst_case_total_bytes(light_budget: usize) -> usize {
    light_list_bytes(light_budget)
        + cluster_table_bytes()
        + light_index_list_worst_case_bytes(light_budget)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_count_matches_the_froxel_grid() {
        assert_eq!(CLUSTER_COUNT, 3456);
    }

    #[test]
    fn gpu_point_light_is_32_bytes_aligned_to_16() {
        assert_eq!(size_of::<GpuPointLight>(), 32);
        assert_eq!(align_of::<GpuPointLight>(), 16);
    }

    #[test]
    fn gpu_cluster_light_range_is_8_bytes_aligned_to_4() {
        assert_eq!(size_of::<GpuClusterLightRange>(), 8);
        assert_eq!(align_of::<GpuClusterLightRange>(), 4);
    }

    #[test]
    fn light_list_bytes_scale_with_the_budget() {
        assert_eq!(light_list_bytes(LIGHT_BUDGET_LOW), 1_024);
        assert_eq!(light_list_bytes(LIGHT_BUDGET_HIGH), 8_192);
    }

    #[test]
    fn cluster_table_bytes_are_fixed_by_the_grid_alone() {
        assert_eq!(cluster_table_bytes(), 27_648);
    }

    #[test]
    fn light_index_list_worst_case_scales_with_clusters_times_budget() {
        assert_eq!(
            light_index_list_worst_case_len(LIGHT_BUDGET_LOW),
            CLUSTER_COUNT * 32
        );
        assert_eq!(light_index_list_worst_case_bytes(LIGHT_BUDGET_LOW), 442_368);
        assert_eq!(
            light_index_list_worst_case_bytes(LIGHT_BUDGET_HIGH),
            3_538_944
        );
    }

    #[test]
    fn worst_case_total_sums_all_three_buffers() {
        assert_eq!(worst_case_total_bytes(LIGHT_BUDGET_LOW), 471_040);
        assert_eq!(worst_case_total_bytes(LIGHT_BUDGET_HIGH), 3_574_784);
    }

    #[test]
    fn worst_case_high_budget_fits_the_weakest_measured_adapter() {
        // The weakest `max_storage_buffer_binding_size` of the three P1 CI adapters, measured by
        // `grimoire_gpu`'s WP3.1 downlevel probe (`cargo test -p grimoire_gpu --test downlevel --
        // --nocapture`) in this work package's pull request (CI run 35049685258): Linux/lavapipe
        // at 134217728 (128 MiB) — weaker than Windows/WARP's 2147483644 (~2 GiB) and macOS/Apple
        // Paravirtual Metal's 3758096384 (~3.5 GiB). Not a live query (this crate has no GPU
        // access, contract §6 layer rule) — a recorded number, updated if a future downlevel probe
        // run measures a smaller value on any P1 CI adapter. See `docs/adr/
        // 0013-downlevel-pruefung-licht-cluster-layout.md` for the full table and the other two
        // adapters' numbers.
        const MEASURED_WEAKEST_MAX_STORAGE_BUFFER_BINDING_SIZE: usize = 134_217_728;
        assert!(
            worst_case_total_bytes(LIGHT_BUDGET_HIGH)
                <= MEASURED_WEAKEST_MAX_STORAGE_BUFFER_BINDING_SIZE
        );
    }
}
