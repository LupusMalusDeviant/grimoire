//! CLI entry point for `sigilc`, the offline Sigil compiler binary (contract §1, engine
//! ADR-0008).
//!
//! Not yet implemented: source parsing, validation and compilation land in Plan-0002 WP4; the
//! `sigilc simulate` subcommand lands in WP5.6. This binary exists today only so the crate map
//! satisfies engine ADR-0008 with a compiling `sigilc` binary that carries the right dependency
//! edges (`grimoire_sigil`, `grimoire_sim`, `grimoire_ecs`, `grimoire_core`).

fn main() {
    eprintln!(
        "sigilc: not yet implemented (Plan-0002 WP4); this binary exists for crate-map compliance (engine ADR-0008)"
    );
    std::process::exit(1);
}
