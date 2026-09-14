# Grimoire

Eigenständige 2.5D-Engine in Rust, gebaut from scratch für massenhafte, deterministische
Projektil-Simulationen. Erster Konsument ist das Spiel *Fiends n Patrons*; die Engine kennt
das Spiel nicht und baut, testet und läuft ohne es.

> Status: **Phase P0 — Fundament** (Fenster, instanzierte Sprites, eigenes ECS,
> Fixed-Timestep-Simulation mit Determinismus-Beweis). Alle Rechte vorbehalten.

## Crates

| Crate | Schicht | Aufgabe |
|-------|---------|---------|
| `grimoire_core` | Fundament | Stabiles Hashing, deterministische Mathematik |
| `grimoire_platform` | Plattform | Fenster, Event-Loop, Roh-Input, Uhren, Dateisystem (Desktop + Headless) |
| `grimoire_gpu` | Plattform | Besitzschicht über wgpu (Gerät, Surfaces, Offscreen-Ziele) |
| `grimoire_render` | Engine | Renderer: instanzierte Sprites, Kamera (später Toon, Lichter, Post-FX) |
| `grimoire_ecs` | Engine | Eigenes Archetyp-ECS mit deterministischer Iteration und Snapshots |
| `grimoire_sim` | Engine | Fixed-Timestep, Seed-RNG, InputFrames, Replays, Zustands-Hashes |
| `grimoire_collide` | Engine | 2D-Kollision und Graze-Abfragen (ab P1/P2) |
| `grimoire_audio` | Engine | Mixer, Beat-Clock, Layer (ab P2) |
| `grimoire_ui` | Engine | Spiel-UI (ab P2) |
| `grimoire_assets` | Engine | Pack-Loader, Hot-Swap-Kanal (ab P1) |
| `grimoire_sigil` | Engine | Sigil-Pattern-DSL (ab P1) |
| `grimoire_debug` | Engine | Debug-IPC, Profiler (ab P1) |
| `grimoire` | Fassade | App-Lifecycle, `GamePlugin`, Hauptschleife |

Abhängigkeiten zeigen ausschließlich nach unten: Spiel → Fassade → Engine → Plattform.
Die verbindlichen Schnittstellen beschreibt [docs/architektur/crate-vertraege.md](docs/architektur/crate-vertraege.md).

## Bauen

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Die Toolchain ist über `rust-toolchain.toml` gepinnt und wird von rustup automatisch installiert.
