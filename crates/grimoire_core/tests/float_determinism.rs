//! Cross-platform floating-point determinism probe (spike OF-2.1, engine ADR 0004).
//!
//! Inputs come from an integer LCG; floats are assembled from raw bits, so input generation
//! cannot diverge between platforms. Every result is fed into a [`StableHasher`] (bit-exact,
//! NaN canonicalised).
//!
//! - basic IEEE-754 operations, casts and `Vec2`: asserted against [`BASIC_GOLDEN`];
//! - every `dmath` function over its domain plus edge values: asserted against [`DMATH_GOLDEN`];
//! - a small particle simulation: asserted against [`MINI_SIM_GOLDEN`];
//! - `std` transcendental functions (platform libm): only recorded, never asserted;
//! - NaN bit patterns and signed-zero `f32::min`/`max`: only recorded.
//!
//! Probe files, which CI uploads:
//! - `core-<os>-<arch>.txt` (`basic_hash`, `dmath_hash`) and `std-trig-<os>-<arch>.txt`
//!   (`std_trig_hash`). Their names carry no build profile, so a later run in the same directory
//!   overwrites them; `core` is profile-independent as long as the goldens hold.
//! - `detail-<os>-<arch>-<profile>.txt` (profile, all hashes, per-function hashes, std-vs-dmath
//!   differences) and `special-<os>-<arch>-<profile>.txt` (NaN bits, signed-zero `min`/`max`),
//!   which do differ between profiles. `<profile>` is `debug` with debug assertions enabled,
//!   otherwise `release`.
//!
//! `<os>` and `<arch>` are `std::env::consts::{OS, ARCH}`. The directory is, in this order:
//! 1. `GRIMOIRE_FLOAT_PROBE_DIR` at run time, so CI can pin the upload path;
//! 2. `<build dir>/float-probe`, where `<build dir>` is the parent of `CARGO_TARGET_TMPDIR`. That is
//!    the target directory (honouring `CARGO_TARGET_DIR`) unless `build.build-dir` /
//!    `CARGO_BUILD_BUILD_DIR` moves it elsewhere;
//! 3. `CARGO_MANIFEST_DIR/../../target/float-probe` if Cargo did not set `CARGO_TARGET_TMPDIR`.
//!
//! Run with `cargo test -p grimoire_core --no-fail-fast`: without it, a failing unit test stops
//! Cargo before this binary writes the files that explain the failure.

use std::fmt::Write as _;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use grimoire_core::math::dmath;
use grimoire_core::{StableHasher, Vec2, hash_of};

const ITERATIONS: u32 = 100_000;

/// Golden hash of [`basic_workload`].
const BASIC_GOLDEN: u64 = 0xd596_5d26_f7f4_27f2;
/// Golden hash of [`dmath_results`].
const DMATH_GOLDEN: u64 = 0x7447_1203_26b9_c11f;
/// Golden hash of [`mini_simulation`].
const MINI_SIM_GOLDEN: u64 = 0xcf8c_7a49_5815_0c94;

const EDGE_VALUES: [f32; 28] = [
    0.0,
    -0.0,
    1.0,
    -1.0,
    0.5,
    -0.5,
    2.0,
    -2.0,
    f32::from_bits(0x3f7f_ffff), // largest float below 1
    f32::from_bits(0x3f80_0001), // smallest float above 1
    dmath::FRAC_PI_2,
    -dmath::FRAC_PI_2,
    dmath::PI,
    -dmath::PI,
    dmath::TAU,
    100.0,
    -100.0,
    1.0e6,
    -1.0e6,
    3.0e38,
    f32::MAX,
    f32::MIN,
    f32::MIN_POSITIVE,
    f32::from_bits(0x0000_0001), // smallest positive subnormal
    f32::from_bits(0x8000_0001), // smallest negative subnormal
    f32::INFINITY,
    f32::NEG_INFINITY,
    f32::NAN,
];

/// 64-bit LCG (Knuth's MMIX constants); outputs take the high state bits.
struct Lcg(u64);

impl Lcg {
    const fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 32) as u32
    }

    fn below(&mut self, bound: u32) -> u32 {
        ((u64::from(self.next_u32()) * u64::from(bound)) >> 32) as u32
    }

    /// Uniform integer in `low..=high`.
    fn int_in(&mut self, low: i32, high: i32) -> i32 {
        let span = u32::try_from(high - low + 1).expect("low <= high");
        low + i32::try_from(self.below(span)).expect("span fits i32")
    }

    /// Finite normal float with magnitude in `[2^min_exp, 2^(max_exp + 1))`.
    fn float(&mut self, min_exp: i32, max_exp: i32, signed: bool) -> f32 {
        let biased = u32::try_from(self.int_in(min_exp, max_exp) + 127).expect("normal exponent");
        assert!(
            (1..=254).contains(&biased),
            "exponent outside the normal range"
        );
        let mantissa = self.next_u32() >> 9;
        let sign = if signed { self.next_u32() >> 31 } else { 0 };
        f32::from_bits((sign << 31) | (biased << 23) | mantissa)
    }

    /// Like [`Lcg::float`], but one draw in eight is an [`EDGE_VALUES`] entry.
    fn float_or_edge(&mut self, min_exp: i32, max_exp: i32, signed: bool) -> f32 {
        if self.below(8) == 0 {
            EDGE_VALUES[self.below(EDGE_VALUES.len() as u32) as usize]
        } else {
            self.float(min_exp, max_exp, signed)
        }
    }

    /// Positive subnormal or zero.
    fn subnormal(&mut self) -> f32 {
        f32::from_bits(self.next_u32() >> 9)
    }
}

fn feed_vec2(hasher: &mut StableHasher, value: Vec2) {
    hasher.write_f32(value.x);
    hasher.write_f32(value.y);
}

/// `f32::min`/`max` leave the sign of a zero result unspecified when both operands are zeros;
/// it changes even with the optimisation level. Equal operands are therefore fed sign-free.
#[allow(clippy::disallowed_methods)] // Measures std min/max for unequal operands on purpose.
fn min_max(a: f32, b: f32) -> [f32; 2] {
    if a == b {
        let zero_sign_free = a + 0.0;
        [zero_sign_free, zero_sign_free]
    } else {
        [a.min(b), a.max(b)]
    }
}

fn basic_workload() -> u64 {
    let mut rng = Lcg::new(0x0001_0001);
    let mut hasher = StableHasher::new();
    for _ in 0..ITERATIONS {
        let a = rng.float_or_edge(-24, 24, true);
        let b = rng.float_or_edge(-24, 24, true);
        for value in [
            a + b,
            a - b,
            a * b,
            a / b,
            a % b,
            -a,
            a.sqrt(),
            a.abs(),
            a.floor(),
            a.ceil(),
            a.round(),
            a.trunc(),
        ] {
            hasher.write_f32(value);
        }
        for value in min_max(a, b) {
            hasher.write_f32(value);
        }

        let int = rng.next_u32() as i32;
        hasher.write_i32(a as i32);
        hasher.write_i32(b as i32);
        hasher.write_f32(int as f32);
        hasher.write_i32(int as f32 as i32);
        hasher.write_f32(rng.int_in(-16_777_216, 16_777_216) as f32);

        let u = if rng.below(64) == 0 {
            Vec2::ZERO
        } else {
            Vec2::new(rng.float_or_edge(-12, 12, true), rng.float(-12, 12, true))
        };
        let v = Vec2::new(rng.float(-12, 12, true), rng.float_or_edge(-12, 12, true));
        let scale = rng.float(-6, 6, true);
        let t = rng.float(-24, -1, false);
        for value in [
            u + v,
            u - v,
            u * scale,
            u / scale,
            -u,
            u.perp(),
            u.normalize_or_zero(),
            v.normalize_or_zero(),
            u.lerp(v, t),
        ] {
            feed_vec2(&mut hasher, value);
        }
        for value in [
            u.dot(v),
            u.perp_dot(v),
            u.length(),
            u.length_squared(),
            u.distance(v),
            u.distance_squared(v),
        ] {
            hasher.write_f32(value);
        }
        let mut w = u;
        w += v;
        w -= u * t;
        w *= scale;
        feed_vec2(&mut hasher, w);
    }
    hasher.finish()
}

type Unary = fn(f32) -> f32;
type Binary = fn(f32, f32) -> f32;

fn angle_input(rng: &mut Lcg) -> f32 {
    rng.float_or_edge(-20, 12, true)
}

fn unit_input(rng: &mut Lcg) -> f32 {
    rng.float_or_edge(-24, -1, true)
}

fn wide_input(rng: &mut Lcg) -> f32 {
    rng.float_or_edge(-30, 30, true)
}

fn exp_input(rng: &mut Lcg) -> f32 {
    rng.float_or_edge(-20, 6, true)
}

fn positive_input(rng: &mut Lcg) -> f32 {
    if rng.below(16) == 0 {
        rng.subnormal()
    } else {
        rng.float_or_edge(-40, 40, false)
    }
}

fn atan2_input(rng: &mut Lcg) -> (f32, f32) {
    (
        rng.float_or_edge(-20, 20, true),
        rng.float_or_edge(-20, 20, true),
    )
}

fn wide_pair_input(rng: &mut Lcg) -> (f32, f32) {
    (wide_input(rng), wide_input(rng))
}

/// Mostly distinct operands, with frequent equal pairs and zeros of opposite sign.
fn min_max_input(rng: &mut Lcg) -> (f32, f32) {
    match rng.below(8) {
        0 => (0.0, -0.0),
        1 => (-0.0, 0.0),
        2 => {
            let x = wide_input(rng);
            (x, x)
        }
        _ => wide_pair_input(rng),
    }
}

fn powf_input(rng: &mut Lcg) -> (f32, f32) {
    if rng.below(8) == 0 {
        // Negative base with an integral exponent has a real result.
        let base = f32::from_bits(rng.float(-4, 4, false).to_bits() | 0x8000_0000);
        (base, rng.int_in(-10, 10) as f32)
    } else {
        (
            rng.float_or_edge(-8, 8, false),
            rng.float_or_edge(-8, 4, true),
        )
    }
}

fn run_unary(seed: u64, input: fn(&mut Lcg) -> f32, f: Unary) -> Vec<f32> {
    let mut results: Vec<f32> = EDGE_VALUES.iter().map(|&x| f(x)).collect();
    let mut rng = Lcg::new(seed);
    results.extend((0..ITERATIONS).map(|_| f(input(&mut rng))));
    results
}

fn run_binary(seed: u64, input: fn(&mut Lcg) -> (f32, f32), f: Binary) -> Vec<f32> {
    let mut results: Vec<f32> = EDGE_VALUES
        .iter()
        .flat_map(|&x| EDGE_VALUES.iter().map(move |&y| f(x, y)))
        .collect();
    let mut rng = Lcg::new(seed);
    results.extend((0..ITERATIONS).map(|_| {
        let (x, y) = input(&mut rng);
        f(x, y)
    }));
    results
}

fn run_vec2_trig(seed: u64) -> Vec<f32> {
    let mut rng = Lcg::new(seed);
    let mut results = Vec::new();
    for _ in 0..ITERATIONS {
        let angle = angle_input(&mut rng);
        let v = Vec2::new(wide_input(&mut rng), wide_input(&mut rng));
        let from_angle = Vec2::from_angle(angle);
        let rotated = v.rotate(angle);
        results.extend([from_angle.x, from_angle.y, rotated.x, rotated.y, v.angle()]);
    }
    results
}

fn hash_values(values: &[f32]) -> u64 {
    let mut hasher = StableHasher::new();
    for &value in values {
        hasher.write_f32(value);
    }
    hasher.finish()
}

fn combine(entries: &[(&'static str, Vec<f32>)]) -> u64 {
    let mut hasher = StableHasher::new();
    for (name, values) in entries {
        hasher.write_str(name);
        hasher.write_u64(hash_values(values));
    }
    hasher.finish()
}

fn dmath_results() -> &'static [(&'static str, Vec<f32>)] {
    static RESULTS: OnceLock<Vec<(&'static str, Vec<f32>)>> = OnceLock::new();
    RESULTS.get_or_init(|| {
        vec![
            ("sin", run_unary(11, angle_input, dmath::sin)),
            ("cos", run_unary(12, angle_input, dmath::cos)),
            ("tan", run_unary(13, angle_input, dmath::tan)),
            ("asin", run_unary(14, unit_input, dmath::asin)),
            ("acos", run_unary(15, unit_input, dmath::acos)),
            ("atan", run_unary(16, wide_input, dmath::atan)),
            ("atan2", run_binary(17, atan2_input, dmath::atan2)),
            ("exp", run_unary(18, exp_input, dmath::exp)),
            ("ln", run_unary(19, positive_input, dmath::ln)),
            ("powf", run_binary(20, powf_input, dmath::powf)),
            ("hypot", run_binary(21, wide_pair_input, dmath::hypot)),
            ("sqrt", run_unary(22, positive_input, dmath::sqrt)),
            ("vec2_trig", run_vec2_trig(23)),
            ("min", run_binary(24, min_max_input, dmath::min)),
            ("max", run_binary(25, min_max_input, dmath::max)),
        ]
    })
}

fn basic_hash() -> u64 {
    static HASH: OnceLock<u64> = OnceLock::new();
    *HASH.get_or_init(basic_workload)
}

fn dmath_hash() -> u64 {
    combine(dmath_results())
}

/// Same inputs (seeds and generators) as the matching `dmath` entries.
#[allow(clippy::disallowed_methods)] // The probe measures the platform libm on purpose.
fn std_trig_results() -> Vec<(&'static str, Vec<f32>)> {
    vec![
        ("sin", run_unary(11, angle_input, f32::sin)),
        ("cos", run_unary(12, angle_input, f32::cos)),
        ("tan", run_unary(13, angle_input, f32::tan)),
        ("atan2", run_binary(17, atan2_input, f32::atan2)),
        ("exp", run_unary(18, exp_input, f32::exp)),
        ("ln", run_unary(19, positive_input, f32::ln)),
        ("powf", run_binary(20, powf_input, f32::powf)),
        ("hypot", run_binary(21, wide_pair_input, f32::hypot)),
    ]
}

/// Particle update loop using the operations gameplay code typically needs.
fn mini_simulation() -> u64 {
    const PARTICLES: usize = 256;
    const TICKS: u32 = 400;
    const BOUND: f32 = 100.0;
    let dt = 1.0 / 60.0;
    let mut rng = Lcg::new(0x5eed);
    let mut particles: Vec<(Vec2, Vec2, f32)> = (0..PARTICLES)
        .map(|_| {
            let position = Vec2::new(rng.float(-8, 6, true), rng.float(-8, 6, true));
            let heading = rng.float(-8, 2, true);
            let speed = rng.float(-2, 4, false);
            (position, Vec2::from_angle(heading) * speed, heading)
        })
        .collect();

    let mut hasher = StableHasher::new();
    for tick in 0..TICKS {
        for (position, velocity, spin) in &mut particles {
            let to_center = (-*position).normalize_or_zero();
            let steer = to_center * velocity.length();
            *velocity = velocity.lerp(steer, 0.02).rotate(*spin * dt);
            *velocity *= dmath::exp(-0.05 * dt);
            *position += *velocity * (dt * 10.0);
            if position.x.abs() > BOUND {
                velocity.x = -velocity.x;
                position.x = position.x.clamp(-BOUND, BOUND);
            }
            if position.y.abs() > BOUND {
                velocity.y = -velocity.y;
                position.y = position.y.clamp(-BOUND, BOUND);
            }
            let heading = dmath::atan2(velocity.y, velocity.x);
            *spin = dmath::sin(heading + tick as f32 * 0.01) * dmath::hypot(position.x, 1.0).sqrt();
        }
        if tick % 10 == 9 {
            for &(position, velocity, spin) in &particles {
                feed_vec2(&mut hasher, position);
                feed_vec2(&mut hasher, velocity);
                hasher.write_f32(spin);
            }
        }
    }
    hasher.finish()
}

fn probe_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("GRIMOIRE_FLOAT_PROBE_DIR").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    let build_dir = match option_env!("CARGO_TARGET_TMPDIR") {
        Some(tmp) => Path::new(tmp)
            .parent()
            .map_or_else(|| PathBuf::from(tmp), Path::to_path_buf),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"),
    };
    build_dir.join("float-probe")
}

const PROFILE: &str = if cfg!(debug_assertions) {
    "debug"
} else {
    "release"
};

/// Whether a probe file name carries [`PROFILE`].
#[derive(Clone, Copy)]
enum Naming {
    Platform,
    PlatformAndProfile,
}

fn write_probe(kind: &str, naming: Naming, contents: &str) {
    let dir = probe_dir();
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|error| panic!("cannot create {}: {error}", dir.display()));
    let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let file = dir.join(match naming {
        Naming::Platform => format!("{kind}-{platform}.txt"),
        Naming::PlatformAndProfile => format!("{kind}-{platform}-{PROFILE}.txt"),
    });
    std::fs::write(&file, contents)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", file.display()));
    println!("wrote {}", file.display());
}

fn same_result(a: f32, b: f32) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

#[test]
fn basic_ieee_operations_match_golden() {
    let actual = basic_hash();
    println!("basic_hash=0x{actual:016x}");
    assert_eq!(
        actual, BASIC_GOLDEN,
        "basic_hash=0x{actual:016x} differs from golden 0x{BASIC_GOLDEN:016x}"
    );
}

#[test]
fn dmath_functions_match_golden() {
    let actual = dmath_hash();
    let details: String = dmath_results()
        .iter()
        .map(|(name, values)| format!("dmath_{name}=0x{:016x}\n", hash_values(values)))
        .collect();
    println!("dmath_hash=0x{actual:016x}\n{details}");
    assert_eq!(
        actual, DMATH_GOLDEN,
        "dmath_hash=0x{actual:016x} differs from golden 0x{DMATH_GOLDEN:016x}; per function:\n{details}"
    );
}

#[test]
fn mini_simulation_matches_golden() {
    let actual = mini_simulation();
    println!("mini_sim_hash=0x{actual:016x}");
    assert_eq!(
        actual, MINI_SIM_GOLDEN,
        "mini_sim_hash=0x{actual:016x} differs from golden 0x{MINI_SIM_GOLDEN:016x}"
    );
}

#[test]
fn core_probe_file_is_written() {
    write_probe(
        "core",
        Naming::Platform,
        &format!(
            "basic_hash=0x{:016x}\ndmath_hash=0x{:016x}\n",
            basic_hash(),
            dmath_hash()
        ),
    );
}

#[test]
fn std_transcendental_probe_is_recorded() {
    let std_results = std_trig_results();
    let std_hash = combine(&std_results);
    println!("std_trig_hash=0x{std_hash:016x}");
    write_probe(
        "std-trig",
        Naming::Platform,
        &format!("std_trig_hash=0x{std_hash:016x}\n"),
    );

    let mut detail = format!(
        "profile={PROFILE}\nbasic_hash=0x{:016x}\ndmath_hash=0x{:016x}\nmini_sim_hash=0x{:016x}\nstd_trig_hash=0x{std_hash:016x}\n",
        basic_hash(),
        dmath_hash(),
        mini_simulation()
    );
    for (name, values) in dmath_results() {
        writeln!(detail, "dmath_{name}=0x{:016x}", hash_values(values)).expect("write to String");
    }
    for (name, std_values) in &std_results {
        let (_, dmath_values) = dmath_results()
            .iter()
            .find(|(dmath_name, _)| dmath_name == name)
            .expect("every std probe has a dmath counterpart");
        let differing = std_values
            .iter()
            .zip(dmath_values)
            .filter(|&(&a, &b)| !same_result(a, b))
            .count();
        writeln!(
            detail,
            "std_{name}=0x{:016x} differs_from_dmath={differing}/{}",
            hash_values(std_values),
            std_values.len()
        )
        .expect("write to String");
    }
    print!("{detail}");
    write_probe("detail", Naming::PlatformAndProfile, &detail);
}

#[test]
#[allow(clippy::zero_divided_by_zero)] // The constant-folded NaN is part of the investigation.
#[allow(clippy::disallowed_methods)] // Records the unspecified zero sign of std min/max.
fn nan_bit_patterns_are_recorded_and_hash_canonically() {
    const FOLDED_ZERO_DIV_ZERO: f32 = 0.0 / 0.0;
    let zero = black_box(0.0f32);
    let infinity = black_box(f32::INFINITY);
    let minus_one = black_box(-1.0f32);
    let nans_f32 = [
        ("zero_div_zero", zero / zero),
        ("inf_minus_inf", infinity - infinity),
        ("sqrt_minus_one", minus_one.sqrt()),
        ("zero_mul_inf", zero * infinity),
        ("neg_zero_div_zero", -(zero / zero)),
        ("folded_zero_div_zero", FOLDED_ZERO_DIV_ZERO),
        ("std_nan_const", f32::NAN),
        ("dmath_ln_minus_one", dmath::ln(minus_one)),
        ("dmath_asin_two", dmath::asin(black_box(2.0))),
        ("dmath_sqrt_minus_one", dmath::sqrt(minus_one)),
    ];
    let zero_f64 = black_box(0.0f64);
    let nans_f64 = [
        ("f64_zero_div_zero", zero_f64 / zero_f64),
        ("f64_sqrt_minus_one", black_box(-1.0f64).sqrt()),
    ];

    let mut report = format!("profile={PROFILE}\n");
    for (name, value) in nans_f32 {
        writeln!(report, "{name}=0x{:08x}", value.to_bits()).expect("write to String");
        assert!(value.is_nan(), "{name} is not NaN");
        assert_eq!(
            hash_of(&value),
            hash_of(&f32::NAN),
            "{name} hashes differently"
        );
    }
    for (name, value) in nans_f64 {
        writeln!(report, "{name}=0x{:016x}", value.to_bits()).expect("write to String");
        assert!(value.is_nan(), "{name} is not NaN");
        assert_eq!(
            hash_of(&value),
            hash_of(&f64::NAN),
            "{name} hashes differently"
        );
    }

    let positive = black_box(0.0f32);
    let negative = black_box(-0.0f32);
    for (name, value) in [
        ("min_pos_neg", positive.min(negative)),
        ("min_neg_pos", negative.min(positive)),
        ("max_pos_neg", positive.max(negative)),
        ("max_neg_pos", negative.max(positive)),
    ] {
        writeln!(report, "{name}_zero=0x{:08x}", value.to_bits()).expect("write to String");
    }
    print!("{report}");
    write_probe("special", Naming::PlatformAndProfile, &report);
}
