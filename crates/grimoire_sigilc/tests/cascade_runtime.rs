//! Compiled transforms run (Plan 0002 WP5.2): the conformance corpus' cascade pattern
//! `03-subemitter-cascade.sigil`, compiled by this crate, driven through `grimoire_sigil::install`.
//!
//! The pattern is seeds → (`become_emitter` after 50 t) shards → (`change_type` after 4 u) embers
//! → (`become_emitter` after 30 t) motes → (`burst` after 40 t) dust, reaching the maximum cascade
//! depth 3. The test checks the generations appear in that order with the depths the compiler's
//! own cascade walk predicts, and that nothing is dropped.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;

use grimoire_core::Vec2;
use grimoire_sigil::{
    BehaviorRegistryBuilder, BulletPool, DespawnCause, Emitter, SigilConfig, SigilLibrary,
    SigilUnit, install,
};
use grimoire_sigilc::compiler::{LoadError, SourceLoader, compile};
use grimoire_sim::{Simulation, TickInput};

struct NoImports;

impl SourceLoader for NoImports {
    fn load(&self, _path: &str) -> Result<String, LoadError> {
        Err(LoadError::NotFound)
    }
}

fn compiled_cascade() -> SigilUnit {
    let path = support::corpus_root().join("valid/03-subemitter-cascade.sigil");
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
        .replace("\r\n", "\n");
    let output = compile(
        "patterns/03-subemitter-cascade.sigil",
        &source,
        &NoImports,
        &BTreeMap::new(),
    );
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    SigilUnit::from_bytes(&output.bytes.expect("compiles")).expect("decodes")
}

// Bullet types are numbered by name, emitters too (`docs/formats/sigil.md` §11.1).
const DUST: u16 = 0;
const EMBER: u16 = 1;
const MOTE: u16 = 2;
const SEED: u16 = 3;
const SHARD: u16 = 4;
const SEEDS_EMITTER: u16 = 1;

#[test]
fn the_compiled_cascade_reaches_every_generation_at_its_depth() {
    let unit = compiled_cascade();
    let unit_id = unit.id();
    let registry = BehaviorRegistryBuilder::new(1).build();
    let library = SigilLibrary::new(vec![unit], Arc::clone(&registry)).expect("library builds");
    let mut sim = Simulation::new(3);
    install(
        &mut sim,
        library,
        registry,
        SigilConfig::new(4096, Vec2::new(-1.0e3, -1.0e3), Vec2::new(1.0e3, 1.0e3)),
    )
    .expect("install succeeds");
    sim.world_mut().spawn((Emitter {
        unit: unit_id,
        emitter: SEEDS_EMITTER,
        origin: Vec2::ZERO,
        rotation: 0.0,
        started_at: 0,
    },));

    // First tick each generation is seen, and the depths seen per generation.
    let mut first_seen: BTreeMap<u16, u64> = BTreeMap::new();
    let mut depths: BTreeMap<u16, std::collections::BTreeSet<u8>> = BTreeMap::new();
    let mut transform_despawns = 0usize;
    for tick in 0..200u64 {
        sim.step(TickInput::default());
        let pool = sim
            .world()
            .resource::<BulletPool>()
            .expect("pool installed");
        let columns = pool.columns();
        for bullet in pool.iter() {
            first_seen.entry(bullet.bullet_type()).or_insert(tick);
            depths
                .entry(bullet.bullet_type())
                .or_default()
                .insert(columns.cascade[bullet.id().index() as usize]);
        }
        transform_despawns += pool
            .events()
            .iter()
            .filter(|event| event.cause == DespawnCause::Transform)
            .count();
        assert_eq!(pool.dropped_spawns(), 0);
    }

    let order: Vec<u16> = {
        let mut by_tick: Vec<(u64, u16)> = first_seen.iter().map(|(&t, &at)| (at, t)).collect();
        by_tick.sort_unstable();
        by_tick
            .into_iter()
            .map(|(_, bullet_type)| bullet_type)
            .collect()
    };
    assert_eq!(
        order,
        vec![SEED, SHARD, EMBER, MOTE, DUST],
        "{first_seen:?}"
    );
    let expected_depths = [(SEED, 0), (SHARD, 1), (EMBER, 1), (MOTE, 2), (DUST, 3)];
    for (bullet_type, depth) in expected_depths {
        assert_eq!(
            depths[&bullet_type].iter().copied().collect::<Vec<_>>(),
            vec![depth],
            "bullet type {bullet_type}"
        );
    }
    // 6 seeds become emitters, 30 embers become emitters, 90 motes burst.
    assert_eq!(transform_despawns, 6 + 30 + 90);
}
