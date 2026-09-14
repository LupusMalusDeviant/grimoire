//! Unit tests of the public `grimoire_ecs` API.

use grimoire_core::{StableHasher, impl_stable_hash};
use grimoire_ecs::{
    CommandBuffer, EcsError, Entity, Schedule, System, With, Without, World, WorldSnapshot,
    system_fn,
};

#[derive(Clone, Debug, PartialEq)]
struct Pos {
    x: f32,
    y: f32,
}
impl_stable_hash!(Pos { x, y });

#[derive(Clone, Debug, PartialEq)]
struct Vel {
    x: f32,
    y: f32,
}
impl_stable_hash!(Vel { x, y });

#[derive(Clone, Debug, PartialEq)]
struct Health {
    value: i32,
}
impl_stable_hash!(Health { value });

#[derive(Clone, Debug, PartialEq)]
struct Tag {
    id: u8,
}
impl_stable_hash!(Tag { id });

#[derive(Clone, Debug, PartialEq)]
struct Counter {
    value: u64,
}
impl_stable_hash!(Counter { value });

#[derive(Clone, Debug, PartialEq)]
struct Label {
    text: String,
}
impl_stable_hash!(Label { text });

fn pos(x: f32) -> Pos {
    Pos { x, y: -x }
}

fn vel(x: f32) -> Vel {
    Vel { x, y: 0.5 }
}

fn hash(world: &World) -> u64 {
    let mut hasher = StableHasher::new();
    world.stable_hash(&mut hasher);
    hasher.finish()
}

// ---------------------------------------------------------------------------------------------
// Entities

#[test]
fn new_world_is_empty() {
    let world = World::new();
    assert_eq!(world.entity_count(), 0);
    assert_eq!(world.query::<Entity>().count(), 0);
    assert_eq!(hash(&world), hash(&World::default()));
}

#[test]
fn spawn_assigns_sequential_ids() {
    let mut world = World::new();
    let a = world.spawn((pos(1.0),));
    let b = world.spawn((pos(2.0), vel(1.0)));
    let c = world.spawn(());
    assert_eq!((a.index(), a.generation()), (0, 0));
    assert_eq!((b.index(), b.generation()), (1, 0));
    assert_eq!((c.index(), c.generation()), (2, 0));
    assert_eq!(world.entity_count(), 3);
    assert!(world.is_alive(a) && world.is_alive(b) && world.is_alive(c));
}

#[test]
fn spawn_eight_component_bundle() {
    #[derive(Clone)]
    struct Extra {
        v: u16,
    }
    impl_stable_hash!(Extra { v });
    let mut world = World::new();
    let entity = world.spawn((
        pos(1.0),
        vel(2.0),
        Health { value: 3 },
        Tag { id: 4 },
        Counter { value: 5 },
        Label { text: "six".into() },
        Extra { v: 7 },
        8u32,
    ));
    assert_eq!(world.get::<Health>(entity), Some(&Health { value: 3 }));
    assert_eq!(world.get::<u32>(entity), Some(&8));
    assert_eq!(world.get::<Extra>(entity).map(|e| e.v), Some(7));
    assert_eq!(
        world.get::<Label>(entity).map(|l| l.text.as_str()),
        Some("six")
    );
}

#[test]
#[should_panic(expected = "contains the same component type more than once")]
fn spawn_duplicate_component_panics() {
    let mut world = World::new();
    world.spawn((pos(1.0), pos(2.0)));
}

#[test]
fn despawn_and_stale_handles() {
    let mut world = World::new();
    let a = world.spawn((pos(1.0),));
    assert!(world.despawn(a));
    assert!(!world.despawn(a), "second despawn must fail");
    assert!(!world.is_alive(a));
    assert_eq!(world.entity_count(), 0);
    assert_eq!(world.get::<Pos>(a), None);
    assert_eq!(world.get_mut::<Pos>(a), None);
    assert_eq!(world.remove::<Pos>(a), None);
    assert_eq!(world.insert(a, vel(1.0)), Err(EcsError::NoSuchEntity(a)));

    let reused = world.spawn((pos(9.0),));
    assert_eq!(reused.index(), a.index());
    assert_eq!(reused.generation(), a.generation() + 1);
    assert!(
        !world.is_alive(a),
        "stale generation must not alias the reused slot"
    );
    assert_eq!(world.get::<Pos>(a), None);
    assert_eq!(world.get::<Pos>(reused), Some(&pos(9.0)));
    assert!(!world.despawn(a));
    assert!(world.is_alive(reused));
}

#[test]
fn never_spawned_entity_is_not_alive() {
    let mut world = World::new();
    let ghost = Entity::from_bits(42);
    assert!(!world.is_alive(ghost));
    assert!(!world.despawn(ghost));
    assert_eq!(
        world.insert(ghost, pos(1.0)),
        Err(EcsError::NoSuchEntity(ghost))
    );
    assert_eq!(world.get::<Pos>(ghost), None);
}

#[test]
fn freed_slots_are_reused_oldest_first() {
    let mut world = World::new();
    let entities: Vec<Entity> = (0..4).map(|i| world.spawn((pos(i as f32),))).collect();
    world.despawn(entities[2]);
    world.despawn(entities[0]);
    world.despawn(entities[3]);
    let reuse: Vec<u32> = (0..4).map(|_| world.spawn(()).index()).collect();
    assert_eq!(reuse, vec![2, 0, 3, 4]);
}

#[test]
fn despawn_swap_remove_fixes_locations() {
    let mut world = World::new();
    let a = world.spawn((pos(1.0), vel(1.0)));
    let b = world.spawn((pos(2.0), vel(2.0)));
    let c = world.spawn((pos(3.0), vel(3.0)));
    assert!(world.despawn(a));
    assert_eq!(world.get::<Pos>(b), Some(&pos(2.0)));
    assert_eq!(world.get::<Pos>(c), Some(&pos(3.0)));
    assert_eq!(world.get::<Vel>(c), Some(&vel(3.0)));
    let order: Vec<Entity> = world.query::<Entity>().collect();
    assert_eq!(order, vec![c, b], "last row is swapped into the freed row");
    assert!(world.despawn(b));
    assert!(world.despawn(c));
    assert_eq!(world.query::<&Pos>().count(), 0);
}

// ---------------------------------------------------------------------------------------------
// Components

#[test]
fn register_component_is_idempotent_and_pins_ids() {
    let mut explicit = World::new();
    explicit.register_component::<Vel>();
    explicit.register_component::<Pos>();
    explicit.register_component::<Vel>();
    explicit.spawn((pos(1.0), vel(1.0)));

    let mut implicit = World::new();
    implicit.spawn((vel(1.0), pos(1.0)));
    assert_eq!(hash(&explicit), hash(&implicit));

    let mut other_order = World::new();
    other_order.spawn((pos(1.0), vel(1.0)));
    assert_ne!(
        hash(&explicit),
        hash(&other_order),
        "types are identified by registration number"
    );
}

#[test]
fn insert_adds_moves_and_replaces() {
    let mut world = World::new();
    let a = world.spawn((pos(1.0),));
    let b = world.spawn((pos(2.0),));
    assert_eq!(world.insert(a, vel(5.0)), Ok(()));
    assert_eq!(world.get::<Pos>(a), Some(&pos(1.0)));
    assert_eq!(world.get::<Vel>(a), Some(&vel(5.0)));
    assert_eq!(
        world.get::<Pos>(b),
        Some(&pos(2.0)),
        "swapped entity keeps its data"
    );
    assert_eq!(world.get::<Vel>(b), None);

    assert_eq!(world.insert(a, vel(6.0)), Ok(()));
    assert_eq!(
        world.get::<Vel>(a),
        Some(&vel(6.0)),
        "insert replaces in place"
    );
    assert_eq!(world.query::<&Vel>().count(), 1);
    assert_eq!(world.entity_count(), 2);
}

#[test]
fn insert_into_empty_entity() {
    let mut world = World::new();
    let entity = world.spawn(());
    world.insert(entity, Tag { id: 1 }).unwrap();
    assert_eq!(world.get::<Tag>(entity), Some(&Tag { id: 1 }));
}

#[test]
fn remove_returns_value_and_moves() {
    let mut world = World::new();
    let a = world.spawn((pos(1.0), vel(1.0), Health { value: 10 }));
    let b = world.spawn((pos(2.0), vel(2.0), Health { value: 20 }));
    assert_eq!(world.remove::<Vel>(a), Some(vel(1.0)));
    assert_eq!(world.remove::<Vel>(a), None, "already removed");
    assert_eq!(world.remove::<Tag>(a), None, "never registered");
    assert_eq!(world.get::<Pos>(a), Some(&pos(1.0)));
    assert_eq!(world.get::<Health>(a), Some(&Health { value: 10 }));
    assert_eq!(world.get::<Vel>(b), Some(&vel(2.0)));
    assert_eq!(world.get::<Health>(b), Some(&Health { value: 20 }));
}

#[test]
fn removing_last_component_keeps_entity_alive() {
    let mut world = World::new();
    let entity = world.spawn((Tag { id: 3 },));
    assert_eq!(world.remove::<Tag>(entity), Some(Tag { id: 3 }));
    assert!(world.is_alive(entity));
    assert_eq!(world.entity_count(), 1);
    assert_eq!(world.query::<Entity>().collect::<Vec<_>>(), vec![entity]);
    assert_eq!(world.query::<&Tag>().count(), 0);
    world.insert(entity, Tag { id: 4 }).unwrap();
    assert_eq!(world.get::<Tag>(entity), Some(&Tag { id: 4 }));
}

#[test]
fn get_mut_changes_value() {
    let mut world = World::new();
    let entity = world.spawn((Health { value: 1 },));
    world.get_mut::<Health>(entity).unwrap().value = 7;
    assert_eq!(world.get::<Health>(entity), Some(&Health { value: 7 }));
    assert_eq!(world.get_mut::<Pos>(entity), None);
}

#[test]
fn archetype_moves_round_trip() {
    let mut world = World::new();
    let entities: Vec<Entity> = (0..10).map(|i| world.spawn((pos(i as f32),))).collect();
    for (i, &entity) in entities.iter().enumerate() {
        if i % 2 == 0 {
            world.insert(entity, vel(i as f32)).unwrap();
        }
    }
    for (i, &entity) in entities.iter().enumerate() {
        assert_eq!(world.get::<Pos>(entity), Some(&pos(i as f32)));
        assert_eq!(world.get::<Vel>(entity).is_some(), i % 2 == 0);
    }
    for (i, &entity) in entities.iter().enumerate() {
        if i % 2 == 0 {
            assert_eq!(world.remove::<Vel>(entity), Some(vel(i as f32)));
        }
    }
    for (i, &entity) in entities.iter().enumerate() {
        assert_eq!(world.get::<Pos>(entity), Some(&pos(i as f32)));
        assert_eq!(world.get::<Vel>(entity), None);
    }
}

// ---------------------------------------------------------------------------------------------
// Queries

#[test]
fn query_reads_and_orders_by_archetype_creation() {
    let mut world = World::new();
    let a = world.spawn((pos(1.0),));
    let b = world.spawn((pos(2.0), vel(2.0)));
    let c = world.spawn((pos(3.0),));
    let items: Vec<(Entity, &Pos)> = world.query::<(Entity, &Pos)>().collect();
    assert_eq!(items, vec![(a, &pos(1.0)), (c, &pos(3.0)), (b, &pos(2.0))]);
    let both: Vec<(Entity, &Pos, &Vel)> = world.query::<(Entity, &Pos, &Vel)>().collect();
    assert_eq!(both, vec![(b, &pos(2.0), &vel(2.0))]);
}

#[test]
fn query_mut_writes() {
    let mut world = World::new();
    let a = world.spawn((pos(1.0), vel(10.0)));
    let b = world.spawn((pos(2.0),));
    for (p, v) in world.query_mut::<(&mut Pos, &Vel)>() {
        p.x += v.x;
    }
    assert_eq!(world.get::<Pos>(a).unwrap().x, 11.0);
    assert_eq!(world.get::<Pos>(b).unwrap().x, 2.0);
    for (entity, p) in world.query_mut::<(Entity, &mut Pos)>() {
        p.y = entity.index() as f32;
    }
    assert_eq!(world.get::<Pos>(b).unwrap().y, 1.0);
}

#[test]
fn single_element_queries() {
    let mut world = World::new();
    world.spawn((Health { value: 1 },));
    world.spawn((Health { value: 2 },));
    for health in world.query_mut::<&mut Health>() {
        health.value *= 10;
    }
    let values: Vec<i32> = world.query::<&Health>().map(|h| h.value).collect();
    assert_eq!(values, vec![10, 20]);
}

#[test]
fn query_with_zero_matches() {
    let mut world = World::new();
    assert_eq!(world.query::<&Pos>().count(), 0, "unregistered component");
    assert_eq!(world.query_mut::<&mut Pos>().count(), 0);
    world.spawn((vel(1.0),));
    assert_eq!(world.query::<(&Pos, &Vel)>().count(), 0);
    world.register_component::<Pos>();
    assert_eq!(world.query::<&Pos>().count(), 0, "registered but unused");
    let entity = world.spawn((pos(1.0),));
    world.despawn(entity);
    assert_eq!(world.query::<&Pos>().count(), 0, "archetype emptied");
}

#[test]
fn optional_elements() {
    let mut world = World::new();
    let a = world.spawn((pos(1.0),));
    let b = world.spawn((pos(2.0), vel(2.0)));
    let c = world.spawn((vel(3.0),));
    let items: Vec<(Entity, Option<&Vel>)> = world.query::<(Entity, Option<&Vel>)>().collect();
    assert_eq!(
        items,
        vec![(a, None), (b, Some(&vel(2.0))), (c, Some(&vel(3.0)))]
    );
    assert_eq!(
        world.query::<Option<&Tag>>().count(),
        3,
        "unregistered optional"
    );

    for (p, v) in world.query_mut::<(&Pos, Option<&mut Vel>)>() {
        if let Some(v) = v {
            v.x += p.x;
        }
    }
    assert_eq!(world.get::<Vel>(b), Some(&Vel { x: 4.0, y: 0.5 }));
    assert_eq!(world.get::<Vel>(c), Some(&vel(3.0)));
}

#[test]
fn with_and_without_filters() {
    let mut world = World::new();
    let a = world.spawn((pos(1.0), Tag { id: 1 }));
    let b = world.spawn((pos(2.0),));
    let with: Vec<Entity> = world
        .query::<(Entity, With<Tag>)>()
        .map(|(e, ())| e)
        .collect();
    assert_eq!(with, vec![a]);
    let without: Vec<Entity> = world
        .query::<(Entity, &Pos, Without<Tag>)>()
        .map(|(e, _, ())| e)
        .collect();
    assert_eq!(without, vec![b]);
    assert_eq!(world.query::<(Entity, With<Health>)>().count(), 0);
    assert_eq!(world.query::<(Entity, Without<Health>)>().count(), 2);
    for (p, ()) in world.query_mut::<(&mut Pos, With<Tag>)>() {
        p.x = 100.0;
    }
    assert_eq!(world.get::<Pos>(a).unwrap().x, 100.0);
    assert_eq!(world.get::<Pos>(b).unwrap().x, 2.0);
    for (p, ()) in world.query_mut::<(&mut Pos, With<Pos>)>() {
        p.y = 0.0;
    }
}

#[test]
fn shared_duplicates_are_allowed() {
    let mut world = World::new();
    world.spawn((pos(1.0), vel(1.0)));
    assert_eq!(world.query::<(&Pos, &Pos)>().count(), 1);
    for (a, b, v) in world.query_mut::<(&Pos, &Pos, &mut Vel)>() {
        v.x = a.x + b.x;
    }
    assert_eq!(world.query::<&Vel>().next().unwrap().x, 2.0);
}

#[test]
#[should_panic(expected = "duplicate mutable access to component")]
fn duplicate_mutable_access_panics() {
    let mut world = World::new();
    world.spawn((pos(1.0),));
    let _ = world.query_mut::<(&mut Pos, &mut Pos)>();
}

#[test]
#[should_panic(expected = "mutable and shared access to component")]
fn mixed_access_panics_even_without_entities() {
    let mut world = World::new();
    let _ = world.query_mut::<(&Vel, Option<&mut Vel>)>();
}

#[test]
fn eight_element_query() {
    let mut world = World::new();
    let entity = world.spawn((pos(1.0), vel(2.0), Health { value: 3 }, Tag { id: 4 }));
    let count = world
        .query::<(
            Entity,
            &Pos,
            &Vel,
            &Health,
            Option<&Tag>,
            With<Pos>,
            Without<Counter>,
            Option<&Label>,
        )>()
        .inspect(|item| assert_eq!(item.0, entity))
        .count();
    assert_eq!(count, 1);
}

#[test]
fn wide_mutable_query_with_scrambled_ids() {
    let mut world = World::new();
    // Component ids deliberately disagree with the query's tuple order.
    world.register_component::<u16>();
    world.register_component::<Label>();
    world.register_component::<Counter>();
    world.register_component::<Health>();
    let full = world.spawn((
        Tag { id: 1 },
        2u32,
        pos(3.0),
        4u16,
        Label {
            text: "five".into(),
        },
        vel(6.0),
        Counter { value: 7 },
        Health { value: 8 },
    ));
    let partial = world.spawn((vel(60.0), pos(30.0), Health { value: 80 }));

    let mut visited = 0;
    for (p, v, h, t, c, l, n, s) in world.query_mut::<(
        &mut Pos,
        &mut Vel,
        &mut Health,
        &mut Tag,
        &mut Counter,
        &mut Label,
        &mut u32,
        &mut u16,
    )>() {
        p.x += 1.0;
        v.x += 1.0;
        h.value += 1;
        t.id += 1;
        c.value += 1;
        l.text.push('!');
        *n += 1;
        *s += 1;
        visited += 1;
    }
    assert_eq!(visited, 1);
    assert_eq!(world.get::<Pos>(full), Some(&Pos { x: 4.0, y: -3.0 }));
    assert_eq!(world.get::<Vel>(full), Some(&vel(7.0)));
    assert_eq!(world.get::<Health>(full), Some(&Health { value: 9 }));
    assert_eq!(world.get::<Tag>(full), Some(&Tag { id: 2 }));
    assert_eq!(world.get::<Counter>(full), Some(&Counter { value: 8 }));
    assert_eq!(
        world.get::<Label>(full).map(|l| l.text.as_str()),
        Some("five!")
    );
    assert_eq!(world.get::<u32>(full), Some(&3));
    assert_eq!(world.get::<u16>(full), Some(&5));
    assert_eq!(world.get::<Pos>(partial), Some(&pos(30.0)));

    let mut seen = Vec::new();
    for (s, p, h, e, l, v, c, ()) in world.query_mut::<(
        Option<&mut u16>,
        &Pos,
        &mut Health,
        Entity,
        Option<&mut Label>,
        &Vel,
        Option<&Counter>,
        With<Vel>,
    )>() {
        h.value += p.x as i32 + v.x as i32;
        if let Some(s) = s {
            *s += 10;
        }
        if let Some(l) = l {
            l.text.push('?');
        }
        seen.push((e, c.map(|c| c.value)));
    }
    assert_eq!(seen, vec![(full, Some(8)), (partial, None)]);
    assert_eq!(world.get::<Health>(full), Some(&Health { value: 20 }));
    assert_eq!(world.get::<Health>(partial), Some(&Health { value: 170 }));
    assert_eq!(world.get::<u16>(full), Some(&15));
    assert_eq!(
        world.get::<Label>(full).map(|l| l.text.as_str()),
        Some("five!?")
    );
    assert_eq!(world.get::<u16>(partial), None);
}

#[test]
fn single_optional_mutable_query() {
    let mut world = World::new();
    let with = world.spawn((pos(1.0), Tag { id: 1 }));
    let without = world.spawn((pos(2.0),));
    let mut visited = 0;
    let mut present = 0;
    for tag in world.query_mut::<Option<&mut Tag>>() {
        visited += 1;
        if let Some(tag) = tag {
            tag.id = 9;
            present += 1;
        }
    }
    assert_eq!((visited, present), (2, 1));
    assert_eq!(world.get::<Tag>(with), Some(&Tag { id: 9 }));
    assert_eq!(world.get::<Tag>(without), None);
    assert_eq!(
        world
            .query_mut::<Option<&mut Counter>>()
            .filter(Option::is_some)
            .count(),
        0,
        "unregistered optional matches every entity with None"
    );
}

#[test]
fn caught_duplicate_bundle_panic_leaves_world_unchanged() {
    #[derive(Clone)]
    struct Fresh {
        v: u8,
    }
    impl_stable_hash!(Fresh { v });

    let mut world = World::new();
    world.spawn((pos(1.0),));
    let before = hash(&world);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        world.spawn((Fresh { v: 1 }, vel(1.0), Fresh { v: 2 }));
    }));
    assert!(result.is_err());
    assert_eq!(hash(&world), before);
    assert_eq!(world.entity_count(), 1);

    let mut reference = World::new();
    reference.spawn((pos(1.0),));
    reference.spawn((vel(2.0), Fresh { v: 3 }));
    world.spawn((vel(2.0), Fresh { v: 3 }));
    assert_eq!(
        hash(&world),
        hash(&reference),
        "registration order unaffected"
    );
}

// ---------------------------------------------------------------------------------------------
// Resources

#[test]
fn resources_insert_replace_remove() {
    let mut world = World::new();
    assert_eq!(world.resource::<Counter>(), None);
    assert_eq!(world.resource_mut::<Counter>(), None);
    assert_eq!(world.remove_resource::<Counter>(), None);

    world.insert_resource(Counter { value: 1 });
    world.insert_resource(Label { text: "a".into() });
    assert_eq!(world.resource::<Counter>(), Some(&Counter { value: 1 }));
    world.resource_mut::<Counter>().unwrap().value = 2;
    world.insert_resource(Counter { value: 3 });
    assert_eq!(world.resource::<Counter>(), Some(&Counter { value: 3 }));
    assert_eq!(
        world.remove_resource::<Counter>(),
        Some(Counter { value: 3 })
    );
    assert_eq!(world.resource::<Counter>(), None);
    assert_eq!(world.remove_resource::<Counter>(), None);
}

#[test]
fn resource_order_survives_replace_and_remove() {
    let mut first = World::new();
    first.insert_resource(Counter { value: 1 });
    first.insert_resource(Label { text: "x".into() });
    first.insert_resource(Counter { value: 5 });

    let mut second = World::new();
    second.insert_resource(Counter { value: 0 });
    second.insert_resource(Label { text: "x".into() });
    second.remove_resource::<Counter>();
    second.insert_resource(Counter { value: 5 });
    assert_eq!(hash(&first), hash(&second));

    let mut swapped = World::new();
    swapped.insert_resource(Label { text: "x".into() });
    swapped.insert_resource(Counter { value: 5 });
    assert_ne!(hash(&first), hash(&swapped));

    first.remove_resource::<Label>();
    let mut absent = World::new();
    absent.insert_resource(Counter { value: 5 });
    assert_ne!(
        hash(&first),
        hash(&absent),
        "presence tag keeps the empty slot"
    );
}

// ---------------------------------------------------------------------------------------------
// Hashing and snapshots

fn reference_world() -> World {
    let mut world = World::new();
    world.register_component::<Pos>();
    world.register_component::<Vel>();
    world.register_component::<Health>();
    let a = world.spawn((pos(1.0), vel(0.25)));
    let b = world.spawn((pos(2.0), Health { value: 9 }));
    let c = world.spawn((pos(3.0), vel(-1.0)));
    world.despawn(a);
    world.insert(b, vel(4.0)).unwrap();
    world.remove::<Vel>(c);
    world.spawn((Health { value: 1 },));
    world.insert_resource(Counter { value: 77 });
    world
}

/// Golden hash of [`reference_world`] with `StableHasher` algorithm version 1.
const REFERENCE_WORLD_HASH: u64 = 1_270_145_807_866_923_928;

#[test]
fn reference_world_hash_in_first_function() {
    assert_eq!(hash(&reference_world()), REFERENCE_WORLD_HASH);
}

#[test]
fn reference_world_hash_in_second_function() {
    assert_eq!(hash(&reference_world()), REFERENCE_WORLD_HASH);
}

#[test]
fn hash_reflects_every_state_part() {
    let base = hash(&reference_world());

    let mut component_value = reference_world();
    let entity = component_value
        .query::<(Entity, &Health)>()
        .next()
        .unwrap()
        .0;
    component_value.get_mut::<Health>(entity).unwrap().value += 1;
    assert_ne!(hash(&component_value), base);

    let mut resource_value = reference_world();
    resource_value.resource_mut::<Counter>().unwrap().value += 1;
    assert_ne!(hash(&resource_value), base);

    let mut allocator = reference_world();
    let spawned = allocator.spawn(());
    allocator.despawn(spawned);
    assert_ne!(hash(&allocator), base, "generation bump is state");

    let mut registry = reference_world();
    registry.register_component::<Tag>();
    assert_ne!(hash(&registry), base);

    let mut world = World::new();
    world.insert_resource(Counter { value: 77 });
    assert_eq!(world.snapshot().clone().restore_hash(), hash(&world));
}

trait RestoreHash {
    fn restore_hash(&self) -> u64;
}

impl RestoreHash for WorldSnapshot {
    fn restore_hash(&self) -> u64 {
        let mut world = World::new();
        world.restore(self);
        hash(&world)
    }
}

#[test]
fn world_implements_stable_hash_trait() {
    let world = reference_world();
    assert_eq!(grimoire_core::hash_of(&world), hash(&world));
}

#[test]
fn snapshot_restore_reproduces_state_and_ids() {
    let mut world = reference_world();
    let snapshot = world.snapshot();
    let before = hash(&world);

    let continued_entity = world.spawn((pos(7.0), Tag { id: 7 }));
    world.insert_resource(Label {
        text: "later".into(),
    });
    let after_continue = hash(&world);

    for entity in world.query::<Entity>().collect::<Vec<_>>() {
        world.despawn(entity);
    }
    world.register_component::<Counter>();
    world.remove_resource::<Counter>();
    assert_ne!(hash(&world), before);

    world.restore(&snapshot);
    assert_eq!(hash(&world), before);
    assert_eq!(world.resource::<Counter>(), Some(&Counter { value: 77 }));
    let replayed_entity = world.spawn((pos(7.0), Tag { id: 7 }));
    world.insert_resource(Label {
        text: "later".into(),
    });
    assert_eq!(replayed_entity, continued_entity);
    assert_eq!(hash(&world), after_continue);

    let mut fresh = World::new();
    fresh.restore(&snapshot.clone());
    assert_eq!(hash(&fresh), before);
    assert!(format!("{snapshot:?}").starts_with("WorldSnapshot"));
    assert!(format!("{fresh:?}").contains("entities: 3"));
}

#[test]
fn snapshot_is_independent_of_later_mutation() {
    let mut world = World::new();
    let entity = world.spawn((Health { value: 1 },));
    let snapshot = world.snapshot();
    world.get_mut::<Health>(entity).unwrap().value = 99;
    world.restore(&snapshot);
    assert_eq!(world.get::<Health>(entity), Some(&Health { value: 1 }));
}

// ---------------------------------------------------------------------------------------------
// Commands

#[test]
fn command_buffer_applies_in_order() {
    let mut world = World::new();
    let existing = world.spawn((pos(1.0),));
    let mut commands = CommandBuffer::new();
    assert!(commands.is_empty());
    commands.insert(existing, vel(1.0));
    commands.spawn((Health { value: 5 },));
    commands.remove::<Pos>(existing);
    commands.insert(existing, pos(2.0));
    commands.despawn(existing);
    commands.spawn(());
    assert_eq!(commands.len(), 6);
    assert!(!commands.is_empty());
    assert!(format!("{commands:?}").contains('6'));
    assert!(world.is_alive(existing), "nothing applied before apply");

    commands.apply(&mut world);
    assert!(commands.is_empty());
    assert!(!world.is_alive(existing));
    assert_eq!(world.entity_count(), 2);
    assert_eq!(world.query::<&Health>().count(), 1);
    let alive: Vec<Entity> = world.query::<Entity>().collect();
    let reused = Entity::from_bits((1 << 32) | u64::from(existing.index()));
    assert!(
        alive.contains(&reused),
        "the final spawn reuses the slot freed by the despawn recorded before it"
    );
}

#[test]
fn command_buffer_matches_immediate_calls() {
    let mut immediate = World::new();
    let a = immediate.spawn((pos(1.0),));
    let mut deferred = World::new();
    let b = deferred.spawn((pos(1.0),));
    assert_eq!(a, b);

    immediate.spawn((vel(1.0),));
    immediate.insert(a, Tag { id: 1 }).unwrap();
    immediate.despawn(a);
    immediate.spawn((pos(3.0),));

    let mut commands = CommandBuffer::default();
    commands.spawn((vel(1.0),));
    commands.insert(b, Tag { id: 1 });
    commands.despawn(b);
    commands.spawn((pos(3.0),));
    commands.apply(&mut deferred);
    assert_eq!(hash(&immediate), hash(&deferred));
}

#[test]
fn commands_on_dead_entities_are_skipped() {
    let mut world = World::new();
    let dead = world.spawn((pos(1.0),));
    world.despawn(dead);
    let before = hash(&world);
    let mut commands = CommandBuffer::new();
    commands.insert(dead, vel(1.0));
    commands.remove::<Pos>(dead);
    commands.despawn(dead);
    commands.apply(&mut world);
    assert_eq!(hash(&world), before);
    assert_eq!(world.entity_count(), 0);
}

// ---------------------------------------------------------------------------------------------
// Systems

struct Doubler;

impl System for Doubler {
    fn name(&self) -> &str {
        "doubler"
    }

    fn run(&mut self, world: &mut World) {
        if let Some(counter) = world.resource_mut::<Counter>() {
            counter.value *= 2;
        }
    }
}

#[test]
fn schedule_runs_systems_in_order() {
    let mut world = World::new();
    world.insert_resource(Counter { value: 1 });
    let mut schedule = Schedule::new();
    assert!(schedule.is_empty());
    let mut runs = 0u32;
    schedule
        .add_system(system_fn("add_three", |world: &mut World| {
            world.resource_mut::<Counter>().unwrap().value += 3;
        }))
        .add_system(Doubler)
        .add_system(system_fn("count", move |_world: &mut World| {
            runs += 1;
            assert!(runs <= 2);
        }));
    assert_eq!(
        schedule.system_names(),
        vec!["add_three", "doubler", "count"]
    );
    assert_eq!(schedule.len(), 3);
    assert!(format!("{schedule:?}").contains("doubler"));

    schedule.run(&mut world);
    assert_eq!(world.resource::<Counter>(), Some(&Counter { value: 8 }));
    schedule.run(&mut world);
    assert_eq!(world.resource::<Counter>(), Some(&Counter { value: 22 }));
}

#[test]
fn system_fn_exposes_name() {
    let mut system = system_fn("spawner", |world: &mut World| {
        world.spawn(());
    });
    assert_eq!(system.name(), "spawner");
    let mut world = World::new();
    system.run(&mut world);
    assert_eq!(world.entity_count(), 1);
}
