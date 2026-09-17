//! Reference patterns (Plan 0002 WP4.5, `docs/formats/sigil.md` §14): at least twelve Sigil
//! patterns under `tests/reference/` that together use every building block, every modifier, every
//! transformation and every trigger at least once. They are documentation, test cases and editor
//! examples at once, so this file checks all three roles:
//!
//! - **They compile** without a diagnostic, through the library and through `sigilc check`, with
//!   the behaviour manifest next to them.
//! - **They run.** Each compiled unit is installed in a simulation, its primary emitters fire, an
//!   aim target is set and the event `phase_end` is raised once. Every bullet type of the unit must
//!   appear, no spawn may be dropped, and the transforms, the sub-emitter and the behaviour must
//!   visibly act (burst and become_emitter despawn bullets, reverse turns bullets inwards, the
//!   behaviour turns a bullet faster than any modifier of the pattern does).
//! - **The coverage table is true.** The table in the format document lists, per pattern, the
//!   blocks, modifiers, transforms, triggers, flags and further features it uses. It is compared
//!   with what the sources actually contain, and its union must cover every kind the compiler
//!   accepts.
//!
//! The same patterns also run through the `set` and `fmt` gates (`tests/set_lossless.rs`,
//! `tests/fmt_canonical.rs`) as part of `support::clean_sigil_sources`.

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use grimoire_core::Vec2;
use grimoire_sigil::{
    AimTarget, BehaviorId, BehaviorInput, BehaviorOutcome, BehaviorRegistryBuilder, BulletMotion,
    BulletPool, DespawnCause, Emitter, EventId, EventRequest, SigilConfig, SigilLibrary, SigilUnit,
    install,
};
use grimoire_sigilc::behaviors::parse_behavior_manifest;
use grimoire_sigilc::cli;
use grimoire_sigilc::compiler::{LoadError, SourceLoader, compile};
use grimoire_sigilc::edit::{ValueKind, list_values};
use grimoire_sigilc::{SyntaxElement, SyntaxKind, SyntaxNode, parse};
use grimoire_sim::{SimRng, Simulation, TickInput};
use support::{reference_root, reference_sigil_files};

/// Every building block, modifier, transformation, trigger and flag the compiler accepts
/// (`crates/grimoire_sigilc/src/compiler/validate.rs`).
const BLOCKS: &[&str] = &["ring", "spiral", "fan", "aimed", "wave", "line", "scatter"];
const MODIFIERS: &[&str] = &[
    "accelerate",
    "sine_offset",
    "rotate",
    "mirror",
    "speed_curve",
    "curve",
];
const TRANSFORMS: &[&str] = &["reverse", "change_type", "burst", "become_emitter"];
const TRIGGERS: &[&str] = &["time", "distance", "event"];
const FLAGS: &[&str] = &["smashable", "reflectable", "env_active", "grazeable"];
/// Further features the reference set must show at least once.
const FEATURES: &[&str] = &[
    "composition",
    "behaviour",
    "sub-emitter",
    "offset",
    "repeat forever",
    "despawn_vfx",
    "interp linear",
    "interp smooth",
];

struct RootLoader;

impl SourceLoader for RootLoader {
    fn load(&self, path: &str) -> Result<String, LoadError> {
        fs::read_to_string(reference_root().join(path)).map_err(|_| LoadError::NotFound)
    }
}

fn manifest_path() -> PathBuf {
    reference_root().join("behaviors.json")
}

fn behavior_ids() -> BTreeMap<String, u32> {
    let text = fs::read_to_string(manifest_path()).expect("tests/reference/behaviors.json");
    parse_behavior_manifest(&text).expect("the reference behaviour manifest is valid")
}

fn compiled(name: &str, source: &str) -> SigilUnit {
    let output = compile(name, source, &RootLoader, &behavior_ids());
    assert!(
        output.diagnostics.is_empty(),
        "{name}: {:#?}",
        output.diagnostics
    );
    SigilUnit::from_bytes(&output.bytes.expect("no diagnostics means bytes"))
        .unwrap_or_else(|error| panic!("{name} decodes: {error}"))
}

#[test]
fn there_are_at_least_twelve_numbered_reference_patterns() {
    let files = reference_sigil_files();
    assert!(files.len() >= 12, "only {} reference patterns", files.len());
    for (index, (name, _)) in files.iter().enumerate() {
        assert!(
            name.starts_with(&format!("{:02}-", index + 1)),
            "{name} is not numbered {:02}",
            index + 1
        );
    }
}

#[test]
fn every_reference_pattern_compiles_through_the_library_and_the_cli() {
    let files = reference_sigil_files();
    for (name, source) in &files {
        compiled(name, source);
    }

    let mut args: Vec<OsString> = vec![
        "check".into(),
        "--behaviors".into(),
        manifest_path().into_os_string(),
    ];
    args.extend(
        files
            .iter()
            .map(|(name, _)| reference_root().join(name).into_os_string()),
    );
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = cli::run(args, &mut stdout, &mut stderr);
    assert_eq!(
        code,
        cli::EXIT_OK,
        "{}{}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    );
}

// ---- Running the patterns --------------------------------------------------------------------

/// cos and sin of one degree: the turn per tick of `drift_orbit`.
const TURN_COS: f32 = 0.999_847_7;
const TURN_SIN: f32 = 0.017_452_406;
const TURN_RAD: f32 = 0.017_453_292;

/// The test's stand-in for the game behaviour `drift_orbit`: turns the velocity by one degree per
/// tick, so a satellite curves instead of flying straight.
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

fn behaviour_function(name: &str) -> (&'static str, grimoire_sigil::BehaviorFn) {
    match name {
        "drift_orbit" => ("drift_orbit", drift_orbit),
        other => panic!("the test has no stand-in for behaviour `{other}`"),
    }
}

/// Names of the entry file's emitters in compiled order (by name), each with whether it is a
/// primary emitter (no `role = sub`). Reference patterns compose only from primary emitters.
fn emitters_of(source: &str) -> Vec<(String, bool)> {
    let tree = parse("pattern.sigil", source).tree;
    let values = list_values(&tree);
    let mut names: Vec<String> = tree
        .children
        .iter()
        .filter_map(|child| match child {
            SyntaxElement::Node(node) if node.kind == SyntaxKind::EmitterItem => {
                Some(item_name(node))
            }
            _ => None,
        })
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let role = format!("emitters.{name}.role");
            let sub = values
                .iter()
                .any(|value| value.node_path == role && value.text == "sub");
            (name, !sub)
        })
        .collect()
}

fn item_name(item: &SyntaxNode) -> String {
    item.children
        .iter()
        .filter_map(|child| match child {
            SyntaxElement::Token(token) if token.kind == SyntaxKind::Ident => {
                Some(token.text.clone())
            }
            _ => None,
        })
        .nth(1)
        .unwrap_or_default()
}

/// What one run of a pattern showed.
#[derive(Debug, Default)]
struct RunReport {
    bullet_types_seen: BTreeSet<u16>,
    max_live: usize,
    transform_despawns: usize,
    /// Bullets seen moving towards the origin (velocity pointing against their position).
    inward_movers: usize,
    /// Bullet ticks in which a bullet's heading turned by more than 0.9 degrees since the tick
    /// before (the `drift_orbit` stand-in turns 1 degree per tick; no modifier of a pattern that
    /// binds it turns that fast).
    sharp_turns: usize,
}

const TICKS: u64 = 480;
/// cos(0.9 degrees).
const COS_SHARP_TURN: f32 = 0.999_876_6;
const EVENT_TICK: u64 = 150;

fn run(name: &str, source: &str) -> (SigilUnit, RunReport) {
    let unit = compiled(name, source);
    let unit_id = unit.id();
    let type_count = unit.bullet_types().len();
    let emitters = emitters_of(source);
    assert_eq!(
        emitters.len(),
        usize::from(unit.emitter_count()),
        "{name}: emitter count"
    );

    let mut builder = BehaviorRegistryBuilder::new(1);
    for (behaviour, id) in behavior_ids() {
        let (static_name, function) = behaviour_function(&behaviour);
        builder
            .register(BehaviorId(id), static_name, function)
            .expect("unique behaviour ids");
    }
    let registry = builder.build();
    let library =
        SigilLibrary::new(vec![unit.clone()], Arc::clone(&registry)).expect("library builds");
    let mut sim = Simulation::new(45);
    install(
        &mut sim,
        library,
        registry,
        SigilConfig::new(16_384, Vec2::new(-60.0, -60.0), Vec2::new(60.0, 60.0)),
    )
    .expect("install succeeds");
    sim.world_mut()
        .insert_resource(AimTarget(Some(Vec2::new(0.0, -4.0))));
    for (index, (_, primary)) in emitters.iter().enumerate() {
        if *primary {
            sim.world_mut().spawn((Emitter {
                unit: unit_id,
                emitter: index as u16,
                origin: Vec2::ZERO,
                rotation: 0.0,
                started_at: 0,
            },));
        }
    }

    let mut report = RunReport::default();
    let mut previous_velocity: BTreeMap<(u32, u32), Vec2> = BTreeMap::new();
    for tick in 0..TICKS {
        if tick == EVENT_TICK {
            sim.world_mut().spawn((EventRequest {
                event: EventId::from_name("phase_end"),
            },));
        }
        sim.step(TickInput::default());
        let pool = sim
            .world()
            .resource::<BulletPool>()
            .expect("pool installed");
        assert_eq!(pool.dropped_spawns(), 0, "{name}: dropped spawns at {tick}");
        report.max_live = report.max_live.max(pool.len() as usize);
        report.transform_despawns += pool
            .events()
            .iter()
            .filter(|event| event.cause == DespawnCause::Transform)
            .count();
        let columns = pool.columns();
        let mut velocities = BTreeMap::new();
        for bullet in pool.iter() {
            let slot = bullet.id().index() as usize;
            report.bullet_types_seen.insert(bullet.bullet_type());
            let position = columns.position[slot];
            let velocity = columns.velocity[slot];
            assert!(
                position.x.is_finite()
                    && position.y.is_finite()
                    && velocity.x.is_finite()
                    && velocity.y.is_finite(),
                "{name}: non-finite motion at tick {tick}"
            );
            let (length, speed) = (position.length(), velocity.length());
            if length > 0.5 && speed > 0.0 {
                let cosine = position.dot(velocity) / (length * speed);
                if cosine < -0.5 {
                    report.inward_movers += 1;
                }
            }
            let key = (bullet.id().index(), columns.generation[slot]);
            if let Some(before) = previous_velocity.get(&key) {
                let lengths = before.length() * speed;
                if lengths > 0.0 && before.dot(velocity) / lengths < COS_SHARP_TURN {
                    report.sharp_turns += 1;
                }
            }
            velocities.insert(key, velocity);
        }
        previous_velocity = velocities;
    }
    assert_eq!(
        report.bullet_types_seen.len(),
        type_count,
        "{name}: every bullet type must appear during the run ({report:?})"
    );
    assert!(report.max_live > 0, "{name}: no bullet was ever alive");
    (unit, report)
}

#[test]
fn every_reference_pattern_runs_and_its_features_act() {
    for (name, source) in reference_sigil_files() {
        let (_, report) = run(&name, &source);
        // Shown with `--nocapture`, and with every failure of this test.
        eprintln!("{name}: {report:?}");
        let coverage = derive_coverage(&source);
        let has = |set: &BTreeSet<String>, what: &str| set.contains(what);

        if has(&coverage.transforms, "burst") || has(&coverage.transforms, "become_emitter") {
            assert!(
                report.transform_despawns > 0,
                "{name}: burst/become_emitter never despawned a bullet ({report:?})"
            );
        }
        if has(&coverage.transforms, "reverse") {
            assert!(
                report.inward_movers > 0,
                "{name}: no reversed bullet was seen moving inwards ({report:?})"
            );
        }
        if has(&coverage.features, "behaviour") {
            assert!(
                report.sharp_turns > 0,
                "{name}: the behaviour never turned a bullet ({report:?})"
            );
        }
    }
}

// ---- Coverage table --------------------------------------------------------------------------

/// What one pattern's entry file uses.
#[derive(Debug, Default, PartialEq, Eq)]
struct Coverage {
    blocks: BTreeSet<String>,
    modifiers: BTreeSet<String>,
    transforms: BTreeSet<String>,
    triggers: BTreeSet<String>,
    flags: BTreeSet<String>,
    features: BTreeSet<String>,
}

fn derive_coverage(source: &str) -> Coverage {
    let parsed = parse("pattern.sigil", source);
    assert!(parsed.diagnostics.is_empty());
    let mut coverage = Coverage::default();

    let mut stack: Vec<&SyntaxNode> = vec![&parsed.tree];
    while let Some(node) = stack.pop() {
        if node.kind == SyntaxKind::NestedMember {
            let mut idents = node.children.iter().filter_map(|child| match child {
                SyntaxElement::Token(token) if token.kind == SyntaxKind::Ident => {
                    Some(token.text.clone())
                }
                _ => None,
            });
            let (Some(keyword), Some(kind)) = (idents.next(), idents.next()) else {
                continue;
            };
            match keyword.as_str() {
                "block" => coverage.blocks.insert(kind),
                "modifier" => coverage.modifiers.insert(kind),
                "transform" => coverage.transforms.insert(kind),
                _ => false,
            };
        }
        if node.kind == SyntaxKind::EmitterItem
            && node.children.iter().any(
                |child| matches!(child, SyntaxElement::Token(t) if t.kind == SyntaxKind::Ident && t.text == "from"),
            )
        {
            coverage.features.insert("composition".to_string());
        }
        for child in &node.children {
            if let SyntaxElement::Node(inner) = child {
                stack.push(inner);
            }
        }
    }

    for value in list_values(&parsed.tree) {
        let segments: Vec<&str> = value.node_path.split('.').collect();
        let last = segments.last().copied().unwrap_or_default();
        if value.kind == ValueKind::Trigger
            && let Some(keyword) = value.text.split_whitespace().next()
        {
            coverage.triggers.insert(keyword.to_string());
        }
        match (segments.first().copied(), last) {
            (Some("bullets"), flag) if flag.starts_with("flags[") => {
                coverage.flags.insert(value.text.clone());
            }
            (Some("bullets"), "behaviour") => {
                coverage.features.insert("behaviour".to_string());
            }
            (Some("bullets"), "despawn_vfx") => {
                coverage.features.insert("despawn_vfx".to_string());
            }
            (Some("emitters"), "role") if value.text == "sub" => {
                coverage.features.insert("sub-emitter".to_string());
            }
            (Some("emitters"), "offset") if segments.len() == 3 => {
                coverage.features.insert("offset".to_string());
            }
            (Some("emitters"), "repeat") if value.text == "forever" => {
                coverage.features.insert("repeat forever".to_string());
            }
            (Some("emitters"), "interp") => {
                coverage.features.insert(format!("interp {}", value.text));
            }
            _ => {}
        }
    }
    coverage
}

/// Reads the coverage table between the `reference-coverage` markers of `docs/formats/sigil.md`.
fn documented_coverage() -> BTreeMap<String, Coverage> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/formats/sigil.md");
    let text =
        fs::read_to_string(&path).unwrap_or_else(|error| panic!("reading {path:?}: {error}"));
    let begin = text
        .find("<!-- reference-coverage:begin -->")
        .expect("begin marker in docs/formats/sigil.md");
    let end = text
        .find("<!-- reference-coverage:end -->")
        .expect("end marker in docs/formats/sigil.md");
    let set = |cell: &str| -> BTreeSet<String> {
        if cell == "—" {
            BTreeSet::new()
        } else {
            cell.split(", ").map(str::to_string).collect()
        }
    };
    text[begin..end]
        .lines()
        .filter(|line| line.starts_with("| `"))
        .map(|line| {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            assert_eq!(cells.len(), 9, "malformed coverage row: {line}");
            (
                format!("{}.sigil", cells[1].trim_matches('`')),
                Coverage {
                    blocks: set(cells[2]),
                    modifiers: set(cells[3]),
                    transforms: set(cells[4]),
                    triggers: set(cells[5]),
                    flags: set(cells[6]),
                    features: set(cells[7]),
                },
            )
        })
        .collect()
}

#[test]
fn the_documented_coverage_table_matches_the_sources_and_covers_everything() {
    let documented = documented_coverage();
    let files = reference_sigil_files();
    let names: Vec<&String> = files.iter().map(|(name, _)| name).collect();
    assert_eq!(
        documented.keys().collect::<Vec<_>>(),
        names,
        "the table lists exactly the reference patterns"
    );

    let mut union = Coverage::default();
    for (name, source) in &files {
        let actual = derive_coverage(source);
        assert_eq!(&documented[name], &actual, "coverage row of {name}");
        union.blocks.extend(actual.blocks);
        union.modifiers.extend(actual.modifiers);
        union.transforms.extend(actual.transforms);
        union.triggers.extend(actual.triggers);
        union.flags.extend(actual.flags);
        union.features.extend(actual.features);
    }

    let all = |list: &[&str]| -> BTreeSet<String> { list.iter().map(|s| s.to_string()).collect() };
    assert_eq!(union.blocks, all(BLOCKS), "every building block");
    assert_eq!(union.modifiers, all(MODIFIERS), "every modifier");
    assert_eq!(union.transforms, all(TRANSFORMS), "every transformation");
    assert_eq!(union.triggers, all(TRIGGERS), "every trigger");
    assert_eq!(union.flags, all(FLAGS), "every flag");
    assert!(
        all(FEATURES).is_subset(&union.features),
        "features missing: {:?}",
        all(FEATURES)
            .difference(&union.features)
            .collect::<Vec<_>>()
    );
}
