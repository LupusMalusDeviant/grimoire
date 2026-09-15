//! `grimoire_assets` pack v1 and `AssetSource` (contract §12, §2a). `sha2` is omitted: the
//! provided `content_hash` body is a stub.

use std::any::{Any, TypeId};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt::{self, Display};
use std::hash::{Hash, Hasher};
use std::io;
use std::marker::PhantomData;
use std::path::Path;
use std::sync::Arc;

use grimoire_core::{StableHash, StableHasher};
use grimoire_platform_p1::FileSystem;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct AssetId(pub u64);

impl Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}
impl StableHash for AssetId {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_u64(self.0);
    }
}
impl AssetId {
    pub fn from_path(path: &AssetPath) -> AssetId {
        let mut hasher = StableHasher::new();
        hasher.write_str("grimoire.asset-id.v1");
        hasher.write_str(path.as_str());
        AssetId(hasher.finish())
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct AssetPath(String);

impl AssetPath {
    pub fn new(path: &str) -> Result<AssetPath, AssetError> {
        Ok(AssetPath(path.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct AssetKind(pub u16);
impl AssetKind {
    pub const SIGIL: Self = Self(1);
    pub const MESH: Self = Self(2);
    pub const MATERIAL: Self = Self(3);
    pub const AUDIO: Self = Self(4);
    pub const TEMPLATE: Self = Self(5);
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Sha256(pub [u8; 32]);
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ContentHash(pub [u8; 32]);

#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AssetEntry {
    pub id: AssetId,
    pub kind: AssetKind,
    pub kind_version: u32,
    pub len: u64,
    pub sha256: Sha256,
}

impl AssetEntry {
    /// For `AssetSource` implementations outside this crate (§2 rule 13, §12).
    pub fn new(id: AssetId, kind: AssetKind, kind_version: u32, len: u64, sha256: Sha256) -> Self {
        Self {
            id,
            kind,
            kind_version,
            len,
            sha256,
        }
    }
}

pub trait AssetSource: Send + Sync {
    fn name(&self) -> &str;
    fn entries(&self) -> &[AssetEntry];
    fn read(&self, id: AssetId) -> Result<Cow<'_, [u8]>, AssetError>;
    fn content_hash(&self) -> ContentHash {
        let _ = self.entries();
        unimplemented!()
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct EmptyAssetSource;

impl AssetSource for EmptyAssetSource {
    fn name(&self) -> &str {
        "empty"
    }
    fn entries(&self) -> &[AssetEntry] {
        &[]
    }
    fn read(&self, id: AssetId) -> Result<Cow<'_, [u8]>, AssetError> {
        Err(AssetError::NotFound(id))
    }
}

#[derive(Clone, Debug)]
pub struct MemorySource {
    name: String,
    entries: Vec<AssetEntry>,
    data: BTreeMap<AssetId, Vec<u8>>,
}

impl MemorySource {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            entries: Vec::new(),
            data: BTreeMap::new(),
        }
    }
    pub fn insert(&mut self, path: &AssetPath, kind: AssetKind, kind_version: u32, bytes: Vec<u8>) -> AssetId {
        let id = AssetId::from_path(path);
        self.entries.retain(|entry| entry.id != id);
        self.entries.push(AssetEntry {
            id,
            kind,
            kind_version,
            len: bytes.len() as u64,
            sha256: Sha256([0; 32]),
        });
        self.entries.sort_by_key(|entry| entry.id);
        self.data.insert(id, bytes);
        id
    }
    pub fn remove(&mut self, id: AssetId) -> bool {
        self.entries.retain(|entry| entry.id != id);
        self.data.remove(&id).is_some()
    }
}

impl AssetSource for MemorySource {
    fn name(&self) -> &str {
        &self.name
    }
    fn entries(&self) -> &[AssetEntry] {
        &self.entries
    }
    fn read(&self, id: AssetId) -> Result<Cow<'_, [u8]>, AssetError> {
        self.data
            .get(&id)
            .map(|bytes| Cow::Borrowed(bytes.as_slice()))
            .ok_or(AssetError::NotFound(id))
    }
}

#[derive(Clone, Debug)]
pub struct PackReader {
    bytes: Arc<[u8]>,
    entries: Vec<AssetEntry>,
    manifest: PackManifest,
}

impl PackReader {
    pub fn from_bytes(bytes: Arc<[u8]>) -> Result<Self, PackError> {
        let _ = bytes;
        unimplemented!()
    }
    pub fn open(fs: &dyn FileSystem, path: &Path) -> Result<Self, AssetError> {
        let bytes = fs
            .read_limited(path, MAX_PACK_LEN)
            .map_err(|error| match error.kind() {
                io::ErrorKind::FileTooLarge => AssetError::TooLarge {
                    path: path.display().to_string(),
                    max: MAX_PACK_LEN,
                },
                kind => AssetError::Io {
                    path: path.display().to_string(),
                    kind,
                },
            })?;
        Ok(Self::from_bytes(bytes.into())?)
    }
    pub fn manifest(&self) -> &PackManifest {
        &self.manifest
    }
}

impl AssetSource for PackReader {
    fn name(&self) -> &str {
        "pack"
    }
    fn entries(&self) -> &[AssetEntry] {
        &self.entries
    }
    fn read(&self, id: AssetId) -> Result<Cow<'_, [u8]>, AssetError> {
        let _ = &self.bytes;
        Err(AssetError::NotFound(id))
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PackManifest {
    compiler: String,
    compiler_version: String,
    paths: Vec<AssetPath>,
    application: Vec<u8>,
}

impl PackManifest {
    pub fn compiler(&self) -> &str {
        &self.compiler
    }
    pub fn compiler_version(&self) -> &str {
        &self.compiler_version
    }
    pub fn path_of(&self, id: AssetId) -> Option<&AssetPath> {
        self.paths.iter().find(|path| AssetId::from_path(path) == id)
    }
    pub fn application(&self) -> &[u8] {
        &self.application
    }
}

pub struct PackWriter {
    compiler: String,
    compiler_version: String,
}

impl PackWriter {
    pub fn new(compiler: &str, compiler_version: &str) -> Self {
        Self {
            compiler: compiler.to_owned(),
            compiler_version: compiler_version.to_owned(),
        }
    }
    pub fn add(&mut self, path: &AssetPath, kind: AssetKind, kind_version: u32, bytes: &[u8]) -> Result<AssetId, PackError> {
        let _ = (&self.compiler, &self.compiler_version, kind, kind_version, bytes);
        Ok(AssetId::from_path(path))
    }
    pub fn application(&mut self, bytes: Vec<u8>) {
        let _ = bytes;
    }
    pub fn finish(self) -> Result<Vec<u8>, PackError> {
        unimplemented!()
    }
}

pub struct Handle<T> {
    id: AssetId,
    marker: PhantomData<fn() -> T>,
}

impl<T> Handle<T> {
    pub fn id(&self) -> AssetId {
        self.id
    }
}
impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Handle<T> {}
impl<T> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl<T> Eq for Handle<T> {}
impl<T> PartialOrd for Handle<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl<T> Ord for Handle<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.id.cmp(&other.id)
    }
}
impl<T> Hash for Handle<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}
impl<T> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Handle").field(&self.id).finish()
    }
}

pub struct AssetStore {
    source: Box<dyn AssetSource>,
    decoded: BTreeMap<(AssetId, TypeId), Box<dyn Any + Send + Sync>>,
}

impl fmt::Debug for AssetStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssetStore")
            .field("source", &self.source.name())
            .field("decoded", &self.decoded.len())
            .finish()
    }
}

impl AssetStore {
    pub fn new(source: Box<dyn AssetSource>) -> Self {
        Self {
            source,
            decoded: BTreeMap::new(),
        }
    }
    pub fn source(&self) -> &dyn AssetSource {
        self.source.as_ref()
    }
    pub fn load<T: Send + Sync + 'static, E: Display>(
        &mut self,
        id: AssetId,
        expected: AssetKind,
        decode: impl FnOnce(&[u8]) -> Result<T, E>,
    ) -> Result<Handle<T>, AssetError> {
        let bytes = self.source.read(id)?;
        let value = decode(&bytes).map_err(|error| AssetError::Decode {
            id,
            message: error.to_string(),
        })?;
        let _ = expected;
        self.decoded.insert((id, TypeId::of::<T>()), Box::new(value));
        Ok(Handle {
            id,
            marker: PhantomData,
        })
    }
    pub fn get<T: 'static>(&self, handle: Handle<T>) -> Option<&T> {
        self.decoded
            .get(&(handle.id, TypeId::of::<T>()))?
            .downcast_ref::<T>()
    }
}

pub const PACK_MAGIC: [u8; 8] = *b"GRIMPACK";
pub const PACK_FORMAT_VERSION: u32 = 1;
pub const PACK_ALIGN: u64 = 16;
pub const MAX_ENTRIES: u32 = 65_536;
pub const MAX_PATH_LEN: usize = 255;
pub const MAX_ENTRY_LEN: u64 = 256 * 1024 * 1024;
pub const MAX_MANIFEST_LEN: u64 = 16 * 1024 * 1024;
pub const MAX_PACK_LEN: u64 = 1024 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AssetError {
    #[error("asset {0} not found")]
    NotFound(AssetId),
    #[error("invalid path {path}: {reason}")]
    InvalidPath { path: String, reason: &'static str },
    #[error("kind mismatch for {id}: expected {expected:?}, found {found:?}")]
    KindMismatch {
        id: AssetId,
        expected: AssetKind,
        found: AssetKind,
    },
    #[error("hash mismatch for {0}")]
    HashMismatch(AssetId),
    #[error("decode {id}: {message}")]
    Decode { id: AssetId, message: String },
    #[error("io {path}: {kind}")]
    Io { path: String, kind: io::ErrorKind },
    #[error("{path} larger than {max}")]
    TooLarge { path: String, max: u64 },
    #[error(transparent)]
    Pack(#[from] PackError),
}

#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PackError {
    #[error("unexpected end")]
    UnexpectedEnd { offset: u64, needed: u64, available: u64 },
    #[error("bad magic")]
    BadMagic,
    #[error("unsupported version {0}")]
    UnsupportedVersion(u32),
    #[error("header length {0}")]
    HeaderLength(u32),
    #[error("file length")]
    FileLength { declared: u64, actual: u64 },
    #[error("non-zero reserved")]
    NonZeroReserved { offset: u64 },
    #[error("too many entries {0}")]
    TooManyEntries(u32),
    #[error("out of bounds")]
    OutOfBounds { what: &'static str, offset: u64, len: u64 },
    #[error("misaligned")]
    Misaligned { index: u32 },
    #[error("overlap")]
    Overlap { index: u32 },
    #[error("unsorted ids")]
    UnsortedIds { index: u32 },
    #[error("invalid kind")]
    InvalidKind { index: u32, kind: u16 },
    #[error("reserved kind")]
    ReservedKind { index: u32, kind: u16 },
    #[error("entry too large")]
    EntryTooLarge { index: u32, len: u64 },
    #[error("manifest: {0}")]
    Manifest(String),
    #[error("manifest mismatch")]
    ManifestMismatch { index: u32 },
}

#[cfg(feature = "conformance")]
pub mod conformance {
    use super::AssetSource;

    pub fn asset_source(source: &dyn AssetSource) {
        let _ = source;
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_safety_and_bounds() {
        let _: Option<&dyn AssetSource> = None;
        let _: Option<&dyn FileSystem> = None;
        fn send_sync<T: Send + Sync>() {}
        send_sync::<PackReader>();
        send_sync::<MemorySource>();
    }
}
