//! Conformance suite for the [`crate::AssetSource`] trait contract (contract §2 rule 12, §12),
//! behind the non-default feature `conformance`.
//!
//! Every implementation of [`crate::AssetSource`], including third-party ones, calls
//! [`asset_source`] from its own tests. The suite checks only properties every implementation —
//! including the null implementation [`crate::EmptyAssetSource`] — must satisfy;
//! implementation-specific behaviour (e.g. pack-format corruption handling) belongs in that
//! implementation's own tests. `tests/source_conformance.rs` runs it against every
//! `AssetSource` this crate ships.

use crate::error::AssetError;
use crate::ids::AssetId;
use crate::source::AssetSource;

/// Checks the [`AssetSource`] contract against `source`: [`AssetSource::entries`] is sorted
/// ascending by [`AssetId`] with no duplicate, [`AssetSource::read`] succeeds for exactly the
/// contained ids and fails with [`AssetError::NotFound`] for one that is not contained, repeated
/// reads of the same id return identical bytes, and [`AssetSource::content_hash`] is stable
/// across calls.
///
/// Whether two *different* sources with equal content agree on `content_hash` is a
/// cross-implementation property this single-source suite cannot check on its own; callers assert
/// it directly (see `tests/source_conformance.rs`).
pub fn asset_source(source: &dyn AssetSource) {
    let entries = source.entries();

    for pair in entries.windows(2) {
        assert!(
            pair[0].id < pair[1].id,
            "AssetSource::entries() must be sorted ascending by AssetId with no duplicate id, \
             found {:?} before {:?}",
            pair[0].id,
            pair[1].id
        );
    }

    for entry in entries {
        let first = source.read(entry.id).unwrap_or_else(|error| {
            panic!("read({}) failed for a contained id: {error}", entry.id)
        });
        assert_eq!(
            first.len() as u64,
            entry.len,
            "read({}) returned {} bytes, but its AssetEntry declares len {}",
            entry.id,
            first.len(),
            entry.len
        );
        let second = source
            .read(entry.id)
            .unwrap_or_else(|error| panic!("second read({}) failed: {error}", entry.id));
        assert_eq!(
            first.as_ref(),
            second.as_ref(),
            "repeated read({}) must return identical bytes",
            entry.id
        );
    }

    // An id that is not in `entries`: search a small set of candidates rather than assuming any
    // single sentinel value is absent, since `source` is arbitrary.
    let absent = (0u64..)
        .map(|salt| AssetId(0xDEAD_BEEF_0000_0000 ^ salt))
        .find(|id| !entries.iter().any(|entry| entry.id == *id))
        .expect("entries() is finite, so some candidate id is always absent");
    match source.read(absent) {
        Err(AssetError::NotFound(id)) => {
            assert_eq!(id, absent, "NotFound must name the requested id")
        }
        Ok(_) => panic!("read({absent}) unexpectedly succeeded for an id that is not in entries()"),
        Err(other) => panic!("read of an absent id must fail with NotFound, got {other:?}"),
    }

    assert_eq!(
        source.content_hash(),
        source.content_hash(),
        "content_hash() must be deterministic across calls on the same source"
    );
}

#[cfg(test)]
mod tests {
    use super::asset_source;
    use crate::ids::{AssetKind, AssetPath};
    use crate::source::{EmptyAssetSource, MemorySource};

    #[test]
    fn empty_source_satisfies_the_suite() {
        asset_source(&EmptyAssetSource);
    }

    #[test]
    fn populated_memory_source_satisfies_the_suite() {
        let mut source = MemorySource::new("conformance-self-test");
        source.insert(
            &AssetPath::new("a").unwrap(),
            AssetKind::SIGIL,
            1,
            vec![1, 2, 3],
        );
        source.insert(
            &AssetPath::new("b/c").unwrap(),
            AssetKind(0x8000),
            2,
            vec![],
        );
        asset_source(&source);
    }
}
