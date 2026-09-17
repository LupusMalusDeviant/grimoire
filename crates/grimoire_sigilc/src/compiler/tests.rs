//! Unit tests for the `compiler` module (Plan 0002 WP4.2). The conformance-corpus-level tests
//! (every valid file compiles cleanly, every new `SIG00xx` invalid case) live in
//! `tests/compiler_corpus.rs`, alongside the WP4.1 parser corpus; this module covers behaviour
//! that is easier to exercise directly against [`super::compile`] with an in-memory
//! [`super::SourceLoader`].

use std::collections::BTreeMap;

use super::{LoadError, SourceLoader, compile};

struct MapLoader(BTreeMap<&'static str, &'static str>);

impl SourceLoader for MapLoader {
    fn load(&self, path: &str) -> Result<String, LoadError> {
        self.0
            .get(path)
            .map(|s| (*s).to_string())
            .ok_or(LoadError::NotFound)
    }
}

fn minimal_source(extra_bullet_field: &str) -> String {
    format!(
        "sigil 1\n\
         bullet orb {{\n\
         \x20 silhouette = orb\n\
         \x20 palette = enemy.crimson\n\
         \x20 glow = 0.5\n\
         \x20 radius = 0.2u\n\
         \x20 damage = 1\n\
         \x20 flags = []\n\
         {extra_bullet_field}\
         }}\n\
         \n\
         emitter burst {{\n\
         \x20 bullet = orb\n\
         \x20 repeat = 1\n\
         \x20 interval = 10t\n\
         \x20 speed = 0.1u/t\n\
         \x20 block ring {{\n\
         \x20\x20 count = 8\n\
         \x20 }}\n\
         }}\n"
    )
}

#[test]
fn compiles_a_minimal_valid_file_with_no_diagnostics() {
    let source = minimal_source("");
    let output = compile(
        "unit.sigil",
        &source,
        &MapLoader(BTreeMap::new()),
        &BTreeMap::new(),
    );
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    assert!(output.bytes.is_some());
    let bytes = output.bytes.unwrap();
    let unit = grimoire_sigil::SigilUnit::from_bytes(&bytes).expect("compiled bytes must decode");
    assert_eq!(unit.bullet_types().len(), 1);
    assert_eq!(unit.emitter_count(), 1);
}

#[test]
fn rejects_unknown_bullet_reference() {
    let source = "sigil 1\nemitter burst {\n  bullet = ghost\n  repeat = 1\n  interval = 1t\n  speed = 0.1u/t\n  block ring { count = 8 }\n}\n";
    let output = compile(
        "unit.sigil",
        source,
        &MapLoader(BTreeMap::new()),
        &BTreeMap::new(),
    );
    assert!(output.bytes.is_none());
    assert!(output.diagnostics.iter().any(|d| d.code == "SIG0014"));
}

#[test]
fn rejects_ring_count_zero_as_nan_risk() {
    let source = "sigil 1\nbullet orb {\n  silhouette = orb\n  palette = enemy.crimson\n  glow = 0.5\n  radius = 0.2u\n  damage = 1\n  flags = []\n}\n\nemitter burst {\n  bullet = orb\n  repeat = 1\n  interval = 1t\n  speed = 0.1u/t\n  block ring { count = 0 }\n}\n";
    let output = compile(
        "unit.sigil",
        source,
        &MapLoader(BTreeMap::new()),
        &BTreeMap::new(),
    );
    assert!(output.bytes.is_none());
    assert!(output.diagnostics.iter().any(|d| d.code == "SIG0017"));
}

#[test]
fn rejects_beats_unit_on_interval() {
    let source = "sigil 1\nbullet orb {\n  silhouette = orb\n  palette = enemy.crimson\n  glow = 0.5\n  radius = 0.2u\n  damage = 1\n  flags = []\n}\n\nemitter burst {\n  bullet = orb\n  repeat = 1\n  interval = 5beats\n  speed = 0.1u/t\n  block ring { count = 8 }\n}\n";
    let output = compile(
        "unit.sigil",
        source,
        &MapLoader(BTreeMap::new()),
        &BTreeMap::new(),
    );
    assert!(output.bytes.is_none());
    assert!(output.diagnostics.iter().any(|d| d.code == "SIG0015"));
}

#[test]
fn rejects_non_enemy_palette_space() {
    let source = minimal_source("").replace("enemy.crimson", "player.crimson");
    let output = compile(
        "unit.sigil",
        &source,
        &MapLoader(BTreeMap::new()),
        &BTreeMap::new(),
    );
    assert!(output.bytes.is_none());
    assert!(output.diagnostics.iter().any(|d| d.code == "SIG0021"));
}

#[test]
fn rejects_two_bullet_types_sharing_a_silhouette() {
    let mut source = minimal_source("").replace("emitter burst", "bullet twin {\n  silhouette = orb\n  palette = enemy.amber\n  glow = 0.2\n  radius = 0.1u\n  damage = 1\n  flags = []\n}\n\nemitter burst");
    source.push_str("");
    let output = compile(
        "unit.sigil",
        &source,
        &MapLoader(BTreeMap::new()),
        &BTreeMap::new(),
    );
    assert!(
        output.diagnostics.iter().any(|d| d.code == "SIG0020"),
        "{:#?}",
        output.diagnostics
    );
}

#[test]
fn resolves_import_and_composition_with_overrides() {
    let base = "sigil 1\nbullet orb {\n  silhouette = orb\n  palette = enemy.crimson\n  glow = 0.5\n  radius = 0.2u\n  damage = 1\n  flags = []\n}\n\nemitter burst {\n  bullet = orb\n  repeat = 4\n  interval = 45t\n  speed = 0.05u/t\n  block ring {\n    count = 24\n    start = 7.5deg\n  }\n}\n";
    let entry = "sigil 1\nimport \"base.sigil\" as ring\n\nemitter finale from ring.burst {\n  repeat = 1\n  block.count = 12\n}\n";
    let mut files = BTreeMap::new();
    files.insert("base.sigil", base);
    let output = compile("entry.sigil", entry, &MapLoader(files), &BTreeMap::new());
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    let bytes = output.bytes.expect("must compile");
    let unit = grimoire_sigil::SigilUnit::from_bytes(&bytes).expect("must decode");
    // The composed unit pulls in `orb` from the imported file even though `entry.sigil` never
    // declares a bullet of its own.
    assert_eq!(unit.bullet_types().len(), 1);
    assert_eq!(unit.emitter_count(), 1);
}

#[test]
fn detects_import_cycle() {
    let a = "sigil 1\nimport \"b.sigil\" as b\n";
    let b = "sigil 1\nimport \"a.sigil\" as a\n";
    let mut files = BTreeMap::new();
    files.insert("b.sigil", b);
    files.insert("a.sigil", a);
    let output = compile("a.sigil", a, &MapLoader(files), &BTreeMap::new());
    assert!(
        output.diagnostics.iter().any(|d| d.code == "SIG0013"),
        "{:#?}",
        output.diagnostics
    );
}

/// Bullet fields every transform test below shares.
fn bullet(name: &str, silhouette: &str, transforms: &str) -> String {
    format!(
        "bullet {name} {{\n  silhouette = {silhouette}\n  palette = enemy.crimson\n  glow = 0.5\n  \
         radius = 0.2u\n  damage = 1\n  flags = []\n{transforms}}}\n\n"
    )
}

/// The body of the `Transforms` section (kind 4) of a compiled unit, if it has one.
fn transforms_section(bytes: &[u8]) -> Option<&[u8]> {
    let read_u32 = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let read_u64 = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap()) as usize;
    let count = read_u32(40) as usize;
    (0..count).find_map(|i| {
        let entry = 44 + i * 24;
        (read_u32(entry) == 4).then(|| {
            let offset = 40 + read_u64(entry + 8);
            &bytes[offset..offset + read_u64(entry + 16)]
        })
    })
}

/// Plan 0002 WP5.2: every transform kind and trigger kind lowers into exactly the `Transforms`
/// record layout of `docs/formats/sigil.md` §10.9, written out here by hand.
#[test]
fn lowers_transforms_into_the_documented_section_layout() {
    let source = format!(
        "sigil 1\n{}{}{}{}\
         emitter burst {{\n  bullet = orb\n  repeat = 1\n  interval = 1t\n  speed = 0.1u/t\n  \
         block ring {{\n    count = 8\n  }}\n}}\n\n\
         emitter spark {{\n  role = sub\n  bullet = dust\n  repeat = 1\n  interval = 1t\n  \
         speed = 0.2u/t\n  block fan {{\n    count = 3\n    spread = 30deg\n  }}\n}}\n",
        bullet("dust", "rice", ""),
        bullet(
            "ember",
            "diamond",
            "  transform become_emitter {\n    when = time 5t\n    emitter = spark\n  }\n"
        ),
        bullet(
            "orb",
            "orb",
            "  transform reverse {\n    when = event phase_end\n  }\n  \
             transform burst {\n    when = time 12t\n    bullet = shard\n    speed = 0.5u/t\n    \
             block ring {\n      count = 4\n    }\n  }\n"
        ),
        bullet(
            "shard",
            "shard",
            "  transform change_type {\n    when = distance 2.5u\n    to = ember\n  }\n"
        ),
    );
    let output = compile(
        "unit.sigil",
        &source,
        &MapLoader(BTreeMap::new()),
        &BTreeMap::new(),
    );
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    let bytes = output.bytes.expect("compiles");
    let unit = grimoire_sigil::SigilUnit::from_bytes(&bytes).expect("decodes");
    assert_eq!(
        unit.program_count(),
        3,
        "two emitter programs, then the burst block"
    );

    // Bullet types by name: dust 0, ember 1, orb 2, shard 3. Emitters by name: burst 0, spark 1.
    let transform = |kind: u8,
                     trigger: u8,
                     at: u32,
                     distance: f32,
                     event: u32,
                     target: u16,
                     program: u16,
                     speed: f32| {
        let mut out = vec![kind, trigger, 0, 0];
        out.extend_from_slice(&at.to_le_bytes());
        out.extend_from_slice(&distance.to_le_bytes());
        out.extend_from_slice(&event.to_le_bytes());
        out.extend_from_slice(&target.to_le_bytes());
        out.extend_from_slice(&program.to_le_bytes());
        out.extend_from_slice(&speed.to_le_bytes());
        out
    };
    let script_head = |bullet_type: u16, transforms: u16| {
        let mut out = bullet_type.to_le_bytes().to_vec();
        out.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]); // no behavior, id 0, no params
        out.extend_from_slice(&transforms.to_le_bytes());
        out
    };
    let phase_end = grimoire_sigil::EventId::from_name("phase_end").0;
    let mut expected = 3u16.to_le_bytes().to_vec();
    expected.extend(script_head(1, 1));
    expected.extend(transform(4, 1, 5, 0.0, 0, 1, 0xFFFF, 0.0));
    expected.extend(script_head(2, 2));
    expected.extend(transform(1, 3, 0, 0.0, phase_end, 0, 0xFFFF, 0.0));
    expected.extend(transform(3, 1, 12, 0.0, 0, 3, 2, 0.5));
    expected.extend(script_head(3, 1));
    expected.extend(transform(2, 2, 0, 2.5, 0, 1, 0xFFFF, 0.0));
    assert_eq!(transforms_section(&bytes), Some(expected.as_slice()));
}

#[test]
fn lowers_a_behaviour_reference_into_its_bullet_types_script() {
    let source = minimal_source("  behaviour = orbit_parent\n");
    let mut ids = BTreeMap::new();
    ids.insert("orbit_parent".to_string(), 0x0102_0304);
    let output = compile("unit.sigil", &source, &MapLoader(BTreeMap::new()), &ids);
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    let bytes = output.bytes.expect("compiles");
    grimoire_sigil::SigilUnit::from_bytes(&bytes).expect("decodes");
    let mut expected = 1u16.to_le_bytes().to_vec();
    expected.extend_from_slice(&0u16.to_le_bytes()); // bullet type 0
    expected.extend_from_slice(&[1, 0]); // has a behavior
    expected.extend_from_slice(&0x0102_0304u32.to_le_bytes());
    expected.extend_from_slice(&0u16.to_le_bytes()); // no params
    expected.extend_from_slice(&0u16.to_le_bytes()); // no transforms
    assert_eq!(transforms_section(&bytes), Some(expected.as_slice()));
}

#[test]
fn rejects_more_transforms_than_one_bullet_type_can_carry() {
    let transforms = "  transform reverse {\n    when = time 10t\n  }\n".repeat(9);
    let source = minimal_source(&transforms);
    let output = compile(
        "unit.sigil",
        &source,
        &MapLoader(BTreeMap::new()),
        &BTreeMap::new(),
    );
    assert!(output.bytes.is_none());
    assert!(
        output
            .diagnostics
            .iter()
            .any(|d| d.code == "SIG0016" && d.node_path == "bullets.orb.transforms"),
        "{:#?}",
        output.diagnostics
    );
}

#[test]
fn rejects_become_emitter_on_an_imported_bullet() {
    let lib = format!(
        "sigil 1\n{}{}\
         emitter seeds {{\n  bullet = seed\n  repeat = 1\n  interval = 1t\n  speed = 0.1u/t\n  \
         block ring {{\n    count = 4\n  }}\n}}\n\n\
         emitter bloom {{\n  role = sub\n  bullet = petal\n  repeat = 1\n  interval = 1t\n  \
         speed = 0.1u/t\n  block ring {{\n    count = 3\n  }}\n}}\n",
        bullet(
            "seed",
            "orb",
            "  transform become_emitter {\n    when = time 5t\n    emitter = bloom\n  }\n"
        ),
        bullet("petal", "shard", ""),
    );
    let entry = "sigil 1\nimport \"lib.sigil\" as lib\n\nemitter finale from lib.seeds {\n  repeat = 2\n}\n";
    let lib: &'static str = Box::leak(lib.into_boxed_str());
    let mut files = BTreeMap::new();
    files.insert("lib.sigil", lib);
    let output = compile("entry.sigil", entry, &MapLoader(files), &BTreeMap::new());
    assert!(output.bytes.is_none());
    assert!(
        output
            .diagnostics
            .iter()
            .any(|d| d.code == "SIG0014" && d.node_path == "bullets.seed.transforms[0]"),
        "{:#?}",
        output.diagnostics
    );
}
