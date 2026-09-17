//! Golden hashes of the reference patterns (Plan 0002 WP5.7, `docs/formats/sigil.md` §14.1): every
//! pattern under `tests/reference/` is compiled through this compiler and run through the
//! interpreter of `grimoire_sigil` in a fixed scenario, and the compiled unit's content hash and the
//! simulation's state hashes at fixed ticks are compared with the table [`GOLDENS`]. `cargo test`
//! runs this on every push on Windows, Linux and macOS, so a pattern that behaves differently on one
//! platform, or after a change to the compiler, the unit format or the runtime, fails here.
//!
//! **The scenario** is the one `tests/reference_patterns.rs` checks the features of: seed
//! [`SEED`], a pool of [`CAPACITY`] bullets inside ±[`BOUND`] units, the aim target [`AIM`], one
//! `Emitter` entity per primary emitter (`role` `0`) at the origin started at tick 0, the event
//! `phase_end` raised once before tick [`EVENT_TICK`], and [`TICKS`] ticks, with a state hash every
//! [`CHECKPOINT_EVERY`] ticks. The behaviour `drift_orbit` of `behaviors.json` is the same stand-in
//! as there (one degree of turn per tick).
//!
//! **Independent of `QUERY_BLOCK_SIZE`.** No reference pattern draws randomness inside the update
//! blocks: the only `scatter` block belongs to a primary emitter, which draws from the per-emitter
//! stream `stream::EMIT`, and the `drift_orbit` stand-in ignores its block generator. Despawns and
//! sub-spawns are folded in block order, which is slot order for any block size. These hashes
//! therefore stay valid if the P1 bench changes the block size (contract §11.7);
//! [`reference_pattern_hashes_do_not_depend_on_the_execution_order`] additionally runs every
//! pattern with permuted block and system orders.
//!
//! **Renewing a golden** is a decision, not a fix: it happens in a separate commit after a PO
//! decision (`CONTRIBUTING.md`). A new reference pattern adds its row in the same change. The
//! table is printed, in this file's syntax, by
//! `cargo test -p grimoire_sigilc --test reference_goldens -- --ignored --nocapture print_reference_golden_table`.

mod support;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::sync::Arc;

use grimoire_core::Vec2;
use grimoire_ecs::{Executor, PermutedExecutor, SequentialExecutor};
use grimoire_sigil::{
    AimTarget, BehaviorId, BehaviorInput, BehaviorOutcome, BehaviorRegistryBuilder, BulletMotion,
    BulletPool, Emitter, EventId, EventRequest, SigilConfig, SigilLibrary, SigilUnit, install,
};
use grimoire_sigilc::behaviors::parse_behavior_manifest;
use grimoire_sigilc::compiler::{LoadError, SourceLoader, compile};
use grimoire_sim::{SimRng, Simulation, TickInput};
use support::{reference_root, reference_sigil_files};

/// Simulation seed of the scenario.
const SEED: u64 = 45;
/// Pool capacity of the scenario.
const CAPACITY: u32 = 16_384;
/// Half extent of the square despawn bounds, in units.
const BOUND: f32 = 60.0;
/// Aim target of aimed blocks.
const AIM: Vec2 = Vec2::new(0.0, -4.0);
/// The event `phase_end` is raised right before this tick is simulated.
const EVENT_TICK: u64 = 150;
/// Ticks simulated per pattern.
const TICKS: u64 = 480;
/// A state hash is taken after every tick that is a multiple of this.
const CHECKPOINT_EVERY: u64 = 120;
/// Number of checkpoints per pattern: `TICKS / CHECKPOINT_EVERY`.
const CHECKPOINTS: usize = 4;

/// The frozen result of one reference pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Golden {
    /// File name under `tests/reference/`.
    file: &'static str,
    /// `SigilUnit::content_hash` of the compiled unit.
    content_hash: u64,
    /// `Simulation::state_hash` after ticks 120, 240, 360 and 480.
    state_hashes: [u64; CHECKPOINTS],
}

/// Golden hashes of the reference patterns, sorted by file name.
const GOLDENS: &[Golden] = &[
    Golden {
        file: "01-opening-ring.sigil",
        content_hash: 0xdb1d_accb_ed38_2e28,
        state_hashes: [
            0xe439_0227_36b4_ccba,
            0xe4ca_940e_70b4_4319,
            0x9890_3000_492e_12f6,
            0x8352_be26_0028_0538,
        ],
    },
    Golden {
        file: "02-spiral-curtain.sigil",
        content_hash: 0x71ba_cb17_f568_80a4,
        state_hashes: [
            0x519a_49d4_6828_ef6d,
            0x75b3_08e8_2aee_25bf,
            0xfe77_30d6_2db3_9b64,
            0xb18d_3277_79f1_ee88,
        ],
    },
    Golden {
        file: "03-aimed-fan.sigil",
        content_hash: 0xe464_35c8_cbe3_3ab7,
        state_hashes: [
            0x8fcf_b0b7_909d_0087,
            0xfb02_7ce3_3e9a_408c,
            0x4f28_271c_bbd1_97f1,
            0x8895_abb5_cc6b_4f7d,
        ],
    },
    Golden {
        file: "04-bending-wave.sigil",
        content_hash: 0x0804_2d79_08de_041e,
        state_hashes: [
            0x2703_efb9_9b58_47cb,
            0x2249_c1fd_69a3_ef4a,
            0x3786_907e_afec_ca55,
            0x51c0_7d4c_8159_d8fa,
        ],
    },
    Golden {
        file: "05-rail-sweep.sigil",
        content_hash: 0x121e_32b9_3a95_51d0,
        state_hashes: [
            0xaf57_db5d_486c_7809,
            0x30ad_d9d6_5d87_9301,
            0x6a08_ab33_f0e7_5da5,
            0x3a90_3ff1_081d_a6d0,
        ],
    },
    Golden {
        file: "06-seeded-scatter.sigil",
        content_hash: 0x1f64_3271_9eda_41a6,
        state_hashes: [
            0xeba1_3e1e_4f15_30bd,
            0xa10f_0f17_7f7c_43b4,
            0x8a37_0edf_8e2c_dbd0,
            0xb384_8398_8d14_647e,
        ],
    },
    Golden {
        file: "07-mirror-bloom.sigil",
        content_hash: 0x1080_f54a_99f0_2df1,
        state_hashes: [
            0x026c_fa7e_fdfd_e262,
            0xba73_d9dd_fae1_c5af,
            0x46ab_0065_0bcb_d9e3,
            0xca95_92f0_2773_ae51,
        ],
    },
    Golden {
        file: "08-returning-needles.sigil",
        content_hash: 0x3aaf_afcc_b149_1d2f,
        state_hashes: [
            0xfa49_5986_7378_3ed8,
            0x0409_72e6_2c0d_a485,
            0x93c6_4069_d2e7_a778,
            0xb20a_fa4b_f83a_7509,
        ],
    },
    Golden {
        file: "09-shatter-orbs.sigil",
        content_hash: 0xf13b_ff60_6cc2_7e0c,
        state_hashes: [
            0x5a69_2873_c8c8_2cee,
            0xee6b_7998_161a_8f1c,
            0xf4ee_72e3_e7e4_9f02,
            0x5ba7_1a3a_172c_58bf,
        ],
    },
    Golden {
        file: "10-phase-shift.sigil",
        content_hash: 0xb5cd_e130_182c_8e9d,
        state_hashes: [
            0x1987_5a34_2a86_54ea,
            0xb2e3_daf7_ed1f_42b8,
            0x0937_5c35_6ad0_16fb,
            0xa42e_3f8a_c1f1_ff01,
        ],
    },
    Golden {
        file: "11-seed-bloom.sigil",
        content_hash: 0x28d5_5299_479b_03b9,
        state_hashes: [
            0x6724_7729_9a4e_de31,
            0xd3c9_4cad_c522_8974,
            0x0d52_e857_fd6c_e09f,
            0x8802_ce6c_18c3_a300,
        ],
    },
    Golden {
        file: "12-finale-composite.sigil",
        content_hash: 0x0486_475b_0c5b_c8a3,
        state_hashes: [
            0x855c_e77a_f766_19d1,
            0x305d_5d11_575e_4ad4,
            0xc251_9b78_6b77_de18,
            0x9c60_e7e8_49e9_9132,
        ],
    },
];

struct RootLoader;

impl SourceLoader for RootLoader {
    fn load(&self, path: &str) -> Result<String, LoadError> {
        fs::read_to_string(reference_root().join(path)).map_err(|_| LoadError::NotFound)
    }
}

fn behavior_ids() -> BTreeMap<String, u32> {
    let path = reference_root().join("behaviors.json");
    let text = fs::read_to_string(path).expect("tests/reference/behaviors.json");
    parse_behavior_manifest(&text).expect("the reference behaviour manifest is valid")
}

/// Compiles `source` (file `name` under `tests/reference/`) and returns the unit and its bytes.
fn compiled(name: &str, source: &str) -> (SigilUnit, Vec<u8>) {
    let output = compile(name, source, &RootLoader, &behavior_ids());
    assert!(
        output.diagnostics.is_empty(),
        "{name}: {:#?}",
        output.diagnostics
    );
    let bytes = output.bytes.expect("no diagnostics means bytes");
    let unit =
        SigilUnit::from_bytes(&bytes).unwrap_or_else(|error| panic!("{name} decodes: {error}"));
    (unit, bytes)
}

/// cos and sin of one degree: the turn per tick of `drift_orbit`.
const TURN_COS: f32 = 0.999_847_7;
const TURN_SIN: f32 = 0.017_452_406;
const TURN_RAD: f32 = 0.017_453_292;

/// The stand-in for the game behaviour `drift_orbit`, identical to `tests/reference_patterns.rs`:
/// turns the velocity by one degree per tick. It never draws from the block generator.
fn drift_orbit(
    _: &BehaviorInput<'_>,
    motion: &mut BulletMotion,
    _: &mut SimRng,
) -> BehaviorOutcome {
    let v = motion.velocity;
    motion.velocity = Vec2::new(
        v.x * TURN_COS - v.y * TURN_SIN,
        v.x * TURN_SIN + v.y * TURN_COS,
    );
    motion.angle += TURN_RAD;
    BehaviorOutcome::Keep
}

/// Whether each emitter of `unit_bytes` is primary (`role` `0`), read from the `Emitters` section
/// (`docs/formats/sigil.md` §10.2 and §10.5); the runtime keeps roles private.
fn primary_emitters(unit_bytes: &[u8]) -> Vec<bool> {
    const HEADER_LEN: usize = 40;
    const ENTRY_LEN: usize = 24;
    const EMITTERS: u32 = 3;
    const RECORD_LEN: usize = 30;
    let read_u32 =
        |at: usize| u32::from_le_bytes(unit_bytes[at..at + 4].try_into().expect("4 bytes"));
    let read_u64 =
        |at: usize| u64::from_le_bytes(unit_bytes[at..at + 8].try_into().expect("8 bytes"));
    let section_count = read_u32(HEADER_LEN) as usize;
    let Some(entry) = (0..section_count)
        .map(|index| HEADER_LEN + 4 + index * ENTRY_LEN)
        .find(|&entry| read_u32(entry) == EMITTERS)
    else {
        return Vec::new();
    };
    let start = HEADER_LEN + usize::try_from(read_u64(entry + 8)).expect("offset fits");
    let count = u16::from_le_bytes(unit_bytes[start..start + 2].try_into().expect("2 bytes"));
    (0..usize::from(count))
        .map(|index| unit_bytes[start + 2 + index * RECORD_LEN + 4] == 0)
        .collect()
}

/// Runs the scenario for `unit` with `executor` and returns the checkpoint state hashes.
fn run(
    name: &str,
    unit: &SigilUnit,
    unit_bytes: &[u8],
    executor: Arc<dyn Executor>,
) -> [u64; CHECKPOINTS] {
    let mut builder = BehaviorRegistryBuilder::new(1);
    for (behaviour, id) in behavior_ids() {
        assert_eq!(
            behaviour, "drift_orbit",
            "the golden scenario has no stand-in for behaviour `{behaviour}`"
        );
        builder
            .register(BehaviorId(id), "drift_orbit", drift_orbit)
            .expect("unique behaviour ids");
    }
    let registry = builder.build();
    let library =
        SigilLibrary::new(vec![unit.clone()], Arc::clone(&registry)).expect("library builds");
    let mut sim = Simulation::new(SEED);
    sim.world_mut().set_executor(executor);
    install(
        &mut sim,
        library,
        registry,
        SigilConfig::new(CAPACITY, Vec2::new(-BOUND, -BOUND), Vec2::new(BOUND, BOUND)),
    )
    .expect("install succeeds");
    sim.world_mut().insert_resource(AimTarget(Some(AIM)));
    let primary = primary_emitters(unit_bytes);
    assert_eq!(
        primary.len(),
        usize::from(unit.emitter_count()),
        "{name}: emitter roles"
    );
    for (index, _) in primary.iter().enumerate().filter(|(_, primary)| **primary) {
        sim.world_mut().spawn((Emitter {
            unit: unit.id(),
            emitter: u16::try_from(index).expect("emitter index fits"),
            origin: Vec2::ZERO,
            rotation: 0.0,
            started_at: 0,
        },));
    }

    let mut hashes = [0; CHECKPOINTS];
    for tick in 0..TICKS {
        if tick == EVENT_TICK {
            sim.world_mut().spawn((EventRequest {
                event: EventId::from_name("phase_end"),
            },));
        }
        sim.step(TickInput::default());
        let pool = sim.world().resource::<BulletPool>().expect("installed");
        assert_eq!(pool.dropped_spawns(), 0, "{name}: dropped spawns at {tick}");
        if sim.tick().is_multiple_of(CHECKPOINT_EVERY) {
            let index = usize::try_from(sim.tick() / CHECKPOINT_EVERY - 1).expect("fits");
            hashes[index] = sim.state_hash();
        }
    }
    hashes
}

/// The scenario's result for every reference pattern, run sequentially, sorted by file name.
fn actual_table() -> Vec<(String, u64, [u64; CHECKPOINTS])> {
    reference_sigil_files()
        .into_iter()
        .map(|(name, source)| {
            let (unit, bytes) = compiled(&name, &source);
            let hashes = run(&name, &unit, &bytes, Arc::new(SequentialExecutor));
            (name, unit.content_hash(), hashes)
        })
        .collect()
}

fn format_table(table: &[(String, u64, [u64; CHECKPOINTS])]) -> String {
    let hex = |value: u64| {
        let digits = format!("{value:016x}");
        format!(
            "0x{}_{}_{}_{}",
            &digits[..4],
            &digits[4..8],
            &digits[8..12],
            &digits[12..]
        )
    };
    let mut out = String::from("const GOLDENS: &[Golden] = &[\n");
    for (name, content_hash, hashes) in table {
        let _ = writeln!(out, "    Golden {{");
        let _ = writeln!(out, "        file: \"{name}\",");
        let _ = writeln!(out, "        content_hash: {},", hex(*content_hash));
        let _ = writeln!(out, "        state_hashes: [");
        for hash in hashes {
            let _ = writeln!(out, "            {},", hex(*hash));
        }
        let _ = writeln!(out, "        ],");
        let _ = writeln!(out, "    }},");
    }
    out.push_str("];\n");
    out
}

#[test]
fn every_reference_pattern_has_exactly_one_golden_row() {
    let files: Vec<String> = reference_sigil_files()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let rows: Vec<&str> = GOLDENS.iter().map(|golden| golden.file).collect();
    assert!(files.len() >= 12, "only {} reference patterns", files.len());
    assert_eq!(
        rows, files,
        "the golden table must list every reference pattern once, sorted by file name"
    );
}

#[test]
fn reference_patterns_match_their_golden_hashes() {
    let actual = actual_table();
    let mismatches: Vec<String> = actual
        .iter()
        .filter_map(|(name, content_hash, hashes)| {
            let golden = GOLDENS.iter().find(|golden| golden.file == name)?;
            let mut problems = Vec::new();
            if golden.content_hash != *content_hash {
                problems.push(format!(
                    "content hash {content_hash:#018x}, golden {:#018x}",
                    golden.content_hash
                ));
            }
            for (index, (hash, frozen)) in hashes.iter().zip(golden.state_hashes).enumerate() {
                if *hash != frozen {
                    let tick = (index as u64 + 1) * CHECKPOINT_EVERY;
                    problems.push(format!(
                        "state hash at tick {tick} {hash:#018x}, golden {frozen:#018x}"
                    ));
                }
            }
            (!problems.is_empty()).then(|| format!("{name}: {}", problems.join("; ")))
        })
        .collect();
    assert!(
        mismatches.is_empty(),
        "reference pattern goldens differ (renewing them needs a PO decision):\n{}\nactual table:\n{}",
        mismatches.join("\n"),
        format_table(&actual)
    );
}

#[test]
fn reference_pattern_hashes_do_not_depend_on_the_execution_order() {
    for (name, source) in reference_sigil_files() {
        let (unit, bytes) = compiled(&name, &source);
        let sequential = run(&name, &unit, &bytes, Arc::new(SequentialExecutor));
        for executor in [
            Arc::new(PermutedExecutor::reversed()) as Arc<dyn Executor>,
            Arc::new(PermutedExecutor::new(0x5EED)),
        ] {
            assert_eq!(
                run(&name, &unit, &bytes, executor),
                sequential,
                "{name}: the state hashes depend on the execution order"
            );
        }
    }
}

#[test]
#[ignore = "prints the golden table for review; renewing it needs a PO decision (module docs)"]
fn print_reference_golden_table() {
    println!("{}", format_table(&actual_table()));
}
