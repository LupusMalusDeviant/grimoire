# Grimoire

Eigenständige 2.5D-Engine in Rust, gebaut from scratch für massenhafte, deterministische
Projektil-Simulationen. Erster Konsument ist das Spiel *Fiends n Patrons*; die Engine kennt
das Spiel nicht und baut, testet und läuft ohne es.

> Status: **Version 0.1.0 — Phase P0 „Fundament“ abgeschlossen.** Fenster, instanzierte Sprites
> (ein Draw-Call), eigenes Archetyp-ECS, Fixed-Timestep-Simulation mit goldenem Determinismus-Hash und
> die Fassade `grimoire` mit `App`, `GamePlugin`, `InputMap` und Hauptschleife stehen. Die CI ist auf
> Windows, Linux und macOS grün; die Golden-Hashes sind auf allen drei Plattformen identisch
> (Engine-ADR-0004 angenommen).

## Hello Grimoire

Ein Spiel hängt nur von `grimoire` ab und beschreibt sich als `GamePlugin`: `build` legt Komponenten,
Systeme und Start-Entities in der Simulation an, `extract` übersetzt den Weltzustand in Sprites.

```rust
use grimoire::prelude::*;

#[derive(Clone)]
struct Position {
    at: Vec2,
}
impl_stable_hash!(Position { at });

struct Hello;

impl GamePlugin for Hello {
    fn name(&self) -> &str {
        "hello"
    }

    fn build(&mut self, sim: &mut Simulation) {
        sim.world_mut().spawn((Position { at: Vec2::ZERO },));
        sim.schedule_mut().add_system(system_fn("walk", |world| {
            let input = world.resource::<TickInput>().copied().unwrap_or_default();
            let step = Vec2::new(input.slots[0].axis(0), input.slots[0].axis(1));
            for position in world.query_mut::<&mut Position>() {
                position.at += step;
            }
        }));
    }

    fn extract(&mut self, world: &World, _alpha: f32, frame: &mut RenderFrame) {
        for position in world.query::<&Position>() {
            frame.sprites.push(SpriteInstance {
                position: position.at.to_array(),
                half_size: [1.0, 1.0],
                shape: shape::CIRCLE,
                color: [1.0, 0.8, 0.2, 1.0],
                ..SpriteInstance::default()
            });
        }
    }
}

fn main() -> Result<(), GrimoireError> {
    App::new(WindowConfig::default())
        .seed(42)
        .exit_key(KeyCode::Escape)
        .plugin(Hello)
        .run()
}
```

WASD oder die Pfeiltasten bewegen den Punkt. Ohne Fenster läuft dieselbe Simulation mit
`run_headless(ticks, &mut |tick| TickInput::default())`, die echte Hauptschleife mit
`run_headless_frames(frames, frame_delta)`. Das vollständige Beispiel mit 5.000 Entities und
Interpolation liegt in [crates/grimoire/examples/sim_loop.rs](crates/grimoire/examples/sim_loop.rs).

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

## Lizenz

Copyright (c) 2026 Lupus Malus Deviant. Alle Rechte vorbehalten. Das Repo ist öffentlich einsehbar,
räumt aber keine Rechte zur Nutzung, Bearbeitung oder Weitergabe ein. Es gilt die Datei
[LICENSE](LICENSE); die Entscheidung beschreibt
[Engine-ADR-0009](docs/adr/0009-lizenz-alle-rechte-vorbehalten.md).
