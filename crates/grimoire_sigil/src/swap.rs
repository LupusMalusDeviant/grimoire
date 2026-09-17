//! Hot swap and checked restore (contract §11.8, Plan 0002 WP5.5).
//!
//! [`replace_unit`] takes `&mut Simulation`, so it can only run between two `Simulation::step`s:
//! systems see `&mut World`/`&World` and command buffers, never the simulation, which makes a swap
//! inside a tick structurally impossible. Its effect starts with the next tick,
//! `effective_tick = sim.tick()`. [`restore_checked`] is the snapshot restore for sessions that
//! allow swaps: a snapshot of a different content epoch is rejected instead of silently undoing
//! the swap.

use grimoire_ecs::World;
use grimoire_sim::{SimSnapshot, Simulation, SwapRecord};

use crate::content::{ContentEpoch, SigilContent};
use crate::emitter::Emitter;
use crate::error::SigilError;
use crate::pool::BulletPool;
use crate::unit::{SigilUnit, UnitId};

/// What one successful [`replace_unit`] did.
///
/// `#[non_exhaustive]` and only produced by [`replace_unit`] (contract §2 rule 13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct SwapReport {
    /// The replaced unit.
    pub unit: UnitId,
    /// Content epoch after the swap (`swaps` one higher, new manifest hash).
    pub epoch: ContentEpoch,
    /// First tick that runs with the new unit: the simulation's tick at the time of the swap.
    pub effective_tick: u64,
    /// Emitters of the unit whose pattern restarted at `effective_tick`.
    pub restarted_emitters: u32,
    /// Bullets of the unit despawned by the swap.
    pub despawned_bullets: u32,
}

/// The replay v2 swap marker of a swap (contract §8.1, §11.8): the tick the new content takes
/// effect at and the content manifest after the swap. A recording session hands it to
/// `ReplayHeader::record_swap`, which merges several swaps at one tick boundary.
impl From<SwapReport> for SwapRecord {
    fn from(report: SwapReport) -> Self {
        SwapRecord::new(report.effective_tick, report.epoch.manifest_hash)
    }
}

/// Replaces the loaded unit with `unit`'s id by `unit`, at the tick boundary the simulation is at
/// (contract §11.8).
///
/// On success, in this order: the content gets a new library (same registry, the unit replaced in
/// place) and a new epoch (`swaps + 1`, new manifest hash); every bullet of the unit is despawned
/// in slot order without despawn events; every [`Emitter`] of the unit whose index the new unit
/// still has restarts (`started_at = sim.tick()`), in query order. Emitters with a larger index
/// become inactive and keep their `started_at`; bullets and emitters of other units are
/// untouched. A byte-identical unit is a swap too.
///
/// # Errors
///
/// Checked before anything changes, so on any error the simulation is unchanged:
/// - [`SigilError::NotInstalled`] if no Sigil content is installed;
/// - [`SigilError::UnknownUnit`] if the loaded library has no unit with `unit`'s id;
/// - [`SigilError::UnknownBehavior`] if `unit` references a behavior the loaded registry lacks;
/// - [`SigilError::SwapLimit`] if the epoch already counts `u32::MAX` swaps.
pub fn replace_unit(sim: &mut Simulation, unit: SigilUnit) -> Result<SwapReport, SigilError> {
    let effective_tick = sim.tick();
    let world = sim.world_mut();
    let content = world
        .resource::<SigilContent>()
        .ok_or(SigilError::NotInstalled)?;
    let library = content.library();
    let unit_id = unit.id();
    let unit_index = library
        .unit_index(unit_id)
        .ok_or(SigilError::UnknownUnit(unit_id))?;
    if let Some(&behavior) = unit
        .behavior_refs()
        .iter()
        .find(|&&behavior| library.registry().get(behavior).is_none())
    {
        return Err(SigilError::UnknownBehavior {
            unit: unit_id,
            behavior,
        });
    }
    if library.epoch().swaps == u32::MAX {
        return Err(SigilError::SwapLimit);
    }

    let emitter_count = unit.emitter_count();
    let swapped = SigilContent::new(library.with_replaced_unit(unit_index, unit));
    let epoch = swapped.epoch();
    // Written into the existing slot: the resource keeps its position in the world hash.
    if let Some(slot) = world.resource_mut::<SigilContent>() {
        *slot = swapped;
    }

    let despawned_bullets = world
        .resource_mut::<BulletPool>()
        .map_or(0, |pool| pool.despawn_unit_for_swap(unit_index));
    let restarted_emitters = restart_emitters(world, unit_id, emitter_count, effective_tick);

    Ok(SwapReport {
        unit: unit_id,
        epoch,
        effective_tick,
        restarted_emitters,
        despawned_bullets,
    })
}

/// Sets `started_at = tick` on every emitter of `unit` whose index is below `emitter_count`, in
/// query order, and returns how many there were.
fn restart_emitters(world: &mut World, unit: UnitId, emitter_count: u16, tick: u64) -> u32 {
    let mut restarted = 0u32;
    for emitter in world.query_mut::<&mut Emitter>() {
        if emitter.unit == unit && emitter.emitter < emitter_count {
            emitter.started_at = tick;
            restarted += 1;
        }
    }
    restarted
}

/// Restores `snapshot` only if its content epoch equals the loaded one (contract §11.8).
///
/// Both epochs are read from [`SigilContent`]: the snapshot's and the simulation's. Equal epochs,
/// or no Sigil content on either side, restore exactly like `Simulation::restore`. Sessions that
/// allow swaps use this instead of the unchecked restore, which would bring back the snapshot's
/// content and silently undo a swap.
///
/// # Errors
///
/// [`SigilError::ContentEpochMismatch`] if the epochs differ (including content on only one
/// side); the simulation is then unchanged.
pub fn restore_checked(sim: &mut Simulation, snapshot: &SimSnapshot) -> Result<(), SigilError> {
    let loaded = sim
        .world()
        .resource::<SigilContent>()
        .map(SigilContent::epoch);
    sim.restore_checked(snapshot, |snapshot| {
        let stored = snapshot.resource::<SigilContent>().map(SigilContent::epoch);
        if stored == loaded {
            Ok(())
        } else {
            Err(SigilError::ContentEpochMismatch {
                snapshot: stored,
                loaded,
            })
        }
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use grimoire_core::Vec2;

    use super::*;
    use crate::behavior::BehaviorRegistryBuilder;
    use crate::content::SigilLibrary;
    use crate::systems::{SigilConfig, install};
    use crate::test_support::{build_unit, plain_bullet_type};

    #[test]
    fn swap_limit_is_reported_before_anything_changes() {
        let unit = build_unit(3, &[plain_bullet_type()], 1);
        let registry = BehaviorRegistryBuilder::new(1).build();
        let library = SigilLibrary::new(vec![unit.clone()], Arc::clone(&registry))
            .expect("library builds")
            .with_swaps(u32::MAX);
        let mut sim = Simulation::new(1);
        install(
            &mut sim,
            library,
            registry,
            SigilConfig::new(8, Vec2::new(-10.0, -10.0), Vec2::new(10.0, 10.0)),
        )
        .expect("install succeeds");
        let before = sim.state_hash();
        assert_eq!(replace_unit(&mut sim, unit), Err(SigilError::SwapLimit));
        assert_eq!(sim.state_hash(), before);
    }
}
