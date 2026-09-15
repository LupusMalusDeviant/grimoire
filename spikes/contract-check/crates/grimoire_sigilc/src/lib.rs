//! `grimoire_sigilc` library stub. The contract fixes no Rust API for sigilc; only the edges and the
//! `UnitId` derivation rule (§11.1: same rule as `AssetId::from_path`, without an assets edge).

use grimoire_core::StableHasher;
use grimoire_sigil::UnitId;

pub fn unit_id_for_canonical_path(path: &str) -> Option<UnitId> {
    let mut hasher = StableHasher::new();
    hasher.write_str("grimoire.asset-id.v1");
    hasher.write_str(path);
    let id = hasher.finish();
    (id != 0).then_some(UnitId(id))
}

pub fn simulate_stub(seed: u64) -> u64 {
    let sim = grimoire_sim::Simulation::new(seed);
    let _ = (sim.world().entity_count(), grimoire_sim_p1::ENGINE_VERSION);
    let _: Option<grimoire_ecs::Entity> = None;
    seed
}
