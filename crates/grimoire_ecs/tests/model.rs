//! Property tests: the world against a `BTreeMap` reference model, hash reproducibility and
//! snapshot/restore continuation.

use std::collections::{BTreeMap, VecDeque};

use grimoire_core::{StableHasher, impl_stable_hash};
use grimoire_ecs::{Entity, With, Without, World};
use proptest::prelude::*;

#[derive(Clone, Debug, PartialEq)]
struct A {
    v: u32,
}
impl_stable_hash!(A { v });

#[derive(Clone, Debug, PartialEq)]
struct B {
    v: i64,
}
impl_stable_hash!(B { v });

#[derive(Clone, Debug, PartialEq)]
struct C {
    v: u8,
}
impl_stable_hash!(C { v });

#[derive(Clone, Debug, PartialEq)]
struct Res {
    v: u16,
}
impl_stable_hash!(Res { v });

#[derive(Clone, Debug)]
enum Op {
    Spawn {
        a: Option<u32>,
        b: Option<i64>,
        c: Option<u8>,
    },
    Despawn(usize),
    InsertA(usize, u32),
    InsertB(usize, i64),
    InsertC(usize, u8),
    RemoveA(usize),
    RemoveB(usize),
    RemoveC(usize),
    Get(usize),
    MutateA(usize),
    InsertRes(u16),
    RemoveRes,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (any::<Option<u32>>(), any::<Option<i64>>(), any::<Option<u8>>())
            .prop_map(|(a, b, c)| Op::Spawn { a, b, c }),
        2 => any::<usize>().prop_map(Op::Despawn),
        2 => (any::<usize>(), any::<u32>()).prop_map(|(e, v)| Op::InsertA(e, v)),
        2 => (any::<usize>(), any::<i64>()).prop_map(|(e, v)| Op::InsertB(e, v)),
        1 => (any::<usize>(), any::<u8>()).prop_map(|(e, v)| Op::InsertC(e, v)),
        2 => any::<usize>().prop_map(Op::RemoveA),
        1 => any::<usize>().prop_map(Op::RemoveB),
        1 => any::<usize>().prop_map(Op::RemoveC),
        2 => any::<usize>().prop_map(Op::Get),
        1 => any::<usize>().prop_map(Op::MutateA),
        1 => any::<u16>().prop_map(Op::InsertRes),
        1 => Just(Op::RemoveRes),
    ]
}

/// Picks a handle among all entities ever spawned, so stale handles are exercised too.
fn pick(spawned: &[Entity], selector: usize) -> Option<Entity> {
    if spawned.is_empty() {
        None
    } else {
        Some(spawned[selector % spawned.len()])
    }
}

/// Spawns a bundle matching the optional components; components are added in a fixed order.
fn spawn_with(world: &mut World, a: Option<u32>, b: Option<i64>, c: Option<u8>) -> Entity {
    let entity = match (a, b) {
        (Some(a), Some(b)) => world.spawn((A { v: a }, B { v: b })),
        (Some(a), None) => world.spawn((A { v: a },)),
        (None, Some(b)) => world.spawn((B { v: b },)),
        (None, None) => world.spawn(()),
    };
    if let Some(c) = c {
        world.insert(entity, C { v: c }).unwrap();
    }
    entity
}

/// Applies `op` to the world only. Returns the spawned entity, if any.
fn apply(world: &mut World, spawned: &mut Vec<Entity>, op: &Op) {
    match *op {
        Op::Spawn { a, b, c } => spawned.push(spawn_with(world, a, b, c)),
        Op::Despawn(e) => {
            if let Some(e) = pick(spawned, e) {
                world.despawn(e);
            }
        }
        Op::InsertA(e, v) => {
            if let Some(e) = pick(spawned, e) {
                let _ = world.insert(e, A { v });
            }
        }
        Op::InsertB(e, v) => {
            if let Some(e) = pick(spawned, e) {
                let _ = world.insert(e, B { v });
            }
        }
        Op::InsertC(e, v) => {
            if let Some(e) = pick(spawned, e) {
                let _ = world.insert(e, C { v });
            }
        }
        Op::RemoveA(e) => {
            if let Some(e) = pick(spawned, e) {
                world.remove::<A>(e);
            }
        }
        Op::RemoveB(e) => {
            if let Some(e) = pick(spawned, e) {
                world.remove::<B>(e);
            }
        }
        Op::RemoveC(e) => {
            if let Some(e) = pick(spawned, e) {
                world.remove::<C>(e);
            }
        }
        Op::Get(e) => {
            if let Some(e) = pick(spawned, e) {
                let _ = (world.get::<A>(e), world.get::<B>(e), world.get::<C>(e));
            }
        }
        Op::MutateA(e) => {
            if let Some(a) = pick(spawned, e).and_then(|e| world.get_mut::<A>(e)) {
                a.v = a.v.wrapping_add(1);
            }
        }
        Op::InsertRes(v) => world.insert_resource(Res { v }),
        Op::RemoveRes => {
            world.remove_resource::<Res>();
        }
    }
}

#[derive(Default, Debug, Clone, PartialEq)]
struct Row {
    a: Option<A>,
    b: Option<B>,
    c: Option<C>,
}

/// Reference model: plain maps plus the documented allocator (FIFO free list, generations).
#[derive(Default)]
struct Model {
    rows: BTreeMap<Entity, Row>,
    generations: Vec<u32>,
    free: VecDeque<u32>,
    resource: Option<Res>,
}

impl Model {
    fn spawn(&mut self, row: Row) -> Entity {
        let entity = match self.free.pop_front() {
            Some(index) => Entity::from_bits(
                (u64::from(self.generations[index as usize]) << 32) | u64::from(index),
            ),
            None => {
                self.generations.push(0);
                Entity::from_bits(self.generations.len() as u64 - 1)
            }
        };
        self.rows.insert(entity, row);
        entity
    }

    fn apply(&mut self, spawned: &mut Vec<Entity>, op: &Op) {
        match *op {
            Op::Spawn { a, b, c } => {
                let entity = self.spawn(Row {
                    a: a.map(|v| A { v }),
                    b: b.map(|v| B { v }),
                    c: c.map(|v| C { v }),
                });
                spawned.push(entity);
            }
            Op::Despawn(e) => {
                if let Some(e) = pick(spawned, e)
                    && self.rows.remove(&e).is_some()
                {
                    self.generations[e.index() as usize] += 1;
                    self.free.push_back(e.index());
                }
            }
            Op::InsertA(e, v) => self.row(spawned, e, |row| row.a = Some(A { v })),
            Op::InsertB(e, v) => self.row(spawned, e, |row| row.b = Some(B { v })),
            Op::InsertC(e, v) => self.row(spawned, e, |row| row.c = Some(C { v })),
            Op::RemoveA(e) => self.row(spawned, e, |row| row.a = None),
            Op::RemoveB(e) => self.row(spawned, e, |row| row.b = None),
            Op::RemoveC(e) => self.row(spawned, e, |row| row.c = None),
            Op::Get(_) => {}
            Op::MutateA(e) => self.row(spawned, e, |row| {
                if let Some(a) = &mut row.a {
                    a.v = a.v.wrapping_add(1);
                }
            }),
            Op::InsertRes(v) => self.resource = Some(Res { v }),
            Op::RemoveRes => self.resource = None,
        }
    }

    fn row(&mut self, spawned: &[Entity], selector: usize, f: impl FnOnce(&mut Row)) {
        if let Some(row) = pick(spawned, selector).and_then(|e| self.rows.get_mut(&e)) {
            f(row);
        }
    }
}

fn hash(world: &World) -> u64 {
    let mut hasher = StableHasher::new();
    world.stable_hash(&mut hasher);
    hasher.finish()
}

fn check_against_model(
    world: &World,
    model: &Model,
    spawned: &[Entity],
) -> Result<(), TestCaseError> {
    prop_assert_eq!(world.entity_count(), model.rows.len());
    for &entity in spawned {
        let row = model.rows.get(&entity);
        prop_assert_eq!(world.is_alive(entity), row.is_some());
        let expected = row.cloned().unwrap_or_default();
        prop_assert_eq!(world.get::<A>(entity), expected.a.as_ref());
        prop_assert_eq!(world.get::<B>(entity), expected.b.as_ref());
        prop_assert_eq!(world.get::<C>(entity), expected.c.as_ref());
    }

    let mut all: Vec<(Entity, Row)> = world
        .query::<(Entity, Option<&A>, Option<&B>, Option<&C>)>()
        .map(|(e, a, b, c)| {
            (
                e,
                Row {
                    a: a.cloned(),
                    b: b.cloned(),
                    c: c.cloned(),
                },
            )
        })
        .collect();
    all.sort_by_key(|(e, _)| *e);
    let expected: Vec<(Entity, Row)> = model.rows.iter().map(|(e, r)| (*e, r.clone())).collect();
    prop_assert_eq!(all, expected);

    let mut a_without_b: Vec<Entity> = world
        .query::<(Entity, &A, Without<B>)>()
        .map(|(e, _, ())| e)
        .collect();
    a_without_b.sort();
    let expected: Vec<Entity> = model
        .rows
        .iter()
        .filter(|(_, r)| r.a.is_some() && r.b.is_none())
        .map(|(e, _)| *e)
        .collect();
    prop_assert_eq!(a_without_b, expected);

    let with_c = world.query::<(&B, With<C>)>().count();
    let expected = model
        .rows
        .values()
        .filter(|r| r.b.is_some() && r.c.is_some())
        .count();
    prop_assert_eq!(with_c, expected);
    prop_assert_eq!(world.resource::<Res>(), model.resource.as_ref());
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn world_matches_reference_model(ops in prop::collection::vec(op(), 0..120)) {
        let mut world = World::new();
        let mut model = Model::default();
        let mut world_spawned = Vec::new();
        let mut model_spawned = Vec::new();
        for op in &ops {
            apply(&mut world, &mut world_spawned, op);
            model.apply(&mut model_spawned, op);
            prop_assert_eq!(&world_spawned, &model_spawned, "entity ids follow the allocator model");
            check_against_model(&world, &model, &world_spawned)?;
        }
    }

    #[test]
    fn equal_operations_give_equal_hashes(ops in prop::collection::vec(op(), 0..120)) {
        let mut first = World::new();
        let mut second = World::new();
        let mut first_spawned = Vec::new();
        let mut second_spawned = Vec::new();
        for op in &ops {
            apply(&mut first, &mut first_spawned, op);
        }
        for op in &ops {
            apply(&mut second, &mut second_spawned, op);
        }
        prop_assert_eq!(hash(&first), hash(&second));
        let first_order: Vec<Entity> = first.query::<Entity>().collect();
        let second_order: Vec<Entity> = second.query::<Entity>().collect();
        prop_assert_eq!(first_order, second_order);
    }

    #[test]
    fn snapshot_restore_continues_identically(
        ops in prop::collection::vec(op(), 1..120),
        split in any::<prop::sample::Index>(),
        noise in prop::collection::vec(op(), 0..40),
    ) {
        let split = split.index(ops.len() + 1);

        let mut reference = World::new();
        let mut reference_spawned = Vec::new();
        for op in &ops {
            apply(&mut reference, &mut reference_spawned, op);
        }

        let mut world = World::new();
        let mut spawned = Vec::new();
        for op in &ops[..split] {
            apply(&mut world, &mut spawned, op);
        }
        let snapshot = world.snapshot();
        let spawned_at_snapshot = spawned.clone();
        let hash_at_snapshot = hash(&world);

        let mut noise_spawned = spawned.clone();
        for op in &noise {
            apply(&mut world, &mut noise_spawned, op);
        }
        world.restore(&snapshot);
        prop_assert_eq!(hash(&world), hash_at_snapshot);

        let mut fresh = World::new();
        fresh.restore(&snapshot);
        let mut fresh_spawned = spawned_at_snapshot;

        for op in &ops[split..] {
            apply(&mut world, &mut spawned, op);
            apply(&mut fresh, &mut fresh_spawned, op);
        }
        prop_assert_eq!(hash(&world), hash(&reference));
        prop_assert_eq!(hash(&fresh), hash(&reference));
        prop_assert_eq!(&spawned, &reference_spawned);
        prop_assert_eq!(&fresh_spawned, &reference_spawned);
    }
}
