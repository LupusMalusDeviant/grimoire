//! Compiler conformance corpus (Plan 0002 WP4.2): the schema pass, name resolution, composition
//! and static validation this module adds on top of the WP4.1 parser.
//!
//! Two claims are checked here, mirroring `corpus.rs`'s own methodology (Plan 0002 WP4.1
//! requirement 5, extended to the schema pass):
//!
//! - Every file under `tests/corpus/valid/` (the existing WP4.1 corpus) still *compiles* cleanly,
//!   not just parses cleanly — the contract's own requirement that "der bestehende Korpus bleibt
//!   gültig". `05-wave-line-composite.sigil`'s `behaviour = orbit_parent` needs a `BehaviorId` to
//!   resolve against; the fixed id used here is this test's own business, not a real game's.
//! - Every file under `tests/corpus/schema-invalid/` compiles with at least the diagnostic code
//!   named in its own comment, and its full diagnostics match a committed `<name>.expected.json`
//!   sidecar byte-for-byte in content (same `DiagnosticsDocument` JSON shape `corpus.rs` uses for
//!   the parser corpus; `CompileOutput::diagnostics` is the same `Diagnostic` type). Files ending
//!   in `-b` (or otherwise only ever named by another fixture's `import`) are not compiled
//!   directly; they exist only to be imported.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use grimoire_sigilc::DiagnosticsDocument;
use grimoire_sigilc::compiler::{LoadError, SourceLoader, compile};

struct DirLoader(PathBuf);

impl SourceLoader for DirLoader {
    fn load(&self, path: &str) -> Result<String, LoadError> {
        fs::read_to_string(self.0.join(path)).map_err(|_| LoadError::NotFound)
    }
}

fn behavior_ids() -> BTreeMap<String, u32> {
    let mut map = BTreeMap::new();
    map.insert("orbit_parent".to_string(), 1);
    map
}

#[test]
fn valid_corpus_files_still_compile_cleanly() {
    let dir = support::corpus_root().join("valid");
    let loader = DirLoader(dir.clone());
    let mut checked = 0;
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("sigil"))
        .collect();
    paths.sort();
    for path in paths {
        checked += 1;
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        let source = fs::read_to_string(&path).unwrap();
        let output = compile(&name, &source, &loader, &behavior_ids());
        assert!(
            output.diagnostics.is_empty(),
            "{name} should compile cleanly, got {:#?}",
            output.diagnostics
        );
        assert!(output.bytes.is_some(), "{name} produced no bytes");
        grimoire_sigil::SigilUnit::from_bytes(&output.bytes.unwrap())
            .unwrap_or_else(|error| panic!("{name}'s compiled bytes must decode: {error}"));
    }
    assert!(checked > 0, "no files found under tests/corpus/valid/");
}

/// Fixtures under `tests/corpus/schema-invalid/` that exist only to be imported by another
/// fixture, so they are never compiled as an entry file in their own right.
const IMPORT_ONLY: &[&str] = &["e-import-cycle-b.sigil"];

#[test]
fn schema_invalid_corpus_matches_expected_diagnostics() {
    let dir = support::corpus_root().join("schema-invalid");
    let loader = DirLoader(dir.clone());
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("sigil"))
        .collect();
    paths.sort();
    let mut checked = 0;
    for path in paths {
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        if IMPORT_ONLY.contains(&name.as_str()) {
            continue;
        }
        checked += 1;
        let source = fs::read_to_string(&path).unwrap();
        let output = compile(&name, &source, &loader, &behavior_ids());
        assert!(
            !output.diagnostics.is_empty(),
            "{name} is in tests/corpus/schema-invalid/ but compiled with no diagnostics"
        );
        assert!(
            output.bytes.is_none(),
            "{name} has diagnostics but still produced bytes"
        );

        let actual = DiagnosticsDocument::new(name.clone(), output.diagnostics);
        let actual_value = serde_json::to_value(&actual).unwrap();
        let expected_path = expected_json_path(&dir, &name);
        let expected_text = fs::read_to_string(&expected_path)
            .unwrap_or_else(|error| panic!("reading {expected_path:?}: {error}"));
        let expected_value: serde_json::Value = serde_json::from_str(&expected_text)
            .unwrap_or_else(|error| panic!("parsing {expected_path:?}: {error}"));
        assert_eq!(
            actual_value, expected_value,
            "diagnostics for {name} do not match {expected_path:?}"
        );
    }
    assert!(
        checked > 0,
        "no files found under tests/corpus/schema-invalid/"
    );
}

fn expected_json_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(name.replace(".sigil", ".expected.json"))
}

#[test]
#[ignore = "writes the schema-invalid corpus' .expected.json golden files; run once by hand \
            after reviewing the printed diagnostics for correctness, the same way the WP4.1 \
            parser corpus' sidecars were produced (see corpus.rs's module docs)"]
fn generate_expected_json() {
    let dir = support::corpus_root().join("schema-invalid");
    let loader = DirLoader(dir.clone());
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("sigil"))
        .collect();
    paths.sort();
    for path in paths {
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        if IMPORT_ONLY.contains(&name.as_str()) {
            continue;
        }
        let source = fs::read_to_string(&path).unwrap();
        let output = compile(&name, &source, &loader, &behavior_ids());
        println!("=== {name} ===");
        for d in &output.diagnostics {
            println!("{}", d.render_text());
        }
        let document = DiagnosticsDocument::new(name.clone(), output.diagnostics);
        let json = document.to_json_pretty().unwrap();
        fs::write(expected_json_path(&dir, &name), json).unwrap();
    }
}
