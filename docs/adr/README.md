# Architekturentscheidungen der Engine

Engine-spezifische Entscheidungen von Grimoire. Projektweite Grundsatzentscheidungen (Rust + C#,
eigenes Engine-Repo, wgpu, eigenes ECS, Determinismus, Sigil, Asset-Pipeline, Avalonia,
Engine-Pin) stehen im Spiel-Repo `fiends-n-patrons` unter `docs/adr/`.

Format: MADR auf Deutsch (Status, Kontext, Anforderungen, Optionen, Entscheidung, Konsequenzen).
Eine ersetzte Entscheidung bleibt stehen und verweist auf ihre Nachfolgerin.

| Nr. | Entscheidung |
|-----|--------------|
| [0001](0001-winit-als-fenster-schicht.md) | winit als Fenster- und Event-Schicht |
| [0002](0002-gpu-kapselungsgrenze.md) | Kapselungsgrenze für wgpu |
| [0003](0003-scheduler-single-threaded.md) | Single-threaded, streng geordneter Scheduler mit Migrationspfad (abgelehnt, ersetzt durch 0006) |
| [0004](0004-deterministische-gleitkommaarithmetik.md) | Deterministische Gleitkommaarithmetik |
| [0005](0005-grimoire-core-als-blatt-crate.md) | `grimoire_core` als abhängigkeitsfreie Blatt-Crate |
| [0006](0006-paralleler-scheduler-deterministische-zusammenfuehrung.md) | Paralleler Scheduler mit deterministischer Zusammenführung |
| [0009](0009-lizenz-alle-rechte-vorbehalten.md) | Lizenz der Engine: Alle Rechte vorbehalten (Nummer vorläufig bis zum Merge der P1-Branches) |

Den aktuellen Status jeder Entscheidung nennt ihre Datei.
