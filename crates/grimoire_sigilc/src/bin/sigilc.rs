//! CLI entry point for `sigilc`, the offline Sigil compiler binary (contract §1, engine
//! ADR-0008).
//!
//! Plan 0002 WP4.1 lands two demonstration paths for the lexer/parser's diagnostics: `sigilc
//! check <file>...` (human-readable text) and `sigilc parse --json <file>` (the JSON document
//! shape contract §2 rule 11 requires of `sigilc --json`, [`grimoire_sigilc::DiagnosticsDocument`]).
//! Source parsing and validation themselves live in the library
//! ([`grimoire_sigilc::parse`]); this binary only wires them to argv and an exit code.
//!
//! Plan 0002 WP4.2 adds the schema pass, name resolution and the `SigilUnit` encoder as a library
//! ([`grimoire_sigilc::compiler::compile`]), but does not wire it into this binary: a real `sigilc
//! build` needs a `SourceLoader` backed by a content root and a way to supply the
//! name-to-`BehaviorId` table `compile` takes (`docs/formats/sigil.md` §11.4), both left to Plan
//! 0002 WP4.3 alongside `set`, `migrate`, `fmt` and `simulate` (WP5.6).

use std::process::ExitCode;

use grimoire_sigilc::{DiagnosticsDocument, parse};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("check") => run_check(args.collect()),
        Some("parse") => run_parse(args.collect()),
        Some("--help" | "-h") | None => {
            print_usage();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("sigilc: unknown subcommand `{other}`\n");
            print_usage();
            ExitCode::FAILURE
        }
    }
}

fn print_usage() {
    eprintln!(
        "sigilc: offline compiler for Sigil (.sigil) source (Plan 0002 WP4)\n\
         \n\
         USAGE:\n\
         \x20 sigilc check <file>...      Parse one or more files; print diagnostics as text.\n\
         \x20 sigilc parse --json <file>  Parse one file; print its diagnostics as JSON.\n\
         \n\
         Exit code is 0 when every given file parses with no diagnostics, 1 otherwise.\n\
         Not yet implemented: `build`, `set`, `migrate`, `fmt`, `simulate` (Plan 0002 WP4.2/WP4.3/WP5.6)."
    );
}

/// Reads and parses `path`, returning its source on success. On an I/O error, prints a message
/// to stderr and returns `None`; never panics on a missing or unreadable file (contract §2 rule
/// 9 in spirit, even though a filesystem read is not itself a "decoder").
fn read_source(path: &str) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(source) => Some(source),
        Err(error) => {
            eprintln!("sigilc: could not read `{path}`: {error}");
            None
        }
    }
}

fn run_check(files: Vec<String>) -> ExitCode {
    if files.is_empty() {
        eprintln!("sigilc check: expected at least one file");
        return ExitCode::FAILURE;
    }
    let mut ok = true;
    for path in files {
        let Some(source) = read_source(&path) else {
            ok = false;
            continue;
        };
        let output = parse(path, &source);
        for diagnostic in &output.diagnostics {
            println!("{}", diagnostic.render_text());
            ok = false;
        }
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn run_parse(args: Vec<String>) -> ExitCode {
    let mut json = false;
    let mut file = None;
    for arg in args {
        if arg == "--json" {
            json = true;
        } else if file.is_none() {
            file = Some(arg);
        } else {
            eprintln!("sigilc parse: unexpected extra argument `{arg}`");
            return ExitCode::FAILURE;
        }
    }
    let Some(path) = file else {
        eprintln!("sigilc parse: expected a file path");
        return ExitCode::FAILURE;
    };
    if !json {
        eprintln!(
            "sigilc parse: only `--json` output is implemented so far (Plan 0002 WP4.1); \
             use `sigilc check` for text diagnostics"
        );
        return ExitCode::FAILURE;
    }
    let Some(source) = read_source(&path) else {
        return ExitCode::FAILURE;
    };
    let output = parse(path.clone(), &source);
    let no_diagnostics = output.diagnostics.is_empty();
    let document = DiagnosticsDocument::new(path, output.diagnostics);
    match document.to_json_pretty() {
        Ok(json) => {
            println!("{json}");
            if no_diagnostics {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("sigilc: could not encode diagnostics as JSON: {error}");
            ExitCode::FAILURE
        }
    }
}
