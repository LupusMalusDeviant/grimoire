# ADR-0011: Kantenglättung für Glanzlichter — geometrisches Spekular-Anti-Aliasing statt TAA (OF-3.5)

- **Status:** Vorgeschlagen (2026-09-16; PO-Entscheid aussteht)
- **Datum:** 2026-09-16
- **Entscheider:** Lupus Malus Deviant (PO), Entscheidung aussteht; vorbereitet durch Claude
- **Bezug:** Spiel-Repo [ADR-0014](https://github.com/LupusMalusDeviant/fiends-n-patrons/blob/main/docs/adr/0014-realistischer-3d-look-statt-toon.md)
  (realistischer 3D-Look, OF-3.5 als offener Punkt), [PRD-0003](https://github.com/LupusMalusDeviant/fiends-n-patrons/blob/main/docs/prd/0003-rendering-und-art.md)
  (FR-01, OF-3.5, Regeln 1/2), [Stilbibel](https://github.com/LupusMalusDeviant/fiends-n-patrons/blob/main/docs/art/stilbibel.md)
  („Offene Punkte für das Look-Review", Empfehlung Spekular-AA), Plan 0002 WP2.5 (Spiel-Repo),
  Engine-Vertrag [`crate-vertraege.md`](../architektur/crate-vertraege.md) §6, Look-Dev-Spike auf
  Branch `p1/wp2-look-dev-spike` (`spikes/look-dev/src/shaders/world.wgsl`, `fs_realistic`)

## Kontext

ADR-0014 (Spiel-Repo) entscheidet den realistischen PBR-Look und benennt als Folgekonsequenz:
„Glanzlichter flimmern ohne TAA oder Spekular-AA" — OF-3.5 fragt, welche der beiden Techniken WP2.5
umsetzt. PRD-0003 FR-01 verlangt „Kantenglättung für Glanzlichter (TAA oder Spekular-AA)"; die
Stilbibel empfiehlt bereits vorab (unter „Offene Punkte für das Look-Review", noch nicht vom PO
bestätigt) geometrisches Spekular-AA „wie im Realistic-Shader des Spikes … günstiger, kein
History-Buffer nötig, Ebene 4/6 ohnehin unberührt".

Der Look-Dev-Spike (`spikes/look-dev`) hat bereits eine Referenzimplementierung: `fs_realistic`
weitet `alpha²` (den quadrierten GGX-Rauheitsparameter) um die Bildschirmraum-Varianz der
Schattierungsnormale (`dpdx`/`dpdy`), eine Standardtechnik (Kaplanyan 2016 „Stable Specular
Highlights"; Tokuyoshis Variante, u. a. in Filament verwendet). WP2.5 hat diese Technik in
`grimoire_render::mesh_pass`/`mesh.wgsl` übernommen (siehe „Entscheidung" unten) und dabei den
Vergleich gegen „keine Kantenglättung" gemessen, den diese ADR dokumentiert. TAA selbst wurde nicht
gebaut (siehe „Betrachtete Optionen", Option B) — es bräuchte einen History-Puffer und
Bewegungsvektoren, die es in P1 noch nicht gibt.

**Kernfrage:** Welche Kantenglättung für Glanzlichter erhält WP2.5 in `grimoire_render`, und
rechtfertigen die gemessenen Zahlen diese Wahl?

## Anforderungen

- PRD-0003 FR-01: Kantenglättung für Glanzlichter, TAA oder Spekular-AA.
- PRD-0003 Regel 1/2 und der OF-3.5-Zusatzkriterium aus Plan 0002 WP2.5: Ebene 4 (Telegraphie) und
  Ebene 6 (Bullets) bleiben von der gewählten Technik unberührt.
- Kosten sind auf dem Software-Adapter relativ messbar (keine GPU-Budget-Zusage in P1, Plan 0002
  WP2.5/OF-3.5).
- Kein unverhältnismäßiger Aufbau: Ein History-Buffer samt Bewegungsvektoren für TAA ist in P1 noch
  nicht vorhanden (keine der bestehenden Kamera- oder Render-Verträge liefert Bewegungsvektoren).

## Betrachtete Optionen

### Option A: Keine Kantenglättung

**Positiv:** kein zusätzlicher Code, keine Kosten.
**Negativ:** ADR-0014 benennt Glanzlicht-Flimmern explizit als Folgeproblem des PBR-Looks; FR-01
verlangt eine der beiden Techniken. Verworfen ohne Messung, weil es die Anforderung nicht erfüllt.

### Option B: TAA (temporales Supersampling)

Akkumuliert über mehrere Frames anhand eines History-Buffers und Bewegungsvektoren.

**Positiv:**
- Glättet nicht nur Glanzlichter, sondern auch Geometriekanten und Schatten — der umfassendere
  Ansatz für spätere Arbeitspakete (WP2.6 Schatten, WP3.4 geclustertes Licht).
- Reduziert echtes Bild-zu-Bild-Flimmern durch Bewegung (temporale Instabilität), nicht nur
  räumliches Spekular-Aliasing innerhalb eines Standbilds.

**Negativ:**
- Braucht einen History-Puffer (ein zusätzliches Ziel pro Frame) und Bewegungsvektoren — Letztere
  existieren in keinem bisherigen Render-Vertrag (§6 hat weder eine vorherige View-Projection noch
  einen Vorwärts-Bewegungsvektor pro Instanz) und müssten für Kamera *und* jede bewegte
  `MeshInstance` neu hergeleitet werden.
- Geisterbilder (Ghosting) bei schneller Kamerabewegung oder Content-Wechsel sind ein bekanntes
  Risiko, gerade beim renderseitigen Kamera-Following aus WP2.4 (Feder plus Vorausschau) und bei
  einem noch schnelleren Bullet-Feld (WP3.5) — beides zusätzlicher Abstimmungsaufwand.
- Nach PRD-0003 Regel 1/2 dürfte ein TAA-Resolve ohnehin nie Ebene 4/6 (Telegraphie/Bullets)
  anfassen; das verlangt einen eigenen Ausschluss-Mechanismus (Bullets/Telegraphie müssten aus dem
  History-Buffer ausmaskiert oder nach dem TAA-Resolve gezeichnet werden — Letzteres ist ohnehin
  schon die Ebenenreihenfolge, siehe „Konsequenzen").
- Zeitbudget: Ein History-Puffer, Reprojektion und Bewegungsvektoren für Kamera und Instanzen sind
  ein eigenständiges Arbeitspaket, kein Bestandteil von WP2.5 (Plan 0002 nennt WP2.5 nur „PBR-Shading
  … Spike OF-3.5 TAA gegen Spekular-AA").
- Nicht gebaut und nicht gemessen (siehe „Bewertung auf dem Papier" unten statt einer Zahl).

### Option C: Geometrisches Spekular-Anti-Aliasing (vorgeschlagen, umgesetzt)

Weitet den GGX-Rauheitsparameter pro Fragment um die Bildschirmraum-Varianz der Schattierungsnormale
(`dpdx`/`dpdy`), wie im Look-Dev-Spikes `fs_realistic`.

**Positiv:**
- Kein History-Puffer, keine Bewegungsvektoren, kein Ghosting-Risiko — arbeitet ausschließlich
  innerhalb des aktuellen Bildes.
- Bereits im Spike erprobt und für WP2.5 direkt in `mesh.wgsl` übernommen (siehe unten); die
  Stilbibel empfiehlt sie schon vorab.
- Ebene 4/6 sind strukturell unberührt: Der Mesh-Pass zeichnet ausschließlich
  `RenderLayer::World` (Vertrag §6); es gibt in P1 keinen Shader-Code, der Telegraphie oder Bullets
  überhaupt anfasst, unabhängig von dieser Technik.
- Aktivierbar per Umschalter im Fragment-Shader (`camera.specular_aa_strength`, uniform pro
  Draw-Call — legale, uniforme Kontrollflusssteuerung für `dpdx`/`dpdy`), sodass sich die Kosten der
  Technik isoliert messen lassen (siehe „Messung").

**Negativ:**
- Glättet nur Spekular-Aliasing, nicht Geometriekanten oder Schatten — anders als TAA kein
  Gesamtpaket für spätere Post-FX-Bedürfnisse.
- Reduziert räumliches (Pixel-zu-Pixel innerhalb eines Bildes) Aliasing, nicht temporale
  Instabilität durch Kamerabewegung — die Messung unten bestätigt das (siehe „Ergebnis Flimmer-Maß").

## Messung

Gemessen mit `cargo test -p grimoire_render --test offscreen -- --ignored --nocapture
measure_specular_aa` (`tests/offscreen.rs::measure_specular_aa_relative_cost_and_shimmer`), auf dem
Software-Adapter (`GRIMOIRE_GPU_ADAPTER=software`, WARP unter Windows/DX12 — **kein GPU-Budget**,
nur eine relative, plattform-eigene Zahl, wie WP2.5/OF-3.5 es verlangt). Szene: 500 PBR-Kugeln mit
zufälligen Materialien (Metallizität, Rauheit) plus 10.000 Sprites, 8 Punktlichter, 1280x720,
Mittel über 30 Frames nach 5 Aufwärm-Frames.

### Ergebnis: relative Kosten

| Variante | CPU-Zeit/Frame (Median dreier Läufe) |
|---|---|
| Spekular-AA an | ≈ 940–1025 µs |
| Spekular-AA aus | ≈ 720–740 µs |

Relativer Mehraufwand ≈ 1,3× (rund 30 %) auf dem Software-Adapter. `mesh.wgsl` berechnet die beiden
`dpdx`/`dpdy`-Aufrufe nur, wenn `camera.specular_aa_strength` gesetzt ist (uniform pro Draw-Call,
kein Sprung pro Fragment) — die Messung isoliert damit tatsächlich die Kosten der Technik selbst,
nicht nur einen konstanten Term, der ohnehin immer berechnet würde. Der absolute Wert bleibt eine
Software-Adapter-Zahl: `dpdx`/`dpdy` sind auf echter GPU-Hardware Kernprimitive (auch für Mipmapping
verwendet) und dort nach aller Erfahrung deutlich günstiger als ein Interpretationslauf auf CPU-
Rasterung; eine Referenz-Hardware-Zahl bräuchte die für P1 vorgesehene Messsitzung (Plan 0002 WP3.3),
nicht diesen Spike.

### Ergebnis: Flimmer-Maß

Gemessen als quadratisches Mittel der Leuchtdichtedifferenz zwischen zwei Frames derselben Szene,
deren Kamera-Neigung um 0,5° auseinanderliegt (ein grober Ersatz für Kamera-Jitter, ohne echte
Bewegung über mehrere Frames):

| Variante | Flimmer-Maß (RMS-Leuchtdichtedifferenz) |
|---|---|
| Spekular-AA an | 21,048 |
| Spekular-AA aus | 20,826 |

Beide Werte liegen praktisch gleich auf (< 1,1 % Unterschied, deterministisch reproduzierbar über
mehrere Läufe). Das ist **kein Fehlschlag der Technik, sondern eine Eigenschaft der Messmethode**:
geometrisches Spekular-AA glättet Aliasing *innerhalb eines Standbilds* (Sub-Pixel-Normalvarianz
eines gekrümmten Materials), nicht das Bild-zu-Bild-Flimmern einer bewegten Kamera zwischen zwei
diskreten Frames — Letzteres ist der eigentliche Anwendungsfall von TAA (zeitliche Akkumulation über
mehrere Frames mit Reprojektion). Ein aussagekräftigeres Maß für den tatsächlichen Nutzen von
Spekular-AA bräuchte einen Vergleich gegen eine supersampelte Referenz (z. B. 4×4-Subpixel-Rendering
und Downsampling als Ziel), was den Zeitrahmen dieses Spikes sprengen würde.

### Bewertung von TAA auf dem Papier (nicht gebaut)

TAA wurde entsprechend Plan 0002 WP2.5 („wenn das für diesen Schritt unverhältnismäßig ist, auf dem
Papier gegen die Messung bewerten") nicht implementiert, weil ein History-Puffer und
Bewegungsvektoren fehlen (siehe Option B). Aufwandsschätzung: mindestens ein zusätzliches
Render-Ziel (History), eine Reprojektionsmatrix pro Frame, Bewegungsvektoren pro Instanz (neu im
Vertrag §6) und eine Ghosting-Abschwächung (Clamping/Clipping der History-Farbe) — deutlich über dem
Umfang eines einzelnen Arbeitspakets. Der erwartete Vorteil (auch Geometriekanten und
temporale Instabilität glätten) ist real, aber nicht Teil der in FR-01 geforderten
Glanzlicht-Kantenglättung allein.

## Entscheidung

**Vorschlag: Option C, geometrisches Spekular-Anti-Aliasing.** Bereits umgesetzt in
`grimoire_render::mesh_pass`/`mesh.wgsl` (WP2.5): Für jedes schattierte Fragment wird `alpha²` um
`min(2 · Varianz, 0.18)` geweitet, wobei die Varianz aus `dpdx(n)`/`dpdy(n)` der (nach
Normal-Mapping) tangentialen Schattierungsnormale berechnet wird, gated durch den
Draw-Call-Uniform `camera.specular_aa_strength` (Produktionspfad `render_stage`: immer aktiv; der
Vergleichs-Pfad `WgpuRenderer::render_stage_with_specular_aa` ist ein Mess-Hook für diese ADR und
für `tests/offscreen.rs`, kein Vertragsbestandteil).

Diese ADR gilt erst nach PO-Freigabe als angenommen; bis dahin bleibt die Umsetzung im Code
lauffähig (die Tests bestehen unabhängig vom Status dieser ADR), aber der Status hier zeigt, dass
die Entscheidung noch nicht formal bestätigt ist.

## Konsequenzen

### Positiv

- FR-01 ist erfüllt, ohne einen History-Puffer oder Bewegungsvektoren einzuführen, die P1 sonst
  nirgends bräuchte.
- Ebene 4/6 bleiben strukturell unberührt (Mesh-Pass zeichnet nur `RenderLayer::World`), unabhängig
  vom Ausgang dieser ADR.
- Die Kosten sind isoliert messbar (siehe „Messung") und bewegen sich in einer Größenordnung, die
  auf echter GPU-Hardware voraussichtlich unauffällig ist (zwei Kernprimitiv-Aufrufe pro Fragment).

### Negativ

- Kein Fortschritt gegen temporale Instabilität (Kamerabewegung, animierte Materialien) — sollte
  sich das im Spiel als Problem zeigen, bleibt TAA (oder ein einfacherer Jitter/Resolve-Ansatz) ein
  mögliches Folge-ADR, dann mit echtem Bewegungsvektor-Bedarf aus dem Gameplay.
- Die Flimmer-Messung dieser ADR ist kein belastbarer Nachweis eines Nutzens gegen echtes
  Bild-zu-Bild-Flimmern (siehe „Ergebnis Flimmer-Maß") — nur ein Hinweis, dass die Technik zumindest
  nicht schadet.
- `camera.specular_aa_strength` und `WgpuRenderer::render_stage_with_specular_aa` sind ein
  Mess-/Vergleichs-Hook, kein Vertragsbestandteil (Vertrag §6 bleibt unverändert); sie können nach
  der PO-Entscheidung wieder entfernt werden, falls nicht mehr gebraucht.
- Sollte der PO stattdessen TAA wählen, ersetzt ein Folge-ADR diese Entscheidung; die
  Spekular-AA-Implementierung bliebe als Fallback für die Zeit bis TAA existiert.
