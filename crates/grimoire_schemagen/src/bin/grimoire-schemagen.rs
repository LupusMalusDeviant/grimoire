//! CLI for `grimoire_schemagen` (project ADR-0011, Plan-0002 WP8.1).
//!
//! ```text
//! cargo run -p grimoire_schemagen -- generate
//! ```
//!
//! Run from the repository root (every path below is repo-root-relative, matching the style of
//! this repo's other `.github/scripts/*.sh` helpers). Regenerates every file in [`TARGETS`] from
//! its schema and overwrites it in place; `.github/scripts/check-schemagen-drift.sh` is what
//! turns "did this change anything" into a CI failure — this binary itself always succeeds if
//! every schema parses and every output path is writable.

use std::path::Path;
use std::process::ExitCode;

/// One schema's fixed output locations.
///
/// This table — not the schema file itself — decides where generated Rust and docs land (project
/// ADR-0011 "Ort"): a schema describes a wire format, and a format can in principle be read by
/// more than one crate, so tying an output path to the schema source would be the wrong owner for
/// that decision.
struct Target {
    /// Path to the `.gschema` source, relative to the repository root.
    schema_path: &'static str,
    /// Path to the generated Rust module, relative to the repository root.
    rust_output: &'static str,
    /// Path to the generated `docs/formats/*.md` page, relative to the repository root.
    docs_output: &'static str,
}

/// Every schema this tool knows about (contract §12 pack manifest, §13 debug protocol payloads).
/// Adding a schema means adding a row here, not teaching the schema format itself about output
/// paths.
const TARGETS: &[Target] = &[
    Target {
        schema_path: "schema/debug_protocol_v1.gschema",
        rust_output: "crates/grimoire_debug/src/generated/debug_protocol.rs",
        docs_output: "docs/formats/debug-protocol.md",
    },
    Target {
        schema_path: "schema/pack_manifest_v1.gschema",
        rust_output: "crates/grimoire_assets/src/generated/pack_manifest.rs",
        docs_output: "docs/formats/pack.md",
    },
];

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("generate") => run_generate(),
        Some(other) => {
            eprintln!("grimoire-schemagen: unknown subcommand {other:?}; expected 'generate'");
            ExitCode::FAILURE
        }
        None => {
            eprintln!("usage: grimoire-schemagen generate");
            ExitCode::FAILURE
        }
    }
}

fn run_generate() -> ExitCode {
    let mut failed = false;
    for target in TARGETS {
        if let Err(error) = generate_one(target) {
            eprintln!("grimoire-schemagen: {}: {error}", target.schema_path);
            failed = true;
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn generate_one(target: &Target) -> Result<(), String> {
    let schema = grimoire_schemagen::load_schema_file(Path::new(target.schema_path))?;
    let rust_code = grimoire_schemagen::emit_rust::emit(&schema)?;
    let docs = grimoire_schemagen::emit_docs::emit(&schema);

    write_creating_parent(Path::new(target.rust_output), &rust_code)?;
    write_creating_parent(Path::new(target.docs_output), &docs)?;
    Ok(())
}

/// Writes `contents` to `path`, creating `path`'s parent directory first if it does not exist yet
/// (the very first `generate` run creates `crates/*/src/generated/` and `docs/formats/`).
fn write_creating_parent(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    std::fs::write(path, contents)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}
