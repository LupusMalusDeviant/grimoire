# WP3.2 spike: bullet representation and light culling under stress

Throwaway crate for plan 0002 WP3.2 (OF-3.3 bullet representation, light culling). Not part of the
runtime path; lives only under `spikes/` (same convention as `spikes/bench-noise`,
`spikes/look-dev`, `spikes/sigil-syntax`). Measured only via
`.github/workflows/spike-wp3.2-light-bullet-stress.yml`, which runs only on branch
`p1/wp3.2-light-bullet-spikes` (or manual `workflow_dispatch`) — never on `main`.

## What this measures

- `src/bin/bullet_stress.rs`: billboard-impostor vs. instanced low-poly-mesh bullets at 10k/20k
  instances under a tilted camera. Extraction (CPU) and upload (CPU) are timed directly; GPU cost
  is a *relative-only* proxy (encode + submit + `device.poll(Wait)` wall time on the software
  adapter) — never an absolute-millisecond judgement (the plan text is explicit about this).
- `src/bin/light_cluster.rs`: CPU-froxel cluster assignment vs. compute-shader clustering at 256
  lights, on the frozen 16x9x24 froxel grid from `grimoire_render::cluster_layout` (reused
  verbatim, not redefined). Measures the CPU-froxel path's assignment cost and its three-buffer
  upload, and the compute path's (much smaller) light-only upload plus its relative GPU dispatch
  time, plus the static memory/upload byte sizes of both paths.

Every number these binaries print is a "Runner-Wert": median of 10 repetitions (`REPS`) after 3
discarded warm-up repetitions (`WARMUP`), with the min/max spread reported alongside the median
(engine ADR-0010: a shared-runner wall-clock number is a trend, not a budget proof). All 10+3
repetitions run inside a single CI job process — WP6.1's bench-noise spike used one CI job *per*
repetition specifically to measure cross-job noise itself; that question is already answered
(ADR-0010), so this spike does not need to re-ask it and uses the far cheaper single-job loop
instead.

## Deviations from an exact production implementation

- **Camera:** `src/camera.rs`'s `TiltedCamera` is a small hand-rolled look-at/perspective camera,
  not `grimoire_render::Camera25D`. `Camera25D::view_projection` (the `stage3d` module's real
  helper) is `pub(crate)`, not part of the crate's public API, so this spike (a separate crate)
  cannot call it. `TiltedCamera` uses the same tilt/FOV vocabulary and looks at a ground-plane
  target from the configured tilt/distance, but is not a claim about the production camera's exact
  framing.
- **Billboard/mesh shading:** both `src/bin/billboard.wgsl` and `src/bin/mesh.wgsl` are simple,
  unlit-ish shaders (a cheap HSV palette function plus, for the mesh variant, one fixed-direction
  N.L term) — nowhere near the real PBR pass (`mesh.wgsl` in `grimoire_render`, WP2.5). Shading
  quality is irrelevant to what this spike measures (CPU extraction/upload cost, relative GPU
  submit+poll wall time); a lighter shader only reduces noise in the GPU-relative number.
- **`MeshBulletInstance` (`src/bullets.rs`):** a spike-local stand-in for what an instanced
  low-poly-mesh bullet would need that the real, contract-owned `BulletInstance` (24 bytes,
  §6) does not carry — a 3D position and a mesh index instead of a 2D position and a silhouette
  table index. **Not** proposed as a code change to `BulletInstance` here; contract §6 makes that
  type's owner the render contract alone, changeable only through a contract PR (WP1.7) before
  WP5.3. This spike only measures what such a change would cost; the report/ADR carries the actual
  proposal.
- **Light-culling geometry:** cluster bounds in `src/lights.rs`/`light_cluster.wgsl` are unit
  froxel cells in an abstract `[0,16] x [0,9] x [0,24]` grid, tested against a light with a plain
  Euclidean sphere-vs-sphere overlap (light range plus a froxel's half-diagonal) — not the
  perspective view-space frustum slicing a real clustered forward+ pass (WP3.4) uses. That
  difference changes *which* lights land in *which* cluster, not the CPU-vs-compute cost
  comparison: both paths run the identical `O(clusters x lights)` loop, one on the CPU
  (`cpu_assign_clusters`), one on the GPU (`light_cluster.wgsl`'s `cs_main`) — verified to agree
  exactly on a small grid by `compute_cluster::tests::compute_path_matches_the_cpu_path_on_a_tiny_grid`.
- **Fixed-stride index list, no atomics:** both paths write each cluster's light indices at a
  fixed offset (`cluster_index * light_budget`), the same worst-case layout
  `grimoire_render::cluster_layout::light_index_list_worst_case_bytes` already sizes for. A real
  WP3.4 implementation could compact the index list (an atomic counter or a two-pass
  count-then-scatter) to shrink the *actual* per-frame upload/GPU-write size well below this
  worst case; that optimisation is out of scope here and left as an open item (see the ADR).
- **No rendering with the culled lights:** `light_cluster.rs` never runs a fragment shader against
  the produced cluster table/index list (that is WP3.4's clustered forward+ pass, explicitly out of
  scope for WP3.2). Only the CPU cost, the compute dispatch's relative GPU cost, and the two paths'
  upload/memory footprint are measured.

## Local testing

`cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings` and
`GRIMOIRE_GPU_ADAPTER=software cargo test --locked` all ran clean locally (WARP on Windows,
contract §6's sanctioned local GPU-test path) before the first push — including a tiny-scale
correctness check of both render pipelines (`src/bin/bullet_stress.rs`'s `tests` module) and of the
compute-clustering shader against the CPU reference (`compute_cluster::tests`). Only `cargo test`
ran locally; the actual `bullet_stress`/`light_cluster` binaries (the stress-scale measurement
runs) were never executed on the development machine — only in this spike's CI workflow, per the
project's "no CPU benches outside a measurement session" rule.

## Runner and versions

Measured on `ubuntu-24.04` with the `mesa-vulkan-drivers` lavapipe package, `wgpu` 30.0.1 (pinned
in the engine workspace's `Cargo.lock`), `GRIMOIRE_GPU_ADAPTER=software`. See the spike's CI run
links in the WP3.2 ADRs/plan entry for the exact run IDs.
