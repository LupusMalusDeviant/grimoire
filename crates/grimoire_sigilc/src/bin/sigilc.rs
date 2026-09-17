//! CLI entry point for `sigilc`, the offline Sigil compiler binary (contract §1, engine
//! ADR-0008; Plan 0002 WP4.3).
//!
//! Everything but the process boundary lives in the library ([`grimoire_sigilc::cli::run`]): this
//! binary only hands it the arguments and the standard streams and turns its result into the exit
//! code. See `docs/formats/sigil.md` §13 for the commands, their JSON documents and exit codes.

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    ExitCode::from(grimoire_sigilc::cli::run(
        std::env::args_os().skip(1),
        &mut stdout,
        &mut stderr,
    ))
}
