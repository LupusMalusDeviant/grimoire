//! [`AssetStore`]: decodes and caches assets read from an [`AssetSource`] (contract §12).

use std::any::{Any, TypeId};
use std::collections::BTreeMap;
use std::fmt::{self, Display};
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::AssetError;
use crate::ids::{AssetId, AssetKind};
use crate::source::AssetSource;

/// Source of [`AssetStore`] identities, so a [`Handle`] remembers which store issued it. Starts at
/// 1; the counter is process-wide and never influences simulation state (the store is not
/// simulation state, contract §12).
static NEXT_STORE_ID: AtomicU64 = AtomicU64::new(1);

/// A typed reference to a value decoded by [`AssetStore::load`].
///
/// A handle remembers the store that issued it: [`AssetStore::get`] returns `None` for a handle of
/// another store, even if that store has decoded the same id (contract §12). Equality, ordering
/// and hashing use the asset id and the issuing store, never `T`; handles of one store order by
/// [`AssetId`].
pub struct Handle<T> {
    id: AssetId,
    store: u64,
    marker: PhantomData<fn() -> T>,
}

impl<T> Handle<T> {
    /// The id of the underlying asset.
    #[must_use]
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
        self.id == other.id && self.store == other.store
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
        (self.id, self.store).cmp(&(other.id, other.store))
    }
}

impl<T> Hash for Handle<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.store.hash(state);
    }
}

impl<T> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Handle").field(&self.id).finish()
    }
}

/// Decodes and caches assets read from one [`AssetSource`].
///
/// Not simulation state: it is neither `Clone` nor `StableHash` and lives outside the ECS world
/// (contract §12).
pub struct AssetStore {
    /// Identity stamped into every [`Handle`] this store issues.
    id: u64,
    source: Box<dyn AssetSource>,
    decoded: BTreeMap<(AssetId, TypeId), Box<dyn Any + Send + Sync>>,
}

impl fmt::Debug for AssetStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssetStore")
            .field("source", &self.source.name())
            .field("decoded_count", &self.decoded.len())
            .finish()
    }
}

impl AssetStore {
    /// Creates a store backed by `source`.
    #[must_use]
    pub fn new(source: Box<dyn AssetSource>) -> Self {
        Self {
            id: NEXT_STORE_ID.fetch_add(1, Ordering::Relaxed),
            source,
            decoded: BTreeMap::new(),
        }
    }

    /// The backing source.
    #[must_use]
    pub fn source(&self) -> &dyn AssetSource {
        self.source.as_ref()
    }

    /// Reads, checks and decodes the asset `id`, caching the result.
    ///
    /// Decodes at most once per `(id, T)`: a later call with the same id and the same `T` returns
    /// the cached handle without reading or decoding again. The kind is checked before `decode`
    /// runs, so a mismatch never invokes it.
    ///
    /// # Errors
    /// Returns [`AssetError::NotFound`] if `id` has no entry, [`AssetError::KindMismatch`] if its
    /// kind is not `expected`, an error from [`AssetSource::read`] (e.g.
    /// [`AssetError::HashMismatch`]) if reading fails, or [`AssetError::Decode`] if `decode`
    /// fails.
    pub fn load<T, E>(
        &mut self,
        id: AssetId,
        expected: AssetKind,
        decode: impl FnOnce(&[u8]) -> Result<T, E>,
    ) -> Result<Handle<T>, AssetError>
    where
        T: Send + Sync + 'static,
        E: Display,
    {
        let key = (id, TypeId::of::<T>());
        if self.decoded.contains_key(&key) {
            return Ok(Handle {
                id,
                store: self.id,
                marker: PhantomData,
            });
        }

        let entries = self.source.entries();
        let index = entries
            .binary_search_by_key(&id, |entry| entry.id)
            .map_err(|_| AssetError::NotFound(id))?;
        let found = entries[index].kind;
        if found != expected {
            return Err(AssetError::KindMismatch {
                id,
                expected,
                found,
            });
        }

        let bytes = self.source.read(id)?;
        let value = decode(&bytes).map_err(|error| AssetError::Decode {
            id,
            message: error.to_string(),
        })?;
        self.decoded.insert(key, Box::new(value));
        Ok(Handle {
            id,
            store: self.id,
            marker: PhantomData,
        })
    }

    /// Returns the decoded value for `handle`, or `None` if it came from another store or was
    /// requested with a different `T` (never panics).
    #[must_use]
    pub fn get<T: 'static>(&self, handle: Handle<T>) -> Option<&T> {
        if handle.store != self.id {
            return None;
        }
        self.decoded
            .get(&(handle.id, TypeId::of::<T>()))?
            .downcast_ref::<T>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::AssetPath;
    use crate::source::MemorySource;

    fn store_with_one_sigil() -> (AssetStore, AssetId) {
        let mut source = MemorySource::new("test");
        let id = source.insert(
            &AssetPath::new("a").unwrap(),
            AssetKind::SIGIL,
            1,
            vec![1, 2, 3],
        );
        (AssetStore::new(Box::new(source)), id)
    }

    #[test]
    fn load_and_get_round_trip() {
        let (mut store, id) = store_with_one_sigil();
        let handle = store
            .load::<Vec<u8>, String>(id, AssetKind::SIGIL, |bytes| Ok(bytes.to_vec()))
            .unwrap();
        assert_eq!(store.get(handle), Some(&vec![1u8, 2, 3]));
    }

    #[test]
    fn load_rejects_kind_mismatch_without_decoding() {
        let (mut store, id) = store_with_one_sigil();
        let mut decode_calls = 0;
        let result = store.load::<Vec<u8>, String>(id, AssetKind::MESH, |bytes| {
            decode_calls += 1;
            Ok(bytes.to_vec())
        });
        assert!(matches!(
            result,
            Err(AssetError::KindMismatch {
                expected: AssetKind::MESH,
                found: AssetKind::SIGIL,
                ..
            })
        ));
        assert_eq!(decode_calls, 0);
    }

    #[test]
    fn load_decodes_at_most_once() {
        let (mut store, id) = store_with_one_sigil();
        let mut decode_calls = 0;
        store
            .load::<Vec<u8>, String>(id, AssetKind::SIGIL, |bytes| {
                decode_calls += 1;
                Ok(bytes.to_vec())
            })
            .unwrap();
        store
            .load::<Vec<u8>, String>(id, AssetKind::SIGIL, |bytes| {
                decode_calls += 1;
                Ok(bytes.to_vec())
            })
            .unwrap();
        assert_eq!(decode_calls, 1);
    }

    #[test]
    fn load_maps_decode_error() {
        let (mut store, id) = store_with_one_sigil();
        let result =
            store.load::<Vec<u8>, String>(id, AssetKind::SIGIL, |_bytes| Err("boom".to_owned()));
        assert!(matches!(result, Err(AssetError::Decode { message, .. }) if message == "boom"));
    }

    #[test]
    fn get_with_handle_from_another_type_or_store_is_none() {
        let (mut store, id) = store_with_one_sigil();
        let handle = store
            .load::<Vec<u8>, String>(id, AssetKind::SIGIL, |bytes| Ok(bytes.to_vec()))
            .unwrap();

        // Same id, different type: `get` must not find it.
        let wrong_type_handle: Handle<u32> = Handle {
            id: handle.id(),
            store: handle.store,
            marker: PhantomData,
        };
        assert_eq!(store.get(wrong_type_handle), None);

        // A handle for an id this store never loaded.
        let other_handle: Handle<Vec<u8>> = Handle {
            id: AssetId(handle.id().0 ^ 1),
            store: handle.store,
            marker: PhantomData,
        };
        assert_eq!(store.get(other_handle), None);
    }

    #[test]
    fn a_handle_of_another_store_is_none_even_for_the_same_decoded_id() {
        let (mut first, id) = store_with_one_sigil();
        let (mut second, second_id) = store_with_one_sigil();
        assert_eq!(id, second_id);
        let decode = |bytes: &[u8]| Ok::<_, String>(bytes.to_vec());
        let from_first = first
            .load::<Vec<u8>, String>(id, AssetKind::SIGIL, decode)
            .unwrap();
        let from_second = second
            .load::<Vec<u8>, String>(id, AssetKind::SIGIL, decode)
            .unwrap();

        assert_eq!(first.get(from_first), Some(&vec![1u8, 2, 3]));
        assert_eq!(second.get(from_second), Some(&vec![1u8, 2, 3]));
        assert_eq!(second.get(from_first), None);
        assert_eq!(first.get(from_second), None);
        assert_ne!(from_first, from_second);
        assert_eq!(from_first.id(), from_second.id());
    }

    #[test]
    fn handles_of_one_store_order_by_asset_id() {
        let mut source = MemorySource::new("test");
        let a = source.insert(&AssetPath::new("a").unwrap(), AssetKind::SIGIL, 1, vec![1]);
        let b = source.insert(&AssetPath::new("b").unwrap(), AssetKind::SIGIL, 1, vec![2]);
        let mut store = AssetStore::new(Box::new(source));
        let decode = |bytes: &[u8]| Ok::<_, String>(bytes.to_vec());
        let handle_a = store
            .load::<Vec<u8>, String>(a, AssetKind::SIGIL, decode)
            .unwrap();
        let handle_b = store
            .load::<Vec<u8>, String>(b, AssetKind::SIGIL, decode)
            .unwrap();
        assert_eq!(handle_a.cmp(&handle_b), a.cmp(&b));
    }
}
