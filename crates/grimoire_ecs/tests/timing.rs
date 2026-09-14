//! Manual timing check (PRD-0002 budget): query over 10,000 entities with two components.
//!
//! Run with `cargo test -p grimoire_ecs --release --test timing -- --ignored --nocapture`.

use std::hint::black_box;
use std::time::Duration;

use grimoire_core::impl_stable_hash;
use grimoire_ecs::World;

#[derive(Clone)]
struct Pos {
    x: f32,
    y: f32,
}
impl_stable_hash!(Pos { x, y });

#[derive(Clone)]
struct Vel {
    x: f32,
    y: f32,
}
impl_stable_hash!(Vel { x, y });

const ENTITIES: usize = 10_000;
const ROUNDS: u32 = 2_000;

// Wall-clock time only measures this benchmark; it never feeds simulation state.
#[allow(clippy::disallowed_methods)]
fn time(mut f: impl FnMut()) -> Duration {
    let start = std::time::Instant::now();
    for _ in 0..ROUNDS {
        f();
    }
    start.elapsed()
}

fn ns_per_entity(elapsed: Duration) -> f64 {
    elapsed.as_nanos() as f64 / (f64::from(ROUNDS) * ENTITIES as f64)
}

#[test]
#[ignore = "timing measurement; run manually in release mode"]
fn query_10k_entities_two_components() {
    let mut world = World::new();
    for i in 0..ENTITIES {
        let f = i as f32;
        world.spawn((Pos { x: f, y: -f }, Vel { x: 1.0, y: 0.5 }));
    }

    let write = time(|| {
        for (pos, vel) in world.query_mut::<(&mut Pos, &Vel)>() {
            pos.x += vel.x;
            pos.y += vel.y;
        }
    });
    let read = time(|| {
        let mut sum = 0.0f32;
        for (pos, vel) in world.query::<(&Pos, &Vel)>() {
            sum += pos.x * vel.x + pos.y * vel.y;
        }
        black_box(sum);
    });

    let mut baseline: Vec<(Pos, Vel)> = (0..ENTITIES)
        .map(|i| {
            let f = i as f32;
            (Pos { x: f, y: -f }, Vel { x: 1.0, y: 0.5 })
        })
        .collect();
    let raw = time(|| {
        for (pos, vel) in &mut baseline {
            pos.x += vel.x;
            pos.y += vel.y;
        }
    });

    println!(
        "query_mut (&mut Pos, &Vel): {:.3} ns/entity; query (&Pos, &Vel): {:.3} ns/entity; \
         raw Vec<(Pos, Vel)> baseline: {:.3} ns/entity",
        ns_per_entity(write),
        ns_per_entity(read),
        ns_per_entity(raw)
    );
    assert_eq!(world.query::<&Pos>().count(), ENTITIES);
}
