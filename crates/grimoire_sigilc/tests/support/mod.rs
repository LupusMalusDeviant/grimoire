//! Shared helper for `corpus.rs` and `roundtrip.rs`: enumerates the conformance corpus under
//! `tests/corpus/{valid,invalid}/*.sigil` (Plan 0002 WP4.1 requirement 5).
//!
//! Lives under `tests/support/` (a subdirectory, `mod.rs`) rather than directly as a file under
//! `tests/`, specifically so Cargo does not also treat it as its own (test-less) integration test
//! binary — only files directly under `tests/` are auto-discovered as separate test targets.

use std::fs;
use std::path::PathBuf;

/// The corpus root, `crates/grimoire_sigilc/tests/corpus`.
pub fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

/// Every `.sigil` file under `valid/` and `invalid/`, as `(display_name, source)` pairs sorted by
/// display name for deterministic test output. `display_name` is corpus-relative with forward
/// slashes (e.g. `"valid/01-ring-burst.sigil"`, `"invalid/e-header-missing.sigil"`) — the same
/// name a sibling `.expected.json` is keyed by, and the same shape a C# conformance test
/// (Plan 0002 WP4.1 requirement 5: this corpus is data, not a Rust-specific encoding) can use to
/// address the same file.
/// `#[allow(dead_code)]`: `compiler_corpus.rs` compiles its own copy of this shared module too
/// (see `expected_json_path`'s doc comment below) but only ever calls [`corpus_root`], not this.
#[allow(dead_code)]
pub fn corpus_sigil_files() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for group in ["valid", "invalid"] {
        let dir = corpus_root().join(group);
        let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
            .unwrap_or_else(|error| panic!("reading corpus directory {dir:?}: {error}"))
            .map(|entry| {
                entry
                    .unwrap_or_else(|error| panic!("reading {dir:?}: {error}"))
                    .path()
            })
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("sigil"))
            .collect();
        paths.sort();
        for path in paths {
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_else(|| panic!("non-UTF-8 corpus file name: {path:?}"));
            let display_name = format!("{group}/{file_name}");
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("reading {path:?}: {error}"));
            out.push((display_name, source));
        }
    }
    out
}

/// The path to `<group>/<name>.expected.json` for a corpus display name such as
/// `"valid/01-ring-burst.sigil"`.
///
/// `#[allow(dead_code)]`: each of `corpus.rs` and `roundtrip.rs` compiles its own copy of this
/// shared module (`tests/`'s per-file integration-test-binary model), and only `corpus.rs` calls
/// this particular helper — a real, intentional case of "not every consumer of a shared module
/// uses every item in it", not a leftover.
#[allow(dead_code)]
pub fn expected_json_path(display_name: &str) -> PathBuf {
    corpus_root().join(display_name.replace(".sigil", ".expected.json"))
}
