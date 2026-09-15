//! CLI entry point for `grimoire-link` (contract §1, engine ADR-0008).
//!
//! Eventually compiles a `.sigil` file via `grimoire_sigilc` and pushes the resulting unit to a
//! running engine over the debug link (`grimoire_debug`, feature `tcp`) — the Rust-only proof of
//! hot-reload without the C# tooling suite (Plan-0002 WP8.4/WP8.5). Not yet implemented; this
//! binary exists today only so the crate map satisfies engine ADR-0008 with a compiling
//! `grimoire-link` binary that carries the right dependency edges. It is not a release artefact
//! in P1 (PO decision P-14).

fn main() {
    eprintln!(
        "grimoire-link: not yet implemented (Plan-0002 WP8.4/WP8.5); this binary exists for crate-map compliance (engine ADR-0008)"
    );
    std::process::exit(1);
}
