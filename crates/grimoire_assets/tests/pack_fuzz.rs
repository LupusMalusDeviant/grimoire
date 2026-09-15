//! Property tests for `PackReader::from_bytes` (contract §2 rule 9, §12): completely random
//! bytes and single-byte/truncation mutations of a valid pack must never panic, only ever return
//! `Ok` or a [`PackError`].
//!
//! These are the same entry points the future fuzz targets (WP11.4) will use.

use std::sync::Arc;

use grimoire_assets::{AssetKind, AssetPath, PackReader, PackWriter};
use proptest::prelude::*;

/// A small, valid pack to mutate. Not the golden fixture on disk: this one is rebuilt in-process
/// so the property test has no file I/O dependency.
fn valid_pack_bytes() -> Vec<u8> {
    let mut writer = PackWriter::new("fuzz", "1");
    writer
        .add(
            &AssetPath::new("a/one").unwrap(),
            AssetKind::SIGIL,
            1,
            b"hello",
        )
        .unwrap();
    writer
        .add(
            &AssetPath::new("b/two").unwrap(),
            AssetKind(0x8000),
            2,
            b"world!!",
        )
        .unwrap();
    writer.application(b"fuzz-app-block".to_vec());
    writer.finish().unwrap()
}

proptest! {
    /// Arbitrary byte sequences of varying length, entirely unrelated to the pack format, must
    /// never panic `from_bytes` — only `Ok` or `Err` is acceptable.
    #[test]
    fn from_bytes_never_panics_on_arbitrary_bytes(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        let _ = PackReader::from_bytes(Arc::from(bytes));
    }

    /// Flipping any single byte of a valid pack must never panic `from_bytes`.
    #[test]
    fn from_bytes_never_panics_on_single_byte_mutation(index in 0usize..512, new_byte in any::<u8>()) {
        let mut bytes = valid_pack_bytes();
        if index < bytes.len() {
            bytes[index] = new_byte;
        }
        let _ = PackReader::from_bytes(Arc::from(bytes));
    }

    /// Truncating a valid pack to any shorter length must never panic `from_bytes`.
    #[test]
    fn from_bytes_never_panics_on_truncation(len in 0usize..512) {
        let bytes = valid_pack_bytes();
        let truncated = bytes[..len.min(bytes.len())].to_vec();
        let _ = PackReader::from_bytes(Arc::from(truncated));
    }
}
