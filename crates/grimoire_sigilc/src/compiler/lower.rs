//! Lowers a validated [`ResolvedUnit`] to `grimoire_sigil::SigilUnit` v1 bytes (Plan 0002 WP4.2).
//!
//! This is an independent encoder, not a call into `grimoire_sigil::SigilUnit::to_bytes` (there
//! is no public constructor for that type besides `from_bytes`, by design: contract §11.1 makes
//! `sigilc` the sole v1 producer and `from_bytes` the sole validator, so this function builds raw
//! bytes straight from the documented layout and hands them to
//! [`grimoire_sigil::SigilUnit::from_bytes`], which both validates and produces the object).
//! `docs/formats/sigil.md`'s binary section is the single normative copy of every offset, tag and
//! field this module writes; keep the two in sync by hand — `grimoire_sigil`'s own golden fixtures
//! (`crates/grimoire_sigil/tests/golden.rs`) are derived from that document, then checked against
//! this encoder, never the other way around (contract §2 rule 10).
//!
//! Contract §11.1/WP4.2: "Richtungs- und Rotationskonstanten, die der Compiler vorberechnet,
//! entstehen ausschließlich über `dmath`." The one place this compiler actually precomputes a
//! direction vector (rather than leaving a bare angle for a future interpreter to turn into one)
//! is a `wave`/`line` block's baked unit vector, via [`dmath::cos`]/[`dmath::sin`] below. Every
//! other angle stored by this module is a plain, IEEE-754-exact degrees-to-radians conversion
//! (`grimoire_core::math`'s own docs: basic arithmetic is already bit-identical across platforms
//! without `dmath`), so no other call site needs it.

use std::collections::{BTreeMap, BTreeSet};

use grimoire_core::StableHasher;
use grimoire_core::math::dmath;

use crate::DeriveUnitIdError;
use crate::catalog;
use crate::compiler::model::{EmitterView, FieldValue, Located};
use crate::compiler::resolve::ResolvedUnit;

const SECTION_BULLET_TYPES: u32 = 1;
const SECTION_PROGRAMS: u32 = 2;
const SECTION_EMITTERS: u32 = 3;
const SECTION_CURVES: u32 = 5;
const SECTION_BEHAVIOR_REFS: u32 = 6;
const SECTION_ENTRY_LEN: u64 = 24;
const HEADER_LEN: usize = 40;
const MAGIC: &[u8; 8] = b"GRIMSIGL";
const FORMAT_VERSION: u32 = 1;

const DEG_TO_RAD: f32 = dmath::PI / 180.0;

const FOREVER: u32 = u32::MAX;
const NO_PROGRAM: u16 = u16::MAX;

const PALETTE_SPACE_HOSTILE: u8 = 1; // duplicated from `grimoire_render::palette_space::HOSTILE`;
// see this module's docs and the pull request description
// for why (no dependency edge to `grimoire_render`, contract
// §1). Sigil bullets never write any other value (PRD-0003
// rule 4, enforced statically in `validate`).

const BLOCK_RING: u8 = 1;
const BLOCK_SPIRAL: u8 = 2;
const BLOCK_FAN: u8 = 3;
const BLOCK_AIMED: u8 = 4;
const BLOCK_WAVE: u8 = 5;
const BLOCK_LINE: u8 = 6;
const BLOCK_SCATTER: u8 = 7;

const MODIFIER_ACCELERATE: u8 = 1;
const MODIFIER_SINE_OFFSET: u8 = 2;
const MODIFIER_ROTATE: u8 = 3;
const MODIFIER_MIRROR: u8 = 4;
const MODIFIER_SPEED_CURVE: u8 = 5;
const MODIFIER_CURVE: u8 = 6;

/// Lowers an already-validated [`ResolvedUnit`] to canonical v1 bytes.
///
/// # Errors
/// Returns [`DeriveUnitIdError`] in the astronomically unlikely case that the entry file's
/// canonical content path happens to hash to the one reserved invalid id (see
/// [`crate::derive_unit_id`]'s own docs).
pub(crate) fn lower(
    resolved: &ResolvedUnit,
    behavior_ids: &BTreeMap<String, u32>,
) -> Result<Vec<u8>, DeriveUnitIdError> {
    let unit_id = crate::derive_unit_id(&resolved.entry_path)?;

    let bullet_index: BTreeMap<(String, String), u16> = resolved
        .bullets
        .iter()
        .enumerate()
        .map(|(i, (file, view))| ((file.clone(), view.name.clone()), i as u16))
        .collect();

    let bullet_types = encode_bullet_types(resolved);

    let mut curves: Vec<(u32, f32)> = Vec::new(); // flattened below; see `curve_records`.
    let mut curve_records: Vec<Vec<(u32, f32)>> = Vec::new();
    let mut programs = Vec::new();
    let mut emitters = Vec::new();
    for emitter in &resolved.emitters {
        let program_index = if emitter.block.is_some() {
            programs.push(encode_program(emitter, &mut curve_records));
            Some((programs.len() - 1) as u16)
        } else {
            None
        };
        emitters.push(encode_emitter(emitter, &bullet_index, program_index));
    }
    let _ = &mut curves; // kept for readability of the two-step curve collection above.

    let behavior_refs: BTreeSet<u32> = resolved
        .bullets
        .iter()
        .filter_map(|(_, b)| match b.fields.get("behaviour").map(|l| &l.value) {
            Some(FieldValue::Ident(name)) => behavior_ids.get(name).copied(),
            _ => None,
        })
        .collect();

    let mut sections: Vec<(u32, Vec<u8>)> = vec![(SECTION_BULLET_TYPES, bullet_types)];
    if !programs.is_empty() {
        sections.push((SECTION_PROGRAMS, encode_programs_section(&programs)));
    }
    sections.push((SECTION_EMITTERS, encode_emitters_section(&emitters)));
    if !curve_records.is_empty() {
        sections.push((SECTION_CURVES, encode_curves_section(&curve_records)));
    }
    if !behavior_refs.is_empty() {
        sections.push((
            SECTION_BEHAVIOR_REFS,
            encode_behavior_refs_section(&behavior_refs),
        ));
    }

    Ok(assemble(unit_id.0, &sections))
}

/// Encodes the `BulletTypes` section. `silhouette` and `palette` are visual catalogue indices
/// (`crate::catalog`, `docs/formats/sigil.md` §10.9), identical for a name in every unit.
/// `validate` has already rejected every name outside the catalogue (`SIG0027`), so the `0`
/// fallbacks below are never written for a unit that reaches this function.
fn encode_bullet_types(resolved: &ResolvedUnit) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(resolved.bullets.len() as u16).to_le_bytes());
    for (_, bullet) in &resolved.bullets {
        let silhouette = match bullet.fields.get("silhouette").map(|l| &l.value) {
            Some(FieldValue::Ident(name)) => catalog::silhouette_index(name).unwrap_or(0),
            _ => 0,
        };
        let palette = match bullet.fields.get("palette").map(|l| &l.value) {
            Some(FieldValue::Ref(segments)) if segments.len() == 2 => {
                catalog::enemy_palette_index(&segments[1]).unwrap_or(0)
            }
            _ => 0,
        };
        let glow = match bullet.fields.get("glow").map(|l| &l.value) {
            Some(FieldValue::Float(v)) => *v,
            Some(FieldValue::Int(v)) => *v as f64,
            _ => 0.0,
        };
        let glow_byte = (glow.clamp(0.0, 1.0) * 255.0).round() as u8;
        let radius = match bullet.fields.get("radius").map(|l| &l.value) {
            Some(FieldValue::Quantity(v, _)) => *v as f32,
            _ => 0.0,
        };
        // No source field for `collision_radius` yet (see the pull request description's open
        // points); the most permissive value consistent with the contract's own
        // `collision_radius <= radius` invariant is equality.
        let collision_radius = radius;
        let mut flags = 0u8;
        if let Some(FieldValue::List(entries)) = bullet.fields.get("flags").map(|l| &l.value) {
            for entry in entries {
                if let FieldValue::Ident(name) = entry {
                    flags |= match name.as_str() {
                        "smashable" => 1,
                        "reflectable" => 2,
                        "env_active" => 4,
                        "grazeable" => 8,
                        _ => 0,
                    };
                }
            }
        }

        out.extend_from_slice(&radius.to_le_bytes());
        out.extend_from_slice(&collision_radius.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // lifetime_ticks: no source field yet, 0 = unbounded.
        out.push(flags);
        out.push(0); // reserved
        out.extend_from_slice(&silhouette.to_le_bytes());
        out.extend_from_slice(&palette.to_le_bytes());
        out.push(PALETTE_SPACE_HOSTILE);
        out.push(glow_byte);
    }
    out
}

fn deg_to_rad(located: Option<&Located<FieldValue>>) -> f32 {
    match located.map(|l| &l.value) {
        Some(FieldValue::Quantity(v, _)) => *v as f32 * DEG_TO_RAD,
        _ => 0.0,
    }
}

fn unit_of(located: Option<&Located<FieldValue>>) -> f32 {
    match located.map(|l| &l.value) {
        Some(FieldValue::Quantity(v, _)) => *v as f32,
        Some(FieldValue::Float(v)) => *v as f32,
        Some(FieldValue::Int(v)) => *v as f32,
        _ => 0.0,
    }
}

fn count_of(located: Option<&Located<FieldValue>>) -> u16 {
    match located.map(|l| &l.value) {
        Some(FieldValue::Int(v)) => (*v).clamp(0, i64::from(u16::MAX)) as u16,
        _ => 0,
    }
}

/// One `Programs`-section record's bytes: a `BlockDef` (kind, count, six `f32` params, a
/// `seed_hash`) followed by its modifier stack. `curve_records` accumulates every `speed_curve`
/// modifier's keyframes so `lower` can build the `Curves` section afterwards and patch each such
/// modifier's `extra` field to the resulting curve index.
fn encode_program(emitter: &EmitterView, curve_records: &mut Vec<Vec<(u32, f32)>>) -> Vec<u8> {
    let mut out = Vec::new();
    let block = emitter
        .block
        .as_ref()
        .expect("caller only calls this when a block is present");
    let fields = &block.fields;
    let (kind, count, mut params, seed_hash): (u8, u16, [f32; 6], u32) = match block.kind.as_str() {
        "ring" => (
            BLOCK_RING,
            count_of(fields.get("count")),
            [deg_to_rad(fields.get("start")), 0.0, 0.0, 0.0, 0.0, 0.0],
            0,
        ),
        "spiral" => (
            BLOCK_SPIRAL,
            count_of(fields.get("arms")),
            [
                deg_to_rad(fields.get("step")),
                deg_to_rad(fields.get("start")),
                0.0,
                0.0,
                0.0,
                0.0,
            ],
            0,
        ),
        "fan" => (
            BLOCK_FAN,
            count_of(fields.get("count")),
            [
                deg_to_rad(fields.get("spread")),
                deg_to_rad(fields.get("center")),
                0.0,
                0.0,
                0.0,
                0.0,
            ],
            0,
        ),
        "aimed" => (
            BLOCK_AIMED,
            count_of(fields.get("count")),
            [deg_to_rad(fields.get("spread")), 0.0, 0.0, 0.0, 0.0, 0.0],
            0,
        ),
        "wave" => {
            let direction = deg_to_rad(fields.get("direction"));
            (
                BLOCK_WAVE,
                count_of(fields.get("count")),
                [
                    unit_of(fields.get("amplitude")),
                    unit_of(fields.get("wavelength")),
                    direction,
                    0.0,
                    dmath::cos(direction),
                    dmath::sin(direction),
                ],
                0,
            )
        }
        "line" => {
            let direction = deg_to_rad(fields.get("direction"));
            (
                BLOCK_LINE,
                count_of(fields.get("count")),
                [
                    unit_of(fields.get("spacing")),
                    direction,
                    0.0,
                    0.0,
                    dmath::cos(direction),
                    dmath::sin(direction),
                ],
                0,
            )
        }
        "scatter" => {
            let seed_name = match fields.get("seed").map(|l| &l.value) {
                Some(FieldValue::Ident(name)) => name.as_str(),
                _ => "",
            };
            let mut hasher = StableHasher::new();
            hasher.write_str("grimoire.sigil-scatter-seed.v1");
            hasher.write_str(seed_name);
            (
                BLOCK_SCATTER,
                count_of(fields.get("count")),
                [
                    deg_to_rad(fields.get("cone")),
                    deg_to_rad(fields.get("direction")),
                    unit_of(fields.get("speed_jitter")),
                    0.0,
                    0.0,
                    0.0,
                ],
                hasher.finish() as u32,
            )
        }
        _ => (0, 0, [0.0; 6], 0), // unreachable once `validate` has run; encoded as inert data.
    };
    // Canonicalise `-0.0` to `+0.0` (contract §11.1: the decoder rejects the non-canonical
    // encoding, and a `deg_to_rad(0.0)` of an *absent* optional field is already `+0.0`, but an
    // explicit `0deg` in source parses to the same `+0.0` too, so this is a safety net, not a
    // real source of `-0.0` today).
    for slot in &mut params {
        if *slot == 0.0 {
            *slot = 0.0;
        }
    }

    out.push(kind);
    out.push(0); // reserved
    out.extend_from_slice(&count.to_le_bytes());
    for slot in &params {
        out.extend_from_slice(&slot.to_le_bytes());
    }
    out.extend_from_slice(&seed_hash.to_le_bytes());

    out.extend_from_slice(&(emitter.modifiers.len() as u16).to_le_bytes());
    for modifier in &emitter.modifiers {
        encode_modifier(modifier, curve_records, &mut out);
    }
    out
}

fn encode_modifier(
    modifier: &crate::compiler::model::ModifierView,
    curve_records: &mut Vec<Vec<(u32, f32)>>,
    out: &mut Vec<u8>,
) {
    let fields = &modifier.fields;
    let (kind, flag, extra, params): (u8, u8, u16, [f32; 3]) = match modifier.kind.as_str() {
        "accelerate" => (
            MODIFIER_ACCELERATE,
            0,
            0,
            [
                unit_of(fields.get("rate")),
                unit_of(fields.get("max_speed")),
                0.0,
            ],
        ),
        "sine_offset" => (
            MODIFIER_SINE_OFFSET,
            0,
            0,
            [
                unit_of(fields.get("amplitude")),
                unit_of(fields.get("period")),
                deg_to_rad(fields.get("phase")),
            ],
        ),
        "rotate" => (
            MODIFIER_ROTATE,
            0,
            0,
            [deg_to_rad(fields.get("rate")), 0.0, 0.0],
        ),
        "mirror" => {
            let folds = match fields.get("folds").map(|l| &l.value) {
                Some(FieldValue::Int(v)) => (*v).clamp(0, i64::from(u16::MAX)) as u16,
                _ => 0,
            };
            (
                MODIFIER_MIRROR,
                0,
                folds,
                [deg_to_rad(fields.get("axis")), 0.0, 0.0],
            )
        }
        "speed_curve" => {
            let keys: Vec<(u32, f32)> = match fields.get("keys").map(|l| &l.value) {
                Some(FieldValue::List(entries)) => entries
                    .iter()
                    .filter_map(|entry| {
                        let FieldValue::Record(record_fields) = entry else {
                            return None;
                        };
                        let at = record_fields.iter().find_map(|(n, v)| {
                            if n == "at"
                                && let FieldValue::Quantity(value, _) = v
                            {
                                return Some(*value as f32);
                            }
                            None
                        })?;
                        let mul = record_fields.iter().find_map(|(n, v)| {
                            if n == "mul" {
                                return Some(match v {
                                    FieldValue::Float(value) => *value as f32,
                                    FieldValue::Int(value) => *value as f32,
                                    _ => return None,
                                });
                            }
                            None
                        })?;
                        Some((at.round() as u32, mul))
                    })
                    .collect(),
                _ => Vec::new(),
            };
            curve_records.push(keys);
            let flag = match fields.get("interp").map(|l| &l.value) {
                Some(FieldValue::Ident(name)) if name == "smooth" => 1,
                _ => 0,
            };
            (
                MODIFIER_SPEED_CURVE,
                flag,
                (curve_records.len() - 1) as u16,
                [0.0; 3],
            )
        }
        "curve" => (
            MODIFIER_CURVE,
            0,
            0,
            [deg_to_rad(fields.get("turn")), 0.0, 0.0],
        ),
        _ => (0, 0, 0, [0.0; 3]), // unreachable once `validate` has run.
    };
    out.push(kind);
    out.push(flag);
    out.extend_from_slice(&extra.to_le_bytes());
    for slot in &params {
        out.extend_from_slice(&slot.to_le_bytes());
    }
}

fn encode_emitter(
    emitter: &EmitterView,
    bullet_index: &BTreeMap<(String, String), u16>,
    program: Option<u16>,
) -> Vec<u8> {
    let bullet_type = match emitter
        .fields
        .get("bullet")
        .map(|l| (&l.value, l.origin_file.clone()))
    {
        Some((FieldValue::Ident(name), origin)) => bullet_index
            .get(&(origin, name.clone()))
            .copied()
            .unwrap_or(0),
        _ => 0,
    };
    let role = match emitter.fields.get("role").map(|l| &l.value) {
        Some(FieldValue::Ident(name)) if name == "sub" => 1u8,
        _ => 0u8,
    };
    let delay_ticks = match emitter.fields.get("delay").map(|l| &l.value) {
        Some(FieldValue::Quantity(v, _)) => v.round() as u32,
        _ => 0,
    };
    let repeat = match emitter.fields.get("repeat").map(|l| &l.value) {
        Some(FieldValue::Ident(text)) if text == "forever" => FOREVER,
        Some(FieldValue::Int(v)) => (*v).clamp(0, i64::from(u32::MAX - 1)) as u32,
        _ => 1,
    };
    let interval_ticks = match emitter.fields.get("interval").map(|l| &l.value) {
        Some(FieldValue::Quantity(v, _)) => v.round() as u32,
        _ => 1,
    };
    let speed = match emitter.fields.get("speed").map(|l| &l.value) {
        Some(FieldValue::Quantity(v, _)) => *v as f32,
        _ => 0.0,
    };
    let (offset_x, offset_y) = match emitter.fields.get("offset").map(|l| &l.value) {
        Some(FieldValue::Record(entries)) => {
            let x = entries
                .iter()
                .find_map(|(n, v)| (n == "x").then(|| unit_of_value(v)).flatten());
            let y = entries
                .iter()
                .find_map(|(n, v)| (n == "y").then(|| unit_of_value(v)).flatten());
            (x.unwrap_or(0.0), y.unwrap_or(0.0))
        }
        _ => (0.0, 0.0),
    };

    let mut out = Vec::with_capacity(30);
    out.extend_from_slice(&bullet_type.to_le_bytes());
    out.extend_from_slice(&program.unwrap_or(NO_PROGRAM).to_le_bytes());
    out.push(role);
    out.push(0); // reserved
    out.extend_from_slice(&delay_ticks.to_le_bytes());
    out.extend_from_slice(&repeat.to_le_bytes());
    out.extend_from_slice(&interval_ticks.to_le_bytes());
    out.extend_from_slice(&speed.to_le_bytes());
    out.extend_from_slice(&offset_x.to_le_bytes());
    out.extend_from_slice(&offset_y.to_le_bytes());
    out
}

fn unit_of_value(value: &FieldValue) -> Option<f32> {
    match value {
        FieldValue::Quantity(v, _) => Some(*v as f32),
        _ => None,
    }
}

fn encode_programs_section(programs: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(programs.len() as u16).to_le_bytes());
    for program in programs {
        out.extend_from_slice(program);
    }
    out
}

fn encode_emitters_section(emitters: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(emitters.len() as u16).to_le_bytes());
    for emitter in emitters {
        out.extend_from_slice(emitter);
    }
    out
}

fn encode_curves_section(curves: &[Vec<(u32, f32)>]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(curves.len() as u16).to_le_bytes());
    for curve in curves {
        out.extend_from_slice(&(curve.len() as u16).to_le_bytes());
        for &(at, mul) in curve {
            out.extend_from_slice(&at.to_le_bytes());
            out.extend_from_slice(&mul.to_le_bytes());
        }
    }
    out
}

fn encode_behavior_refs_section(refs: &BTreeSet<u32>) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(refs.len() as u16).to_le_bytes());
    for id in refs {
        out.extend_from_slice(&id.to_le_bytes());
    }
    out
}

/// Assembles the header and section table around already-encoded section contents (contract
/// §11.1): sections in ascending `kind` order, tightly packed, `content_hash` computed last.
fn assemble(unit_id: u64, sections: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut sections = sections.to_vec();
    sections.sort_by_key(|&(kind, _)| kind);

    let section_count = sections.len() as u32;
    let table_len = 4u64 + u64::from(section_count) * SECTION_ENTRY_LEN;
    let mut table = Vec::new();
    table.extend_from_slice(&section_count.to_le_bytes());
    let mut body = Vec::new();
    let mut offset = table_len;
    for (kind, bytes) in &sections {
        table.extend_from_slice(&kind.to_le_bytes());
        table.extend_from_slice(&0u32.to_le_bytes());
        table.extend_from_slice(&offset.to_le_bytes());
        table.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        offset += bytes.len() as u64;
        body.extend_from_slice(bytes);
    }
    let mut payload = table;
    payload.extend_from_slice(&body);

    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&unit_id.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    out.extend_from_slice(&payload);

    let mut hasher = StableHasher::new();
    hasher.write_bytes(&out[0..24]);
    hasher.write_bytes(&out[32..out.len()]);
    let hash = hasher.finish();
    out[24..32].copy_from_slice(&hash.to_le_bytes());
    out
}
