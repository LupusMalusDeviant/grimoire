//! §7.1 "Veränderliche Ressourcen in Blöcken": take the resource, run blocks over `world.executor()`
//! while reading other resources through `&World`, write it back.

use grimoire_core::{StableHash, StableHasher};
use grimoire_ecs::{QUERY_BLOCK_SIZE, World};
use grimoire_ecs_p1::{NoopObserver, SystemObserver, run_blocks, slice_block_ranges};

#[derive(Clone, Default, Debug)]
struct Pool {
    values: Vec<f32>,
}

impl StableHash for Pool {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        self.values.stable_hash(hasher);
    }
}

#[derive(Clone, Copy, Debug)]
struct Scale(f32);

impl StableHash for Scale {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_f32(self.0);
    }
}

fn update(world: &mut World) {
    let mut pool = std::mem::take(world.resource_mut::<Pool>().expect("pool"));
    let mut rest: &mut [f32] = &mut pool.values;
    let mut blocks = Vec::new();
    for range in slice_block_ranges(rest.len()) {
        let (head, tail) = std::mem::take(&mut rest).split_at_mut(range.len());
        blocks.push((range.start / QUERY_BLOCK_SIZE, head));
        rest = tail;
    }
    let world_ref: &World = world;
    let indices = run_blocks(world_ref.executor(), blocks, |(index, slice)| {
        let scale = world_ref.resource::<Scale>().map_or(1.0, |scale| scale.0);
        for value in slice.iter_mut() {
            *value *= scale;
        }
        index
    });
    assert_eq!(indices, (0..indices.len()).collect::<Vec<_>>());
    *world.resource_mut::<Pool>().expect("pool") = pool;
}

#[test]
fn mutable_resource_blocks_compile() {
    let mut world = World::new();
    world.insert_resource(Pool {
        values: vec![1.0; 3000],
    });
    world.insert_resource(Scale(2.0));
    update(&mut world);
    let observer: &mut dyn SystemObserver = &mut NoopObserver;
    let _ = observer;
}
