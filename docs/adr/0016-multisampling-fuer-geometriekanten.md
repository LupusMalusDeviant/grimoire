# ADR-0016: Multisampling für Geometriekanten (Texturqualität, Strang B1)

- **Status:** Akzeptiert (PO, 2026-09-17)
- **Datum:** 2026-09-16
- **Entscheider:** Lupus Malus Deviant (PO), vorbereitet durch Claude
- **Bezug:** Festlegung „Texturqualität" (PO-Entscheide 2026-09-16, Spiel-Repo-Scratchpad, Strang B1),
  Engine-Vertrag [`crate-vertraege.md`](../architektur/crate-vertraege.md) §6, engine
  [ADR-0011](0011-spekulares-anti-aliasing-statt-taa.md) (Spekular-Anti-Aliasing — siehe
  „Abgrenzung" unten), `grimoire_render::mesh_pass`

## Kontext

Der gemessene Befund, der das Paket „Texturqualität" begründet, nennt zwei getrennte Ursachen für
sichtbare Kanten im Bild: fehlende Mipmaps/Multisampling (dieses ADR und sein Geschwister-Arbeitspaket
B1 zu Mipmaps) und die fehlende Tangente aus der Normalenkarte (Strang B2, eigener Pull Request). Der
PO hat entschieden, **dass** die Engine Multisampling bekommt — nicht, mit welchem Typnamen oder
welcher Vorgabe. `Msaa` (`Off`/`X4`) und `StageRendererConfig::msaa` mit Vorgabe `X4` sind der
Vorschlag dieses Arbeitspakets zur gebündelten Freigabe (Vertrag §2b, Stufe A nach V-20) — nicht selbst
ein PO-Entscheid, genau die Verwechslung, die in WP3.4s Vertragsabschnitt stand und dort berichtigt
werden musste.

**Kernfrage:** Welche Technik glättet Geometriekanten (Dreieckssilhouetten) im Mesh-Pass, und
rechtfertigen die gemessenen Zahlen sie?

**Abgrenzung zu ADR-0011:** ADR-0011 glättet *Spekular-Aliasing* — das Flimmern von Glanzlichtern
durch die GGX-Verteilung bei rauen Materialien, behoben durch eine Roughness-Weitung anhand der
Bildschirmraum-Normalvarianz (`dpdx`/`dpdy`), rein innerhalb des Fragment-Shaders. Dieses ADR
glättet etwas anderes: die *Dreieckssilhouette* selbst — die harte Kante zwischen einem Mesh und
dem, was dahinter liegt (Hintergrund, ein anderes Mesh, der Schattenwurf auf dem Boden). Ein Material
kann perfekt spekular geglättet und trotzdem an seiner Silhouette treppig sein, oder umgekehrt; beide
Techniken ergänzen sich, keine ersetzt die andere, und ADR-0011s Umsetzung (`specular_aa_strength`)
bleibt unverändert aktiv.

## Anforderungen

- Geometriekanten (Dreieckssilhouetten) im Mesh-Pass werden geglättet.
- Schattenpass (reine Tiefe, ohnehin PCF-weichgezeichnet) und die WP3.4-Cluster-Compute-Zuordnung
  (arbeitet auf Lichtvolumen, nicht auf Bildschirmpixeln) bleiben unberührt — keine der beiden
  Anforderungen aus dem Befund verlangt das.
- Kosten sind auf dem Software-Adapter relativ messbar (Median, keine GPU-Budget-Zusage in P1,
  dieselbe Einschränkung wie ADR-0011).
- Referenzbilder dürfen sich ändern, aber nur an Kanten — gemessen vor dem Erneuern, nicht nur
  behauptet.
- Additiver Vertragsweg (kein neues `RendererConfig`-Feld, siehe WP3.4s `StageRendererConfig`-
  Präzedenzfall).

## Betrachtete Optionen

### Option A: Keine Kantenglättung für Geometrie

**Positiv:** kein zusätzlicher Code, keine Kosten, kein zusätzlicher Speicher.
**Negativ:** Der PO hat bereits entschieden, dass die Engine Multisampling bekommt; der Befund nennt
explizit `sample_count: 1` als Ursache der Pixeligkeit. Verworfen ohne Messung, weil es die
Anforderung nicht erfüllt.

### Option B: Supersampling (SSAA)

Rendert den gesamten Frame (Mesh- **und** Sprite-Pass) bei einem Mehrfachen der Zielauflösung und
tastet beim Auflösen ab.

**Positiv:** glättet jede Kante in jedem Kanal (Sprites eingeschlossen), konzeptionell am
einfachsten.
**Negativ:** Die Fragment-Schattierung selbst läuft mit der vollen Vervielfachung (bei 4× also
viermal so oft wie MSAA, das eine Schattierung über alle deckenden Samples eines Dreiecks teilt) —
für den PBR-Mesh-Pass mit GGX, Schattenkarten-Sampling und Cluster-Lookup pro Fragment deutlich
teurer als nötig. Bräuchte außerdem eine Größenänderung der gesamten Ziel-Kette (Sprite-Pass,
Offscreen-Rücklese), nicht nur des Mesh-Passes — der Vertrag beschränkt dieses Arbeitspaket aber
ausdrücklich auf den Mesh-Pass (die Sprite-Pass-Ebenen 4/6 bleiben laut Festlegung unberührt gewollte
Bildänderung). Nicht gebaut.

### Option C: Multisampling (MSAA), 4×, nur der Mesh-Pass (vorgeschlagen, umgesetzt)

Der Mesh-Pass rendert in ein 4×-Farb- und -Tiefenziel und löst vor dem Sprite-Pass in das bisherige,
einfach abgetastete Ziel auf; Schattenpass und Cluster-Compute bleiben bei 1×.

**Positiv:**
- Teilt eine Fragment-Schattierung über alle deckenden Samples eines Dreiecks — glättet exakt die
  Silhouette, ohne die Schattierungskosten zu vervielfachen (anders als Option B).
- Auf den Mesh-Pass begrenzt: Der Sprite-Pass zeichnet unverändert mit `LoadOp::Load` auf das
  aufgelöste Ziel, exakt wie vor diesem Arbeitspaket — Ebene 4/6 sind strukturell unberührt.
- Additiver Vertragsweg über `StageRendererConfig::msaa` (`Msaa::Off`/`X4`, Vorgabe `X4`), demselben
  Muster wie `LightBudget` (WP3.4): `WgpuRenderer::new_for_window`/`new_offscreen` (unveränderte
  Signatur) wählen die Vorgabe automatisch.
- Kosten sind isoliert messbar (siehe „Messung"), da `Msaa` pro Konstruktion gewählt wird.

**Negativ:**
- Glättet nur Dreieckskanten, nicht Textur- oder Alpha-Test-Kanten (in P1 nicht relevant: Materialien
  zeichnen laut Vertrag §6 ohnehin immer opak) und nicht Spekular-Aliasing (ADR-0011s Aufgabe).
- Zusätzlicher Speicher für das 4×-Farb- und -Tiefenziel bei der Zielauflösung, unabhängig davon, ob
  eine Szene überhaupt sichtbare Kanten hat.
- Die drei Pipelines, die in denselben Render-Pass zeichnen (starr, geskinnt, Blob-Schatten-Decals),
  müssen alle dieselbe `sample_count` tragen — eine weitere Pipeline im Mesh-Pass müsste diese Regel
  kennen.

### Option D: Nachträgliches Kanten-Weichzeichnen (FXAA o. ä.)

Ein Fragment-Shader-Nachbearbeitungsschritt, der Helligkeitskanten im fertigen Bild erkennt und
weichzeichnet.

**Positiv:** billig, unabhängig von der Geometrie, würde auch Sprite-Kanten mitnehmen.
**Negativ:** arbeitet auf einer Heuristik (Helligkeitssprung), nicht auf echter Dreiecksabdeckung —
kann echte, gewollte scharfe Kanten (Telegraphie-Symbole, UI) mitverwischen, was PRD-0003s
Ebenen-Regeln (4/6 bleiben unberührt) eher verletzen als einhalten würde. Nicht gebaut, keine
Referenzimplementierung vorhanden, gegenüber der gemessenen Option C nur auf dem Papier bewertet.

## Messung

Gemessen mit `cargo test --release -p grimoire_render --test offscreen -- --ignored --nocapture
measure_msaa_relative_cost` (`tests/offscreen.rs::measure_msaa_relative_cost`), auf dem
Software-Adapter (`GRIMOIRE_GPU_ADAPTER=software`, WARP unter Windows/DX12 — **kein GPU-Budget**,
dieselbe Einschränkung wie ADR-0011). Szene: dieselbe wie ADR-0011s Messung (500 PBR-Kugeln mit
zufälligen Materialien plus 10.000 Sprites, 8 Punktlichter, 1280×720), 30 Bilder nach 5
Aufwärm-Bildern, Median dreier Läufe.

`RenderStats::cpu_time` (nur das Aufzeichnen der Befehle, `submit` blockiert nicht) zeigt **keinen**
verlässlichen Unterschied — die zusätzliche Arbeit von MSAA entsteht auf dem WARP-Treiber-Thread und
schlägt sich dort nicht nieder. Erst die Wanduhrzeit über den gesamten Stapel, erzwungen bis zum
Abschluss durch eine abschließende `read_offscreen_rgba` (eine blockierende Rücklese), macht sie
sichtbar:

| Variante | Wanduhrzeit/Bild (Median dreier Läufe, inkl. Treiber-Abschluss) |
|---|---|
| `Msaa::Off` | ≈ 57,2 ms |
| `Msaa::X4` | ≈ 64,6 ms |

Relativer Mehraufwand ≈ 1,13× (rund 13 %) auf dem Software-Adapter. Wie bei ADR-0011 ist das eine
plattformeigene WARP-Zahl, kein GPU-Budget; eine Referenz-Hardware-Zahl bräuchte die für P1
vorgesehene Messsitzung (Plan 0002 WP3.3).

## Entscheidung

**Vorschlag: Option C, 4×-Multisampling im Mesh-Pass.** `StageRendererConfig` wächst additiv um
`msaa: Msaa` (`Off`/`X4`, Vorgabe `X4`) — Stufe A nach V-20, gebündelte PO-Freigabe am 2026-09-17 erteilt (siehe
„Kontext" oben zur Abgrenzung von PO-Entscheid und Vorschlag). Der Mesh-Pass rendert in ein
4×-Farb- und -Tiefenziel und löst vor dem Sprite-Pass in das bisherige Ziel auf; Schattenpass und
Cluster-Compute bleiben bei 1×. Referenzbilder von `pbr_materials`, `shadows` und `camera_tilt`
wurden nach Messung (siehe Pull-Request-Text: 94–100 % der abweichenden Pixel je Szene liegen neben
einem Farb-/Helligkeitssprung im alten Referenzbild) bewusst neu erzeugt.

Diese ADR gilt erst nach PO-Freigabe als angenommen; bis dahin bleibt die Umsetzung im Code
lauffähig (die Tests bestehen unabhängig vom Status dieser ADR), aber der Status hier zeigt, dass
die Entscheidung noch nicht formal bestätigt ist.

## Konsequenzen

### Positiv

- Der im Befund genannte Grund für Pixeligkeit an Geometriekanten (`sample_count: 1`) ist behoben,
  ohne Ebene 4/6 (Sprite-Pass, Telegraphie, Bullets) anzufassen.
- Kosten sind isoliert messbar (siehe „Messung") und liegen in einer Größenordnung, die auf echter
  GPU-Hardware voraussichtlich günstiger ausfällt (MSAA ist eine Kernfunktion praktisch jeder
  GPU, anders als der Software-Rasterisierer, den WARP für die Abdeckungsauflösung emuliert).
- Additiv und rückwärtskompatibel: `WgpuRenderer::new_for_window`/`new_offscreen` bleiben
  unverändert und bekommen die Vorgabe `X4` automatisch, wie `LightBudget` es in WP3.4 vormacht.

### Negativ

- Zusätzlicher GPU-Speicher für das 4×-Ziel bei der Zielauflösung, unabhängig vom tatsächlichen
  Kantenanteil einer Szene; unter `Msaa::Off` entsteht dieser Speicher gar nicht.
- Kein Fortschritt gegen Spekular-Aliasing (bleibt ADR-0011s Aufgabe) oder gegen die dunklen Stellen
  an der Normalenkarte (Strang B2, eigener Pull Request).
- Die gemessene relative Zusatzkost (≈ 13 %) ist eine Software-Adapter-Zahl; ob sie auf Zielhardware
  spürbar ist, bleibt offen bis WP3.3s Messsitzung.
- Sollte der PO die vorgeschlagenen Typen oder die Vorgabe `X4` bei der gebündelten Freigabe ändern
  (zum Beispiel `Off` als Vorgabe, oder ein zusätzliches `X2`), ist das eine spätere additive
  Vertragsänderung, kein Widerruf dieser ADR — der PO-Entscheid war „dass" Multisampling kommt, nicht
  „mit genau diesen Werten".
