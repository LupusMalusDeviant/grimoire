//! Sigil → Collision adapter (contract §9.6).
//!
//! Only the resource types ship with WP1.3, because the player proxy (`crate::fixtures`, feature
//! `fixtures`, §9.5; not an intra-doc link, since that module does not exist for a `cargo doc`
//! run without the feature) writes [`GrazeProbe`] every tick it runs. `SigilCollideConfig`,
//! `SigilCollidePlugin` and the
//! `collide.broadphase`/`collide.graze` systems that produce [`GrazeHits`] from a live
//! `grimoire_sigil::BulletPool` follow in WP11.2.

use grimoire_collide::{GrazeRing, Hit, LayerMask};
use grimoire_core::{StableHash, StableHasher, impl_stable_hash};

/// Resource: the graze-ring query to run this tick, written by whatever owns the graze centre
/// (the player proxy in WP1.3; a game's own system once it replaces the proxy in P2).
///
/// Default mask is [`LayerMask::NONE`], so an installed but unconfigured probe matches nothing
/// until something sets a real mask.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GrazeProbe {
    /// Ring to query with.
    pub ring: GrazeRing,
    /// Layers the ring query considers.
    pub mask: LayerMask,
}
impl_stable_hash!(GrazeProbe { ring, mask });

/// Resource: the graze-ring hits of the most recent `collide.graze` system run (WP11.2). Empty
/// until that system exists.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GrazeHits(pub Vec<Hit>);

impl StableHash for GrazeHits {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        self.0.stable_hash(hasher);
    }
}
