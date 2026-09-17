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
| [0007](0007-sigil-quelltextsyntax-v1.md) | Sigil-Quelltextsyntax v1 (akzeptiert 2026-09-15) |
| [0008](0008-crate-map-erweiterung-p1.md) | Crate-Map-Erweiterung P1 (akzeptiert 2026-09-15) |
| [0009](0009-lizenz-alle-rechte-vorbehalten.md) | Lizenz der Engine: Alle Rechte vorbehalten |
| [0010](0010-benchmark-strategie.md) | Benchmark-Strategie für das Regressions-Gate (OF-17.3, akzeptiert 2026-09-15) |
| [0011](0011-spekulares-anti-aliasing-statt-taa.md) | Kantenglättung für Glanzlichter: geometrisches Spekular-Anti-Aliasing statt TAA (OF-3.5, angenommen 2026-09-16) |
| [0012](0012-schatten-key-light-shadowmap-plus-blob.md) | Schatten: Shadowmap für das Key-Light plus Blob-Schatten (OF-3.2, angenommen 2026-09-16; Punktlicht-Schattenwerfer zurückgestellt) |
| [0013](0013-downlevel-pruefung-licht-cluster-layout.md) | Downlevel-Prüfung und Licht-/Cluster-Datenlayout für Clustered Forward+ (WP3.1, angenommen 2026-09-16) |
| [0014](0014-bullet-darstellung-billboard-impostor.md) | Bullet-Darstellung: Billboard-Impostor statt instanzierte Low-Poly-Meshes (OF-3.3, angenommen 2026-09-16) |
| [0015](0015-licht-culling-compute-clustering.md) | Licht-Culling: Compute-Clustering statt CPU-Froxel-Zuordnung (angenommen 2026-09-16) |
| [0016](0016-multisampling-fuer-geometriekanten.md) | Multisampling für Geometriekanten (Texturqualität, Strang B1, angenommen 2026-09-17) |
| [0017](0017-skelettanimation-abtastung-in-der-praesentation.md) | Skelettanimation: Abtastung in der Präsentation, Takt aus der Simulation (angenommen 2026-09-17) |
| [0018](0018-subsystem-hashes-erkennen-alle-n-ticks-eingrenzen-je-system.md) | Subsystem-Hashes: erkennen alle 60 Ticks, eingrenzen je System (OF-18.1, angenommen 2026-09-18) |

Den aktuellen Status jeder Entscheidung nennt ihre Datei.
