//! # grimoire_assets
//!
//! Pack v1: an offline-compiled binary asset pack format, and `AssetSource`: the trait behind it
//! (PRD-0002 FR-02/FR-10, contract §12, ADR-0007).
//!
//! - [`AssetSource`] is the read-only, content-addressed trait every asset backend implements:
//!   [`PackReader`] (a pack file), [`MemorySource`] (assets held in memory) and
//!   [`EmptyAssetSource`] (the null implementation). A foreign implementation calls
//!   `conformance::asset_source` from its own tests (feature `conformance`; not an intra-doc
//!   link, since that module does not exist for a `cargo doc` run without the feature).
//! - [`PackWriter`] is the reference writer used to build pack files and test fixtures.
//! - [`AssetStore`] decodes and caches assets read from one [`AssetSource`]; it is a plain type,
//!   not simulation state.
//!
//! Every decoder in this crate treats its input as foreign bytes (contract §2 rule 9):
//! [`PackReader::from_bytes`] never panics, for any input, and reports a structural problem as a
//! [`PackError`] instead.
//!
//! ## Generated manifest codec (project ADR-0011, Plan-0002 WP8.1/WP8.3)
//!
//! [`PackManifestBody`] is the schema-generated codec for the contract §12 manifest layout
//! (`schema/pack_manifest_v1.gschema`). [`PackReader`] and [`PackWriter`] encode and decode the
//! manifest only through it and map its errors onto [`PackError::Manifest`]; the header, the table
//! of contents, payload alignment and the cross-checks between manifest and TOC stay in this crate.
//! [`PackManifest`] is the read-only view a reader hands out. `docs/formats/pack.md` documents the
//! complete pack v1 layout.

mod error;
mod generated;
mod ids;
mod pack;
mod source;
mod store;

#[cfg(feature = "conformance")]
pub mod conformance;

pub use error::{AssetError, PackError};
pub use generated::pack_manifest::{PackManifestBody, PackManifestV1Error};
pub use ids::{AssetEntry, AssetId, AssetKind, AssetPath, ContentHash, Sha256};
pub use pack::{PackManifest, PackReader, PackWriter};
pub use source::{AssetSource, EmptyAssetSource, MemorySource};
pub use store::{AssetStore, Handle};

/// Magic bytes at the start of every pack v1 file.
pub const PACK_MAGIC: [u8; 8] = *b"GRIMPACK";
/// Binary format version this crate reads and writes.
pub const PACK_FORMAT_VERSION: u32 = 1;
/// Every payload offset in a pack is a multiple of this many bytes.
pub const PACK_ALIGN: u64 = 16;
/// Upper bound on the number of entries a pack may declare.
pub const MAX_ENTRIES: u32 = 65_536;
/// Upper bound (bytes) on an [`AssetPath`]'s length.
pub const MAX_PATH_LEN: usize = 255;
/// Upper bound (bytes) on one entry's payload length.
pub const MAX_ENTRY_LEN: u64 = 256 * 1024 * 1024;
/// Upper bound (bytes) on the manifest section's length.
pub const MAX_MANIFEST_LEN: u64 = 16 * 1024 * 1024;
/// Upper bound (bytes) on a whole pack file, enforced by [`PackReader::open`].
pub const MAX_PACK_LEN: u64 = 1024 * 1024 * 1024;
