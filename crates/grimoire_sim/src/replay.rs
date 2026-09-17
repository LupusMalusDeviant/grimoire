//! Replay binary format version 2 (contract §8.1): an additive header on top of [`InputLog`].
//!
//! [`InputLog`] version 1 (magic, [`InputLog::FORMAT_VERSION`], `to_bytes`/`from_bytes`) is
//! unchanged and stays readable: [`InputLog::from_bytes`] still reads only version 1.
//! [`Replay::from_bytes`] reads both versions; [`Replay::to_bytes`] writes version 2 when
//! `header` is `Some`, and with `header: None` writes byte-identical output to
//! [`InputLog::to_bytes`].
//!
//! # Binary format (version 2)
//!
//! All integers little-endian, texts UTF-8 with an explicit length prefix:
//!
//! | Field | Type | Rule |
//! |-------|------|------|
//! | magic | 8 bytes | `b"GRIMREPL"`, as in v1 |
//! | version | `u32` | `2` (`1` takes the v1 path; anything else is [`SimError::UnsupportedVersion`]) |
//! | `header_len` | `u32` | bytes from the end of this field to right before `frame_count`; `<=` remaining input |
//! | `seed` | `u64` | |
//! | `tick_rate_hz` | `u32` | `!= 0` |
//! | `engine_version` | `u8` length + bytes | 1..=[`MAX_ENGINE_VERSION_BYTES`], charset `[0-9A-Za-z.+-]` |
//! | `engine_build` | 20 bytes | all-zero = unknown |
//! | `content_manifest` | `u64` | |
//! | `swap_count` | `u32` | `<=` [`MAX_SWAP_RECORDS`], then that many 16-byte `(tick: u64, content_manifest: u64)` entries |
//! | `meta_count` | `u16` | `<=` [`MAX_APP_METADATA`], then that many `(key_len: u8, key, value_len: u16, value)` entries |
//! | `frame_count` | `u64` | then that many 48-byte frames, byte-identical to the v1 frame encoding |
//!
//! Header fields never influence [`crate::Simulation::state_hash`], a snapshot or
//! [`crate::replay`]: the caller decides what a header (in particular a mismatched
//! `content_manifest`) means; this crate only stores it.

use std::collections::BTreeMap;

use grimoire_core::{StableHash, StableHasher};

use crate::binio::{FRAME_SIZE, Reader, read_frame, write_frame};
use crate::error::SimError;
use crate::input::InputLog;

/// `grimoire_sim` version that produced this build (`env!("CARGO_PKG_VERSION")`, e.g. `"0.1.2"`).
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Maximum length in bytes of [`ReplayHeader::engine_version`].
pub const MAX_ENGINE_VERSION_BYTES: usize = 64;

/// Maximum number of entries in [`ReplayHeader::swaps`].
pub const MAX_SWAP_RECORDS: usize = 4096;

/// Maximum number of entries in [`ReplayHeader::app_metadata`].
pub const MAX_APP_METADATA: usize = 32;

/// Maximum length in bytes of one [`ReplayHeader::app_metadata`] key.
pub const MAX_APP_KEY_BYTES: usize = 64;

/// Maximum length in bytes of one [`ReplayHeader::app_metadata`] value.
pub const MAX_APP_VALUE_BYTES: usize = 1024;

/// Byte size of one swap-record entry on disk (`tick: u64` + `content_manifest: u64`).
const SWAP_RECORD_SIZE: usize = 16;

/// Smallest possible byte size of one metadata entry (`key_len: u8` + 1-byte key +
/// `value_len: u16` + an empty value), used to bound `meta_count` before allocating.
const MIN_METADATA_ENTRY_SIZE: usize = 1 + 1 + 2;

/// Byte size of an encoded [`BuildHash`].
const BUILD_HASH_SIZE: usize = 20;

/// Git commit (20-byte SHA-1) of the engine build that produced a replay.
///
/// [`BuildHash::UNKNOWN`] (all-zero bytes) marks a build without a recorded commit: a local
/// build, or one built from a `cargo` git checkout.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BuildHash(pub [u8; 20]);

impl BuildHash {
    /// A build without a recorded commit hash (all bytes zero). Equal to [`BuildHash::default`].
    pub const UNKNOWN: BuildHash = BuildHash([0; BUILD_HASH_SIZE]);

    /// Whether this is a recorded commit hash, as opposed to [`BuildHash::UNKNOWN`].
    #[must_use]
    pub const fn is_known(&self) -> bool {
        let mut index = 0;
        while index < BUILD_HASH_SIZE {
            if self.0[index] != 0 {
                return true;
            }
            index += 1;
        }
        false
    }

    /// Lowercase hex encoding, always exactly 40 characters.
    #[must_use]
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// Parses the encoding produced by [`BuildHash::to_hex`].
    ///
    /// # Errors
    ///
    /// [`SimError::InvalidHex`] unless `hex` is exactly 40 lowercase hex digits.
    pub fn from_hex(hex: &str) -> Result<Self, SimError> {
        let mut bytes = [0u8; BUILD_HASH_SIZE];
        parse_hex_into(hex, &mut bytes, "engine_build")?;
        Ok(Self(bytes))
    }
}

/// Content-manifest hash of a session (contract §11.8, computed by `grimoire_sigil`); this crate
/// only stores it in a [`ReplayHeader`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentManifestHash(pub u64);

impl ContentManifestHash {
    /// No content installed yet. Equal to [`ContentManifestHash::default`].
    pub const EMPTY: ContentManifestHash = ContentManifestHash(0);

    /// Lowercase hex encoding, always exactly 16 characters.
    #[must_use]
    pub fn to_hex(&self) -> String {
        format!("{:016x}", self.0)
    }

    /// Parses the encoding produced by [`ContentManifestHash::to_hex`].
    ///
    /// # Errors
    ///
    /// [`SimError::InvalidHex`] unless `hex` is exactly 16 lowercase hex digits.
    pub fn from_hex(hex: &str) -> Result<Self, SimError> {
        let mut bytes = [0u8; 8];
        parse_hex_into(hex, &mut bytes, "content_manifest")?;
        Ok(Self(u64::from_be_bytes(bytes)))
    }
}

impl StableHash for ContentManifestHash {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_u64(self.0);
    }
}

/// Parses `hex` (must be exactly `2 * out.len()` lowercase hex digits, big-endian byte order)
/// into `out`.
fn parse_hex_into(hex: &str, out: &mut [u8], field: &'static str) -> Result<(), SimError> {
    let bytes = hex.as_bytes();
    if bytes.len() != out.len() * 2 {
        return Err(SimError::InvalidHex { field });
    }
    for (index, slot) in out.iter_mut().enumerate() {
        let hi = hex_nibble(bytes[index * 2]).ok_or(SimError::InvalidHex { field })?;
        let lo = hex_nibble(bytes[index * 2 + 1]).ok_or(SimError::InvalidHex { field })?;
        *slot = (hi << 4) | lo;
    }
    Ok(())
}

/// Value of one lowercase hex digit, or `None` if `byte` is not one.
const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// Parses `GRIMOIRE_BUILD_HASH` (40 lowercase hex characters) at compile time.
///
/// # Panics
///
/// If the environment variable is set to anything other than exactly 40 lowercase hex
/// characters. This function is only ever evaluated in the `const` initialiser of
/// [`ENGINE_BUILD`], so the panic can only happen while compiling this crate, never at runtime:
/// an invalid value is a build error by design (contract §8.1). This is the one deliberate
/// exception to "no panics on any valid runtime input" in this crate, because it cannot execute
/// at runtime at all.
const fn parse_build_env(hex: &str) -> BuildHash {
    let bytes = hex.as_bytes();
    assert!(
        bytes.len() == BUILD_HASH_SIZE * 2,
        "GRIMOIRE_BUILD_HASH must be exactly 40 lowercase hex characters"
    );
    let mut out = [0u8; BUILD_HASH_SIZE];
    let mut index = 0;
    while index < BUILD_HASH_SIZE {
        let hi = match hex_nibble(bytes[index * 2]) {
            Some(value) => value,
            None => panic!("GRIMOIRE_BUILD_HASH must be lowercase hex"),
        };
        let lo = match hex_nibble(bytes[index * 2 + 1]) {
            Some(value) => value,
            None => panic!("GRIMOIRE_BUILD_HASH must be lowercase hex"),
        };
        out[index] = (hi << 4) | lo;
        index += 1;
    }
    BuildHash(out)
}

/// Build commit of this binary, read from `GRIMOIRE_BUILD_HASH` at compile time, else
/// [`BuildHash::UNKNOWN`].
///
/// Local builds and builds from a `cargo` git checkout do not set the variable and are always
/// [`BuildHash::UNKNOWN`]; engine CI and the release workflow set it to the built commit.
pub const ENGINE_BUILD: BuildHash = match option_env!("GRIMOIRE_BUILD_HASH") {
    Some(hex) => parse_build_env(hex),
    None => BuildHash::UNKNOWN,
};

/// One content hot-swap recorded in a replay (contract §11.8).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapRecord {
    /// Index of the first tick that runs with the new content.
    pub tick: u64,
    /// Content manifest installed at [`SwapRecord::tick`].
    pub content_manifest: ContentManifestHash,
}

impl SwapRecord {
    /// Creates a swap record.
    #[must_use]
    pub const fn new(tick: u64, content_manifest: ContentManifestHash) -> Self {
        Self {
            tick,
            content_manifest,
        }
    }
}

/// Version-2 replay header: engine identity, content epoch and application metadata.
///
/// Never influences [`crate::Simulation::state_hash`], a snapshot or [`crate::replay`]; it is
/// information for the tool reading the replay, not simulation state. Build with
/// [`ReplayHeader::for_this_build`] and then assign fields, since the type is
/// `#[non_exhaustive]`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayHeader {
    /// `grimoire_sim` version that recorded the replay: 1..=[`MAX_ENGINE_VERSION_BYTES`] bytes,
    /// charset `[0-9A-Za-z.+-]`. Informational only; no reader rejects a replay for its value.
    pub engine_version: String,
    /// Git commit of the engine build that recorded the replay.
    pub engine_build: BuildHash,
    /// Content manifest right after installation, before any recorded swap.
    pub content_manifest: ContentManifestHash,
    /// Content hot-swaps during the run. Ticks strictly ascending, at most
    /// [`MAX_SWAP_RECORDS`] entries.
    pub swaps: Vec<SwapRecord>,
    /// Application-defined metadata, at most [`MAX_APP_METADATA`] entries. The engine never
    /// reads these; the `grimoire.` key prefix is reserved for it by convention only.
    pub app_metadata: BTreeMap<String, String>,
}

impl ReplayHeader {
    /// Header for a replay recorded by this build right now: [`ENGINE_VERSION`],
    /// [`ENGINE_BUILD`], the given content manifest, no swaps and no metadata.
    #[must_use]
    pub fn for_this_build(content_manifest: ContentManifestHash) -> Self {
        Self {
            engine_version: ENGINE_VERSION.to_string(),
            engine_build: ENGINE_BUILD,
            content_manifest,
            swaps: Vec::new(),
            app_metadata: BTreeMap::new(),
        }
    }

    /// Whether a golden-master tool may accept this header: no swap ever changed the content
    /// during the run.
    #[must_use]
    pub fn is_golden_eligible(&self) -> bool {
        self.swaps.is_empty()
    }
}

/// A recorded run in the version-1 or version-2 binary format; see the module documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replay {
    /// Version-2 header, or `None` for a version-1 replay.
    pub header: Option<ReplayHeader>,
    /// The recorded input, exactly as in a plain [`InputLog`].
    pub log: InputLog,
}

impl Replay {
    /// Binary format version written by [`Replay::to_bytes`] when `header` is `Some`.
    pub const FORMAT_VERSION: u32 = 2;

    /// Decodes a replay written by [`Replay::to_bytes`], version 1 or version 2.
    ///
    /// Never panics: every malformed input yields a [`SimError`], and every declared count is
    /// checked against its maximum and against the remaining input before anything is allocated.
    ///
    /// # Errors
    ///
    /// [`SimError::BadMagic`] and [`SimError::UnsupportedVersion`] as in [`InputLog::from_bytes`];
    /// for version 2, additionally [`SimError::HeaderLength`], [`SimError::FieldTooLong`],
    /// [`SimError::TooManyEntries`], [`SimError::InvalidText`], [`SimError::MetadataKeyOrder`],
    /// [`SimError::SwapOrder`], [`SimError::SwapOutOfRange`] and [`SimError::FrameDataLength`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SimError> {
        let mut reader = Reader::new(bytes);
        if reader.take::<8>()? != InputLog::MAGIC {
            return Err(SimError::BadMagic);
        }
        let version = u32::from_le_bytes(reader.take()?);
        match version {
            InputLog::FORMAT_VERSION => Ok(Self {
                header: None,
                log: InputLog::from_bytes(bytes)?,
            }),
            Self::FORMAT_VERSION => decode_v2(&mut reader),
            other => Err(SimError::UnsupportedVersion(other)),
        }
    }

    /// Encodes this replay.
    ///
    /// With `header: None` the result is byte-identical to `self.log.to_bytes()`. Never panics:
    /// every condition [`InputLog::to_bytes`] would panic on, and every header field that would
    /// not round-trip, is reported as an [`Err`] instead.
    ///
    /// # Errors
    ///
    /// [`SimError::InvalidTickRate`] if `log.tick_rate_hz == 0`; for `header: Some`, additionally
    /// [`SimError::FieldTooLong`], [`SimError::TooManyEntries`], [`SimError::InvalidText`],
    /// [`SimError::SwapOrder`] and [`SimError::SwapOutOfRange`].
    pub fn to_bytes(&self) -> Result<Vec<u8>, SimError> {
        if self.log.tick_rate_hz == 0 {
            return Err(SimError::InvalidTickRate);
        }
        match &self.header {
            None => Ok(self.log.to_bytes()),
            Some(header) => encode_v2(header, &self.log),
        }
    }
}

/// Decodes the version-2 body, `reader` positioned right after the format-version field.
fn decode_v2(reader: &mut Reader<'_>) -> Result<Replay, SimError> {
    let header_len = u32::from_le_bytes(reader.take()?);
    let available_before_header = reader.remaining();
    if header_len as usize > available_before_header {
        return Err(SimError::HeaderLength {
            declared: header_len,
            consumed: available_before_header,
        });
    }
    let header_start = reader.offset();

    let seed = u64::from_le_bytes(reader.take()?);
    let tick_rate_hz = u32::from_le_bytes(reader.take()?);
    if tick_rate_hz == 0 {
        return Err(SimError::InvalidTickRate);
    }
    let engine_version = read_engine_version(reader)?;
    let engine_build = BuildHash(reader.take::<BUILD_HASH_SIZE>()?);
    let content_manifest = ContentManifestHash(u64::from_le_bytes(reader.take()?));
    let swaps = read_swaps(reader)?;
    let app_metadata = read_metadata(reader)?;

    let consumed = reader.offset() - header_start;
    if consumed != header_len as usize {
        return Err(SimError::HeaderLength {
            declared: header_len,
            consumed,
        });
    }

    let frame_count = u64::from_le_bytes(reader.take()?);
    let remaining = reader.remaining();
    let expected = usize::try_from(frame_count)
        .ok()
        .and_then(|count| count.checked_mul(FRAME_SIZE));
    if expected != Some(remaining) {
        return Err(SimError::FrameDataLength {
            frames: frame_count,
            remaining,
        });
    }
    let count = remaining / FRAME_SIZE;
    let mut frames = Vec::with_capacity(count);
    for _ in 0..count {
        frames.push(read_frame(reader)?);
    }
    let frame_len = frames.len() as u64;
    for swap in &swaps {
        if swap.tick > frame_len {
            return Err(SimError::SwapOutOfRange {
                tick: swap.tick,
                frames: frame_len,
            });
        }
    }

    Ok(Replay {
        header: Some(ReplayHeader {
            engine_version,
            engine_build,
            content_manifest,
            swaps,
            app_metadata,
        }),
        log: InputLog {
            seed,
            tick_rate_hz,
            frames,
        },
    })
}

/// Whether `byte` is allowed in [`ReplayHeader::engine_version`] (`[0-9A-Za-z.+-]`); every such
/// byte is ASCII, so a validated string is always valid UTF-8.
const fn is_engine_version_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'-')
}

/// Whether `byte` is allowed in an [`ReplayHeader::app_metadata`] key (`[a-z0-9._-]`); every such
/// byte is ASCII, so a validated key is always valid UTF-8.
const fn is_metadata_key_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
}

fn read_engine_version(reader: &mut Reader<'_>) -> Result<String, SimError> {
    let len = reader.take::<1>()?[0] as usize;
    if len == 0 {
        return Err(SimError::InvalidText {
            field: "engine_version",
        });
    }
    if len > MAX_ENGINE_VERSION_BYTES {
        return Err(SimError::FieldTooLong {
            field: "engine_version",
            len,
            max: MAX_ENGINE_VERSION_BYTES,
        });
    }
    let bytes = reader.take_slice(len)?;
    if !bytes.iter().all(|&byte| is_engine_version_byte(byte)) {
        return Err(SimError::InvalidText {
            field: "engine_version",
        });
    }
    Ok(String::from_utf8(bytes.to_vec())
        .expect("is_engine_version_byte only allows ASCII, which is always valid UTF-8"))
}

fn read_swaps(reader: &mut Reader<'_>) -> Result<Vec<SwapRecord>, SimError> {
    let declared = u32::from_le_bytes(reader.take()?);
    let count = check_count(
        u64::from(declared),
        MAX_SWAP_RECORDS,
        SWAP_RECORD_SIZE,
        reader.remaining(),
        "swaps",
    )?;
    let mut swaps = Vec::with_capacity(count);
    let mut previous_tick: Option<u64> = None;
    for index in 0..count {
        let tick = u64::from_le_bytes(reader.take()?);
        let manifest = ContentManifestHash(u64::from_le_bytes(reader.take()?));
        if previous_tick.is_some_and(|previous| tick <= previous) {
            return Err(SimError::SwapOrder { index });
        }
        previous_tick = Some(tick);
        swaps.push(SwapRecord::new(tick, manifest));
    }
    Ok(swaps)
}

fn read_metadata(reader: &mut Reader<'_>) -> Result<BTreeMap<String, String>, SimError> {
    let declared = u16::from_le_bytes(reader.take()?);
    let count = check_count(
        u64::from(declared),
        MAX_APP_METADATA,
        MIN_METADATA_ENTRY_SIZE,
        reader.remaining(),
        "app_metadata",
    )?;
    let mut map = BTreeMap::new();
    let mut previous_key: Option<Vec<u8>> = None;
    for index in 0..count {
        let key_len = reader.take::<1>()?[0] as usize;
        if key_len == 0 {
            return Err(SimError::InvalidText {
                field: "app_metadata.key",
            });
        }
        if key_len > MAX_APP_KEY_BYTES {
            return Err(SimError::FieldTooLong {
                field: "app_metadata.key",
                len: key_len,
                max: MAX_APP_KEY_BYTES,
            });
        }
        let key_bytes = reader.take_slice(key_len)?;
        if !key_bytes.iter().all(|&byte| is_metadata_key_byte(byte)) {
            return Err(SimError::InvalidText {
                field: "app_metadata.key",
            });
        }
        if previous_key
            .as_deref()
            .is_some_and(|previous| key_bytes <= previous)
        {
            return Err(SimError::MetadataKeyOrder { index });
        }
        previous_key = Some(key_bytes.to_vec());
        let key = String::from_utf8(key_bytes.to_vec())
            .expect("is_metadata_key_byte only allows ASCII, which is always valid UTF-8");

        let value_len = u16::from_le_bytes(reader.take()?) as usize;
        if value_len > MAX_APP_VALUE_BYTES {
            return Err(SimError::FieldTooLong {
                field: "app_metadata.value",
                len: value_len,
                max: MAX_APP_VALUE_BYTES,
            });
        }
        let value_bytes = reader.take_slice(value_len)?;
        let value = String::from_utf8(value_bytes.to_vec()).map_err(|_| SimError::InvalidText {
            field: "app_metadata.value",
        })?;
        map.insert(key, value);
    }
    Ok(map)
}

/// Checks a declared entry count against `max` and against `remaining` (`count * entry_size`)
/// before any allocation sized by it, returning the count as `usize` on success.
fn check_count(
    count: u64,
    max: usize,
    entry_size: usize,
    remaining: usize,
    field: &'static str,
) -> Result<usize, SimError> {
    if count > max as u64 {
        return Err(SimError::TooManyEntries { field, count, max });
    }
    // `count <= max` and `max` is one of this module's small constants, so this never truncates.
    let count = count as usize;
    match count.checked_mul(entry_size) {
        Some(bytes) if bytes <= remaining => Ok(count),
        _ => Err(SimError::TooManyEntries {
            field,
            count: count as u64,
            max,
        }),
    }
}

/// Encodes the version-2 body for `header` and `log`.
fn encode_v2(header: &ReplayHeader, log: &InputLog) -> Result<Vec<u8>, SimError> {
    validate_engine_version(&header.engine_version)?;
    validate_swaps(&header.swaps, log.frames.len() as u64)?;
    validate_metadata(&header.app_metadata)?;

    let mut head = Vec::new();
    head.extend_from_slice(&log.seed.to_le_bytes());
    head.extend_from_slice(&log.tick_rate_hz.to_le_bytes());

    let version_bytes = header.engine_version.as_bytes();
    // Length was validated to be in 1..=MAX_ENGINE_VERSION_BYTES (255), so this never truncates.
    head.push(version_bytes.len() as u8);
    head.extend_from_slice(version_bytes);

    head.extend_from_slice(&header.engine_build.0);
    head.extend_from_slice(&header.content_manifest.0.to_le_bytes());

    // Validated to be at most MAX_SWAP_RECORDS, so this never truncates.
    head.extend_from_slice(&(header.swaps.len() as u32).to_le_bytes());
    for swap in &header.swaps {
        head.extend_from_slice(&swap.tick.to_le_bytes());
        head.extend_from_slice(&swap.content_manifest.0.to_le_bytes());
    }

    // Validated to be at most MAX_APP_METADATA, so this never truncates.
    head.extend_from_slice(&(header.app_metadata.len() as u16).to_le_bytes());
    for (key, value) in &header.app_metadata {
        // Validated to be at most MAX_APP_KEY_BYTES (64), so this never truncates.
        head.push(key.len() as u8);
        head.extend_from_slice(key.as_bytes());
        // Validated to be at most MAX_APP_VALUE_BYTES (1024), so this never truncates.
        head.extend_from_slice(&(value.len() as u16).to_le_bytes());
        head.extend_from_slice(value.as_bytes());
    }

    let mut bytes = Vec::with_capacity(8 + 4 + 4 + head.len() + 8 + FRAME_SIZE * log.frames.len());
    bytes.extend_from_slice(&InputLog::MAGIC);
    bytes.extend_from_slice(&Replay::FORMAT_VERSION.to_le_bytes());
    // head.len() fits comfortably in u32: bounded by the small constants above.
    bytes.extend_from_slice(&(head.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&head);
    bytes.extend_from_slice(&(log.frames.len() as u64).to_le_bytes());
    for frame in &log.frames {
        write_frame(&mut bytes, frame);
    }
    Ok(bytes)
}

fn validate_engine_version(version: &str) -> Result<(), SimError> {
    let len = version.len();
    if len == 0 {
        return Err(SimError::InvalidText {
            field: "engine_version",
        });
    }
    if len > MAX_ENGINE_VERSION_BYTES {
        return Err(SimError::FieldTooLong {
            field: "engine_version",
            len,
            max: MAX_ENGINE_VERSION_BYTES,
        });
    }
    if !version.bytes().all(is_engine_version_byte) {
        return Err(SimError::InvalidText {
            field: "engine_version",
        });
    }
    Ok(())
}

fn validate_swaps(swaps: &[SwapRecord], frame_count: u64) -> Result<(), SimError> {
    if swaps.len() > MAX_SWAP_RECORDS {
        return Err(SimError::TooManyEntries {
            field: "swaps",
            count: swaps.len() as u64,
            max: MAX_SWAP_RECORDS,
        });
    }
    let mut previous_tick: Option<u64> = None;
    for (index, swap) in swaps.iter().enumerate() {
        if previous_tick.is_some_and(|previous| swap.tick <= previous) {
            return Err(SimError::SwapOrder { index });
        }
        if swap.tick > frame_count {
            return Err(SimError::SwapOutOfRange {
                tick: swap.tick,
                frames: frame_count,
            });
        }
        previous_tick = Some(swap.tick);
    }
    Ok(())
}

fn validate_metadata(metadata: &BTreeMap<String, String>) -> Result<(), SimError> {
    if metadata.len() > MAX_APP_METADATA {
        return Err(SimError::TooManyEntries {
            field: "app_metadata",
            count: metadata.len() as u64,
            max: MAX_APP_METADATA,
        });
    }
    for (key, value) in metadata {
        let key_len = key.len();
        if key_len == 0 {
            return Err(SimError::InvalidText {
                field: "app_metadata.key",
            });
        }
        if key_len > MAX_APP_KEY_BYTES {
            return Err(SimError::FieldTooLong {
                field: "app_metadata.key",
                len: key_len,
                max: MAX_APP_KEY_BYTES,
            });
        }
        if !key.bytes().all(is_metadata_key_byte) {
            return Err(SimError::InvalidText {
                field: "app_metadata.key",
            });
        }
        if value.len() > MAX_APP_VALUE_BYTES {
            return Err(SimError::FieldTooLong {
                field: "app_metadata.value",
                len: value.len(),
                max: MAX_APP_VALUE_BYTES,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_hash_hex_round_trips() {
        let hash = BuildHash([0x1a; 20]);
        assert!(hash.is_known());
        let hex = hash.to_hex();
        assert_eq!(hex.len(), 40);
        assert_eq!(BuildHash::from_hex(&hex), Ok(hash));
        assert!(!BuildHash::UNKNOWN.is_known());
        assert_eq!(BuildHash::default(), BuildHash::UNKNOWN);
    }

    #[test]
    fn build_hash_rejects_bad_hex() {
        assert_eq!(
            BuildHash::from_hex("too short"),
            Err(SimError::InvalidHex {
                field: "engine_build"
            })
        );
        assert_eq!(
            BuildHash::from_hex(&"Z".repeat(40)),
            Err(SimError::InvalidHex {
                field: "engine_build"
            })
        );
    }

    #[test]
    fn content_manifest_hex_round_trips() {
        let hash = ContentManifestHash(0x0123_4567_89ab_cdef);
        let hex = hash.to_hex();
        assert_eq!(hex, "0123456789abcdef");
        assert_eq!(ContentManifestHash::from_hex(&hex), Ok(hash));
        assert_eq!(ContentManifestHash::EMPTY, ContentManifestHash::default());
    }

    #[test]
    fn engine_build_without_the_env_var_is_unknown() {
        // GRIMOIRE_BUILD_HASH is not set for this local test build (engine CI sets it in a
        // separate step this crate does not depend on).
        assert_eq!(ENGINE_BUILD, BuildHash::UNKNOWN);
    }

    #[test]
    fn engine_version_matches_the_crate_version() {
        assert_eq!(ENGINE_VERSION, env!("CARGO_PKG_VERSION"));
    }

    // --- golden fixture generation -----------------------------------------------------------
    //
    // `tests/replay.rs` reads `tests/fixtures/*.bin` with `include_bytes!`, which runs at
    // *compile* time: the files must already exist before that integration test binary can even
    // compile. This ignored unit test is the "small helper #[test]" that builds them and asserts
    // the round trip before saving the bytes; it lives here (not in `tests/replay.rs`) precisely
    // to avoid that chicken-and-egg compile order. Run it manually, after a deliberate format
    // change, with:
    //
    //   cargo test -p grimoire_sim --lib -- --ignored regenerate_fixtures
    //
    // The three builders below (`v1_fixture_replay`, `minimal_fixture_replay`,
    // `full_fixture_replay`) must stay byte-for-byte in sync with the expectations in
    // `tests/replay.rs`, which re-derive the same values from the public API and additionally
    // write every fixture out byte by byte from the layout table, without the encoder.
    //
    // The fixtures pin the byte format, not the version of the day: their `engine_version` and
    // `engine_build` are fixed values, never `ENGINE_VERSION` or `ENGINE_BUILD`, so neither a
    // release that raises the crate version nor a build with `GRIMOIRE_BUILD_HASH` changes them.

    use crate::input::{InputFrame, TickInput};

    /// `engine_version` of every v2 fixture, fixed on purpose (see above).
    const FIXTURE_ENGINE_VERSION: &str = "0.4.0";

    /// A v2 header with the fixed fixture identity: [`FIXTURE_ENGINE_VERSION`], an unknown build,
    /// `content_manifest`, no swaps and no metadata.
    fn fixture_header(content_manifest: ContentManifestHash) -> ReplayHeader {
        ReplayHeader {
            engine_version: FIXTURE_ENGINE_VERSION.to_string(),
            engine_build: BuildHash::UNKNOWN,
            content_manifest,
            swaps: Vec::new(),
            app_metadata: BTreeMap::new(),
        }
    }

    fn fixture_log(frame_count: usize) -> InputLog {
        let frames = (0..frame_count)
            .map(|tick| {
                let mut input = TickInput::default();
                input.slots[0] = InputFrame {
                    axes: [tick as i16, -(tick as i16), 1, -1],
                    buttons: tick as u32,
                };
                input
            })
            .collect();
        InputLog {
            seed: 0x1234_5678_9abc_def0,
            tick_rate_hz: 60,
            frames,
        }
    }

    fn v1_fixture_replay() -> Replay {
        Replay {
            header: None,
            log: fixture_log(4),
        }
    }

    fn minimal_fixture_replay() -> Replay {
        Replay {
            header: Some(fixture_header(ContentManifestHash::EMPTY)),
            log: fixture_log(3),
        }
    }

    fn full_fixture_replay() -> Replay {
        let mut header = fixture_header(ContentManifestHash(0x0011_2233_4455_6677));
        header.engine_build = BuildHash([0xab; 20]);
        header.swaps = vec![
            SwapRecord::new(2, ContentManifestHash(0xaa)),
            SwapRecord::new(4, ContentManifestHash(0xbb)),
        ];
        header
            .app_metadata
            .insert("app.name".to_string(), "grimoire-harness".to_string());
        header
            .app_metadata
            .insert("app.version".to_string(), "0.1.1".to_string());
        Replay {
            header: Some(header),
            log: fixture_log(5),
        }
    }

    #[test]
    #[ignore = "regenerates the checked-in fixtures under tests/fixtures/; run manually after a deliberate format change, never for a release"]
    fn regenerate_fixtures() {
        write_fixture("replay_v1.bin", &v1_fixture_replay());
        write_fixture("replay_v2_minimal.bin", &minimal_fixture_replay());
        write_fixture("replay_v2_full.bin", &full_fixture_replay());
    }

    fn write_fixture(name: &str, replay: &Replay) {
        let bytes = replay
            .to_bytes()
            .expect("fixture replay satisfies every constraint");
        assert_eq!(
            &Replay::from_bytes(&bytes).expect("just-encoded fixture bytes decode"),
            replay,
            "fixture {name} does not round-trip"
        );
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        std::fs::create_dir_all(path.parent().expect("fixtures dir has a parent"))
            .expect("create tests/fixtures");
        std::fs::write(&path, &bytes).expect("write fixture");
    }
}
