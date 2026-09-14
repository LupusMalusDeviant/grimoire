//! Golden values for [`StableHasher`] algorithm version 1.
//!
//! The constants were measured once and freeze the algorithm: they must match on every platform
//! (Windows x86_64, Linux x86_64, macOS arm64) and every Rust release. A mismatch means the
//! algorithm changed; that requires bumping `StableHasher::ALGORITHM_VERSION` and renewing every
//! stored golden master (engine ADR 0004).

use grimoire_core::{StableHash, StableHasher, Vec2, hash_of, impl_stable_hash};

struct Probe {
    id: u32,
    position: Vec2,
    tags: Vec<String>,
    target: Option<u16>,
}
impl_stable_hash!(Probe {
    id,
    position,
    tags,
    target
});

fn seeded_hash<T: StableHash>(seed: u64, value: &T) -> u64 {
    let mut hasher = StableHasher::with_seed(seed);
    value.stable_hash(&mut hasher);
    hasher.finish()
}

/// Compares every case and reports all mismatches at once, formatted for pasting back.
fn check(cases: &[(&str, u64, u64)]) {
    let mismatches: Vec<String> = cases
        .iter()
        .filter(|(_, actual, expected)| actual != expected)
        .map(|(name, actual, expected)| {
            format!("{name}: actual 0x{actual:016x}, expected 0x{expected:016x}")
        })
        .collect();
    assert!(
        mismatches.is_empty(),
        "stable hash v1 golden mismatch ({} of {}):\n{}",
        mismatches.len(),
        cases.len(),
        mismatches.join("\n")
    );
}

#[test]
fn algorithm_version_is_one() {
    assert_eq!(StableHasher::ALGORITHM_VERSION, 1);
}

#[test]
fn empty_hasher() {
    check(&[
        ("new", StableHasher::new().finish(), 0xe220a8397b1dcdaf),
        (
            "with_seed",
            StableHasher::with_seed(0xfeed).finish(),
            0x3365e73ff6c1e17b,
        ),
        ("unit", hash_of(&()), 0xe220a8397b1dcdaf),
    ]);
}

#[test]
fn integers() {
    check(&[
        ("u8 0", hash_of(&0u8), 0x910a2dec89025cc1),
        ("u8 max", hash_of(&u8::MAX), 0x7eeb92d99f03c4b8),
        ("u16", hash_of(&0xbeefu16), 0xa4402e2ad8fb3e14),
        ("u32", hash_of(&0xdead_beefu32), 0xb7b580cd76497f6d),
        (
            "u64",
            hash_of(&0x0123_4567_89ab_cdefu64),
            0x32ee8c3e95111d89,
        ),
        ("u64 max", hash_of(&u64::MAX), 0xa27426695db18009),
        ("usize", hash_of(&42usize), 0x0bae66064dd79257),
        ("i8 -1", hash_of(&-1i8), 0x7eeb92d99f03c4b8),
        ("i8 min", hash_of(&i8::MIN), 0xb794c59095ffcd9b),
        ("i16", hash_of(&-12_345i16), 0x98e669321dc88a20),
        ("i32 min", hash_of(&i32::MIN), 0x32c2260124997bfb),
        ("i64 -1", hash_of(&-1i64), 0xa27426695db18009),
        ("isize", hash_of(&-42isize), 0xf4f25f23310460e3),
        ("bool true", hash_of(&true), 0xfbcf090ca3749c94),
        ("bool false", hash_of(&false), 0x910a2dec89025cc1),
        ("char", hash_of(&'\u{0416}'), 0x416975cd6ddee8fa),
    ]);
}

#[test]
fn widths_are_distinguished() {
    // Same numeric value, different width: the length counter must separate them.
    let hashes = [
        hash_of(&1u8),
        hash_of(&1u16),
        hash_of(&1u32),
        hash_of(&1u64),
    ];
    for (i, a) in hashes.iter().enumerate() {
        for b in &hashes[i + 1..] {
            assert_ne!(a, b);
        }
    }
}

#[test]
fn floats() {
    check(&[
        ("f32 0.0", hash_of(&0.0f32), 0x6e73e372e2338aca),
        ("f32 -0.0", hash_of(&-0.0f32), 0x32c2260124997bfb),
        ("f32 1.0", hash_of(&1.0f32), 0x4fea4163dd816f93),
        ("f32 -1.5", hash_of(&-1.5f32), 0x3c1ec051dc91d174),
        ("f32 inf", hash_of(&f32::INFINITY), 0x730a6d2a231fcc4d),
        ("f32 -inf", hash_of(&f32::NEG_INFINITY), 0x2f25b948f8e019f0),
        (
            "f32 subnormal",
            hash_of(&f32::from_bits(1)),
            0x85fa10ebed4a0cb9,
        ),
        (
            "f32 min positive",
            hash_of(&f32::MIN_POSITIVE),
            0x691073141a936038,
        ),
        ("f32 max", hash_of(&f32::MAX), 0x6741855891bda0f8),
        ("f64 0.0", hash_of(&0.0f64), 0x9e5651b0ef953636),
        ("f64 -0.0", hash_of(&-0.0f64), 0x34a13c23ddea78d2),
        ("f64 inf", hash_of(&f64::INFINITY), 0x59fab84208eeec32),
        ("f64 -inf", hash_of(&f64::NEG_INFINITY), 0x59bef60ad90a2563),
        (
            "f64 subnormal",
            hash_of(&f64::from_bits(1)),
            0xd5849f23b37b6acd,
        ),
        (
            "f64 pi",
            hash_of(&core::f64::consts::PI),
            0x342a344f6a50a3d3,
        ),
    ]);
}

#[test]
fn strings() {
    check(&[
        ("len 0", hash_of(""), 0x9e5651b0ef953636),
        ("len 1", hash_of("a"), 0x3aac921ad1ecc6dc),
        ("len 7", hash_of("abcdefg"), 0x69866da172d4fcb7),
        ("len 8", hash_of("abcdefgh"), 0x5ab5bbb697220222),
        ("len 9", hash_of("abcdefghi"), 0x8f531c0bbc885e82),
        ("len 16", hash_of("0123456789abcdef"), 0x8ada947df66d659a),
        ("utf8", hash_of("gr\u{fc}\u{df}e"), 0x8ad9be402847cf2c),
        (
            "String",
            hash_of(&String::from("abcdefgh")),
            0x5ab5bbb697220222,
        ),
    ]);
    assert_eq!(hash_of("abcdefgh"), hash_of(&String::from("abcdefgh")));
}

#[test]
fn sequences_and_options() {
    let nested: Vec<Vec<u8>> = vec![vec![], vec![1], vec![2, 3]];
    let nested_floats: Vec<Vec<f32>> = vec![vec![0.5, -0.0], vec![], vec![f32::INFINITY]];
    let deep: Vec<Vec<Vec<u16>>> = vec![vec![vec![1, 2], vec![]], vec![vec![3]]];
    check(&[
        ("empty vec", hash_of(&Vec::<u32>::new()), 0x9e5651b0ef953636),
        ("nested vec", hash_of(&nested), 0x9c0784b23a46700f),
        (
            "nested float vec",
            hash_of(&nested_floats),
            0x2cc58ffc8d86fb7d,
        ),
        ("deep vec", hash_of(&deep), 0xd31be98dc7a7dc05),
        ("slice", hash_of(&[1u32, 2, 3][..]), 0xf90d0270fab6640f),
        ("array", hash_of(&[1u32, 2, 3]), 0x07d40c1e46f9a21a),
        ("option none", hash_of(&None::<u32>), 0x910a2dec89025cc1),
        ("option some", hash_of(&Some(7u32)), 0x872b606fa8b620f0),
        ("box", hash_of(&Box::new(7u32)), 0x02546f649d2c9a60),
        (
            "vec of strings",
            hash_of(&vec!["a", "", "abcdefghi"]),
            0xefdabac668cfbfc7,
        ),
    ]);
}

#[test]
fn tuples() {
    check(&[
        ("1-tuple", hash_of(&(1u8,)), 0xfbcf090ca3749c94),
        (
            "mixed 4-tuple",
            hash_of(&(1u8, -2i16, 3.5f32, "x")),
            0xffc08e0ce0857026,
        ),
        (
            "8-tuple",
            hash_of(&(1u8, 2u16, 3u32, 4u64, -5i8, -6i16, -7i32, -8i64)),
            0x8035756fa51bd8c6,
        ),
        (
            "nested tuple",
            hash_of(&((1u32, 2u32), (3u32,), ())),
            0x07d40c1e46f9a21a,
        ),
    ]);
}

#[test]
fn vec2_and_structs() {
    let probe = Probe {
        id: 17,
        position: Vec2::new(-3.25, 1e-3),
        tags: vec!["boss".into(), String::new()],
        target: Some(4),
    };
    check(&[
        ("vec2 zero", hash_of(&Vec2::ZERO), 0x9e5651b0ef953636),
        ("vec2", hash_of(&Vec2::new(1.0, -2.0)), 0xbfe4aa4dab03e3a9),
        (
            "vec2 neg zero",
            hash_of(&Vec2::new(-0.0, 0.0)),
            0x6a870219356b12a5,
        ),
        (
            "vec of vec2",
            hash_of(&vec![Vec2::X, Vec2::Y, Vec2::splat(0.1)]),
            0x3b581d09bfc8acc1,
        ),
        ("struct macro", hash_of(&probe), 0x8685daaec605285a),
        (
            "seeded",
            seeded_hash(0xfeed, &0x0123_4567_89ab_cdefu64),
            0xf7aed3244168eeeb,
        ),
    ]);
}
