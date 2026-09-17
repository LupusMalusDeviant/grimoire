//! Value-listing conformance corpus (Plan 0002 WP4.3, `docs/formats/sigil.md` §9 and §13.5).
//!
//! Every file under `tests/corpus/valid/` has a `<name>.values.json` sidecar next to it: exactly
//! what `sigilc parse --json --values valid/<name>.sigil` prints when run from the corpus root. The
//! Sigil editor's parameter panel (Plan 0002 WP10.3) reads this document instead of parsing Sigil
//! in C#, so its C# tests can run against the same sidecars. Here the document is produced through
//! the real CLI entry point with a package-relative path, and only its `file` field is rewritten to
//! the corpus-relative name before the comparison.
//!
//! The sidecars were generated once with the ignored test below and reviewed by hand: every node
//! path follows §5, every text is the exact source slice, every line and column points at it.

mod support;

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use grimoire_sigilc::cli::{EXIT_OK, run};
use serde_json::Value;
use support::{corpus_root, corpus_sigil_files};

fn values_document(display_name: &str) -> (Value, String) {
    let path = format!("tests/corpus/{display_name}");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = run(
        ["parse", "--json", "--values", path.as_str()].map(OsString::from),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(
        code,
        EXIT_OK,
        "{display_name}: {}",
        String::from_utf8_lossy(&stderr)
    );
    let mut document: Value = serde_json::from_slice(&stdout).unwrap();
    document["file"] = Value::from(display_name);
    let text = String::from_utf8(stdout).unwrap().replacen(
        &format!("\"file\": \"{path}\""),
        &format!("\"file\": \"{display_name}\""),
        1,
    );
    (document, text)
}

fn sidecar_path(display_name: &str) -> PathBuf {
    corpus_root().join(display_name.replace(".sigil", ".values.json"))
}

fn valid_names() -> Vec<String> {
    let names: Vec<String> = corpus_sigil_files()
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| name.starts_with("valid/"))
        .collect();
    assert!(!names.is_empty());
    names
}

#[test]
fn valid_corpus_values_match_their_sidecars() {
    for name in valid_names() {
        let (actual, _) = values_document(&name);
        let path = sidecar_path(&name);
        let expected: Value = serde_json::from_str(
            &fs::read_to_string(&path).unwrap_or_else(|error| panic!("reading {path:?}: {error}")),
        )
        .unwrap_or_else(|error| panic!("parsing {path:?}: {error}"));
        assert_eq!(actual, expected, "values of {name} do not match {path:?}");
        assert_eq!(expected["schema"], "grimoire.sigilc.values");
        assert!(!expected["values"].as_array().unwrap().is_empty());
    }
}

#[test]
#[ignore = "writes the valid corpus' .values.json sidecars; run by hand, then review the diff"]
fn generate_values_sidecars() {
    for name in valid_names() {
        let (_, text) = values_document(&name);
        fs::write(sidecar_path(&name), text).unwrap();
    }
}
