//! P1 additions to `grimoire_sim` (contract §8.1–§8.4).

use std::collections::BTreeMap;

use grimoire_core::{StableHash, StableHasher};
use grimoire_ecs::Resource;
use grimoire_ecs_p1::SystemObserver;
use grimoire_sim::{InputLog, SimSnapshot, Simulation, TickInput};

// ---- §8.1 -------------------------------------------------------------------------------------

pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const ENGINE_BUILD: BuildHash = match option_env!("GRIMOIRE_BUILD_HASH") {
    Some(hex) => BuildHash::parse_const(hex),
    None => BuildHash::UNKNOWN,
};

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct BuildHash(pub [u8; 20]);

const fn nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => panic!("GRIMOIRE_BUILD_HASH must be 40 lowercase hex digits"),
    }
}

impl BuildHash {
    pub const UNKNOWN: Self = Self([0; 20]);

    const fn parse_const(hex: &str) -> Self {
        let bytes = hex.as_bytes();
        assert!(bytes.len() == 40, "GRIMOIRE_BUILD_HASH must be 40 lowercase hex digits");
        let mut out = [0u8; 20];
        let mut i = 0;
        while i < 20 {
            out[i] = (nibble(bytes[2 * i]) << 4) | nibble(bytes[2 * i + 1]);
            i += 1;
        }
        Self(out)
    }

    pub fn is_known(self) -> bool {
        self != Self::UNKNOWN
    }

    pub fn to_hex(self) -> String {
        unimplemented!()
    }

    pub fn from_hex(text: &str) -> Result<Self, SimError> {
        let _ = text;
        unimplemented!()
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ContentManifestHash(pub u64);

impl ContentManifestHash {
    pub const EMPTY: Self = Self(0);

    pub fn to_hex(self) -> String {
        format!("{:016x}", self.0)
    }

    pub fn from_hex(text: &str) -> Result<Self, SimError> {
        let _ = text;
        unimplemented!()
    }
}

impl StableHash for ContentManifestHash {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_u64(self.0);
    }
}

#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SwapRecord {
    pub tick: u64,
    pub content_manifest: ContentManifestHash,
}

impl SwapRecord {
    pub fn new(tick: u64, content_manifest: ContentManifestHash) -> Self {
        Self {
            tick,
            content_manifest,
        }
    }
}

#[non_exhaustive]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ReplayHeader {
    pub engine_version: String,
    pub engine_build: BuildHash,
    pub content_manifest: ContentManifestHash,
    pub swaps: Vec<SwapRecord>,
    pub app_metadata: BTreeMap<String, String>,
}

impl ReplayHeader {
    pub fn for_this_build(content_manifest: ContentManifestHash) -> Self {
        Self {
            engine_version: ENGINE_VERSION.to_owned(),
            engine_build: ENGINE_BUILD,
            content_manifest,
            swaps: Vec::new(),
            app_metadata: BTreeMap::new(),
        }
    }

    pub fn is_golden_eligible(&self) -> bool {
        self.swaps.is_empty()
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Replay {
    pub header: Option<ReplayHeader>,
    pub log: InputLog,
}

impl Replay {
    pub const FORMAT_VERSION: u32 = 2;

    pub fn from_bytes(bytes: &[u8]) -> Result<Replay, SimError> {
        let _ = bytes;
        unimplemented!()
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, SimError> {
        unimplemented!()
    }
}

pub const MAX_ENGINE_VERSION_BYTES: usize = 64;
pub const MAX_SWAP_RECORDS: usize = 4096;
pub const MAX_APP_METADATA: usize = 32;
pub const MAX_APP_KEY_BYTES: usize = 64;
pub const MAX_APP_VALUE_BYTES: usize = 1024;

/// Stand-in for the real `SimError` with the P1 variants appended (the real enum cannot be
/// extended from outside). P0 variants copied from `grimoire_sim/src/error.rs` shape.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SimError {
    #[error("unexpected end")]
    UnexpectedEnd {
        offset: usize,
        needed: usize,
        available: usize,
    },
    #[error("bad magic")]
    BadMagic,
    #[error("unsupported version {0}")]
    UnsupportedVersion(u32),
    #[error("invalid tick rate")]
    InvalidTickRate,
    #[error("frame data length")]
    FrameDataLength { frames: u64, remaining: usize },
    #[error("header length")]
    HeaderLength { declared: u32, consumed: usize },
    #[error("field too long")]
    FieldTooLong {
        field: &'static str,
        len: usize,
        max: usize,
    },
    #[error("too many entries")]
    TooManyEntries {
        field: &'static str,
        count: u64,
        max: usize,
    },
    #[error("invalid text")]
    InvalidText { field: &'static str },
    #[error("invalid hex")]
    InvalidHex { field: &'static str },
    #[error("metadata key order")]
    MetadataKeyOrder { index: usize },
    #[error("swap order")]
    SwapOrder { index: usize },
    #[error("swap out of range")]
    SwapOutOfRange { tick: u64, frames: u64 },
}

// ---- §8.2 / §8.4 ------------------------------------------------------------------------------

/// Stand-in for the inherent method `SimSnapshot::resource`.
pub trait SimSnapshotResource {
    fn resource<R: Resource>(&self) -> Option<&R>;
}

impl SimSnapshotResource for SimSnapshot {
    fn resource<R: Resource>(&self) -> Option<&R> {
        unimplemented!()
    }
}

/// Stand-in for the inherent methods `Simulation::restore_checked` and `step_observed`.
pub trait SimulationP1 {
    fn restore_checked<E>(
        &mut self,
        snapshot: &SimSnapshot,
        check: impl FnOnce(&SimSnapshot) -> Result<(), E>,
    ) -> Result<(), E>;
    fn step_observed(&mut self, input: TickInput, observer: &mut dyn SystemObserver);
}

impl SimulationP1 for Simulation {
    fn restore_checked<E>(
        &mut self,
        snapshot: &SimSnapshot,
        check: impl FnOnce(&SimSnapshot) -> Result<(), E>,
    ) -> Result<(), E> {
        check(snapshot)?;
        self.restore(snapshot);
        Ok(())
    }

    fn step_observed(&mut self, input: TickInput, observer: &mut dyn SystemObserver) {
        let _ = (input, observer);
        unimplemented!()
    }
}

// ---- §8.3 -------------------------------------------------------------------------------------

pub mod stream {
    pub const ENGINE_BIT: u64 = 1 << 63;
    pub const OWNER_SHIFT: u32 = 48;
    pub const LOCAL_MASK: u64 = (1 << 48) - 1;

    pub const fn engine_stream(owner: u16, local: u64) -> u64 {
        ENGINE_BIT | app_stream(owner, local)
    }

    pub const fn app_stream(owner: u16, local: u64) -> u64 {
        assert!(owner != 0 && owner <= 0x7fff, "stream owner must be in 1..=0x7fff");
        assert!(local <= LOCAL_MASK, "local stream number exceeds 48 bits");
        ((owner as u64) << OWNER_SHIFT) | local
    }

    pub const fn is_engine_stream(stream: u64) -> bool {
        stream & ENGINE_BIT != 0
    }

    pub const fn owner_of(stream: u64) -> u16 {
        ((stream >> OWNER_SHIFT) & 0x7fff) as u16
    }

    pub mod owner {
        pub const SIM: u16 = 0x0001;
        pub const FACADE: u16 = 0x0002;
        pub const SIGIL: u16 = 0x0010;
        pub const COLLIDE: u16 = 0x0011;
    }
}

#[cfg(feature = "claim-const-panic")]
pub const BAD_STREAM: u64 = stream::engine_stream(0, 1);
