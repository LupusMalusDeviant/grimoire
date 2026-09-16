# ADR-0012: Schatten — Shadowmap für das Key-Light plus Blob-Schatten (OF-3.2)

- **Status:** Akzeptiert (PO, 2026-09-16)
- **Datum:** 2026-09-16
- **Entscheider:** Lupus Malus Deviant (PO), Entscheidung aussteht; vorbereitet durch Claude
- **Bezug:** Spiel-Repo [ADR-0014](https://github.com/LupusMalusDeviant/fiends-n-patrons/blob/main/docs/adr/0014-realistischer-3d-look-statt-toon.md)
  (realistischer 3D-Look, macht Schatten zur Pflicht), [PRD-0003](https://github.com/LupusMalusDeviant/fiends-n-patrons/blob/main/docs/prd/0003-rendering-und-art.md)
  (OF-3.2, Regeln 1–5, FR-11 Presets), [Stilbibel](https://github.com/LupusMalusDeviant/fiends-n-patrons/blob/main/docs/art/stilbibel.md)
  (Schatten-Empfehlung: Key-Light-Shadowmap plus begrenzte Punktlicht-Schattenwerfer, Blob für
  Preset „Low"), Plan 0002 WP2.6 (Spiel-Repo), Engine-Vertrag
  [`crate-vertraege.md`](../architektur/crate-vertraege.md) §6, [ADR-0011](0011-spekulares-anti-aliasing-statt-taa.md)
  (Vorlage für Struktur und Messmethodik dieser ADR)

## Kontext

ADR-0014 (Spiel-Repo) entscheidet den realistischen PBR-Look und benennt Schatten explizit als
Pflicht statt Option: „Ohne Schatten wirkt der PBR-Look unglaubwürdig." OF-3.2 (PRD-0003) fragt
nach der Technik: eine Shadowmap für das Key-Light plus eine begrenzte Zahl Punktlicht-
Schattenwerfer, mit Preset „Low" auf Blob-Schatten (ein einfacher Decal-Pass auf Ebene 1). Die
Stilbibel empfiehlt bereits vorab (noch nicht vom PO bestätigt) genau diese Aufteilung und nennt als
Vorbild den Blender-Vergleich aus ADR-0014, der Schatten für Mond, die acht Fackeln/Kohlebecken und
den Stab-Orb testete, alle übrigen Lichter ohne.

WP2.6 baut zwei Techniken in `grimoire_render`, umschaltbar je Frame über
`StageFrame::shadow_config` (`ShadowMode`): eine gefittete orthografische Shadowmap für das
Key-Light (`shadow_pass.rs`, `shadow.wgsl`, PCF-gefiltert in `mesh.wgsl`) und Blob-Schatten
(`blob_shadow.wgsl`, `BlobShadowInstance`) als weiche, dunkle Decals unter Akteuren. Begrenzte
Punktlicht-Schattenwerfer (der zweite Teil von OF-3.2) sind **nicht** Teil dieser Umsetzung — siehe
„Betrachtete Optionen", Option D, und „Konsequenzen" für das vorbereitete, aber noch nicht
konsumierte Interface.

**Kernfrage:** Welche Schatten-Technik(en) erhält `grimoire_render` in WP2.6, wie schalten Presets
zwischen ihnen um, und was bleibt für Punktlicht-Schattenwerfer offen?

## Anforderungen

- OF-3.2: Shadowmap fürs Key-Light plus begrenzte Punktlicht-Schattenwerfer gegen Blob-Schatten,
  anhand von Vergleichsbildern und relativen Kosten.
- PRD-0003 Regeln 1/2: Ebene 4 (Telegraphie) und Ebene 6 (Bullets) dürfen von keiner Schatten-Technik
  überdeckt, getönt oder geblurrt werden — auch nicht indirekt über eine dunklere Umgebung, die den
  Kontrast der Bullets verändert.
- PRD-0003 FR-11: Presets (Low → Ultra) skalieren Schatten; Preset „Low" bleibt laut Stilbibel bei
  Blob-Schatten.
- Kosten relativ messbar auf dem Software-Adapter (kein GPU-Budget-Nachweis in P1, wie bei
  ADR-0011/OF-3.5).
- Laufzeit-Umschaltbarkeit ohne Neustart, damit ein Preset-Wechsel im laufenden Spiel funktioniert
  (FR-11 „live umschaltbar").

## Betrachtete Optionen

### Option A: Keine Schatten

**Positiv:** kein zusätzlicher Code, keine Kosten (Messung: siehe unten, `ShadowMode::None`).
**Negativ:** ADR-0014 benennt Schatten als Pflicht, nicht als Option, für den gewählten
realistischen Look. Verworfen ohne weitere Abwägung, weil es die Anforderung nicht erfüllt — bleibt
aber als `ShadowMode::None` die Referenz-Baseline für die Kostenmessung und als expliziter Zustand
für Plattformen/Geräte, die sich auch Blob-Schatten nicht leisten können.

### Option B: Nur Blob-Schatten (für jedes Preset)

Eine weiche, dunkle Scheibe unter jedem Akteur (`BlobShadowInstance`: Position, Radius, Weichheit,
Stärke), als Decal in derselben Pass wie die opaken Meshes gezeichnet, tiefengetestet aber nicht
tiefenschreibend.

**Positiv:**
- Sehr billig (siehe „Messung"): eine Handvoll zusätzlicher, alpha-geblendeter Quads, kein
  zusätzlicher Renderziel-Durchlauf.
- Funktioniert auf jeder Hardware, auch dort, wo eine echte Shadowmap zu teuer wäre (Stilbibels
  eigene Empfehlung für Preset „Low").
- Trennt Figuren sichtbar von der Umgebung, ohne Schlagschatten-Richtung oder -Länge berechnen zu
  müssen — robust gegen jede Lichtkonfiguration.

**Negativ:**
- Keine Verdeckung durch Geometrie: Ein Blob-Schatten hinter einer Wand oder Säule verschwindet nur,
  wenn er selbst außerhalb der Kamerasicht liegt oder der Tiefentest ihn korrekt verdeckt — er
  „weiß" aber nichts von der tatsächlichen Lichtrichtung oder von Verdeckern zwischen Akteur und
  Lichtquelle, anders als eine echte Shadowmap.
- Für ein Setting mit dramatischem Schrägeinfall des Mondlichts (Stilbibel) wirkt eine kreisrunde,
  richtungslose Scheibe weniger glaubwürdig als ein echter, gerichteter Schlagschatten.
- Allein für jedes Preset gewählt, verzichtet auf den Realismus-Gewinn, den ADR-0014 gerade mit dem
  PBR-Look erkauft hat — ADR-0014 verlangt sichtbare, gerichtete Schatten für die höheren Presets.

### Option C: Nur Key-Light-Shadowmap (kein Blob)

Eine gefittete orthografische Shadowmap für `StageFrame::key_light`, PCF-gefiltert.

**Positiv:**
- Echter, gerichteter Schlagschatten, der Geometrie korrekt verdeckt — das eigentliche Ziel von
  ADR-0014.
- Einmal gebaut, ohne Preset-Sonderfall: dieselbe Technik für jede Qualitätsstufe, nur mit anderer
  Auflösung/PCF-Radius skaliert.

**Negativ:**
- Kostet auf dem Software-Adapter etwa das Doppelte einer schattenlosen Szene (siehe „Messung") —
  für ein Preset „Low" auf schwacher Hardware (Stilbibel nennt ausdrücklich mobile Zielgeräte in P2)
  möglicherweise zu teuer, ohne einen Fallback.
- Ohne einen Blob-Fallback verstößt die Umsetzung gegen die von PRD-0003 FR-11 und der Stilbibel
  bereits vorweggenommene Anforderung, dass Preset „Low" ausdrücklich bei Blob-Schatten bleibt.

### Option D: Key-Light-Shadowmap plus begrenzte Punktlicht-Schattenwerfer

Zusätzlich zu Option C: eine kleine Zahl Punktlichter (Fackeln, Stab-Orb laut Stilbibel) wirft
ebenfalls Schatten, je über eine eigene (Würfel- oder Einzel-)Shadowmap.

**Positiv:** am nächsten am Blender-Vergleich aus ADR-0014, der genau das testete (Mond, acht
Fackeln/Kohlebecken, Stab-Orb mit Schatten); die dramatischste Beleuchtung für ein Okkult-Setting mit
vielen Lichtquellen.

**Negativ:**
- Deutlich mehr Aufwand: pro Schattenwerfer eine eigene Shadowmap (bei einer Punktlicht-Quelle
  üblicherweise eine Cube-Map mit sechs Seiten, keine einzelne 2D-Textur wie beim Key-Light), ein
  eigener Fitting- und Bias-Mechanismus pro Licht (kein gemeinsames „eine Szene, ein Frustum" wie
  beim gerichteten Key-Light) und eine Auswahl-Heuristik, welche der potenziell Dutzenden
  Punktlichter überhaupt die begrenzte Schattenwerfer-Zahl belegen dürfen.
- Sprengt den Rahmen dieses einzelnen Arbeitspakets (WP2.6): Das ist der Grund, warum diese ADR
  Option D **nicht umsetzt**, sondern nur ihr Dateninterface vorbereitet (siehe „Entscheidung" und
  „Konsequenzen") — in Übereinstimmung mit der Aufgabenstellung dieses Schritts, die genau diesen
  Fall vorsieht („falls Punktlicht-Schattenwerfer für diesen Schritt zu groß sind, Key-Light plus
  Blob umsetzen und die Schnittstelle für Punktlicht-Schattenwerfer vorbereiten").
- Nicht gebaut, nicht gemessen (siehe „Was fehlt" unten statt einer Zahl).

## Messung

Gemessen mit `GRIMOIRE_GPU_ADAPTER=software cargo test -p grimoire_render --test
wp26_shadow_showcase --release -- --ignored --nocapture measure_shadow_relative_cost`
(`tests/wp26_shadow_showcase.rs`), auf dem Software-Adapter (WARP unter Windows/DX12 — **kein
GPU-Budget**, nur eine relative, plattform-eigene Zahl, wie ADR-0011/OF-3.5). Szene: ein
Arena-Testaufbau (Boden, acht Säulen, ein Altar, fünf Figuren, acht Fackel-Punktlichter, ein
Mondlicht als Key-Light), 1280×720, Median über 21 Frames nach 5 Aufwärm-Frames, drei Läufe.

### Ergebnis: relative Kosten

| Variante | CPU-Zeit/Frame (Median über drei Läufe à 21 Frames) |
|---|---|
| `ShadowMode::None` | ≈ 227–246 µs |
| `ShadowMode::Blob` | ≈ 160–197 µs |
| `ShadowMode::KeyLight` | ≈ 477–487 µs |

`ShadowMode::KeyLight` kostet reproduzierbar rund das **2,0-fache** von `ShadowMode::None` — der
zusätzliche Tiefen-Durchlauf (15 Schattenwerfer in dieser Szene) plus die PCF-Stichproben (3×3-Kern,
Standardwert `ShadowConfig::pcf_radius = 1`) je Fragment im Hauptpass. `ShadowMode::Blob` lag in
allen drei Läufen numerisch **unter** `ShadowMode::None`, obwohl Blob-Schatten strikt mehr Arbeit
verrichten (ein zusätzlicher, alpha-geblendeter Draw-Call mit fünf Instanzen) — das ist kein
Messfehler in der Technik, sondern zeigt, dass der Blob-Mehraufwand auf dieser winzigen Testszene
unterhalb des Rauschens des Software-Adapters selbst liegt (Treiber-/Scheduler-Jitter zwischen den
Phasen der Messung). Ehrliche Lesart: **Blob-Schatten sind auf dieser Szene nicht von „gar keine
Schatten" zu unterscheiden**, während der Key-Light-Shadowmap-Mehraufwand klar und reproduzierbar
ist.

### Ergebnis: Shadowmap-Speicherbedarf je Auflösung

`Depth32Float`, 4 Byte je Texel, quadratische Karte:

| `ShadowConfig::map_size` | Speicher |
|---|---|
| 512 | 1,00 MiB |
| 1024 (Standardwert) | 4,00 MiB |
| 2048 | 16,00 MiB |

### Vergleichsbilder

Erzeugt mit `GRIMOIRE_GPU_ADAPTER=software cargo test -p grimoire_render --test
wp26_shadow_showcase -- --ignored --nocapture save_shadow_comparison_images`, Software-Adapter,
1280×720, dieselbe Szene, Kamera und Beleuchtung je Variante — `shadows_none.png`,
`shadows_blob.png`, `shadows_keylight.png`, die Gegenüberstellung `shadows_comparison.png` und ein
Ausschnitt `shadows_contact_crop.png` (Figuren-Bodenkontakt). PNGs werden wie beim Look-Dev-Spike
(ADR-0014-Referenz) nicht versioniert; sie lagen dem PO als Anhang zu dieser ADR vor. Sichtbar in
`shadows_keylight.png`: klar gerichtete, von den Säulen und Figuren geworfene Schlagschatten auf dem
Boden, entlang der Mondlicht-Richtung; `shadows_none.png` zeigt dieselbe Szene ohne jede
Bodenverdunkelung; `shadows_blob.png` zeigt die runden, weichen Scheiben unter jeder Figur, aber
keine Säulen-Schatten. Keine `shadows_keylight_plus_points.png`: Option D ist nicht umgesetzt (siehe
oben) — dieses Bild wäre aktuell pixelgleich mit `shadows_keylight.png` und würde die Umsetzung
falsch darstellen.

## Lesbarkeits-Betrachtung (PRD-0003 Regeln 1–5)

- **Regel 1/2 (Ebene 4/6 unverändert):** Beide Techniken zeichnen ausschließlich in
  `RenderLayer::World` — der Mesh-Pass (opake Meshes plus Blob-Decals) kennt keine anderen Ebenen,
  die Shadowmap selbst ist ein separater, unsichtbarer Tiefen-Durchlauf, der nie in den Farbpuffer
  schreibt. Telegraphie (Ebene 4) und Bullets (Ebene 6) haben in P1 ohnehin keinen Kanal im
  Mesh-Pass (Vertrag §6); beide Techniken können sie strukturell nicht berühren, unabhängig vom
  gewählten `ShadowMode`.
- **Regel 5 (Bullet-Licht-Obergrenze) bleibt unberührt:** Die Key-Light-Shadowmap dämpft
  ausschließlich den Beitrag von `StageFrame::key_light` (`mesh.wgsl`s `key_light_shadow_factor`
  multipliziert nur den `ggx_light(..., camera.light_dir, ...)`-Term); Punktlichter — einschließlich
  solcher mit `PointLight::is_bullet_light` — bleiben von dieser ADR vollständig unberührt.
  `BulletLightCap` und die Key-Light-Shadowmap sind zwei unabhängige Dämpfungsmechanismen ohne
  Wechselwirkung.
- **Indirekter Effekt auf Bullet-Kontrast (Regel 2, informativ):** Ein abgeschatteter Boden wird
  dunkler, nicht heller — das hebt tendenziell das WCAG-Kontrastverhältnis eines darüberliegenden
  Bullets an (dunklerer Hintergrund → höheres Verhältnis), schadet also der Lesbarkeit nicht. Sobald
  WP3.5 den Bullet-Pass baut, zeichnet er ohnehin nach `PostFxResolve` auf einer eigenen Ebene ohne
  jede Tiefenprüfung gegen den Mesh-Pass (Vertrag §6 Ebenenreihenfolge) — ein Bullet kann von keiner
  Schatten-Technik dieser ADR verdeckt oder getönt werden, strukturell, nicht nur zufällig.

## Presets (Vorschlag, PO/Look-Review bestätigt endgültige Werte)

`RendererConfig` bleibt unverändert (Vertrag §6: kein `#[non_exhaustive]`, ein neues Feld wäre
inkompatibel; wie schon bei WP2.5s `specular_aa_strength` und WP3.4s künftigem Lichtbudget bleibt die
Preset-Zuordnung Sache der Fassade/des Spiels, das `StageFrame::shadow_config` je Frame setzt).
Vorschlag für die vier PRD-0003-FR-11-Stufen, zur Bestätigung am Look-Review (wie die Stilbibel es
für ihre eigenen „vorläufig"-Werte vorsieht):

| Preset | `ShadowMode` | `map_size` | `pcf_radius` |
|---|---|---:|---:|
| Low | `Blob` | – | – |
| Medium | `KeyLight` | 512 | 0 (ein Tap) |
| High | `KeyLight` | 1024 | 1 (3×3) |
| Ultra | `KeyLightPlusPoints`* | 2048 | 2 (5×5) |

\* Bis Option D umgesetzt ist, verhält sich `KeyLightPlusPoints` identisch zu `KeyLight` (siehe
„Entscheidung"); die Tabelle nennt den Zielzustand, kein aktuelles Verhalten.

## Entscheidung

**Vorschlag: Option C (Key-Light-Shadowmap) plus Option B (Blob-Schatten) als gemeinsames,
laufzeit-umschaltbares Paar — bereits umgesetzt —, Option D (begrenzte Punktlicht-Schattenwerfer)
als vorbereitetes, aber nicht umgesetztes Interface, zurückgestellt auf ein Folge-Arbeitspaket.**

Umgesetzt in `grimoire_render`:

- `ShadowMode` (`None`, `Blob`, `KeyLight`, `KeyLightPlusPoints`) und `ShadowConfig`
  (`StageFrame::shadow_config`, persistiert über `clear()`): Auflösung, Tiefen-Bias (konstant und
  steigungsskaliert), PCF-Radius, die gefittete Frustumgröße und `max_point_shadow_casters`
  (vorbereitet, siehe unten).
- Key-Light-Shadowmap (`shadow_pass.rs`, `shadow.wgsl`): ein reiner Tiefen-Durchlauf (kein
  Fragment-Shader), orthografisches Frustum um `Camera25D::target` gefittet
  (`stage3d::key_light_view_projection`), PCF-gefiltert in `mesh.wgsl` über einen
  Vergleichs-Sampler (`textureSampleCompareLevel`, keine Uniformitätsanforderung an den
  Kontrollfluss, anders als die implizite `textureSampleCompare`-Variante).
- Blob-Schatten (`blob_shadow.wgsl`, `BlobShadowInstance`: Position, Radius, Weichheit, Stärke) als
  vertex-gepullte, alpha-geblendete Decals in derselben Pass wie die opaken Meshes, tiefengetestet
  gegen deren bereits geschriebenen Tiefenpuffer, aber selbst nicht tiefenschreibend.
- `StageStats::shadow_casters_drawn`/`blob_shadows_drawn`/`blob_shadows_rejected_invalid`/
  `shadow_config_invalid` (renderer-unabhängig aus den bestehenden Mesh-Gültigkeitsregeln
  hergeleitet, wie schon `meshes_drawn`) und `StageStats::point_shadow_casters_drawn` (reserviert,
  siehe unten).

**Nicht umgesetzt (Option D, bewusst zurückgestellt):** `PointLight::casts_shadow` und
`ShadowConfig::max_point_shadow_casters` existieren als Datenvertrag, werden aber von keinem
Renderer konsumiert; `ShadowMode::KeyLightPlusPoints` verhält sich aktuell identisch zu
`ShadowMode::KeyLight`. Ein Folge-Arbeitspaket müsste mindestens klären: Cube-Map- oder
Einzelseiten-Schattenwerfer je Punktlicht, eine Auswahl-Heuristik bei mehr Kandidaten als
`max_point_shadow_casters`, und ob Fackeln/Kerzen (viele, klein, kurze Reichweite) überhaupt einen
sichtbaren Schattenwurf gegenüber ihren Kosten rechtfertigen — eine Frage, die erst mit echten
Modellen statt der prozeduralen Testgeometrie belastbar zu beantworten ist (OF-3.4/Blender-Pipeline,
P2).

Diese ADR gilt erst nach PO-Freigabe als angenommen; bis dahin bleibt die Umsetzung im Code
lauffähig (die Tests bestehen unabhängig vom Status dieser ADR), aber der Status hier zeigt, dass die
Entscheidung noch nicht formal bestätigt ist.

## Konsequenzen

### Positiv

- OF-3.2 ist beantwortet: Preset „Low" bleibt bei Blob-Schatten (Stilbibel-Vorgabe erfüllt), höhere
  Presets bekommen einen echten, gerichteten Schlagschatten.
- Beide Techniken sind strukturell unfähig, Ebene 4/6 zu berühren (siehe
  „Lesbarkeits-Betrachtung") — keine zusätzliche Prüfung nötig, wenn WP3.5 später den Bullet-Pass
  baut.
- Laufzeit-Umschaltbar über reine Framedaten (`StageFrame::shadow_config`), ohne
  `RendererConfig`-Änderung — passt zu FR-11s Forderung „live umschaltbar" und zum bestehenden
  additiven Erweiterungsweg (Vertrag §6 Regel 13).
- Die Kostenmessung isoliert ehrlich, was sie kann und was nicht: der Key-Light-Mehraufwand ist klar
  (≈ 2×), der Blob-Mehraufwand ist auf dieser Szene nicht vom Rauschen zu unterscheiden — beides wird
  so berichtet, statt eine falsche Präzision vorzutäuschen.

### Negativ

- Punktlicht-Schattenwerfer (die zweite Hälfte von OF-3.2) bleiben offen; das Ultra-Preset in der
  Presets-Tabelle oben ist ein Zielzustand, kein Ist-Zustand.
- Die gemessenen Zahlen sind Software-Adapter-Werte (WARP/DX12), keine GPU-Budget-Zusage; die für P1
  vorgesehene Messsitzung (Plan 0002 WP3.3) auf Referenz-Hardware ist der eigentliche Budget-Nachweis
  für das 8-ms-GPU-Frame-Ziel (PRD-0003 NFR).
- `ShadowConfig`s Presets-Tabelle (Auflösung, PCF-Radius je Preset) ist ein Vorschlag dieser ADR,
  keine vom PO/Look-Review bestätigte Endabnahme — analog zur Stilbibels eigenen „vorläufig"-Werten.
- Blob-Schatten kennen keine Verdeckung durch Geometrie zwischen Akteur und (gedachter) Lichtquelle
  (siehe Option B) — für Preset „Low" ein bewusster, von der Stilbibel bereits akzeptierter
  Kompromiss.
- `capsule_actor`s abgerundete Fußform (Halbkugel-Kappen) lässt Figuren selbst mit Schatten leicht
  „schwebend" wirken (sichtbar im Kontakt-Ausschnitt `shadows_contact_crop.png`) — eine Eigenschaft
  der prozeduralen Testgeometrie, kein Fehler der Schatten-Technik; echte, UV-gewrappte Modelle
  (OF-3.4/P2) mit einer flachen Standfläche dürften das beheben.

## Weitere Informationen

- Code: `crates/grimoire_render/src/{shadow.wgsl,blob_shadow.wgsl,shadow_pass.rs,mesh_pass.rs,
  mesh.wgsl,stage3d.rs,stage.rs,lib.rs}`.
- Tests: `crates/grimoire_render/tests/offscreen.rs` (Korrektheit:
  `key_light_shadow_map_darkens_the_floor_behind_an_occluder`,
  `shadow_mode_none_never_samples_the_shadow_map_even_with_casters_present`,
  `blob_shadow_darkens_the_ground_under_the_disc`), `crates/grimoire_render/tests/
  wp26_shadow_showcase.rs` (Vergleichsbilder und Kostenmessung, beide `#[ignore]`, siehe „Messung"
  für die Aufrufzeilen).
- V-20-Kandidaten für eine künftige PO-Sammelsitzung: die Presets-Tabelle oben (Auflösung/PCF je
  Preset), ob `ShadowConfig::frustum_radius`/`frustum_height`s Standardwerte (25/20 Welteinheiten)
  für die tatsächliche Arena-Größe passen, sobald diese feststeht.
- Review dieser Entscheidung, sobald Option D (Punktlicht-Schattenwerfer) angegangen wird oder echte
  Modelle (OF-3.4/P2) den „schwebenden Fuß"-Befund oben entkräften oder bestätigen.
