//! Compiled Sigil unit fixtures of runtime crates stay current (contract §1: "Tests der
//! Laufzeit-Crates nutzen eingecheckte, von `sigilc` erzeugte Unit-Fixtures; ein Test in
//! `grimoire_sigilc` prüft, dass die Fixtures aktuell sind").
//!
//! Runtime crates may not depend on this compiler, not even as a dev-dependency (engine ADR-0008),
//! so they include compiled `.bin` units checked in next to their `.sigil` sources. Every entry of
//! [`FIXTURES`] is compiled here from its source and compared byte for byte with the checked-in
//! unit; a mismatch means the source, the compiler or the format changed and the fixture must be
//! regenerated (and its dependent goldens reviewed):
//!
//! `cargo test -p grimoire_sigilc --test unit_fixtures -- --ignored regenerate_unit_fixtures`

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use grimoire_sigilc::compiler::{LoadError, SourceLoader, compile};

/// One compiled fixture: the canonical content path its `UnitId` derives from, and the source and
/// compiled unit paths relative to the workspace's `crates/` directory.
struct Fixture {
    canonical_path: &'static str,
    source: &'static str,
    unit: &'static str,
}

const FIXTURES: &[Fixture] = &[
    Fixture {
        // Plan 0002 WP3.5/WP5.3: the facade's bullet-pass offscreen proof.
        canonical_path: "fixtures/bullet_showcase.sigil",
        source: "grimoire/tests/fixtures/bullet_showcase.sigil",
        unit: "grimoire/tests/fixtures/bullet_showcase_unit_v1.bin",
    },
    Fixture {
        // Plan 0002 WP5.3: visual ids outside the bullet pass tables.
        canonical_path: "fixtures/unmapped_visual.sigil",
        source: "grimoire/tests/fixtures/unmapped_visual.sigil",
        unit: "grimoire/tests/fixtures/unmapped_visual_unit_v1.bin",
    },
    Fixture {
        // Plan 0002 WP5.7: the M2 showcase, example `sigil_curtain` and its offscreen GIF.
        canonical_path: "fixtures/sigil_curtain.sigil",
        source: "grimoire/tests/fixtures/sigil_curtain.sigil",
        unit: "grimoire/tests/fixtures/sigil_curtain_unit_v1.bin",
    },
];

fn crates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate lives under crates/")
        .to_path_buf()
}

/// No fixture imports another file; any import is a fixture authoring error.
struct NoImports;

impl SourceLoader for NoImports {
    fn load(&self, _path: &str) -> Result<String, LoadError> {
        Err(LoadError::NotFound)
    }
}

fn compile_fixture(fixture: &Fixture) -> Vec<u8> {
    let path = crates_dir().join(fixture.source);
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
        .replace("\r\n", "\n");
    let output = compile(
        fixture.canonical_path,
        &source,
        &NoImports,
        &BTreeMap::new(),
    );
    assert!(
        output.diagnostics.is_empty(),
        "{} must compile cleanly: {:#?}",
        fixture.source,
        output.diagnostics
    );
    output.bytes.expect("no diagnostics means bytes")
}

#[test]
fn checked_in_unit_fixtures_match_their_sources() {
    for fixture in FIXTURES {
        let compiled = compile_fixture(fixture);
        let path = crates_dir().join(fixture.unit);
        let checked_in =
            fs::read(&path).unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
        assert!(
            compiled == checked_in,
            "{} is stale: recompile it (see this file's module docs)",
            fixture.unit
        );
        grimoire_sigil::SigilUnit::from_bytes(&checked_in)
            .unwrap_or_else(|error| panic!("{} must decode: {error}", fixture.unit));
    }
}

#[test]
#[ignore = "writes the compiled unit fixtures; run by hand after changing a fixture source"]
fn regenerate_unit_fixtures() {
    for fixture in FIXTURES {
        let compiled = compile_fixture(fixture);
        let path = crates_dir().join(fixture.unit);
        fs::write(&path, compiled)
            .unwrap_or_else(|error| panic!("writing {}: {error}", path.display()));
    }
}
