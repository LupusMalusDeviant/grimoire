//! Conformance-corpus tests (Plan 0002 WP4.1 requirement 5): every file under
//! `tests/corpus/valid/` parses with no diagnostics, every file under `tests/corpus/invalid/`
//! parses with at least one, and for every file the full diagnostics — code, severity, file,
//! line, column, node path, offending token, cause, fix hint and any related location — match a
//! committed `<name>.expected.json` sidecar byte-for-byte in *content* (compared as JSON values,
//! so pretty-printing is not load-bearing).
//!
//! Each `.expected.json` is exactly what `sigilc parse --json <file>` prints for that file (the
//! sidecar was produced by running the compiled CLI and reviewing the diagnostics for
//! correctness, then committing the output as the golden answer — the standard way a conformance
//! corpus like this is built). That JSON shape has nothing Rust-specific about it (contract §2
//! rule 11's plain, cross-language document format), so a later C# asset-compiler test (this
//! corpus's stated purpose, WP4.1 requirement 5) can run the same `.sigil` files and compare
//! against the same `.expected.json` sidecars without going anywhere near this crate.

mod support;

use grimoire_sigilc::DiagnosticsDocument;
use support::{corpus_sigil_files, expected_json_path};

#[test]
fn corpus_matches_expected_diagnostics() {
    let files = corpus_sigil_files();
    assert!(!files.is_empty(), "conformance corpus is empty");
    for (name, source) in files {
        let output = grimoire_sigilc::parse(name.clone(), &source);
        let actual = DiagnosticsDocument::new(name.clone(), output.diagnostics);
        let actual_value = serde_json::to_value(&actual)
            .unwrap_or_else(|error| panic!("serialising diagnostics for {name}: {error}"));

        let expected_path = expected_json_path(&name);
        let expected_text = std::fs::read_to_string(&expected_path)
            .unwrap_or_else(|error| panic!("reading {expected_path:?}: {error}"));
        let expected_value: serde_json::Value = serde_json::from_str(&expected_text)
            .unwrap_or_else(|error| panic!("parsing {expected_path:?}: {error}"));

        assert_eq!(
            actual_value, expected_value,
            "diagnostics for {name} do not match {expected_path:?}"
        );
    }
}

#[test]
fn valid_corpus_files_have_no_diagnostics() {
    let mut checked = 0;
    for (name, source) in corpus_sigil_files() {
        if !name.starts_with("valid/") {
            continue;
        }
        checked += 1;
        let output = grimoire_sigilc::parse(&name, &source);
        assert!(
            output.diagnostics.is_empty(),
            "{name} should have no diagnostics, got {:?}",
            output.diagnostics
        );
    }
    assert!(checked > 0, "no files found under tests/corpus/valid/");
}

#[test]
fn invalid_corpus_files_have_at_least_one_diagnostic() {
    let mut checked = 0;
    for (name, source) in corpus_sigil_files() {
        if !name.starts_with("invalid/") {
            continue;
        }
        checked += 1;
        let output = grimoire_sigilc::parse(&name, &source);
        assert!(
            !output.diagnostics.is_empty(),
            "{name} is in tests/corpus/invalid/ but parsed with no diagnostics"
        );
    }
    assert!(checked > 0, "no files found under tests/corpus/invalid/");
}
