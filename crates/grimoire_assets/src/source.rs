//! The [`AssetSource`] trait and its null and in-memory implementations (contract §12, §2a).

use std::borrow::Cow;
use std::collections::BTreeMap;

use sha2::{Digest, Sha256 as Sha256Hasher};

use crate::error::AssetError;
use crate::ids::{AssetEntry, AssetId, AssetKind, AssetPath, ContentHash, sha256_of};

/// A read-only, content-addressed collection of assets.
///
/// Implementations: [`PackReader`](crate::PackReader) (an offline-compiled pack file),
/// [`MemorySource`] (assets held in memory, e.g. for tests or tools) and [`EmptyAssetSource`]
/// (the null implementation). Foreign implementations (game, tools, test doubles) build their
/// entries with [`AssetEntry::new`] and check themselves against `crate::conformance` (behind the
/// `conformance` feature; not an intra-doc link here so `cargo doc` without that feature does not
/// report a broken link) (contract §2 rule 12).
pub trait AssetSource: Send + Sync {
    /// A short name for diagnostics (e.g. a file path or `"memory"`).
    fn name(&self) -> &str;

    /// The source's directory: ascending by [`AssetId`], with no duplicate id.
    fn entries(&self) -> &[AssetEntry];

    /// Reads the payload of `id`.
    ///
    /// # Errors
    /// Returns [`AssetError::NotFound`] if no entry with `id` exists, or
    /// [`AssetError::HashMismatch`] if the payload's SHA-256 digest does not match its entry (this
    /// method verifies it on every call, before returning).
    fn read(&self, id: AssetId) -> Result<Cow<'_, [u8]>, AssetError>;

    /// A digest that identifies the source's content: two sources with the same entries (id,
    /// kind, kind version, length and SHA-256, in the same order) always produce the same hash,
    /// regardless of implementation, path names, compiler metadata or read order.
    ///
    /// Computed as SHA-256 of `b"grimoire.content.v1\0"`, the entry count as a little-endian
    /// `u32`, then for each entry in [`AssetSource::entries`] order: `id` (`u64` LE), `kind`
    /// (`u16` LE), `kind_version` (`u32` LE), `len` (`u64` LE) and the 32 raw SHA-256 bytes
    /// (contract §12). Paths, the compiler name/version and the application block never enter
    /// this hash, so it is not the simulation's content-manifest hash (that one is built by
    /// `grimoire_sigil`, contract §11.8).
    fn content_hash(&self) -> ContentHash {
        let mut hasher = Sha256Hasher::new();
        hasher.update(b"grimoire.content.v1\0");
        let entries = self.entries();
        // `entries.len()` never realistically approaches `u32::MAX`; saturate rather than panic
        // on a pathological foreign implementation instead of trusting an infallible cast.
        let count = u32::try_from(entries.len()).unwrap_or(u32::MAX);
        hasher.update(count.to_le_bytes());
        for entry in entries {
            hasher.update(entry.id.0.to_le_bytes());
            hasher.update(entry.kind.0.to_le_bytes());
            hasher.update(entry.kind_version.to_le_bytes());
            hasher.update(entry.len.to_le_bytes());
            hasher.update(entry.sha256.0);
        }
        let digest = hasher.finalize();
        let mut bytes = [0u8; 32];
        // `Sha256Hasher::finalize` always returns exactly 32 bytes for the SHA-256 algorithm.
        bytes.copy_from_slice(&digest);
        ContentHash(bytes)
    }
}

/// The null [`AssetSource`]: no entries, every read fails with [`AssetError::NotFound`].
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

/// An [`AssetSource`] backed by in-memory byte buffers, for tests and tools.
#[derive(Clone, Debug)]
pub struct MemorySource {
    name: String,
    entries: Vec<AssetEntry>,
    data: BTreeMap<AssetId, Vec<u8>>,
}

impl MemorySource {
    /// Creates an empty source named `name` (used only for [`AssetSource::name`]).
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            entries: Vec::new(),
            data: BTreeMap::new(),
        }
    }

    /// Inserts or replaces the asset at `path`, computing its id and SHA-256 digest.
    ///
    /// If an entry with the same id already exists (same path, or a hash collision on a
    /// different path), it is replaced.
    pub fn insert(
        &mut self,
        path: &AssetPath,
        kind: AssetKind,
        kind_version: u32,
        bytes: Vec<u8>,
    ) -> AssetId {
        let id = AssetId::from_path(path);
        let sha256 = sha256_of(&bytes);
        let entry = AssetEntry::new(id, kind, kind_version, bytes.len() as u64, sha256);
        match self
            .entries
            .binary_search_by_key(&id, |existing| existing.id)
        {
            Ok(index) => self.entries[index] = entry,
            Err(index) => self.entries.insert(index, entry),
        }
        self.data.insert(id, bytes);
        id
    }

    /// Removes the asset with id `id`. Returns whether it was present.
    pub fn remove(&mut self, id: AssetId) -> bool {
        if let Ok(index) = self.entries.binary_search_by_key(&id, |entry| entry.id) {
            self.entries.remove(index);
        }
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
        let bytes = self.data.get(&id).ok_or(AssetError::NotFound(id))?;
        let index = self
            .entries
            .binary_search_by_key(&id, |entry| entry.id)
            .expect("MemorySource: `entries` and `data` always share the same key set");
        let expected = self.entries[index].sha256;
        if sha256_of(bytes) != expected {
            return Err(AssetError::HashMismatch(id));
        }
        Ok(Cow::Borrowed(bytes.as_slice()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(s: &str) -> AssetPath {
        AssetPath::new(s).unwrap()
    }

    #[test]
    fn empty_source_has_no_entries_and_never_finds_anything() {
        let source = EmptyAssetSource;
        assert!(source.entries().is_empty());
        assert!(matches!(
            source.read(AssetId(0)),
            Err(AssetError::NotFound(AssetId(0)))
        ));
    }

    #[test]
    fn memory_source_round_trips_and_stays_sorted() {
        let mut source = MemorySource::new("test");
        let id_b = source.insert(&path("b"), AssetKind::SIGIL, 1, vec![4, 5, 6]);
        let id_a = source.insert(&path("a"), AssetKind::SIGIL, 1, vec![1, 2, 3]);
        assert!(
            source
                .entries()
                .windows(2)
                .all(|pair| pair[0].id < pair[1].id)
        );
        assert_eq!(source.read(id_a).unwrap().as_ref(), [1, 2, 3]);
        assert_eq!(source.read(id_b).unwrap().as_ref(), [4, 5, 6]);
    }

    #[test]
    fn memory_source_insert_replaces_same_id() {
        let mut source = MemorySource::new("test");
        let id = source.insert(&path("a"), AssetKind::SIGIL, 1, vec![1]);
        let replaced = source.insert(&path("a"), AssetKind::SIGIL, 2, vec![9, 9]);
        assert_eq!(id, replaced);
        assert_eq!(source.entries().len(), 1);
        assert_eq!(source.read(id).unwrap().as_ref(), [9, 9]);
        assert_eq!(source.entries()[0].kind_version, 2);
    }

    #[test]
    fn memory_source_remove() {
        let mut source = MemorySource::new("test");
        let id = source.insert(&path("a"), AssetKind::SIGIL, 1, vec![1]);
        assert!(source.remove(id));
        assert!(!source.remove(id));
        assert!(source.entries().is_empty());
        assert!(matches!(source.read(id), Err(AssetError::NotFound(_))));
    }

    #[test]
    fn content_hash_ignores_name_and_depends_on_entries() {
        let mut a = MemorySource::new("a");
        a.insert(&path("x"), AssetKind::SIGIL, 1, vec![1, 2, 3]);
        let mut b = MemorySource::new("b");
        b.insert(&path("x"), AssetKind::SIGIL, 1, vec![1, 2, 3]);
        assert_eq!(a.content_hash(), b.content_hash());

        let mut c = MemorySource::new("a");
        c.insert(&path("x"), AssetKind::SIGIL, 1, vec![1, 2, 4]);
        assert_ne!(a.content_hash(), c.content_hash());
    }
}
