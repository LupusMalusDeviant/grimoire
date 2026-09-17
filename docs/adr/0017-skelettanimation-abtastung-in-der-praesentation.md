# ADR-0017: Skelettanimation — Abtastung in der Präsentation, Takt aus der Simulation

- **Status:** Vorgeschlagen
- **Datum:** 2026-09-17
- **Entscheider:** Lupus Malus Deviant (PO), Entscheidung aussteht; vorbereitet durch Claude
- **Bezug:** Spiel-Repo [PRD-0002](https://github.com/LupusMalusDeviant/fiends-n-patrons/blob/main/docs/prd/0002-grimoire-engine-architektur.md)
  (FR-04, FR-06, FR-07, Non-Goals, NFR-Budgets), [PRD-0003](https://github.com/LupusMalusDeviant/fiends-n-patrons/blob/main/docs/prd/0003-rendering-und-art.md)
  (Ebenen, FR-13, FR-14), [PRD-0005](https://github.com/LupusMalusDeviant/fiends-n-patrons/blob/main/docs/prd/0005-kampfsystem-spieler.md)
  (FR-03, FR-07, FR-08, FR-14, NFR), [PRD-0007](https://github.com/LupusMalusDeviant/fiends-n-patrons/blob/main/docs/prd/0007-gegner-und-director.md)
  (FR-11, NFR), [PRD-0008](https://github.com/LupusMalusDeviant/fiends-n-patrons/blob/main/docs/prd/0008-bosse.md)
  (NFR), [PRD-0016](https://github.com/LupusMalusDeviant/fiends-n-patrons/blob/main/docs/prd/0016-tooling-suite.md)
  (FR-09, FR-10), [PRD-0018](https://github.com/LupusMalusDeviant/fiends-n-patrons/blob/main/docs/prd/0018-teststrategie.md)
  (FR-06, OF-18.2); Spiel-Repo Plan 0002, Nachtrag „Knochenverformung" (Engine-PR #23); Engine-Vertrag
  [`crate-vertraege.md`](../architektur/crate-vertraege.md) §1, §2 (Regeln 4, 9, 10, 15), §2b, §3, §6
  (Knochenverformung), §9.3, §9.7, §9.10, §12; [ADR-0013](0013-downlevel-pruefung-licht-cluster-layout.md)
  (Palette im Storage-Buffer), [ADR-0014](0014-bullet-darstellung-billboard-impostor.md) (Extraktionswerte
  der Bullets, Vorlage für Struktur)

## Kontext

Der PO hat Figuren mit KI-Werkzeugen erzeugt und in Blender geriggt und animiert: die Knochenmasken-Hexe
(Spielerfigur, 31 Knochen, 18 Clips) und einen Imp (26 Knochen, Pilot mit `idle` und `walk`). Die Dateien
liegen im Showcase-Ordner des PO außerhalb beider Repos (`players__bone_mask_witch__animated.glb` samt
`…_report.json`, `enemies__imp__rig_pilot.glb`). PO-Entscheid: Der Import-Pilot der Hexe läuft zuerst
statisch; bevor Clips in Packs gebaut werden, muss feststehen, wie Animation in die Engine kommt.

Die Engine kann Figuren verformen, aber nicht abspielen. Seit Engine-PR #23 trägt `MeshVertex` vier
Knochenindizes und Gewichte, `MeshInstance::skin` zeigt über `SkinBinding` in `StageFrame::joint_matrices`
(`MAX_SKIN_JOINTS` = 256), die Palette liegt in einem Storage-Buffer, und geskinnte Instanzen desselben
Meshes zeichnen in einem `draw_indexed`-Aufruf. `grimoire_render::figure_format` dekodiert `FNP_SKELETON`
und bietet `compute_skin_matrices`/`rest_pose_skin_matrices` — laut Vertrag §6 und Doc-Kommentar
ausdrücklich „kein Animationssystem (keine Zeitachse, keine Interpolation)". Der Aufrufer liefert die
Matrizen je Bild. Der Spiel-Prototyp zeigt heute die Ruhepose und eine vorberechnete, eingesackte
Trefferpose (`FigureVisual` in `fnp_game`). PRD-0002 schließt einen „Skeletal-Animation-Vollausbau" als
Ziel aus.

**Kernfrage:** Wie kommt das Abspielen von Skelettanimationen in Grimoire, und welche Animationsfakten
gehören deterministisch in den Tick-Takt der Simulation statt in die je Bild interpolierte Darstellung?

## Befund: die gelieferten Clips

Ausgelesen mit einem Wegwerfskript direkt aus den Accessor-Daten der GLB-Dateien (nicht aus Blender) und
aus dem Bericht `…_report.json`.

| Eigenschaft | Hexe | Imp |
|---|---|---|
| Knochen, Wurzel | 31, eine Wurzel `root`, Eltern stets vor Kindern | 26, ebenso |
| Clips | 18 (`idle` … `victory`), 12 bis 49 Bilder, 0,50 bis 2,04 s | 2 (`idle`, `walk`), je 25 Bilder, 1,04 s |
| Kanäle je Clip | 93 = 31 × Translation/Rotation/Skalierung | 78 = 26 × 3 |
| davon veränderlich | 5 bis 11: stets die Wurzel-Translation, dazu 4 bis 10 Rotationen; Skalierung nie | 5 bzw. 6 |
| Schlüssel | 24 fps, jedes Bild ein Schlüssel (`LINEAR`); konstante Kanäle als 2-Schlüssel-`STEP`; erster Schlüssel bei t = 1/24 s statt 0 | ebenso |
| Schleifenform | letztes Bild gleich dem ersten in 16 von 18 Clips (nicht `death`, `spawn`); Schleifen damit 48 (`idle`), 16 (`walk`), 12 (`run`) Bilder lang | letztes gleich erstes |
| Quaternionen | Normabweichung ≤ 1,4·10⁻⁷; **22 Vorzeichenwechsel** zwischen Nachbarschlüsseln (Skalarprodukt < 0) in 7 Clips, fast nur an den Oberschenkeln; größter echter Drehschritt 60° | keine Wechsel, größter Schritt 1,2° |
| Wurzelversatz | netto null in 16 von 18 Clips; Ausschlag horizontal bis 0,089 (`melee_3`), vertikal bis 0,10 (`dash`) bei rund 1,0 Figurenhöhe; netto −0,27 (`death`) und +0,20 (`spawn`) horizontal | netto null, Ausschlag ≤ 0,004 |
| Ereignisse | nur im Bericht, **nicht im GLB** (glTF 2.0 kennt keine Animationsereignisse): 15 Markierungen in 13 Clips, z. B. `dash` `invulnerable_start` 3/`_end` 10, `melee_1` `hit` 6, `parry` 3–9, `skill_cast` `cast` 8, `ultimate` `release` 18, `death` `dead` 41 | keine |
| Sockel | `socket_weapon_r`, `socket_skillshot_l` | — |
| Geometrie | 100.000 Dreiecke, 299.984 Vertices im GLB (70.743 in Blender) | 70.000 Dreiecke, 43.303 Vertices im GLB |

Zwei Befunde prägen die Entscheidung:

- **Autorenbild und Tick passen nicht aufeinander.** Ein Bild bei 24 fps dauert 2,5 Ticks der 60-Hz-Simulation
  (`DEFAULT_TICK_RATE_HZ`, §9). Je nach Zählbasis der Markierungen liegen 5 oder 10 der 15 Markierungen auf
  einem halben Tick. Die Zählbasis ist nicht dokumentiert; `dead` = 41 in einem Clip mit 41 Bildern spricht
  für die Blender-Zählung ab 1. {{TODO: Zählbasis der Markierungen im Bericht bestätigen}}
- **Die Clips sind ortsfest.** Die README des Showcase-Ordners sagt es („die Spielsteuerung bestimmt die
  eigentliche Strecke"), und die Wurzeldaten bestätigen es: Root Motion trägt keine Spielinformation.

## Anforderungen

1. **Determinismus** (PRD-0002 FR-04, FR-07; Vertrag §3): Der Sim-Zustand bleibt eine reine Funktion aus
   Seed und Eingaben. Präsentationszustand (`alpha`, interpolierte Werte) liegt außerhalb der Welt.
2. **Rewind und Replay** (PRD-0002 FR-06, PRD-0005 FR-07, PRD-0016 Replay-Viewer): Nach einem Snapshot-Restore
   oder beim Scrubben zeigt die Figur die richtige Pose, ohne dass Darstellungszustand wiederhergestellt wird.
3. **Timing ist Balancing-Datum** (PRD-0005 FR-14, NFR): Reichweiten, Fenster und Cooldowns liegen als Daten im
   Content-Pack; das Parade-Fenster ist ≥ 8 Ticks und „ein Balancing-Datum, kein Hardcode"; Eingabe bis zur
   sichtbaren Reaktion ≤ 2 Bilder.
4. **Trefferform aus der Simulation** (PRD-0005 FR-03, FR-08): Kapsel in Modellgröße, Schwungbögen als
   Geometrie der Simulation — keine knochengenauen Trefferzonen gefordert.
5. **Telegraphie** (PRD-0007 FR-11): Aufladeanimationen folgen einheitlichen Zeitklassen der Simulation.
6. **Massen und Budgets** (PRD-0007 NFR: 100 aktive Gegner; PRD-0008 NFR: Boss-Phasen als Worst Case;
   PRD-0002 NFR; `DEFAULT_BUDGETS` in §9.7): Extract 0,5 ms, Render-CPU 3 ms, GPU 8 ms.
7. **Verträge:** Fremde Bytes ergeben Fehler statt Panic (§2 Regel 9); Binärformate mit Version,
   Formatdokument und Golden-Fixture (§2 Regel 10); neue Kanten nur per Crate-Map-ADR (§2 Regel 15); neue
   Drittabhängigkeiten über `[workspace.dependencies]` mit Begründung (§2 Regel 4); gepinnter stabiler
   Compiler (`rust-toolchain.toml`: 1.98.1).
8. **Tests** (PRD-0018 FR-06, OF-18.2): goldene Werte und Render-Szenen mit Referenzen je Treiberfamilie.
9. **Umfangsgrenze** (PRD-0002 Non-Goals): kein Vollausbau — keine Zustandsautomaten-Editoren, kein IK, kein
   Retargeting in der Engine.
10. **Pipeline** (PRD-0003 FR-13, PRD-0016 FR-09/FR-10): glTF aus Blender, offline in Packs kompiliert; der
    Figuren-Konverter liegt heute im Spiel-Repo (Python-Skripte und ein Rust-Packer).

## Was gehört in den Tick-Takt, was in die Darstellung?

| Animationsfakt | Wirkt auf das Spiel? | Ort (Vorschlag) |
|---|---|---|
| Aktionsphasen: Trefferbild, Auslösung, i-Frames, Parade-Fenster, Detonation | ja | Simulation, in Ticks, als Content-Daten (PRD-0005 FR-14) |
| Welche Aktion läuft, seit welchem Tick; vorige Aktion und Wechsel-Tick | ja (Zustandsautomat des Spiels) | Simulation, Komponenten des Spiels |
| Root Motion | nein (gemessen ortsfest) | keine; die Simulation bewegt, der Wurzelversatz bleibt Bild |
| Knochenposen, Sockel | nein (Kapsel und Bögen sind Geometrie) | Darstellung; braucht die Simulation einen Startpunkt am Sockel, dann als gebackene Content-Konstante |
| Interpolation zwischen Schlüsseln, Überblendung, Schrittlängen-Anpassung | nein | Darstellung, je Render-Bild mit `alpha` |
| Markierungen im Clip | nein, sie sind Anker | Darstellung (Zeit-Verzerrung) und Konverter-Prüfung |

## Betrachtete Optionen

### Option A: Engine-Animationsmodul

Gemeinsamer Kern beider Varianten: eigene Pack-Art für Clips neben den Figuren-Nutzlasten, panikfreier
Dekoder in `figure_format`, zustandslose CPU-Abtastung (Translation und Skalierung linear, Rotation nlerp auf
dem kürzeren Weg), Überblendung zweier Posen, Palette über den bestehenden Weg `StageFrame::joint_matrices`.
Die Varianten unterscheiden sich darin, wer den Takt der Ereignisse bestimmt.

#### Variante A1: Takt aus dem Clip

Ein Animationssystem der Engine läuft im Tick, hält Clip und Zeit je Entität in der Welt und meldet
Markierungen als Sim-Ereignisse. Der Konverter rechnet Bilder in Ticks um.

**Positiv:**
- Eine Quelle für Timing: Was der Animator in Blender setzt, gilt im Spiel.
- Ereignisse entstehen ohne Zusatzdaten des Spiels.

**Negativ:**
- Clip-Daten werden simulationsrelevant: Der Content-Manifest-Hash muss sie aufnehmen (§12, „Werden
  Nicht-Sigil-Assets simulationsrelevant …"), jedes Nachtimen eines Clips ändert Zustands-Hashes, Replays und
  Golden Master des Spiels.
- Widerspricht PRD-0005 FR-14: Ein Parade-Fenster zu balancieren hieße, ein Animations-Asset neu zu
  exportieren.
- Halbe Ticks brauchen eine Rundungsregel, die in den Sim-Content eingeht (5 oder 10 von 15 Markierungen).
- Abtast- oder Ereigniscode müsste in die Determinismus-Menge (identische `clippy.toml`, §3) und brauchte neue
  Crate-Kanten zwischen Sim-Seite und Figurendaten — Stufe I, Crate-Map-ADR.

#### Variante A2: Takt aus der Simulation, der Clip folgt

Die Simulation des Spiels kennt je Aktion Dauer und Anker in Ticks (Content-Daten). Die Darstellung rechnet
in `extract_stage` aus Aktion, Start-Tick, aktuellem Tick und `alpha` eine Clip-Zeit und bildet dabei die
Sim-Anker stückweise linear auf die Markierungen des Clips ab. Die Simulation liest nie Clip-Daten.

**Positiv:**
- Clips berühren weder Zustands-Hash noch Replay noch Golden Master; Timing bleibt Balancing-Datum.
- Die Pose ist eine reine Funktion aus Welt und `alpha`: nach Rewind, Restore und beim Scrubben richtig, ohne
  eigenen Darstellungszustand.
- Halbe Ticks sind kein Problem: Die Verzerrung bildet Tick plus `alpha` stetig auf Clip-Zeit ab, eine
  Rundungsregel entfällt.
- Kein Code wandert in die Determinismus-Menge, keine neue Crate-Kante; Stufe A.

**Negativ:**
- Zwei Datenpunkte für dasselbe Ereignis (Sim-Anker und Clip-Markierung) können auseinanderlaufen; die
  Verzerrung kaschiert das als Tempowechsel. Nötig ist eine Konverter-Prüfung mit Grenzen.
- Der Animator kann Spiel-Timing nicht allein in Blender festlegen.

### Option B: Engine bleibt minimal, das Spiel baut Animation selbst

Die Engine behält nur Skinning und `compute_skin_matrices`. Clip-Format, Dekoder, Abtastung und Überblendung
entstehen in den `fnp_*`-Crates; das Spiel macht heute schon eine Kleinform davon (zwei vorberechnete Posen).

**Positiv:**
- Keine Vertragsänderung, schnellster Start; entspricht dem Non-Goal wörtlich.
- Das Spiel kann Format und Verhalten ohne Engine-Release ändern.

**Negativ:**
- Die Figuren-Nutzlasten würden geteilt: Mesh, Material, Textur und Skelett dekodiert die Engine mit
  Panikfreiheits-Tests, Clips das Spiel — ohne `Cursor`, Grenzen-Disziplin und Zufallstests der Engine, und
  das Formatdokument läge nicht unter `docs/formats/`.
- Keine Render-Testszene für Animation im Engine-Repo; ein Bruch im Skinning-Pfad fällt erst im Spiel auf.
- Jedes weitere Spiel auf Grimoire wiederholt die Arbeit.

### Option C: GPU-gebackene Animation für Massen, A für Helden und Bosse

C1: Knochenpaletten je Clip und Bild vorberechnet in einen GPU-Puffer, der Shader wählt das Bild über eine
Zeit je Instanz. C2: Vertex-Animation-Textures mit Positionen und Normalen je Vertex und Bild.

**Positiv:**
- Die CPU-Kosten je animierter Instanz fallen praktisch weg.
- C1 braucht wenig Speicher (Imp-`walk`: 24 × 26 × 48 Byte ≈ 30 KiB).

**Negativ:**
- Spart genau den billigsten Teil: Die CPU-Abtastung kostet gemessen 0,04 ms für 100 Imps (siehe Messung).
  Die teure Arbeit, das Skinning je Vertex, bleibt bei C1 gleich; Instanzen desselben Meshes teilen sich schon
  heute einen Draw-Call.
- C2 kostet Speicher je Vertex: Imp-`walk` rund 43.303 × 24 × 40 Byte ≈ 40 MiB je Clip; Texturformat und
  Kompression sind noch offen (OF-3.4); Tangenten der Normalenkarte laufen nicht mehr über dieselben Matrizen.
- Neue Instanzfelder (Clip, Bildzeit) im geskinnten Pfad und damit eine Vertragsänderung an §6; Überblendung,
  Verzerrung und prozedurale Korrekturen werden auf der GPU schwerfällig.
- Zwei Animationspfade zu pflegen und zu testen.

### Option D: fertige Rust-Crate

Geprüft auf crates.io und in den Repositories (Stand 2026-09-17):

| Crate | Lizenz | Stand | Befund |
|---|---|---|---|
| `ozz-animation-rs` 0.11.0 | MPL-2.0 | Release 2025-10, Repository 2026-09 aktiv | wirbt mit plattformübergreifendem Determinismus; **braucht Nightly** (`#![feature(portable_simd)]`) gegen den gepinnten stabilen Compiler; liest nur `.ozz`-Archive aus dem C++-Werkzeugsatz von ozz-animation; zieht `glam`, `glam-ext`, `bimap` und standardmäßig `rkyv`/`serde`; Datei-Copyleft neben „Alle Rechte vorbehalten" (ADR-0009) bräuchte eine Lizenzprüfung |
| `fyrox-animation` 1.0.1 | MIT | Release 2026-03 | zieht `fyrox-core` und `fyrox-resource` samt eigenem Ressourcen- und Mathematikmodell |
| `bevy_animation` 0.20.0-rc.1 | MIT OR Apache-2.0 | Release 2026-09 | hängt an `bevy_ecs`, `bevy_app`, `bevy_asset`, `bevy_reflect` und rund 20 weiteren Crates |
| `skeletal_animation` 0.47.0 | MIT | letzter Release 2023-11 | hängt an `gfx`, `collada`, `rustc-serialize` |

**Positiv:**
- `ozz-animation-rs` bringt Überblendung, IK, Root Motion und einen Determinismus-Anspruch fertig mit.

**Negativ:**
- Keine Crate erfüllt die harten Regeln: Nightly-Zwang, fremdes Engine-Rahmenwerk oder veralteter
  Grafikstapel.
- Der Funktionsumfang liegt weit über dem Bedarf (Non-Goal), der eigentliche Bedarf — Abtasten, nlerp,
  Überblenden — ist klein.
- Import-Crates wie `gltf` lösen nur den Import, und der läuft offline im Konverter des Spiels.

## Messung und Abschätzung

### CPU-Abtastung

Wegwerf-Messprogramm außerhalb des Repos: bildet `trs_matrix`, `mat4_mul` und die Hierarchie-Schleife von
`compute_skin_matrices` nach (Puffer wiederverwendet statt je Aufruf alloziert) und tastet dichte
24-fps-Clips ab, **alle** Kanäle veränderlich (schlechter als die gemessenen 5 bis 11 von 93). Einfädig,
Release-Optimierung ohne zielspezifische CPU-Flags, mit einem älteren stabilen Compiler als dem gepinnten,
2.000 Wiederholungen nach 200 Aufwärmläufen, zwei Läufe. **Lokaler Entwicklungsrechner, kein Runner- und kein
Referenz-Hardware-Wert** — eine Größenordnung, kein Budgetnachweis.

| Szene | Knochen je Bild | Median | p95 | je Knochen | Median am Extract-Budget 0,5 ms |
|---|---:|---:|---:|---:|---:|
| 30 Imps + Hexe, ein Clip | 811 | 0,011 ms | 0,015 ms | 13–14 ns | 2 % |
| 30 Imps + Hexe, Überblendung überall | 811 | 0,015–0,019 ms | 0,023–0,033 ms | 19–24 ns | 3–4 % |
| 100 Imps + Hexe, ein Clip | 2.631 | 0,040–0,041 ms | 0,068–0,073 ms | 15 ns | 8 % |
| 100 Imps + Hexe, Überblendung überall | 2.631 | 0,058–0,059 ms | 0,107–0,108 ms | 22 ns | 12 % |
| 300 Imps + Hexe, ein Clip | 7.831 | 0,116–0,117 ms | 0,200–0,209 ms | 15 ns | 23 % |

Zum Vergleich: Die Bullet-Extraktion bei 10.000 Bullets lag im Runner-Wert von ADR-0014 bei 17,31 µs. Beides
zusammen bleibt bei 100 animierten Gegnern im Median unter einem Fünftel des Extract-Budgets.

nlerp statt slerp: Bei einem Drehschritt von 60° (größter gemessener Schritt) weicht nlerp höchstens 0,27° ab,
bei 30° 0,03°. nlerp braucht nur Grundrechenarten und `sqrt`, die IEEE 754 exakt festlegt, also keine
Transzendentalfunktion und kein `dmath`; die Ergebnisse sind auf allen drei Betriebssystemen bitgleich prüfbar.

### Speicher

| Ablage (40 Byte je Knochen und Bild bei vollen f32-TRS) | Hexe, 18 Clips | Imp, 2 Clips |
|---|---:|---:|
| dicht, 24 fps | 452 KiB | 51 KiB |
| dicht, auf 60 Hz umgetastet | 1.136 KiB | 128 KiB |
| konstante Spuren einmal, veränderliche je Bild, f32 | 67 KiB | 6,1 KiB |
| wie zuvor, Rotationen mit 48 Bit („smallest three") | 42 KiB | 3,9 KiB |

Eine 8192×8192-Textur, wie sie das GLB der Hexe zweimal enthält, belegt als `FNP_TEXTURE_RAW` (RGBA8)
256 MiB; alle 18 Clips dicht sind 0,17 % davon. Laufzeit je Instanz: Pose 31 × 40 Byte ≈ 1,2 KiB,
Palette 31 × 64 Byte ≈ 2 KiB.

### GPU

Die Palette geht heute schon jedes Bild hoch, auch für statische Posen: 811 Matrizen ≈ 51 KiB, 2.631 ≈ 164 KiB
je Bild. Die Bindungsgröße von mindestens 16 MiB reicht für 262.144 Matrizen, also rund 10.000 Imps.
Animation ändert an Upload, Draw-Calls und Vertex-Skinning nichts — nur die Werte der Matrizen.
{{TODO: GPU-Kosten des Vertex-Skinnings (Hexe ≈ 300.000, Imp ≈ 43.300 Vertices im GLB) in der Messsitzung auf Referenz-Hardware}}

## Clip-Datenformat (Vorschlag für den Vertrags-PR)

- **Art und Pfad:** eigene Anwendungsart `FNP_CLIP` im freien Bereich `0x8000..=0xFFFF` (§12, keine Änderung am
  Pack-Format v1), eigene `kind_version` 1 wie bei `FNP_MESH`. Pfad `figures/<figur>/clip/<clip>`; die ID folgt
  aus dem Pfad, `FNP_FIGURE` bleibt in Fassung 1. {{TODO: Artnummer festlegen; `0x8005` meiden, die der
  Konverter-Strang A zeitweise für `FNP_MATERIAL` benutzte}}
- **Kopf:** Version, `joint_count` (1 bis `MAX_SKIN_JOINTS`, muss dem Skelett entsprechen), Skelett-Fingerabdruck
  (`StableHasher` über Knochenzahl und Elternindizes, erkennt einen Clip für ein anderes Rig gleicher
  Knochenzahl), Autorenrate in Hz, Bildzahl, Flags (Bit 0: Schleife).
- **Ablage:** Abtastwerte in der Autorenrate, kein Umtasten auf die Tick-Rate, keine Schlüsselreduktion, keine
  Quantisierung. Je Knochen in Skelettreihenfolge die Spuren T, R, S in dieser Reihenfolge; jede Spur beginnt
  mit einer Art (konstant: ein Wert; abgetastet: ein Wert je Bild). Genau eine Leseart, damit Konverter und
  Dekoder nicht wieder zwei stimmige, zueinander unbrauchbare Hälften bauen (Lehre aus Engine-PR #23).
- **Zeitbasis:** Bild k liegt bei t = k / Rate, das erste bei t = 0 (der Konverter entfernt den gemessenen
  Versatz von einem Bild). Eine Schleife wiederholt das erste Bild am Ende und dauert (Bildzahl − 1) / Rate;
  ein Einzel-Clip klemmt. So sind die Schleifen der Hexe 120, 40 und 30 Ticks und die des Imps 60 Ticks lang —
  ganze Tick-Zahlen, die sich per Ganzzahl-Modulo ohne Drift über einen 18-Minuten-Lauf abbilden lassen.
- **Quaternionen:** `xyzw` als f32; der Dekoder lehnt nicht endliche Werte und eine Normabweichung über 10⁻³ ab
  (nie still korrigiert, wie bei der Tangente). Vorzeichenwechsel zwischen Nachbarn sind erlaubt; die
  Abtastung nimmt immer den kürzeren Weg (gemessen nötig: 22 Wechsel in 7 Clips der Hexe). Der Konverter
  glättet die Vorzeichen zusätzlich.
- **Markierungen:** Liste aus Bildindex und Name (≤ 63 Byte UTF-8), nach Bild sortiert. Die Engine deutet
  Namen nicht; sie dienen als Anker der Zeit-Verzerrung und der Konverter-Prüfung.
- **Grenzen und Fehler:** dokumentierte Obergrenzen vor jeder Allokation (§2 Regel 9), keine Restbytes.
  {{TODO: Grenzen für Bildzahl und Markierungen, Vorschlag 4.096 bzw. 64}}
- **Formatdokument:** `docs/formats/figure-clip.md` mit byteweiser Golden-Fixture (§2 Regel 10), vor dem
  Konverter gemergt; bei Abweichung gilt die Engine-Fassung.
- **Nicht in Fassung 1:** Quantisierung, Kurven, Root-Motion-Extraktion, additive Clips, Masken für
  Teilkörper. Jede davon ist eine spätere Fassung derselben Art (Stufe A, solange Fassung 1 lesbar bleibt).

## Empfehlung

**Option A, Variante A2**, als Modul in `grimoire_render` neben `figure_format`, zustandslos: Clip dekodieren,
abtasten, zwei Posen überblenden, Zeit an Markierungen verzerren. Die Simulation des Spiels hält Aktion,
Start-Tick, vorige Aktion und Wechsel-Tick sowie die Anker in Ticks; die Darstellung rechnet daraus in
`extract_stage` die Pose. Begründung:

- A2 erfüllt PRD-0005 FR-14 und hält Clips aus Hashes, Replays und Golden Master heraus, A1 nicht; die
  gemessene Tick-Unverträglichkeit der 24-fps-Markierungen wird dabei gegenstandslos statt per Rundungsregel
  verewigt.
- Kein Spielfakt braucht Knochendaten: Die Clips sind ortsfest, die Trefferform ist eine Kapsel, Bögen sind
  Geometrie. Damit gibt es keinen Grund, Animationscode in die Determinismus-Menge zu holen.
- Die Kosten sind klein (≤ 12 % des Extract-Budgets für 100 Imps und die Hexe mit Überblendung, lokal
  gemessen) und die Speicherfrage stellt sich nicht; C optimiert den billigsten Teil, D scheitert an harten
  Regeln.
- Gegenüber B bleibt die Figuren-Nutzlast eine Familie mit einem Dekoder-Standard, einem Formatdokument und
  Render-Testszenen im Engine-Repo, und der Umfang bleibt unter dem Non-Goal: keine Zustandsautomaten, kein IK,
  kein Retargeting, keine Root Motion.

## Entscheidung

**Vorschlag:** Option A2 wie unter „Empfehlung". PO-Entscheid aussteht; die Einzelfragen stehen unter „Offene
PO-Entscheidungen". Diese ADR gilt erst nach PO-Freigabe als angenommen.

### Einstufung nach §2b

- **Dieser PR** ändert keinen Vertrag (nur ADR und Index).
- **Umsetzung als Modul in `grimoire_render` (Empfehlung): Stufe A.** Neue öffentliche Typen und Funktionen in
  `grimoire_render` und `grimoire::adapters::figure_assets`, eine neue Anwendungsart ohne Änderung an §12, ein
  neues Formatdokument; kein Hash, kein `repr(C)`-Layout, keine Kante ändert sich. Vertrags-PR mit Code, Tests
  und CHANGELOG unter `[Unreleased]`, PATCH-Version, gebündelte PO-Freigabe (V-20). Das Spiel nutzt es erst
  nach einem Release-Tag.
- **Eigene Crate `grimoire_anim`: Stufe I** (neue Crate-Kanten), zusätzlich Crate-Map-ADR, MINOR-Version.
- **Variante A1 oder spätere knochengenaue Spiel-Logik: Stufe I** (Content-Manifest-Hash nach §12,
  Determinismus-Menge, neue Kanten) und ein eigenes ADR.

### Minimaler Schnitt für den Hexen-Piloten

Ziel: Die Hexe steht im Prototyp in einer `idle`-Schleife und läuft in einer `walk`-Schleife.

1. **Engine (ein Vertrags-PR, Stufe A):** `figure_format::decode_clip` mit Formatdokument, Golden-Fixture und
   Zufallstests wie bei den übrigen Figuren-Dekodern; Abtastung einer Schleife in `JointPose`-Werte; Laden
   über die Fassade mit Prüfung von Knochenzahl und Fingerabdruck; optional `compute_skin_matrices` in einen
   wiederverwendeten Puffer. Markierungen werden dekodiert, aber noch nicht benutzt.
2. **Spiel:** Der Figuren-Konverter exportiert `idle` und `walk` als `FNP_CLIP`; `extract_stage` wählt anhand
   der Sim-Geschwindigkeit, rechnet die Zeit aus Ganzzahl-Ticks plus `alpha` und schneidet hart um.
3. **Nicht im Schnitt:** Überblendung, Zeit-Verzerrung, Aktions-Clips, Schrittlängen-Anpassung.

{{TODO: Aufwand des minimalen Schnitts schätzen}}

Danach, vor dem Kampf-Kit in P2: Überblendung (mit einer Obergrenze für latenzkritische Aktionen wie Dash und
Parade, PRD-0005 NFR ≤ 2 Bilder), Zeit-Verzerrung an Markierungen und die Konverter-Prüfung der Anker.
{{TODO: Grenzen der Verzerrung für die Prüfung, Vorschlag Tempofaktor 0,5 bis 2,0 je Abschnitt}}

### Einordnung in den Plan

P2-Vorbereitung. Plan 0002 schließt P1 ohne Animation; PRD-0000 ordnet P2 den Kampf-Kern zu (Spieler-Kit
komplett mit Melee, Skillshot, Dash, Graze, Ulti und Items; drei Gegner-Archetypen), und dafür braucht die
Hexe ihre Aktions-Clips mit Ankern. Der minimale Schnitt kann direkt nach dem statischen Hexen-Piloten laufen;
der Plan für P2 ist noch nicht angelegt.

## Tests

- **Dekoder:** byteweise Golden-Fixture, Zufallstests mit beliebigen, abgeschnittenen und einzeln veränderten
  Eingaben, je Grenze und je Fehlervariante ein Fall (§2 Regel 9).
- **Goldene Posen:** `hash_of` über abgetastete Posen zu festen Zeiten — genau auf Bildern, zwischen Bildern,
  am Schleifenübergang, beim Klemmen eines Einzel-Clips, über einen Vorzeichenwechsel hinweg und bei den
  Überblendgewichten 0, 0,5 und 1. Der Hash muss auf Windows, Linux und macOS gleich sein; das ersetzt die
  fehlende `clippy.toml` für diesen Code. Dazu analytische Fälle: Abtastung auf Bild k ergibt Schlüssel k
  bitgenau, Ruhepose-Clip ergibt Einheitsmatrizen.
- **Takt:** Die Zeit-Verzerrung trifft jeden Anker bei `alpha` = 0 exakt und ist monoton. Im Spiel: Ein Clip mit
  geänderter Zeitlage lässt alle Zustands-Hashes gleich; ein Headless-Lauf mit und ohne animierende Darstellung
  ergibt dieselben Hashes (Muster aus `asset_hook.rs`).
- **Render:** neue Offscreen-Szene mit einer prozedural erzeugten Testfigur und einem kleinen synthetischen
  Clip, ohne Asset-Pipeline, aufgenommen zu mehreren festen Clip-Zeiten. Referenzen je Treiberfamilie nach
  OF-18.2: blockierend unter Windows (WARP) und Linux (lavapipe), macOS im Warnmodus bis P3; Toleranz
  unverändert (Mittel ≤ 3,0, Maximum ≤ 60); Windows- und Linux-Referenz vor dem Merge. Ein Gegenfall belegt,
  dass die Szene eine nicht angewandte Pose über der Toleranz erkennt.
- **Messung:** ein Fall in `grimoire_bench` für 100 × 26 Knochen mit Überblendung als Trend.
  {{TODO: ob der Fall ins Bench-Gate gehört}}

## Konsequenzen

### Positiv

- Nachtimen, Austauschen oder Neuexportieren von Clips ändert keinen Zustands-Hash, kein Replay und keinen
  Golden Master; Timing bleibt im Balancing-Dashboard.
- Rewind, Snapshot-Restore und Replay-Scrubbing zeigen ohne Zusatzaufwand die richtige Pose.
- Keine neue Crate-Kante, keine Änderung an Determinismus-Menge, Pack-Format oder Skinning-Vertrag; Stufe A.
- Der vorhandene GPU-Pfad (Palette im Storage-Buffer, ein Draw-Call je Mesh) trägt unverändert.
- Die Figuren-Nutzlasten bleiben eine Familie mit einheitlichen Dekoder- und Testregeln.
- Der minimale Schnitt ist klein und entblockt den Piloten, ohne spätere Fassungen zu verbauen.

### Negativ

- Zwei Datenpunkte je Ereignis (Anker im Content, Markierung im Clip) können auseinanderlaufen. Die
  Verzerrung versteckt das als Tempowechsel; ohne Konverter-Prüfung und Sichtprüfung merkt es niemand.
- Der Animator bestimmt Spiel-Timing nicht in Blender; Timing-Änderungen laufen über Content-Daten.
- Knochengenaue Spiel-Logik (Trefferzonen an Gliedmaßen, Schwachpunkte eines Bosses, exakte Sockel zur
  Laufzeit) ist ausgeschlossen und bräuchte ein neues ADR mit Stufe I.
- Der Abtastcode liegt außerhalb der Determinismus-Menge; ein `std`-Aufruf wie `sin` fiele keinem Clippy-Lauf
  auf, nur dem goldenen Posen-Hash, und auch dem nur, wenn er plattformabhängig ist.
- `grimoire_render` wächst um ein Format und ein Modul; wer Posen ohne GPU berechnen will, zieht trotzdem
  `wgpu` mit.
- Die Engine übernimmt ein weiteres Format samt Dokument, Fixture und Tests; das Zusammenspiel mit dem
  Python-Konverter im Spiel-Repo bleibt eine Fehlerquelle.
- Eine Überblendung als reine Funktion braucht zusätzlichen gehashten Zustand im Spiel (vorige Aktion,
  Wechsel-Tick); ein Darstellungs-Cache stattdessen springt nach jedem Rewind einmal.
- Der Wurzelversatz der Clips weicht sichtbar von der Sim-Position ab: `death` endet 0,27, `spawn` 0,20 neben
  der Kapsel der Simulation (bei rund 1,0 Figurenhöhe). {{TODO: ob die Verschiebung im Bild stört}}
- Geskinnte Figuren werfen weiterhin keinen Shadowmap-Schatten (bekannte Lücke, §6); in Bewegung fällt das
  stärker auf.
- Vorzeichenwechsel der Quaternionen und die Schleifenform (letztes Bild gleich erstes) müssen Dekoder,
  Abtastung und Konverter genau einhalten; ein Fehler zeigt sich nur in einzelnen Clips (Oberschenkel in
  `walk` und `run`) als Drehsprung oder als Ruckler je Schleife.
- Die GPU-Kosten des Skinnings sind nicht gemessen und werden durch diese Entscheidung nicht kleiner; ein
  späterer GPU-Pfad für sehr große Schwärme (Option C) wäre eine eigene Vertragsänderung.

## Offene PO-Entscheidungen

1. **Grundsatz:** Engine-Modul (A) statt Spiel-eigener Animation (B), GPU-Backen (C) oder fertiger Crate (D)?
   Empfehlung: A.
2. **Takt:** Simulation führt, Clip folgt (A2) statt Clip führt (A1)? Empfehlung: A2.
3. **Ort:** Modul in `grimoire_render` (Stufe A) statt eigener Crate `grimoire_anim` (Stufe I, Crate-Map-ADR)?
   Empfehlung: Modul in `grimoire_render`.
4. **Root Motion:** keine in der Simulation; Wurzelversatz bleibt Bild; braucht die Simulation einen
   Sockelpunkt, dann als gebackene Content-Konstante? Empfehlung: ja.
5. **Ablage:** Abtastwerte in der Autorenrate, konstante Spuren einmal, keine Quantisierung, kein Umtasten auf
   60 Hz, eigene Art `FNP_CLIP` mit Pfadkonvention, `FNP_FIGURE` unverändert? Empfehlung: ja.
6. **Autorenrate:** 24 fps beibehalten (das Format speichert die Rate) statt Export mit 30 oder 60 fps, bei dem
   jedes Bild auf einem ganzen Tick läge? Empfehlung: 24 fps beibehalten; mit A2 bringt eine andere Rate nichts.
7. **Schnitt und Zeitpunkt:** minimaler Schnitt (`idle`/`walk`, harter Wechsel) als P2-Vorbereitung direkt nach
   dem statischen Piloten; Überblendung und Verzerrung vor dem Kampf-Kit? Empfehlung: ja.
8. **GPU-Backen:** zurückstellen und neu bewerten, wenn die Abtastung auf Referenz-Hardware mehr als ein Viertel
   des Extract-Budgets braucht oder das Spiel mehr als 300 gleichzeitig animierte Figuren plant? Empfehlung:
   zurückstellen.
9. **Überblend-Quelle (Spielseite):** vorige Aktion und Wechsel-Tick im Sim-Zustand statt Cache in der
   Darstellung? Empfehlung: Sim-Zustand, weil Rewind und Replay-Viewer sonst springen.

## Weitere Informationen

- **Offene Fakten** (im Text als {{TODO}} markiert): Zählbasis der Markierungen; Artnummer `FNP_CLIP`; Grenzen
  für Bildzahl und Markierungen; Grenzen der Verzerrung; GPU-Skinning-Kosten auf Referenz-Hardware; Aufwand
  des minimalen Schnitts; ob der Wurzelversatz von `death`/`spawn` stört; ob der Bench-Fall ins Gate gehört.
- **Beobachtung außerhalb dieser Entscheidung:** Die bestehenden Figuren-Nutzlasten (`FNP_MESH`,
  `FNP_MATERIAL`, `FNP_TEXTURE_RAW`, `FNP_SKELETON`, `FNP_FIGURE`) haben kein Dokument unter `docs/formats/`,
  obwohl §2 Regel 10 eines je Format verlangt; §6 nennt sie „kein Vertragsbestandteil". Das Clip-Format sollte
  diese Lücke nicht erben.
- **Datenstand:** Die Clips der Hexe gelten laut Showcase-README als „Bewegungs- und Skinning-Blocking"; eine
  Mocap-Probe mit gebackenem Stoff treibt zehn Umhang- und Haarknochen. Mehr veränderliche Spuren ändern die
  Entscheidung nicht: Für das heutige Rig ist die dichte Zeile oben die Speicher-Obergrenze (452 KiB für
  18 Clips), die Abtastkosten sind bereits für alle Kanäle gemessen, und zusätzliche Knochen wachsen linear.
- **Verwandt:** [ADR-0013](0013-downlevel-pruefung-licht-cluster-layout.md) (Storage-Buffer im Vertex-Stage, auf
  dem die Palette liegt), [ADR-0014](0014-bullet-darstellung-billboard-impostor.md) (Extraktionswerte und
  Messmethodik), Spiel-Repo [ADR-0005](https://github.com/LupusMalusDeviant/fiends-n-patrons/blob/main/docs/adr/0005-voll-deterministische-simulation.md)
  (voll deterministische Simulation) und [ADR-0007](https://github.com/LupusMalusDeviant/fiends-n-patrons/blob/main/docs/adr/0007-offline-asset-kompilierung.md)
  (Offline-Asset-Kompilierung).
