//! `sigilc simulate` (Plan 0002 WP5.6, `docs/formats/sigil.md` §13.8), run in-process through
//! `grimoire_sigilc::cli::run`.
//!
//! - A hand-checked run: positions of a plain ring after a few ticks are computed here by hand.
//! - Golden documents: three reference patterns, one per input feature (a scripted aim target,
//!   events, an import with a stubbed behaviour), compared byte for byte with the documents under
//!   `tests/simulate/`. They run on Windows, Linux and macOS, so the preview is platform-identical.
//!   Regenerate after an intended change with
//!   `cargo test -p grimoire_sigilc --test simulate -- --ignored regenerate_simulate_goldens`
//!   (renewing a golden is a Product Owner decision, contract §2b).
//! - Usage errors and the exit codes.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use grimoire_sigilc::cli::{EXIT_ERROR, EXIT_OK, EXIT_PROBLEMS, run};
use serde_json::Value;

struct Output {
    code: u8,
    stdout: String,
    stderr: String,
}

fn sigilc(args: &[&str]) -> Output {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = run(
        args.iter().map(OsString::from).collect::<Vec<_>>(),
        &mut stdout,
        &mut stderr,
    );
    Output {
        code,
        stdout: String::from_utf8(stdout).expect("stdout is UTF-8"),
        stderr: String::from_utf8(stderr).expect("stderr is UTF-8"),
    }
}

fn json(output: &Output) -> Value {
    serde_json::from_str(&output.stdout)
        .unwrap_or_else(|error| panic!("stdout is not JSON ({error}):\n{}", output.stdout))
}

/// A scratch directory, removed again when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "grimoire-sigilc-simulate-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn write(&self, relative: &str, contents: &str) -> String {
        let path = self.0.join(relative);
        fs::write(&path, contents).unwrap();
        path.to_str().unwrap().to_string()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const PLAIN_RING: &str = "sigil 1
bullet dot {
  silhouette = orb
  palette = enemy.hex_magenta
  glow = 0.5
  radius = 0.2u
  damage = 1
  flags = []
}

emitter ring {
  bullet = dot
  repeat = 1
  interval = 1t
  speed = 0.5u/t
  block ring {
    count = 2
  }
}

emitter orbit {
  role = sub
  bullet = dot
  repeat = 1
  interval = 1t
  speed = 9u/t
  block ring {
    count = 8
  }
}
";

fn number(value: &Value) -> f64 {
    value.as_f64().expect("a JSON number")
}

#[test]
fn a_plain_ring_moves_as_computed_by_hand() {
    let scratch = Scratch::new("ring");
    let file = scratch.write("ring.sigil", PLAIN_RING);
    let output = sigilc(&["simulate", "--json", "--ticks", "4", &file]);
    assert_eq!(output.code, EXIT_OK, "{}", output.stderr);
    assert!(
        output.stdout.ends_with("}\n") && output.stdout.lines().count() == 1,
        "compact"
    );
    let document = json(&output);
    assert_eq!(document["schema"], "grimoire.sigilc.simulate");
    assert_eq!(document["ok"], true);
    // Emitters are numbered by name: `orbit` (sub) is 0, `ring` (primary) is 1.
    assert_eq!(document["emitters"], serde_json::json!([1]));
    assert_eq!(document["settings"]["target"]["kind"], "none");
    let frames = document["frames"].as_array().expect("frames");
    assert_eq!(frames.len(), 4);
    for (tick, frame) in frames.iter().enumerate() {
        assert_eq!(frame["tick"], tick as u64);
        assert_eq!(frame["live"], 2, "only the primary ring fires");
        let bullets = frame["bullets"].as_array().expect("bullets");
        // Spawned at tick 0 and moved once per later tick: 0.5 u per tick along +x and -x.
        let travelled = 0.5 * tick as f64;
        assert!((number(&bullets[0][3]) - travelled).abs() < 1e-5, "{frame}");
        assert!(number(&bullets[0][4]).abs() < 1e-5);
        assert!((number(&bullets[1][3]) + travelled).abs() < 1e-5, "{frame}");
        assert_eq!(bullets[0][7], tick as u64, "age");
    }
    assert_eq!(
        document["final_state_hash"], frames[3]["state_hash"],
        "the final hash is the last frame's"
    );

    // Identical inputs give identical bytes.
    let again = sigilc(&["simulate", "--json", "--ticks", "4", &file]);
    assert_eq!(again.stdout, output.stdout);
}

#[test]
fn a_fixed_target_a_seed_and_bounds_are_reported_and_used() {
    let scratch = Scratch::new("settings");
    let file = scratch.write("ring.sigil", PLAIN_RING);
    let output = sigilc(&[
        "simulate",
        "--json",
        "--ticks",
        "8",
        "--target=3,-4.5",
        "--seed",
        "42",
        "--capacity",
        "16",
        "--bounds",
        "-2,-2,2,2",
        &file,
    ]);
    assert_eq!(output.code, EXIT_OK, "{}", output.stderr);
    let document = json(&output);
    let settings = &document["settings"];
    assert_eq!(settings["seed"], "000000000000002a");
    assert_eq!(settings["capacity"], 16);
    assert_eq!(
        settings["bounds"],
        serde_json::json!([-2.0, -2.0, 2.0, 2.0])
    );
    assert_eq!(
        settings["target"]["position"],
        serde_json::json!([3.0, -4.5])
    );
    let frames = document["frames"].as_array().expect("frames");
    assert_eq!(frames[0]["target"], serde_json::json!([3.0, -4.5]));
    // At 0.5 u per tick the ring leaves the 2-unit bounds in tick 5: both bullets despawn there
    // with cause 1 (bounds).
    let despawned = frames[5]["despawned"].as_array().expect("despawned");
    assert_eq!(despawned.len(), 2, "{}", frames[5]);
    assert!(despawned.iter().all(|row| row[5] == 1));
    assert_eq!(frames[7]["live"], 0);
}

#[test]
fn only_primary_emitters_run_and_sub_emitters_fire_through_transforms() {
    // Emitters by name: `bloom` (sub) 0, `seeds` 1, `spark` (sub) 2. The seeds become bloom
    // emitters after 50 ticks, whose shards become spark emitters later.
    let output = sigilc(&[
        "simulate",
        "--json",
        "--ticks",
        "90",
        "tests/corpus/valid/03-subemitter-cascade.sigil",
    ]);
    assert_eq!(output.code, EXIT_OK, "{}", output.stderr);
    let document = json(&output);
    assert_eq!(document["emitters"], serde_json::json!([1]));
    let frames = document["frames"].as_array().expect("frames");
    assert_eq!(frames[0]["live"], 6, "one ring of six seeds");
    let transformed = frames[50]["despawned"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row[5] == 2)
        .count();
    assert_eq!(transformed, 6, "every seed becomes an emitter at age 50");
    assert_eq!(frames[50]["live"], 30, "five shards per seed");
}

#[test]
fn compile_errors_are_reported_in_the_document() {
    let scratch = Scratch::new("broken");
    let file = scratch.write(
        "broken.sigil",
        "sigil 1\nemitter e {\n  bullet = ghost\n}\n",
    );
    let output = sigilc(&["simulate", "--json", "--ticks", "3", &file]);
    assert_eq!(output.code, EXIT_PROBLEMS);
    let document = json(&output);
    assert_eq!(document["ok"], false);
    assert!(!document["diagnostics"].as_array().unwrap().is_empty());
    assert_eq!(document["frames"], serde_json::json!([]));
    assert_eq!(document["final_state_hash"], Value::Null);
}

#[test]
fn usage_errors_and_unreadable_inputs_exit_with_two() {
    let scratch = Scratch::new("usage");
    let file = scratch.write("ring.sigil", PLAIN_RING);
    let bad_path = scratch.write(
        "path.json",
        r#"{"schema":"grimoire.sigilc.target_path","schema_version":1,"points":[]}"#,
    );
    let cases: Vec<Vec<&str>> = vec![
        vec!["simulate", "--ticks", "3", &file],
        vec!["simulate", "--json", &file],
        vec!["simulate", "--json", "--ticks", "0", &file],
        vec!["simulate", "--json", "--ticks", "36001", &file],
        vec!["simulate", "--json", "--ticks", "3"],
        vec!["simulate", "--json", "--ticks", "3", "--target", "1", &file],
        vec![
            "simulate", "--json", "--ticks", "3", "--target", "1,inf", &file,
        ],
        vec![
            "simulate",
            "--json",
            "--ticks",
            "3",
            "--target",
            "1,2",
            "--target-path",
            &bad_path,
            &file,
        ],
        vec![
            "simulate",
            "--json",
            "--ticks",
            "3",
            "--target-path",
            &bad_path,
            &file,
        ],
        vec![
            "simulate",
            "--json",
            "--ticks",
            "3",
            "--target-path",
            "missing.json",
            &file,
        ],
        vec![
            "simulate", "--json", "--ticks", "3", "--events", "boom@3", &file,
        ],
        vec![
            "simulate", "--json", "--ticks", "3", "--events", "bo om@1", &file,
        ],
        vec![
            "simulate", "--json", "--ticks", "3", "--events", "boom", &file,
        ],
        vec![
            "simulate", "--json", "--ticks", "3", "--bounds", "1,1,0,2", &file,
        ],
        vec![
            "simulate",
            "--json",
            "--ticks",
            "3",
            "--capacity",
            "0",
            &file,
        ],
        vec!["simulate", "--json", "--ticks", "3", "--seed", "-1", &file],
    ];
    for args in cases {
        let output = sigilc(&args);
        assert_eq!(output.code, EXIT_ERROR, "{args:?}: {}", output.stdout);
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(!output.stderr.is_empty(), "{args:?}");
    }
}

/// One golden document: its file name under `tests/simulate/` and the arguments that produce it,
/// with paths relative to the package root (the working directory of integration tests).
struct Golden {
    name: &'static str,
    args: &'static [&'static str],
}

const GOLDENS: &[Golden] = &[
    Golden {
        name: "03-aimed-fan.json",
        args: &[
            "simulate",
            "--json",
            "--ticks",
            "40",
            "--target-path",
            "tests/simulate/dodge-path.json",
            "tests/reference/03-aimed-fan.sigil",
        ],
    },
    Golden {
        name: "10-phase-shift.json",
        args: &[
            "simulate",
            "--json",
            "--ticks",
            "36",
            "--events",
            "phase_end@30",
            "tests/reference/10-phase-shift.sigil",
        ],
    },
    Golden {
        name: "12-finale-composite.json",
        args: &[
            "simulate",
            "--json",
            "--ticks",
            "20",
            "--seed",
            "7",
            "--root",
            "tests/reference",
            "--behaviors",
            "tests/reference/behaviors.json",
            "tests/reference/12-finale-composite.sigil",
        ],
    },
];

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from("tests/simulate").join(name)
}

#[test]
fn reference_pattern_previews_match_their_golden_documents() {
    for golden in GOLDENS {
        let output = sigilc(golden.args);
        assert_eq!(output.code, EXIT_OK, "{}: {}", golden.name, output.stderr);
        let expected = fs::read_to_string(golden_path(golden.name))
            .unwrap_or_else(|error| panic!("reading {}: {error}", golden.name))
            .replace("\r\n", "\n");
        assert!(
            output.stdout == expected,
            "{} differs from its golden document",
            golden.name
        );
    }
}

#[test]
fn the_golden_documents_show_what_they_are_for() {
    let document = |name: &str| -> Value {
        serde_json::from_str(&fs::read_to_string(golden_path(name)).unwrap()).unwrap()
    };

    let aimed = document("03-aimed-fan.json");
    let targets: Vec<&Value> = aimed["frames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|frame| &frame["target"])
        .collect();
    assert_ne!(targets.first(), targets.last(), "the scripted target moves");

    let phase = document("10-phase-shift.json");
    let frames = phase["frames"].as_array().unwrap();
    assert_eq!(frames[30]["events"], serde_json::json!(["phase_end"]));
    let types = |frame: &Value| -> Vec<u64> {
        frame["bullets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row[2].as_u64().unwrap())
            .collect()
    };
    // Bullet types are numbered by name: `angry` 0, `calm` 1, `drifter` 2. The event turns every
    // calm orb angry in the tick it is raised.
    assert!(
        types(&frames[29]).contains(&1),
        "calm orbs before the event"
    );
    assert!(!types(&frames[30]).contains(&1), "no calm orb after it");
    assert!(types(&frames[30]).contains(&0), "angry diamonds instead");

    let finale = document("12-finale-composite.json");
    assert_eq!(
        finale["stubbed_behaviors"],
        serde_json::json!([{ "name": "drift_orbit", "id": 1 }])
    );
}

#[test]
#[ignore = "writes tests/simulate/*.json; run by hand after an intended change and review the diff"]
fn regenerate_simulate_goldens() {
    for golden in GOLDENS {
        let output = sigilc(golden.args);
        assert_eq!(output.code, EXIT_OK, "{}: {}", golden.name, output.stderr);
        fs::write(golden_path(golden.name), output.stdout).unwrap();
    }
}
