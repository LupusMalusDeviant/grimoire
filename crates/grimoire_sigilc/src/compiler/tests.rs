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
