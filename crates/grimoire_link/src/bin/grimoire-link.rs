//! CLI entry point for `grimoire-link` (contract §1, engine ADR-0008; Plan 0002 WP8.5).
//!
//! Compiles a `.sigil` file with `grimoire_sigilc` and pushes the resulting unit to a running
//! engine over the debug link (`grimoire_debug`, feature `tcp`) — the Rust-only proof of hot
//! reload without the C# tooling suite. Everything lives in [`grimoire_link::cli`]; this binary
//! only forwards the arguments and the standard streams. Not a release artefact in P1 (PO decision
//! P-14).

fn main() -> std::process::ExitCode {
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    let code = grimoire_link::cli::run(std::env::args_os().skip(1), &mut stdout, &mut stderr);
    std::process::ExitCode::from(code)
}
