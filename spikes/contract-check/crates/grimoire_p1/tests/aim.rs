//! §9.4 table cases that the code block claims.

use grimoire_core::Vec2;
use grimoire_p1::quantize_aim;

#[test]
fn quantize_aim_edge_cases() {
    assert_eq!(quantize_aim(Vec2::new(f32::NAN, 0.0)), [0, 0]);
    assert_eq!(quantize_aim(Vec2::new(f32::INFINITY, 0.0)), [0, 0]);
    assert_eq!(quantize_aim(Vec2::new(0.005, 0.0)), [0, 0]);
    assert_eq!(quantize_aim(Vec2::new(1.0, 0.0)), [32767, 0]);
    assert_eq!(quantize_aim(Vec2::new(-1.0e30, 0.0)), [-32767, 0]);
    let [x, y] = quantize_aim(Vec2::new(1.0e30, 1.0e30));
    assert_eq!(x, y);
    assert!((23169..=23170).contains(&x));
    let [x, y] = quantize_aim(Vec2::new(f32::MAX, -f32::MAX));
    assert_eq!(x, -y);
}

#[test]
fn observer_conformance_is_callable_from_facade_tests() {
    let _suite: fn(&mut dyn grimoire_ecs_p1::SystemObserver) =
        grimoire_ecs_p1::conformance::system_observer;
}
