//! Hand-built `SigilUnit` v1 bytes for this crate's integration tests (Plan 0002 WP5.2).
//!
//! Every section is assembled field by field from `docs/formats/sigil.md` §10, independent of
//! [`SigilUnit::to_bytes`], following the convention of `tests/interpreter.rs` and
//! `src/test_support.rs`: the decoder is exercised against externally constructed bytes. Only the
//! public API is reachable from here, so tag numbers are named again.

#![allow(dead_code)] // each test binary uses a different subset.

use grimoire_core::StableHasher;
use grimoire_sigil::SigilUnit;

/// Block kind tags (`docs/formats/sigil.md` §10.4).
pub mod block_kind {
    pub const RING: u8 = 1;
    pub const SPIRAL: u8 = 2;
    pub const FAN: u8 = 3;
    pub const AIMED: u8 = 4;
    pub const WAVE: u8 = 5;
    pub const LINE: u8 = 6;
    pub const SCATTER: u8 = 7;
}

/// Modifier kind tags (`docs/formats/sigil.md` §10.4).
pub mod modifier_kind {
    pub const ACCELERATE: u8 = 1;
    pub const SINE_OFFSET: u8 = 2;
    pub const ROTATE: u8 = 3;
    pub const MIRROR: u8 = 4;
    pub const SPEED_CURVE: u8 = 5;
    pub const CURVE: u8 = 6;
}

/// Transform kind tags (`docs/formats/sigil.md` §10.9).
pub mod transform_kind {
    pub const REVERSE: u8 = 1;
    pub const CHANGE_TYPE: u8 = 2;
    pub const BURST: u8 = 3;
    pub const BECOME_EMITTER: u8 = 4;
}

/// `0xFFFF`: no program (`docs/formats/sigil.md` §10.5 and §10.9).
pub const NO_PROGRAM: u16 = 0xFFFF;

/// Flag bits (`docs/formats/sigil.md` §10.3).
pub mod flags {
    pub const SMASHABLE: u8 = 1;
    pub const REFLECTABLE: u8 = 2;
    pub const ENV_ACTIVE: u8 = 4;
    pub const GRAZEABLE: u8 = 8;
}

/// One `BulletTypes` record: radius `1.0`, `collision_radius` `1.0`, the given lifetime and
/// flags, silhouette = palette = `visual`, hostile palette space, no glow.
pub fn bullet_type(lifetime_ticks: u32, flags: u8, visual: u16) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&1.0f32.to_le_bytes());
    out.extend_from_slice(&1.0f32.to_le_bytes());
    out.extend_from_slice(&lifetime_ticks.to_le_bytes());
    out.push(flags);
    out.push(0);
    out.extend_from_slice(&visual.to_le_bytes());
    out.extend_from_slice(&visual.to_le_bytes());
    out.push(1);
    out.push(0);
    out
}

/// Prefixes `records` with their `u16` count, the shape of every v1 section.
pub fn counted(records: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(records.len() as u16).to_le_bytes());
    for record in records {
        out.extend_from_slice(record);
    }
    out
}

/// One `BlockDef` without a scatter seed.
pub fn block(kind: u8, count: u16, params: [f32; 6]) -> Vec<u8> {
    let mut out = vec![kind, 0];
    out.extend_from_slice(&count.to_le_bytes());
    for param in params {
        out.extend_from_slice(&param.to_le_bytes());
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    out
}

/// One `ModifierDef`.
pub fn modifier(kind: u8, flag: u8, extra: u16, params: [f32; 3]) -> Vec<u8> {
    let mut out = vec![kind, flag];
    out.extend_from_slice(&extra.to_le_bytes());
    for param in params {
        out.extend_from_slice(&param.to_le_bytes());
    }
    out
}

/// One `Programs` record: a block followed by its modifier stack.
pub fn program(block: Vec<u8>, modifiers: &[Vec<u8>]) -> Vec<u8> {
    let mut out = block;
    out.extend_from_slice(&counted(modifiers));
    out
}

/// Emitter timing of an [`emitter`] record.
#[derive(Clone, Copy)]
pub struct Timing {
    pub delay: u32,
    pub repeat: u32,
    pub interval: u32,
}

/// Fires once, at local tick `0`.
pub const ONCE: Timing = Timing {
    delay: 0,
    repeat: 1,
    interval: 1,
};

/// One `Emitters` record (`role` `0` primary, `1` sub), no offset.
pub fn emitter(bullet_type: u16, program: u16, role: u8, timing: Timing, speed: f32) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&bullet_type.to_le_bytes());
    out.extend_from_slice(&program.to_le_bytes());
    out.push(role);
    out.push(0);
    out.extend_from_slice(&timing.delay.to_le_bytes());
    out.extend_from_slice(&timing.repeat.to_le_bytes());
    out.extend_from_slice(&timing.interval.to_le_bytes());
    out.extend_from_slice(&speed.to_le_bytes());
    out.extend_from_slice(&0f32.to_le_bytes());
    out.extend_from_slice(&0f32.to_le_bytes());
    out
}

/// A transform trigger (`docs/formats/sigil.md` §10.9).
#[derive(Clone, Copy)]
pub enum When {
    Time(u32),
    Distance(f32),
    Event(u32),
}

/// One 24-byte transform record; `program`/`speed` only matter for `burst`.
pub fn transform(kind: u8, when: When, target: u16, program: u16, speed: f32) -> Vec<u8> {
    let (trigger, at_ticks, distance, event) = match when {
        When::Time(ticks) => (1u8, ticks, 0.0f32, 0u32),
        When::Distance(units) => (2, 0, units, 0),
        When::Event(id) => (3, 0, 0.0, id),
    };
    let mut out = vec![kind, trigger];
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&at_ticks.to_le_bytes());
    out.extend_from_slice(&distance.to_le_bytes());
    out.extend_from_slice(&event.to_le_bytes());
    out.extend_from_slice(&target.to_le_bytes());
    out.extend_from_slice(&program.to_le_bytes());
    out.extend_from_slice(&speed.to_le_bytes());
    out
}

/// A `reverse` transform record.
pub fn reverse(when: When) -> Vec<u8> {
    transform(transform_kind::REVERSE, when, 0, NO_PROGRAM, 0.0)
}

/// A `change_type` transform record.
pub fn change_type(when: When, to: u16) -> Vec<u8> {
    transform(transform_kind::CHANGE_TYPE, when, to, NO_PROGRAM, 0.0)
}

/// A `burst` transform record.
pub fn burst(when: When, bullet_type: u16, program: u16, speed: f32) -> Vec<u8> {
    transform(transform_kind::BURST, when, bullet_type, program, speed)
}

/// A `become_emitter` transform record.
pub fn become_emitter(when: When, emitter: u16) -> Vec<u8> {
    transform(
        transform_kind::BECOME_EMITTER,
        when,
        emitter,
        NO_PROGRAM,
        0.0,
    )
}

/// One `Transforms` record for `bullet_type`, with an optional `(behavior id, params)` binding.
pub fn script(
    bullet_type: u16,
    behavior: Option<(u32, &[f32])>,
    transforms: &[Vec<u8>],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&bullet_type.to_le_bytes());
    let (flag, id, params): (u8, u32, &[f32]) = match behavior {
        Some((id, params)) => (1, id, params),
        None => (0, 0, &[]),
    };
    out.push(flag);
    out.push(0);
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&(params.len() as u16).to_le_bytes());
    out.extend_from_slice(&(transforms.len() as u16).to_le_bytes());
    for param in params {
        out.extend_from_slice(&param.to_le_bytes());
    }
    for transform in transforms {
        out.extend_from_slice(transform);
    }
    out
}

/// A `BehaviorRefs` section body.
pub fn behavior_refs(ids: &[u32]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(ids.len() as u16).to_le_bytes());
    for id in ids {
        out.extend_from_slice(&id.to_le_bytes());
    }
    out
}

/// The sections of a unit under construction, by kind.
#[derive(Default)]
pub struct Sections {
    pub bullet_types: Vec<Vec<u8>>,
    pub programs: Vec<Vec<u8>>,
    pub emitters: Vec<Vec<u8>>,
    pub scripts: Vec<Vec<u8>>,
    pub behavior_refs: Vec<u32>,
}

impl Sections {
    /// Assembles the unit bytes: header, packed section table, `content_hash`.
    pub fn bytes(&self, id: u64) -> Vec<u8> {
        let mut contents: Vec<(u32, Vec<u8>)> = vec![(1, counted(&self.bullet_types))];
        if !self.programs.is_empty() {
            contents.push((2, counted(&self.programs)));
        }
        contents.push((3, counted(&self.emitters)));
        if !self.scripts.is_empty() {
            contents.push((4, counted(&self.scripts)));
        }
        if !self.behavior_refs.is_empty() {
            contents.push((6, behavior_refs(&self.behavior_refs)));
        }
        assemble(id, contents)
    }

    /// Assembles and decodes the unit.
    pub fn unit(&self, id: u64) -> SigilUnit {
        SigilUnit::from_bytes(&self.bytes(id)).expect("hand-built fixture must decode")
    }
}

/// Assembles a unit's bytes from already-encoded section bodies.
pub fn assemble(id: u64, mut contents: Vec<(u32, Vec<u8>)>) -> Vec<u8> {
    contents.sort_by_key(|&(kind, _)| kind);
    let table_len = 4u64 + contents.len() as u64 * 24;
    let mut table = Vec::new();
    table.extend_from_slice(&(contents.len() as u32).to_le_bytes());
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

    let mut out = Vec::new();
    out.extend_from_slice(b"GRIMSIGL");
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    out.extend_from_slice(&payload);
    let mut hasher = StableHasher::new();
    hasher.write_bytes(&out[0..24]);
    hasher.write_bytes(&out[32..]);
    let hash = hasher.finish();
    out[24..32].copy_from_slice(&hash.to_le_bytes());
    out
}
