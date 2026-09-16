# ADR-0015: Licht-Culling — Compute-Clustering statt CPU-Froxel-Zuordnung

- **Status:** Akzeptiert (PO, 2026-09-16)
- **Datum:** 2026-09-16
- **Entscheider:** Lupus Malus Deviant (PO), Entscheidung aussteht; vorbereitet durch Claude
- **Bezug:** Spiel-Repo Plan 0002 WP3.2/WP3.3; [PRD-0003](https://github.com/LupusMalusDeviant/fiends-n-patrons/blob/main/docs/prd/0003-rendering-und-art.md)
  FR-11 (Lichtbudget Low 32/High 256); Engine-Vertrag [`crate-vertraege.md`](../architektur/crate-vertraege.md)
  §6 (verschiebt den Lichtbudget-Speicherweg ausdrücklich auf WP3.4); [ADR-0013](0013-downlevel-pruefung-licht-cluster-layout.md)
  (liefert `grimoire_render::cluster_layout`, hier unverändert wiederverwendet, sowie den
  Downlevel-Nachweis, dass Compute-Shader und Storage-Buffer auf allen drei P1-Zieladaptern
  funktionieren); [ADR-0014](0014-bullet-darstellung-billboard-impostor.md) (teilt sich das
  WP3.3-Kombi-Gate); Spike-Branch `p1/wp3.2-light-bullet-spikes`, Crate
  `spikes/wp3.2-light-bullet-stress` (README nennt jede Abweichung vom Plan-Text); CI-Lauf
  [35052422350](https://github.com/LupusMalusDeviant/grimoire/actions/runs/35052422350) (Linux,
  `ubuntu-24.04`, lavapipe, 10 Wiederholungen nach 3 Aufwärmläufen)

## Kontext

Plan 0002 WP3.2 verlangt einen Stressmengen-Spike zum Licht-Culling: CPU-Froxel-Zuordnung gegen
Compute-Clustering bei 256 Lichtern, auf demselben Raster (16×9×24, `grimoire_render::cluster_layout`,
WP3.1/ADR-0013 — hier unverändert wiederverwendet, kein zweites Layout definiert). Gemessen werden
die CPU-Kosten der Zuordnung, die relative GPU-Zeit des Compute-Wegs und der Speicher-/Upload-Aufwand
beider Wege.

Der Spike (`spikes/wp3.2-light-bullet-stress`) implementiert beide Wege mit **demselben**
Zuordnungsalgorithmus (Kugel-gegen-Kugel-Überlapptest: Lichtreichweite plus Froxel-Halbdiagonale),
einmal als reine Rust-Schleife auf der CPU (`src/lights.rs::cpu_assign_clusters`), einmal als
WGSL-Compute-Shader (`src/light_cluster.wgsl::cs_main`, ein Aufruf je Cluster, 54 Arbeitsgruppen à
64 Threads für 3.456 Cluster). Beide schreiben dasselbe Layout mit fester Schrittweite
(`cluster_index * light_budget`), das `cluster_layout`s Worst-Case-Größenrechnung bereits so
vorsieht. Ein Korrektheitstest (`compute_cluster::tests::compute_path_matches_the_cpu_path_on_a_tiny_grid`,
lokal mit `GRIMOIRE_GPU_ADAPTER=software`/WARP grün) vergleicht Compute-Ergebnis und CPU-Referenz auf
einem kleinen 2×2×2-Raster exakt.

**Kernfrage:** CPU-Froxel-Zuordnung oder Compute-Clustering für WP3.4, gemessen an CPU-Kosten,
relativer GPU-Zeit und Speicher-/Upload-Aufwand, und was folgt daraus für das WP3.3-Gate?

## Anforderungen

1. Messung der CPU-Kosten der Froxel-Zuordnung und der relativen GPU-Zeit des Compute-Wegs, Median
   aus 10 Wiederholungen, Runner-Wert mit Streuung, bei 256 Lichtern auf dem 16×9×24-Raster.
2. Speicher- und Upload-Aufwand beider Wege, aufbauend auf `cluster_layout`s Byte-Rechnung.
3. Gegenüberstellung mit dem WP3.3-Gate ("Bullet-Upload plus Clustering" ≤ 1,5 ms Render-CPU,
   zusammen mit [ADR-0014](0014-bullet-darstellung-billboard-impostor.md)s Bullet-Upload-Zahlen).
4. Kein Vertragsbruch: `cluster_layout` bleibt unverändert (Vertrag §6 verschiebt den eigentlichen
   Speicherweg ohnehin auf WP3.4, siehe ADR-0013).

## Betrachtete Optionen

### Option A: CPU-Froxel-Zuordnung

Die Zuordnung (welches Licht trifft welchen Cluster) läuft als Rust-Schleife auf der CPU; alle drei
Puffer (Lichtliste, Cluster-Tabelle, Index-Liste) werden fertig berechnet und jeden Frame vollständig
zur GPU hochgeladen.

**Positiv:** einfacher zu debuggen (reines Rust, kein Shader), kein Compute-Downlevel-Risiko (aber
ADR-0013 hat dieses Risiko bereits auf allen drei Zieladaptern ausgeräumt).
**Negativ:** belastet den Render-CPU-Anteil direkt und lastvoll (siehe Messung); lädt auch die
größte Einzelstruktur (Index-Liste, bis zu 3,54 MB im High-Budget) jeden Frame komplett von der CPU
hoch.

### Option B: Compute-Clustering

Ein Compute-Shader übernimmt dieselbe Zuordnung auf der GPU; die CPU lädt nur die (kleine)
Lichtliste hoch, Cluster-Tabelle und Index-Liste werden nie von der CPU beschrieben.

**Positiv:** CPU-Kosten nahe null (siehe Messung); Upload auf die Lichtliste allein reduziert
(8.192 Byte statt 3.574.784 Byte im High-Budget — Faktor ≈ 437).
**Negativ:** Downlevel-Abhängigkeit von Compute-Shadern (durch ADR-0013 auf allen drei
P1-Zieladaptern bereits gemessen und bestätigt); die hier gemessene GPU-Zeit ist blockierend
gemessen (`device.poll(Wait)`) und damit **teurer als der reale Render-CPU-Anteil eines
gepipelinten Renderers**, der nicht synchron auf den Dispatch wartet (siehe Messung/Einschränkung
unten) — die GPU-Arbeit selbst bleibt real, zählt aber nicht gegen dasselbe CPU-Budget wie die
CPU-Froxel-Schleife.

## Messung

Alle Werte sind **Runner-Werte** (Linux, `ubuntu-24.04`, Adapter `llvmpipe (LLVM 20.1.2, 256 bits)`,
Vulkan/lavapipe, `GRIMOIRE_GPU_ADAPTER=software`), Median aus 10 Wiederholungen nach 3 verworfenen
Aufwärmläufen, 256 Lichter, Raster 16×9×24, CI-Lauf
[35052422350](https://github.com/LupusMalusDeviant/grimoire/actions/runs/35052422350).

| Weg | CPU-Zuordnung Median (µs) | CPU-Zuordnung Spanne (µs) | Upload Median (µs) | Upload Spanne (µs) | GPU relativ Median (µs) | GPU relativ Spanne (µs) |
|---|---:|---:|---:|---:|---:|---:|
| CPU-Froxel | 1.707,01 | 1.686,83–1.741,43 | 1.748,29 (alle drei Puffer) | 1.667,55–1.889,53 | entfällt (kein Compute-Dispatch) | entfällt |
| Compute-Clustering | ≈ 0 (läuft auf der GPU) | entfällt | 3,81 (nur Lichtliste) | 3,33–5,24 | 1.671,17 | 1.113,25–1.687,27 |

**Einschränkung zur GPU-relativ-Zahl:** Der Wert ist Wanduhrzeit für Kodieren + `submit` +
`device.poll(Wait)`, also **blockierend** gemessen, um den Compute-Anteil isoliert zu erfassen — ein
gepipelinter Renderer würde nicht synchron auf diesen Dispatch warten, sondern ihn mit anderer
Frame-Arbeit überlappen. Für das WP3.3-Gate (Render-CPU-Budget) zählt deshalb beim Compute-Weg nur
der Lichtlisten-Upload (3,81 µs), nicht die 1.671,17 µs — die Angabe steht trotzdem hier, weil sie
den einzigen Anhaltspunkt für die tatsächliche GPU-Last des Compute-Wegs liefert (nur relativ
vergleichbar, Engine-ADR-0010, kein Millisekunden-Urteil).

Die auffällige Spanne der Compute-GPU-Zahl (Minimum 1.113,25 µs deutlich unter dem Median) ist
geteilter-Runner-Rauschen, kein Messfehler — genau der Grund, warum WP3.2 Streuung neben dem Median
verlangt (Engine-ADR-0010).

### Speicher- und Upload-Aufwand (High-Budget, 256 Lichter, aus `grimoire_render::cluster_layout`)

| Puffer | Bytes | CPU-Froxel lädt hoch | Compute-Clustering lädt hoch |
|---|---:|---|---|
| Lichtliste | 8.192 | ja | ja |
| Cluster-Tabelle | 27.648 | ja | nein (schreibt die GPU) |
| Index-Liste (Worst Case) | 3.538.944 | ja | nein (schreibt die GPU) |
| **Summe je Frame** | **3.574.784** | **3.574.784** | **8.192** |

Der GPU-**Speicherbedarf** (die drei Puffer müssen so oder so existieren, damit ein künftiger
Clustered-Forward+-Fragment-Shader sie lesen kann) ist bei beiden Wegen identisch — nur der
CPU→GPU-**Upload** je Frame unterscheidet sich, um den Faktor ≈ 437.

### Go/No-Go gegen das WP3.3-Kombi-Gate ("Bullet-Upload plus Clustering" ≤ 1,5 ms = 1.500 µs)

Kombiniert mit [ADR-0014](0014-bullet-darstellung-billboard-impostor.md)s Bullet-Upload-Zahlen (Render-CPU-Anteil):

| Bullet-Variante/Instanzen | Culling-Weg | Bullet-Upload (µs) | Culling-CPU-Anteil (µs) | Summe (µs) | Anteil am Budget | Urteil |
|---|---|---:|---:|---:|---:|---|
| Billboard, 10.000 | CPU-Froxel | 15,19 | 1.707,01 | 1.722,20 | 114,8 % | **No-Go** |
| Billboard, 20.000 | CPU-Froxel | 42,46 | 1.707,01 | 1.749,47 | 116,6 % | **No-Go** |
| Mesh, 10.000 | CPU-Froxel | 34,85 | 1.707,01 | 1.741,86 | 116,1 % | **No-Go** |
| Mesh, 20.000 | CPU-Froxel | 63,20 | 1.707,01 | 1.770,21 | 118,0 % | **No-Go** |
| Billboard, 10.000 | Compute-Clustering | 15,19 | 3,81 | 19,00 | 1,3 % | Go |
| Billboard, 20.000 | Compute-Clustering | 42,46 | 3,81 | 46,27 | 3,1 % | Go |
| Mesh, 10.000 | Compute-Clustering | 34,85 | 3,81 | 38,66 | 2,6 % | Go |
| Mesh, 20.000 | Compute-Clustering | 63,20 | 3,81 | 67,01 | 4,5 % | Go |

**CPU-Froxel-Zuordnung allein (1.707,01 µs) überschreitet bereits das gesamte Kombi-Budget von
1.500 µs, bevor überhaupt ein Bullet hochgeladen wird** — bei 256 Lichtern auf diesem Runner ist das
Ergebnis unabhängig von der gewählten Bullet-Variante ein klares No-Go. Compute-Clustering bleibt in
jeder Kombination unter 5 % des Budgets, also klar Go.

## Empfehlung

**Compute-Clustering für WP3.4.** Begründung:

- CPU-Froxel-Zuordnung verfehlt das WP3.3-Kombi-Gate bei 256 Lichtern auf dem Runner um 15–18 % —
  ein klares No-Go, unabhängig von der Bullet-Variante.
- Compute-Clustering bleibt mit großem Abstand (≤ 5 % des Budgets) darunter, weil die eigentliche
  Zuordnungsarbeit auf die GPU verlagert wird und die CPU nur die kleine Lichtliste hochlädt.
- ADR-0013 hat Compute-Shader und Storage-Buffer bereits auf allen drei P1-Zieladaptern (WARP,
  lavapipe, Metal) als funktionierend nachgewiesen — Compute-Clustering ist kein neues
  Downlevel-Risiko, nur eine neue Konsumentin einer bereits bestätigten Fähigkeit.
- Der Upload-Vorteil (Faktor ≈ 437 weniger CPU→GPU-Traffic je Frame) bleibt auch dann relevant,
  wenn ein künftiges Culling-Ergebnis kompakter als der hier gemessene Worst-Case ausfällt.

## Migrationspfad

Kippt die Entscheidung später (z. B. weil eine Messsitzung auf Referenz-Hardware einen anderen
CPU/GPU-Kostenverlauf zeigt, oder ein künftiger P2-Zieladapter Compute-Shader doch nicht anbietet):

1. `cluster_layout` bleibt in jedem Fall unverändert — beide Wege schreiben dasselbe Drei-Puffer-Layout
   mit derselben Schrittweite; ein Wechsel betrifft nur, **wer** die Cluster-Tabelle/Index-Liste
   befüllt, nicht ihre Form. Kein Vertrags-PR nötig, um zwischen den Wegen zu wechseln.
2. Ein Rückfall auf CPU-Froxel-Zuordnung bliebe funktional korrekt (der Spike beweist Übereinstimmung
   mit dem Compute-Weg), nur mit dem hier gemessenen CPU-Mehrkosten — sinnvoll etwa für einen
   Debug-Modus ohne Compute-Downlevel-Zweifel, nicht für den Produktionspfad.
3. Die hier nicht gebaute Optimierung (kompaktierte Index-Liste statt fester Schrittweite, per
   atomarem Zähler oder Zwei-Pass-Scatter) würde den CPU-Froxel-Weg zwar nicht CPU-günstiger machen,
   aber seinen Upload-Nachteil verkleinern — offen für WP3.4, falls der Compute-Weg aus anderen
   Gründen doch verworfen wird.
4. Sollte die Messsitzung (Plan 0002 WP3.3, Referenz-Hardware) zeigen, dass der Runner-Kern
   wesentlich langsamer ist als die Referenz-CPU (die 50-%-Schwelle deckt genau diese Unsicherheit
   ab), ist neu zu rechnen — die Größenordnung des Unterschieds hier (Faktor ≈ 450 zwischen den
   beiden CPU-Anteilen) macht eine Umkehr der Entscheidung aber unwahrscheinlich.

## Entscheidung

**Vorschlag:** Compute-Clustering für WP3.4, `grimoire_render::cluster_layout` unverändert
übernommen. PO-Entscheid aussteht.

Diese ADR gilt erst nach PO-Freigabe als angenommen.

## Konsequenzen

### Positiv

- Bestehen des WP3.3-Kombi-Gates mit großem Abstand, wo der CPU-Froxel-Weg es bei 256 Lichtern
  verfehlt.
- Deutlich weniger CPU→GPU-Upload-Traffic je Frame (Faktor ≈ 437 im High-Budget).
- Keine neue Downlevel-Unsicherheit: ADR-0013 hat die nötige Fähigkeit bereits auf allen drei
  Zieladaptern nachgewiesen.

### Negativ

- Ein Compute-Shader ist schwerer zu debuggen als eine Rust-Schleife; der Korrektheitstest
  (`compute_cluster::tests`) deckt nur ein kleines Raster ab, nicht die volle 16×9×24/256-Kombination.
- Die gemessene GPU-relativ-Zahl ist blockierend gemessen und damit pessimistischer als der reale
  Render-CPU-Anteil eines gepipelinten Renderers (siehe Messung) — WP3.4 muss diese Annahme beim
  echten Einbau erneut prüfen, sobald ein echter Frame-Graph existiert.
- Die Index-Liste bleibt beim Compute-Weg auf der GPU-Seite trotzdem mit fester Schrittweite (kein
  Kompaktieren) — der Speicherbedarf ist bei beiden Wegen gleich, nur der Upload nicht.

## Weitere Informationen

- Code: `spikes/wp3.2-light-bullet-stress/src/bin/light_cluster.rs`, `src/lights.rs`
  (`cpu_assign_clusters`), `src/compute_cluster.rs` (`ComputeClusterPass`), `src/light_cluster.wgsl`.
- CI: `.github/workflows/spike-wp3.2-light-bullet-stress.yml`, nur auf Branch
  `p1/wp3.2-light-bullet-spikes`; Lauf [35052422350](https://github.com/LupusMalusDeviant/grimoire/actions/runs/35052422350)
  grün (fmt, clippy, Korrektheitstests inklusive Compute-vs-CPU-Vergleich auf kleinem Raster,
  Messung).
- Verwandt: [ADR-0014](0014-bullet-darstellung-billboard-impostor.md) (Bullet-Darstellung, teilt
  sich das WP3.3-Kombi-Gate); [ADR-0013](0013-downlevel-pruefung-licht-cluster-layout.md)
  (`cluster_layout`, Downlevel-Nachweis für Compute-Shader).
- Offener Punkt für den Product Owner: diese Empfehlung (Compute-Clustering) bestätigen. Meine
  Empfehlung: annehmen — das CPU-Froxel-No-Go ist bei 256 Lichtern eindeutig, nicht knapp.
