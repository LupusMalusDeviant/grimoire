# ADR-0013: Downlevel-Prüfung und Licht-/Cluster-Datenlayout für Clustered Forward+ (WP3.1)

- **Status:** Vorgeschlagen (2026-09-16; PO-Entscheid aussteht)
- **Datum:** 2026-09-16
- **Entscheider:** Lupus Malus Deviant (PO), Entscheidung aussteht; vorbereitet durch Claude
- **Bezug:** Plan 0002 WP3.1 (Spiel-Repo), Risiko R2 (Plan 0002: „256 Clustered Lights + 10k
  Bullets sprengen das 8-ms-GPU-Budget oder Storage-Buffer-Limits auf Metal/WARP/lavapipe"),
  PRD-0003 FR-11 (Lichtbudget Low 32/High 256), PRD-0018 (Teststrategie, OF-18.2 Adapter-Tabelle),
  Engine-Vertrag [`crate-vertraege.md`](../architektur/crate-vertraege.md) §6 (verschiebt den
  Lichtbudget-Speicherweg ausdrücklich auf WP3.4), [ADR-0011](0011-spekulares-anti-aliasing-statt-taa.md)/[ADR-0012](0012-schatten-key-light-shadowmap-plus-blob.md)
  (Vorlage für Struktur und Messmethodik)

## Kontext

WP3.4 (Clustered Forward+ mit 256 Punktlichtern) und WP3.5 (Bullet-Pass) brauchen Storage-Buffer im
Fragment-Stage und in Compute-Passes. Risiko R2 benennt genau die Gefahr: Diese Fähigkeit auf einem
der drei CI-Zieladapter (Windows/WARP, Linux/lavapipe, macOS „Apple Paravirtual device"/Metal) zu
knapp bemessen oder ganz zu vermissen, bevor ein Shader geschrieben ist. WP3.1 beantwortet das mit
gemessenen statt vermuteten Zahlen und legt das Datenlayout für Licht- und Cluster-Puffer fest, bevor
WP3.4 beginnt.

**Kernfrage:** Können WP3.4/WP3.5 mit echten Storage-Buffern im Fragment-Stage und in Compute
bauen, oder braucht es einen Rückfallweg — und wie sieht das Licht-/Cluster-Datenlayout aus, das
beide Wege bedienen könnte?

## Anforderungen

1. Gemessener Fähigkeitsbericht je Adapter (Features, `DownlevelCapabilities`,
   `max_storage_buffers_per_shader_stage`, `max_storage_buffer_binding_size`,
   Compute-Workgroup-Grenzen, `max_uniform_buffer_binding_size`, `max_bind_groups`,
   `max_texture_dimension_2d`), sichtbar je CI-Runner im Job-Summary (wie WP2.1).
2. Echter Funktionsnachweis: eine Fragment-Stage, die aus einem Storage-Buffer liest, und ein
   Compute-Pass, der einen Storage-Buffer beschreibt und zurückgelesen prüft — nicht nur ein
   Fähigkeitsabruf. Fehlende Fähigkeit wird übersprungen und gezählt, nie stillschweigend
   übergangen (WP2.1-Muster).
3. Ein festgelegtes Licht-/Cluster-Datenlayout als `#[repr(C)]`-Typen mit Größen-/Ausrichtungstest,
   passend zu einem Froxel-Raster von 16×9×24 (3456 Cluster) und einem Lichtbudget von 32 (Low) bis
   256 (High), plus die Rechnung des Speicherbedarfs im schlimmsten Fall.
4. Eine Empfehlung für WP3.4: Storage-Buffer im Fragment-Stage bauen oder ein Rückfallweg
   (Uniform-Buffer mit kleinerem Budget, Texturen als Datenträger)?

## Messung

Gemessen über `grimoire_gpu`s neuen WP3.1-Test `crates/grimoire_gpu/tests/downlevel.rs`
(`cargo test -p grimoire_gpu --test downlevel -- --nocapture`), je Betriebssystem im CI-Job-Summary
unter „WP3.1 Downlevel-Fähigkeiten" (`.github/scripts/report-gpu-downlevel.sh`). Ein separater,
elevierter `GpuContext` (`GpuContext::new_offscreen_with_limits`, WP3.1) fragt die Grenzen direkt
vom Adapter ab (`wgpu::Adapter::limits()`/`get_downlevel_capabilities()`/`features()`) — unabhängig
von der bewusst konservativen WebGL2-Basislinie, mit der der eigentliche Renderer
(`GpuContext::new_offscreen`) seine Geräte anfordert und die weder Storage-Buffer noch Compute
erlaubt (`max_storage_buffers_per_shader_stage = 0`, `max_compute_invocations_per_workgroup = 0`).

### Ergebnis: gemessene Grenzwerte je Adapter

| Adapter | `fragment_storage` | `fragment_writable_storage` | `compute_shaders` | `vertex_storage` | `shader_model` | `max_storage_buffers_per_shader_stage` | `max_storage_buffer_binding_size` | `max_compute_workgroup_size` | `max_compute_invocations_per_workgroup` | `max_uniform_buffer_binding_size` | `max_bind_groups` | `max_texture_dimension_2d` | Funktionsnachweis |
|---|---|---|---|---|---|---:|---:|---|---:|---:|---:|---:|---|
| Windows/WARP (Dx12) | true | true | true | true | Sm5 | 262144 | 2147483644 (≈ 2 GiB) | 1024×1024×64 | 1024 | 65536 (64 KiB) | 8 | 16384 | beide Pipelines liefen wirklich (lokal gemessen, `GRIMOIRE_GPU_ADAPTER=software`) |
| Linux/lavapipe (Vulkan) | _CI ausstehend_ | _CI ausstehend_ | _CI ausstehend_ | _CI ausstehend_ | _CI ausstehend_ | _CI ausstehend_ | _CI ausstehend_ | _CI ausstehend_ | _CI ausstehend_ | _CI ausstehend_ | _CI ausstehend_ | _CI ausstehend_ | _CI ausstehend_ |
| macOS „Apple Paravirtual device" (Metal) | _CI ausstehend_ | _CI ausstehend_ | _CI ausstehend_ | _CI ausstehend_ | _CI ausstehend_ | _CI ausstehend_ | _CI ausstehend_ | _CI ausstehend_ | _CI ausstehend_ | _CI ausstehend_ | _CI ausstehend_ | _CI ausstehend_ | _CI ausstehend_ |

Windows/WARP lokal gemessen (kein Fenster, `GRIMOIRE_GPU_ADAPTER=software`); Linux/lavapipe und
macOS laufen nur in der CI (Windows-Entwicklungsrechner, keine Belastung der lokalen GPU, Vorgabe
dieses Arbeitspakets) — diese ADR wird nach dem ersten grünen CI-Lauf mit den echten Zahlen aus dem
Job-Summary ergänzt, bevor sie als angenommen gilt.

## Datenlayout

`grimoire_render::cluster_layout` (neu, additiv, **nicht** Teil des Render-Vertrags — Vertrag §6
verschiebt den Lichtbudget-Speicherweg ausdrücklich auf WP3.4 und schließt ihn von "diesem Vertrag"
ausdrücklich aus; dieses Modul ist genau die dort erwartete Vorarbeit, kein Vertragsbruch):

Drei Storage-Buffer, die Standardform für Clustered-Forward+-Licht-Culling (Lichtliste,
Cluster-Tabelle, Index-Liste):

1. **Lichtliste** (`GpuPointLight`, 32 Byte, `#[repr(C, align(16))]`): `position`+`range` und
   `color`+`intensity` als je ein 16-Byte-std430-Element — keine Auffüllbytes zwischen
   Array-Einträgen nötig.
2. **Cluster-Tabelle** (`GpuClusterLightRange`, 8 Byte, `#[repr(C)]`): `offset`+`count` je Cluster,
   fest bei 3456 Einträgen (16×9×24-Raster), unabhängig vom Lichtbudget.
3. **Index-Liste** (`u32`): flache Liste von Lichtindizes, je Cluster durch die Cluster-Tabelle
   geschnitten. Ihre reale Größe hängt vom tatsächlichen Licht-/Cluster-Überlapp ab (Sache der
   Culling-Logik, die WP3.4 erst baut); `light_index_list_worst_case_len`/`_bytes` liefern die
   sichere Obergrenze (jedes Licht erreicht jeden Cluster).

Alle Typen und Größenberechnungen sind mit `size_of`/`align_of`-Tests eingefroren
(`crates/grimoire_render/src/cluster_layout.rs`).

### Ergebnis: Speicherbedarf im schlimmsten Fall

| | Low (32 Lichter) | High (256 Lichter) |
|---|---:|---:|
| Lichtliste (32 Byte × Budget) | 1.024 Byte | 8.192 Byte |
| Cluster-Tabelle (8 Byte × 3456, budgetunabhängig) | 27.648 Byte | 27.648 Byte |
| Index-Liste, schlimmster Fall (4 Byte × 3456 × Budget) | 442.368 Byte | 3.538.944 Byte |
| **Summe** | **471.040 Byte (≈ 460 KiB)** | **3.574.784 Byte (≈ 3,41 MiB)** |

### Abgleich gegen die gemessene Grenze

Gegen Windows/WARPs gemessenes `max_storage_buffer_binding_size = 2147483644` (≈ 2 GiB) liegt selbst
das High-Budget-Total (≈ 3,41 MiB) um mehr als das 500-fache darunter — ein Test
(`worst_case_high_budget_fits_the_measured_warp_limit` in `cluster_layout.rs`) hält diesen Vergleich
gegen die aufgezeichnete WARP-Zahl fest. Da jeder der drei Puffer einzeln gebunden wird, ist ohnehin
nur die *größte einzelne* Grenze relevant (die Index-Liste, ≈ 3,375 MiB im High-Budget) — auch das
ist auf WARP kein Problem. Lavapipe- und Metal-Zahlen ergänzen diesen Abschnitt nach dem ersten
grünen CI-Lauf; sollte einer von beiden `max_storage_buffer_binding_size` unter einigen MiB melden
(unwahrscheinlich für Vulkan/Metal, beide WebGPU-Kernfunktionalität), wäre das der erste konkrete
Hinweis auf einen nötigen Rückfallweg.

## Empfehlung

**Vorschlag: WP3.4 mit echten Storage-Buffern im Fragment-Stage und in Compute bauen — kein
Rückfallweg auf Uniform-Buffer oder Texturen nötig, vorbehaltlich der ausstehenden lavapipe-/
Metal-Zahlen.**

Begründung:

- WARP (Windows) erfüllt alle vier relevanten Downlevel-Flags (`fragment_storage`,
  `fragment_writable_storage`, `compute_shaders`, `vertex_storage`) und hat mit ≈ 2 GiB
  `max_storage_buffer_binding_size` keinerlei praktische Enge für ≈ 3,41 MiB Spitzenbedarf.
- DX12, Vulkan und Metal sind, anders als WebGL/GLES 3.0 (auf das die bestehende
  `new_offscreen`-Konfiguration bewusst zusätzlich vorbereitet ist), reguläre native Backends, für
  die Storage-Buffer im Fragment-Stage und Compute-Shader Kernfunktionalität sind, keine optionalen
  Erweiterungen für schwache Hardware. Der Funktionsnachweis (Fragment-Storage-Read, Compute-Write
  plus Readback) ist deshalb auf Vulkan/lavapipe und Metal/Apple-Paravirtual mit hoher
  Wahrscheinlichkeit ebenso grün — zu bestätigen durch den ersten CI-Lauf, siehe „Messung".
- Sollte lavapipe oder die macOS-Paravirtual-Metal-Implementierung wider Erwarten eine der vier
  Downlevel-Flags oder eine der Speichergrenzen verfehlen, bleibt der hier festgelegte
  Drei-Puffer-Aufbau (Lichtliste/Cluster-Tabelle/Index-Liste) auch mit Uniform-Buffern denkbar **nur
  für die Lichtliste selbst** (8.192 Byte im High-Budget passen unter praktisch jede
  `max_uniform_buffer_binding_size`, auch die hier gemessenen 64 KiB) — Cluster-Tabelle (27.648
  Byte) und erst recht die Index-Liste (bis zu 3,54 MB) sprengen einen typischen
  Uniform-Buffer-Grenzwert deutlich. Ein echter Rückfallweg müsste deshalb entweder die
  Cluster-Auflösung drastisch reduzieren, das Lichtbudget für den Fallback-Pfad weiter senken als
  PRD-0003 FR-11 vorsieht, oder Cluster-Tabelle/Index-Liste als Textur kodieren (z. B. eine
  `R32Uint`-2D-Textur, adressiert über Cluster-Koordinate statt linearem Index) — keine dieser
  Optionen ist mit dieser ADR entschieden, weil die Messung sie aller Voraussicht nach nicht braucht.

**Offener Punkt für den Product Owner:** Bestätigung dieser Empfehlung, sobald die lavapipe-/
Metal-Zahlen aus dem ersten grünen CI-Lauf oben eingetragen sind (siehe „Messung"). Meine Empfehlung:
bei durchweg grünem Funktionsnachweis auf allen drei Betriebssystemen gilt der Rückfallweg als nicht
nötig und WP3.4 kann direkt auf dem hier festgelegten Storage-Buffer-Layout aufbauen.

## Entscheidung

**Vorschlag:** Downlevel-Fähigkeitsbericht und Funktionsnachweis wie oben umgesetzt
(`grimoire_gpu`); Licht-/Cluster-Datenlayout wie oben festgelegt (`grimoire_render::cluster_layout`),
noch nicht mit `StageFrame`/`Renderer` verdrahtet — das bleibt WP3.4 vorbehalten, per eigenem
Vertrags-PR (§2b) vor dessen Start. Kein Rückfallweg wird jetzt gebaut; die Empfehlung oben hält
fest, unter welcher Bedingung einer nötig würde.

Diese ADR gilt erst nach PO-Freigabe als angenommen; bis dahin bleibt die Umsetzung im Code lauffähig
(die Tests bestehen unabhängig vom Status dieser ADR).

## Konsequenzen

### Positiv

- Risiko R2 ist mit gemessenen (nicht vermuteten) Zahlen beantwortet, bevor WP3.4 einen einzigen
  Shader schreibt.
- Das Licht-/Cluster-Layout ist eingefroren und größengetestet, bevor WP3.4 beginnt — Änderungen
  danach laufen über das Vertragsänderungs-Protokoll (§2b) wie bei `BulletInstance`.
- Der Funktionsnachweis prüft echtes Verhalten (zwei laufende Pipelines mit festen Erwartungswerten),
  nicht nur gemeldete Flags, die eine Implementierung theoretisch falsch melden könnte.
- Skip-und-Zähl-Verhalten (WP2.1-Muster) macht eine fehlende Fähigkeit auf einem zukünftigen
  Ziel-Adapter sofort sichtbar, statt sie stillschweigend zu übergehen.

### Negativ

- Die Empfehlung ist bis zum ersten grünen CI-Lauf vorläufig (nur Windows/WARP lokal bestätigt);
  diese ADR muss vor der PO-Freigabe mit den lavapipe-/Metal-Zahlen ergänzt werden.
- Der Rückfallweg (Uniform-Buffer/Texturen) ist nur skizziert, nicht implementiert oder gemessen —
  sollte er doch gebraucht werden, ist das ein eigenes Arbeitspaket.
- Das Index-Listen-Worst-Case (jedes Licht erreicht jeden Cluster) ist bewusst pessimistisch; der
  reale Speicherbedarf nach echtem Culling dürfte deutlich kleiner sein, aber das misst erst WP3.4.

## Weitere Informationen

- Code: `crates/grimoire_gpu/src/context.rs` (`GpuContext::{capability_report_lines,
  new_offscreen_with_limits}`), `crates/grimoire_gpu/tests/downlevel.rs`,
  `crates/grimoire_render/src/cluster_layout.rs`.
- CI: `.github/scripts/report-gpu-downlevel.sh`, `.github/workflows/ci.yml` (Schritte „Test WP3.1
  downlevel capability probe on the CPU adapter" und „Report WP3.1 downlevel capabilities").
- V-20-Kandidat für eine künftige PO-Sammelsitzung: Bestätigung der Empfehlung oben nach dem ersten
  grünen CI-Lauf; ob `grimoire_render::cluster_layout` unverändert in WP3.4 übernommen wird oder vor
  dessen Start noch einmal per Vertrags-PR angepasst werden soll.
- Review dieser Entscheidung an WP3.4s Start (das Layout hier ist Eingabe, keine Umsetzung).
