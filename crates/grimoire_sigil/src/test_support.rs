//! Test-only fixture builders.
//!
//! [`build_unit_bytes`] hand-assembles a valid v1 unit's bytes directly from the documented
//! header/section-table layout, independent of [`SigilUnit::to_bytes`], so tests that decode it
//! exercise [`SigilUnit::from_bytes`] against externally constructed bytes rather than only its
//! own encoder's round trip.

use std::sync::Arc;

use grimoire_core::StableHasher;

use crate::behavior::{BehaviorId, BehaviorRegistry, BehaviorRegistryBuilder};
use crate::content::{BulletFlags, BulletType, BulletVisual, SigilContent, SigilLibrary};
use crate::unit::SigilUnit;

/// Hand-builds the bytes of a valid v1 unit, following the header + section-table layout
/// documented in `crate::unit`, independently of [`SigilUnit::to_bytes`].
pub(crate) fn build_unit_bytes(
    id: u64,
    bullet_types: &[BulletType],
    emitter_count: u16,
    program_count: Option<u16>,
    behavior_refs: Option<&[BehaviorId]>,
) -> Vec<u8> {
    let mut contents: Vec<(u32, Vec<u8>)> = Vec::new();

    let mut bullet_type_bytes = Vec::new();
    bullet_type_bytes.extend_from_slice(&(bullet_types.len() as u16).to_le_bytes());
    for bullet_type in bullet_types {
        bullet_type_bytes.extend_from_slice(&bullet_type.radius.to_le_bytes());
        bullet_type_bytes.extend_from_slice(&bullet_type.collision_radius.to_le_bytes());
        bullet_type_bytes.extend_from_slice(&bullet_type.lifetime_ticks.to_le_bytes());
        bullet_type_bytes.push(bullet_type.flags.0);
        bullet_type_bytes.push(0); // reserved
        bullet_type_bytes.extend_from_slice(&bullet_type.visual.silhouette.to_le_bytes());
        bullet_type_bytes.extend_from_slice(&bullet_type.visual.palette.to_le_bytes());
        bullet_type_bytes.push(bullet_type.visual.palette_space);
        bullet_type_bytes.push(bullet_type.visual.glow);
    }
    contents.push((1, bullet_type_bytes));

    if let Some(count) = program_count {
        // `count` minimal-but-well-formed program records (Plan 0002 WP4.2 real `Programs`
        // layout: a `BlockDef` -- kind 1 (`ring`), reserved 0, `count: u16 = 0`, six finite-zero
        // `f32` params, `seed_hash: u32 = 0` -- followed by a `modifier_count: u16 = 0`), rather
        // than the WP1.3 bare-count placeholder: this crate's tests only care about
        // `SigilUnit::program_count()`, never about a program's actual shape, so any record that
        // decodes is fine here.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&count.to_le_bytes());
        for _ in 0..count {
            bytes.push(1); // block kind: ring
            bytes.push(0); // reserved
            bytes.extend_from_slice(&0u16.to_le_bytes()); // block count
            for _ in 0..6 {
                bytes.extend_from_slice(&0f32.to_le_bytes());
            }
            bytes.extend_from_slice(&0u32.to_le_bytes()); // seed_hash
            bytes.extend_from_slice(&0u16.to_le_bytes()); // modifier_count
        }
        contents.push((2, bytes));
    }

    let mut emitters_bytes = Vec::new();
    emitters_bytes.extend_from_slice(&emitter_count.to_le_bytes());
    for _ in 0..emitter_count {
        // Minimal well-formed `EmitterRecord` (Plan 0002 WP4.2 layout): bullet type 0, no program,
        // primary role, zero timing/offset. Bullet type 0 is always valid here since every caller
        // of this helper passes at least one bullet type.
        emitters_bytes.extend_from_slice(&0u16.to_le_bytes()); // bullet_type
        emitters_bytes.extend_from_slice(&0xFFFFu16.to_le_bytes()); // program: none
        emitters_bytes.push(0); // role: primary
        emitters_bytes.push(0); // reserved
        emitters_bytes.extend_from_slice(&0u32.to_le_bytes()); // delay_ticks
        emitters_bytes.extend_from_slice(&1u32.to_le_bytes()); // repeat
        emitters_bytes.extend_from_slice(&1u32.to_le_bytes()); // interval_ticks
        emitters_bytes.extend_from_slice(&0f32.to_le_bytes()); // speed
        emitters_bytes.extend_from_slice(&0f32.to_le_bytes()); // offset_x
        emitters_bytes.extend_from_slice(&0f32.to_le_bytes()); // offset_y
    }
    contents.push((3, emitters_bytes));

    if let Some(refs) = behavior_refs {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(refs.len() as u16).to_le_bytes());
        for id in refs {
            bytes.extend_from_slice(&id.0.to_le_bytes());
        }
        contents.push((6, bytes));
    }

    contents.sort_by_key(|&(kind, _)| kind);

    let section_count = contents.len() as u32;
    let table_len = 4u64 + u64::from(section_count) * 24;
    let mut table = Vec::new();
    table.extend_from_slice(&section_count.to_le_bytes());
    let mut body = Vec::new();
    let mut offset = table_len;
    for (kind, bytes) in &contents {
        table.extend_from_slice(&kind.to_le_bytes());
        table.extend_from_slice(&0u32.to_le_bytes());
        table.extend_from_slice(&offset.to_le_bytes());
        table.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        offset += bytes.len() as u64;
        body.extend_from_slice(bytes);
    }
    let mut payload = table;
    payload.extend_from_slice(&body);

    let mut out = Vec::with_capacity(SigilUnit::HEADER_LEN + payload.len());
    out.extend_from_slice(&SigilUnit::MAGIC);
    out.extend_from_slice(&SigilUnit::FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes()); // content_hash placeholder
    out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    out.extend_from_slice(&payload);

    let mut hasher = StableHasher::new();
    hasher.write_bytes(&out[0..24]);
    hasher.write_bytes(&out[32..out.len()]);
    let hash = hasher.finish();
    out[24..32].copy_from_slice(&hash.to_le_bytes());
    out
}

/// Decodes a minimal valid unit with `bullet_types`, `emitter_count` emitters, no programs and no
/// behavior refs.
pub(crate) fn build_unit(id: u64, bullet_types: &[BulletType], emitter_count: u16) -> SigilUnit {
    let bytes = build_unit_bytes(id, bullet_types, emitter_count, None, None);
    SigilUnit::from_bytes(&bytes).expect("hand-built test fixture must decode")
}

/// A single unremarkable bullet type, usable wherever the exact values do not matter.
pub(crate) fn plain_bullet_type() -> BulletType {
    BulletType {
        visual: BulletVisual {
            silhouette: 0,
            palette: 0,
            palette_space: 0,
            glow: 0,
        },
        radius: 1.0,
        collision_radius: 1.0,
        lifetime_ticks: 0,
        flags: BulletFlags(0),
    }
}

/// Builds an empty behavior registry at version `1`.
pub(crate) fn empty_registry() -> Arc<BehaviorRegistry> {
    BehaviorRegistryBuilder::new(1).build()
}

/// Builds content wrapping a library of `units` validated against an empty registry.
pub(crate) fn build_content(units: Vec<SigilUnit>) -> SigilContent {
    let library = SigilLibrary::new(units, empty_registry()).expect("test fixture must build");
    SigilContent::new(library)
}

/// Builds content with one unit (id `1`) with `bullet_type_count` plain bullet types,
/// `emitter_count` emitters and `program_count` programs.
pub(crate) fn build_simple_content(
    bullet_type_count: u16,
    emitter_count: u16,
    program_count: u16,
) -> SigilContent {
    let bullet_types = vec![plain_bullet_type(); bullet_type_count as usize];
    let bytes = build_unit_bytes(1, &bullet_types, emitter_count, Some(program_count), None);
    let unit = SigilUnit::from_bytes(&bytes).expect("test fixture must decode");
    build_content(vec![unit])
}
