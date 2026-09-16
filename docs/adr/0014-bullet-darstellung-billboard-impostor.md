# ADR-0014: Bullet-Darstellung — Billboard-Impostor statt instanzierte Low-Poly-Meshes (OF-3.3)

- **Status:** Akzeptiert (PO, 2026-09-16)
- **Datum:** 2026-09-16
- **Entscheider:** Lupus Malus Deviant (PO), Entscheidung aussteht; vorbereitet durch Claude
- **Bezug:** Spiel-Repo Plan 0002 WP3.2/WP3.3, OF-3.3; [PRD-0003](https://github.com/LupusMalusDeviant/fiends-n-patrons/blob/main/docs/prd/0003-rendering-und-art.md)
  Regeln 3/4 (Lesbarkeit), FR-05 (10k+ Bullets); Engine-Vertrag
  [`crate-vertraege.md`](../architektur/crate-vertraege.md) §6 (`BulletInstance`, 24 Byte,
  Vertragsänderungs-Protokoll §2b/WP1.7); [ADR-0013](0013-downlevel-pruefung-licht-cluster-layout.md)
  (Vorlage für Struktur und Messmethodik, liefert `grimoire_render::cluster_layout`); Spike-Branch
  `p1/wp3.2-light-bullet-spikes`, Crate `spikes/wp3.2-light-bullet-stress` (README nennt jede
  Abweichung vom Plan-Text); CI-Lauf [35052422350](https://github.com/LupusMalusDeviant/grimoire/actions/runs/35052422350)
  (Linux, `ubuntu-24.04`, lavapipe, 10 Wiederholungen nach 3 Aufwärmläufen)

## Kontext

Plan 0002 WP3.2 verlangt einen Stressmengen-Spike zu OF-3.3: Billboard-Impostor gegen instanzierte
Low-Poly-Meshes für Bullets, bei 10.000 und 20.000 Instanzen unter der gekippten 2.5D-Kamera,
gemessen headless auf dem Linux-Runner (Median aus 10 Wiederholungen, als „Runner-Wert"
gekennzeichnet). Der Vertrag legt `BulletInstance` bereits fest (24 Byte, `repr(C)`: 2D-Position auf
der Spielebene, Radius, Rotation, Silhouetten-/Paletten-Indizes, Palettenraum, Glow) — passend zu
einem Billboard, aber ohne 3D-Erhebung oder Mesh-Auswahl. Anpassungen daran laufen nur per
Vertrags-PR (WP1.7) vor WP5.3.

Der Spike (`spikes/wp3.2-light-bullet-stress/src/bin/bullet_stress.rs`) baut zwei eigene,
minimale Render-Pipelines (Wegwerf-Code, kein PBR): einen instanzierten Billboard-Pass (Quad aus
dem Vertex-Index, echte Kamera-Ausrichtung über Rechts-/Auf-Vektoren) und einen instanzierten
Mesh-Pass (ein Icosphere `procedural::icosphere(0, 1.0)`, 12 Ecken/20 Dreiecke, als Stand-in für
eine „Low-Poly"-Kugel). Für den Mesh-Pfad definiert der Spike `MeshBulletInstance` (28 Byte:
3D-Position + Mesh-Index statt 2D-Position + Silhouetten-Index) — **nicht** als Code-Änderung an
`BulletInstance`, nur um die Kosten einer solchen Änderung zu messen.

**Kernfrage:** Billboard-Impostor oder instanzierte Low-Poly-Meshes für die Bullet-Ebene (WP3.5),
gemessen an Extraktion, Upload und relativer GPU-Zeit, sowie ihre Konsequenz für PRD-0003 Regeln 3
und 4?

## Anforderungen

1. Getrennte Messung von Extraktion (CPU), Upload (CPU) und GPU-Zeit (nur relativ, Software-Adapter)
   bei 10.000 und 20.000 Instanzen, Median aus 10 Wiederholungen, Runner-Wert mit Streuung.
2. Bewertung, was jede Variante für PRD-0003 Regel 3 (Silhouette statt Farbe: Bullet-Typen einer
   Unit unterscheiden sich in der Silhouette) und Regel 4 (getrennte Palettenräume) bedeutet.
3. Gegenüberstellung der gemessenen Werte mit dem WP3.3-Gate (Extraktion ≤ 0,25 ms).
4. Ein etwaiger Änderungsbedarf an `BulletInstance` wird benannt, aber nicht umgesetzt — er geht
   als Vertrags-PR (WP1.7) vor WP5.3.

## Betrachtete Optionen

### Option A: Billboard-Impostor

Kamera-ausgerichtetes Quad, `BulletInstance` unverändert (24 Byte, passt exakt). Silhouette könnte
nur über eine Textur (Atlas, per `silhouette`-Index adressiert) variieren — im Spike nicht gebaut
(unlit SDF-Kreis für jede Silhouette gleich), da reine Geometrie eines Quads immer dieselbe
Umrisslinie liefert.

**Positiv:** kein Vertragsbruch, kleinste Instanzgröße, güntigste GPU-Kosten (siehe Messung).
**Negativ:** Regel 3 (Silhouette) ist mit reiner Geometrie nicht erfüllbar — braucht einen
Textur-Atlas (Content-/Speicherarbeit, nicht Teil dieses Spikes) statt struktureller Erzwingung
durch Mesh-Auswahl.

### Option B: Instanzierte Low-Poly-Meshes

Ein registriertes Mesh (oder mehrere über einen Mesh-Index) je Silhouette, echte 3D-Kontur unter
der gekippten Kamera erkennbar — Regel 3 strukturell statt über Textur-Inhalt erfüllt. Braucht
`MeshBulletInstance`-artige Felder (3D-Position, Mesh-Index), die `BulletInstance` heute nicht hat.

**Positiv:** Regel 3 strukturell erfüllt, ohne Textur-Atlas.
**Negativ:** Vertragsänderung an `BulletInstance` nötig (Vertrags-PR, WP1.7, vor WP5.3); ~4,2-4,4-fache
relative GPU-Kosten gegenüber Billboard (siehe Messung); größere Instanz (28 statt 24 Byte).

## Messung

Alle Werte sind **Runner-Werte** (Linux, `ubuntu-24.04`, Adapter `llvmpipe (LLVM 20.1.2, 256 bits)`,
Vulkan/lavapipe, `GRIMOIRE_GPU_ADAPTER=software`), Median aus 10 Wiederholungen nach 3 verworfenen
Aufwärmläufen, CI-Lauf [35052422350](https://github.com/LupusMalusDeviant/grimoire/actions/runs/35052422350).
GPU-Zeit ist Wanduhrzeit für Kodieren + `submit` + `device.poll(Wait)` auf dem Software-Adapter —
**nur relativ zwischen den Varianten vergleichbar, kein Millisekunden-Urteil** (Engine-ADR-0010).

| Variante | Instanzen | Extraktion Median (µs) | Extraktion Spanne (µs) | Upload Median (µs) | Upload Spanne (µs) | GPU relativ Median (µs) | GPU relativ Spanne (µs) |
|---|---:|---:|---:|---:|---:|---:|---:|
| Billboard | 10.000 | 17,31 | 16,53–29,45 | 15,19 | 14,35–18,49 | 4.971,24 | 4.851,45–5.232,93 |
| Mesh (Icosphere, 20 Tris) | 10.000 | 41,07 | 37,55–42,92 | 34,85 | 28,51–36,63 | 21.814,07 | 20.759,02–23.666,22 |
| Billboard | 20.000 | 68,85 | 66,55–77,82 | 42,46 | 35,57–56,58 | 9.725,09 | 9.558,10–14.625,70 |
| Mesh (Icosphere, 20 Tris) | 20.000 | 75,09 | 72,76–87,18 | 63,20 | 60,56–86,29 | 40.656,88 | 40.112,50–40.951,10 |

Rohwerte im CI-Artefakt `wp32-light-bullet-stress-results` (`results/bullet_stress.txt`) des oben
verlinkten Laufs.

**Relative GPU-Kosten Mesh/Billboard:** ≈ 4,39× bei 10.000, ≈ 4,18× bei 20.000 Instanzen — konsistent
mit dem zehnfachen Dreieckscount pro Instanz (2 vs. 20 Dreiecke) plus Tiefentest/Normalen-Schattierung,
die der Billboard-Pfad nicht braucht.

### Go/No-Go gegen das WP3.3-Extraktions-Gate (≤ 0,25 ms = 250 µs)

| Variante/Instanzen | Extraktion Median | Anteil am Budget | Urteil |
|---|---:|---:|---|
| Billboard, 10.000 | 17,31 µs | 6,9 % | Go |
| Billboard, 20.000 | 68,85 µs | 27,5 % | Go |
| Mesh, 10.000 | 41,07 µs | 16,4 % | Go |
| Mesh, 20.000 | 75,09 µs | 30,0 % | Go |

Beide Varianten liegen bei beiden Instanzzahlen deutlich unter der 50-%-Schwelle — die Extraktion ist
für keine der beiden Varianten der Engpass. Die "Bullet-Upload plus Clustering"-Hälfte des Gates
(1,5 ms) steht, weil sie den Licht-Culling-Anteil einschließt, in [ADR-0015](0015-licht-culling-compute-clustering.md).

## Lesbarkeits-Betrachtung (PRD-0003 Regeln 3 und 4)

- **Regel 3 (Silhouette statt Farbe allein):** Ein Billboard-Quad hat immer denselben Umriss,
  unabhängig vom `silhouette`-Index — der Spike zeichnet bewusst denselben SDF-Kreis für jede
  Silhouette, um genau das sichtbar zu machen. Regel 3 ist mit reiner Billboard-Geometrie **nicht**
  erfüllbar; sie bräuchte einen Textur-Atlas (pro Silhouette ein Sprite), den `BulletInstance`s
  `silhouette`-Feld laut seinem Doc-Kommentar ohnehin schon für eine künftige Tabelle vorsieht — das
  ist Content-/Speicherarbeit für WP3.5/WP2.7 (Stilbibel), keine Vertragsänderung. Instanzierte
  Meshes erfüllen Regel 3 dagegen strukturell: unterschiedliche Mesh-Indizes ergeben unterschiedliche
  3D-Konturen, auch unter der gekippten Kamera erkennbar, ohne Textur-Inhalt.
- **Regel 4 (getrennte Palettenräume):** Von der Bullet-Darstellung unabhängig — `palette_space`
  sitzt bereits in `BulletInstance` und wird vom Bullet-Pass strukturell geprüft (Vertrag §6). Beide
  Varianten im Spike setzen denselben `HOSTILE`-Konstantwert; Regel 4 ist kein Unterscheidungsmerkmal
  zwischen Billboard und Mesh.

## Empfehlung

**Billboard-Impostor für WP3.5**, mit einem Textur-Atlas (statt Geometrie) als Weg zu Regel 3.
Begründung:

- Keine Vertragsänderung: `BulletInstance` (24 Byte) passt exakt, `BulletInstance`s Leistungsvorgabe
  (Extraktion ≤ 0,5 ms bei 10.000, Vertrag §6) bleibt mit großem Abstand erfüllt (siehe Messung).
- Deutlich günstiger auf dem Software-Adapter (≈ 4,2–4,4× weniger relative GPU-Zeit als die
  Mesh-Variante) — auch wenn P1 keine absoluten GPU-Budgets aus Runner-Werten ableitet (Engine-ADR-0010),
  ist der Faktor groß genug, um als Trend ernst genommen zu werden, bevor WP3.4 das echte
  Clustered-Forward+-Rendering hinzufügt und das GPU-Budget weiter belastet.
- Regel 3 ist mit einem Textur-Atlas lösbar, ohne den Vertrag anzufassen — das ist der günstigere
  Weg im Vergleich zu einer Vertragsänderung plus Mesh-Registrierung plus höherem GPU-Preis.

## Migrationspfad

Kippt die Entscheidung später (z. B. weil der Textur-Atlas für Regel 3 optisch nicht überzeugt, oder
die Messsitzung auf Referenz-Hardware einen anderen Kostenverlauf zeigt):

1. Vertrags-PR (WP1.7) vor WP5.3, additiv wo möglich: `BulletInstance` bekäme entweder eine dritte
   Positions-Komponente (Bruch, da `repr(C)`-Layout und Größe sich ändern — Kompatibilitätsklasse I
   nach §2b) oder einen parallelen `MeshBulletInstance`-Kanal in `StageFrame` (additiv, `#[non_exhaustive]`
   erlaubt das, siehe Vertrag §6 "Trait-Entscheid Renderer-Erweiterung"). Der Spike-Typ
   `MeshBulletInstance` (`spikes/wp3.2-light-bullet-stress/src/bullets.rs`, 28 Byte: 3D-Position,
   Rotation, Skalierung, Mesh-Index, Palette, gepacktes Palettenraum/Glow-Feld) ist ein Vorschlag für
   diesen zweiten Weg, nicht bindend.
2. Der Bullet-Pass (WP3.5) müsste zwei Pipelines pflegen (Billboard weiterhin für die Masse,
   Mesh optional für besonders auffällige/seltene Bullet-Typen) oder vollständig wechseln — beides
   ist mit dem hier gemessenen ~4×-Kostenfaktor gegen das 8-ms-GPU-Budget der Messsitzung
   (Plan 0002 WP3.3) neu zu prüfen.
3. Der Icosphere-Platzhalter (`grimoire_render::procedural::icosphere`) ist keine Endform — eine
   echte Low-Poly-Bullet-Form bräuchte eigene Autorenarbeit oder eine kleine Formbibliothek.

## Entscheidung

**Vorschlag:** Billboard-Impostor für WP3.5, kein Änderungsbedarf an `BulletInstance`. PO-Entscheid
aussteht.

Diese ADR gilt erst nach PO-Freigabe als angenommen.

## Konsequenzen

### Positiv

- Kein Vertrags-PR vor WP5.3 nötig; `BulletInstance` bleibt wie vom WP1.2-Vertrag festgelegt.
- Messbar günstigere relative GPU-Kosten, wichtig, weil WP3.4 (Clustered Forward+) und WP3.5
  (Bullet-Pass) auf demselben Frame-Budget konkurrieren.
- Extraktion bleibt bei beiden Instanzzahlen weit unter dem WP3.3-Gate.

### Negativ

- Regel 3 braucht einen Textur-Atlas (WP2.7/WP3.5-Arbeit), keine strukturelle Erzwingung durch
  Geometrie — ein Bug im Atlas-Inhalt (zwei Silhouetten sehen zufällig ähnlich aus) fällt nicht
  automatisch auf, wie es ein Mesh-Unterschied täte.
- Die relative GPU-Zahl ist ein Software-Adapter-Trend, kein Budget-Nachweis (Engine-ADR-0010) —
  die Messsitzung auf Referenz-Hardware (Plan 0002 WP3.3) kann den Faktor verschieben.
- Der Icosphere-Vergleichswert (20 Dreiecke) ist eine von vielen möglichen "Low-Poly"-Größen; eine
  noch einfachere Mesh-Form (z. B. 8–12 Dreiecke) wäre günstiger als hier gemessen, aber immer noch
  eine Vertragsänderung.

## Weitere Informationen

- Code: `spikes/wp3.2-light-bullet-stress/src/bin/bullet_stress.rs`,
  `src/bullets.rs` (`MeshBulletInstance`, Extraktion), `src/bin/billboard.wgsl`, `src/bin/mesh.wgsl`,
  `src/camera.rs` (Kamera-Stand-in, siehe README "Abweichungen").
- CI: `.github/workflows/spike-wp3.2-light-bullet-stress.yml`, nur auf Branch
  `p1/wp3.2-light-bullet-spikes`; Lauf [35052422350](https://github.com/LupusMalusDeviant/grimoire/actions/runs/35052422350)
  grün (fmt, clippy, Korrektheitstests, Messung).
- Verwandt: [ADR-0015](0015-licht-culling-compute-clustering.md) (Licht-Culling, teilt sich das
  WP3.3-Kombi-Gate "Bullet-Upload plus Clustering").
- Offener Punkt für den Product Owner: diese Empfehlung (Billboard, Textur-Atlas für Regel 3)
  bestätigen oder die Mesh-Variante trotz höherer Kosten für besondere Bullet-Typen vorsehen. Meine
  Empfehlung: Billboard annehmen, Mesh-Migrationspfad als dokumentierte Option offenhalten statt
  jetzt zu bauen.
