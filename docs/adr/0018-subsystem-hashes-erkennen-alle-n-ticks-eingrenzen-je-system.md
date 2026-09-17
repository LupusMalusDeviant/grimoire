# ADR-0018: Subsystem-Hashes — erkennen alle 60 Ticks, eingrenzen je System (OF-18.1)

- **Status:** Vorgeschlagen (Entscheidung durch den PO offen)
- **Datum:** 2026-09-17
- **Entscheider:** Lupus Malus Deviant (PO), Entscheidung aussteht; vorbereitet durch Claude
- **Bezug:** Spiel-Repo Plan 0002 WP7.2 (Spike OF-18.1) und WP7.4/WP7.5 (Sim-Harness, Golden Master);
  [PRD-0018](https://github.com/LupusMalusDeviant/fiends-n-patrons/blob/main/docs/prd/0018-teststrategie.md)
  (NFR „Diagnostik“, US-03, OF-18.1); Engine-Vertrag [`crate-vertraege.md`](../architektur/crate-vertraege.md)
  §7.2 (`SystemObserver`), §8.4 (`step_observed`), §8.5 (neu, `grimoire_sim::trace`);
  [ADR-0006](0006-paralleler-scheduler-deterministische-zusammenfuehrung.md) (Stufenregel);
  [ADR-0010](0010-benchmark-strategie.md) (Messgrößen); Spike-Crate `spikes/wp7.2-subsystem-hash`,
  Workflow `.github/workflows/spike-wp7.2-subsystem-hash.yml`, CI-Lauf
  [35247178166](https://github.com/LupusMalusDeviant/grimoire/actions/runs/35247178166)
  (`ubuntu-24.04`, AMD EPYC 9V74, Valgrind 3.22.0)

## Kontext

PRD-0018 verlangt, dass jeder Determinismus-Bruch automatisch den ersten abweichenden Tick und das
verursachende System nennt, „damit die Suche Minuten dauert, nicht Tage“ (US-03). Offen war, wie
dicht dafür gehasht wird (OF-18.1): nach jedem System in jedem Tick oder nur alle N Ticks.

WP7.2 hat beides umgesetzt, damit die Entscheidung eine Einstellung ist und kein Umbau
(`grimoire_sim::trace`, Vertrag §8.5):

- `SystemHasher` ist ein `SystemObserver`. Er bildet in `system_finished` den Welt-Hash nach jedem
  System. Für parallele Systeme liegt dieser Zeitpunkt nach der Anwendung ihres Befehlspuffers, der
  Hash hängt also weder vom Executor noch von `StageMode` ab (Test mit `Isolated`, `Grouped`,
  sequentiellem und permutiertem Executor im Parallel-Szenario).
- `trace(sim, log, granularity)` spielt ein `InputLog` ab und zeichnet einen `HashTrace` auf:
  `PER_SYSTEM_PER_TICK`, `every(N)` oder `every(N).with_system_hashes()`.
- `first_divergence(reference, candidate)` nennt den ersten abweichenden Tick, den letzten
  übereinstimmenden davor und, wo beide Traces System-Hashes haben, das erste abweichende System
  samt Subsystem (Namenspräfix vor dem ersten Punkt, nach dem auch der Profiler gruppiert). `is_exact()` sagt, ob der
  Zustand einen Tick vorher noch übereinstimmte; nur dann ist das System die Ursache.

Gemessen hat der Spike die echte Implementierung auf drei Szenarien, je 100 Ticks, Callgrind-`Ir`
als rauschfreie Größe (ADR-0010) und Wanduhr-Median aus 7 Läufen als Trend:

| Szenario | Zustand | Systeme |
|---|---|---:|
| `sigil10k` | `sigil_update_10k` aus `grimoire_bench`: 10.000 aktive Bullets mit Transformationen | 5 |
| `sigilchurn` | `sigil_churn_2k`: 10.000 aktiv, je Tick 2.000 Spawns und Despawns | 5 |
| `parallel12k` | Parallel-Golden-Szenario von `grimoire_sim`: 12.000 Entities, parallele Stufen | 8 |

## Messung

`Ir` je Tick (Aufbau abgezogen) und Faktor gegenüber dem reinen `step`; Wanduhr in Millisekunden je
Tick auf dem Runner:

| Modus | `sigil10k` Ir | Faktor | Wanduhr | `sigilchurn` Faktor | `parallel12k` Faktor |
|---|---:|---:|---:|---:|---:|
| `step` | 2.502.635 | 1,00 | 0,160 ms | 1,00 | 1,00 |
| `step_observed` mit `NoopObserver` | 2.502.640 | 1,00 | 0,161 ms | 1,00 | 1,00 |
| ein `state_hash` (ohne Schritt) | 2.110.894 | 0,84 | 0,283 ms | 0,76 | 0,19 |
| Zustands-Hash jeden Tick (`every(1)`) | 4.634.715 | 1,85 | 0,450 ms | 1,76 | 1,19 |
| Zustands-Hash alle 60 Ticks (`every(60)`) | 2.587.102 | 1,03 | 0,172 ms | 1,02 | 1,01 |
| je System je Tick (`PER_SYSTEM_PER_TICK`) | 15.189.924 | 6,07 | 1,862 ms | 5,38 | 2,70 |
| alle 60 Ticks mit System-Hashes | 2.903.754 | 1,16 | 0,214 ms | 1,10 | 1,04 |

Befunde:

1. **Der Beobachter-Haken kostet nichts:** 5 `Ir` je Tick.
2. **Hashen kostet so viel wie Simulieren:** Ein Welt-Hash bei 10.000 Bullets braucht 2,1 Mio. `Ir`,
   84 % eines Sigil-Schritts; auf dem Runner ist er sogar teurer als der Schritt (0,28 ms gegen
   0,16 ms). Die Kosten folgen der Zustandsgröße, nicht der Arbeit der Systeme
   (Parallel-Szenario: 19 %).
3. **Je System je Tick** bildet `Systeme + 1` Hashes je Tick (gemessen 6,0 bei 5 Systemen, 9,1 bei
   8): 6,1-fache `Ir`, 11,6-fache Wanduhr, 1,86 ms je Tick. Das liegt allein schon über dem
   Sigil-Budget von 1,0 ms. Eine Harness-Suite mit 10.000 Bullets würde 6- bis 12-mal länger laufen,
   und jedes zusätzliche Spiel-System verteuert jeden Tick um einen weiteren Welt-Hash.
4. **Alle 60 Ticks** kostet +3,4 % `Ir` (+7 % Wanduhr) und grenzt einen Bruch auf eine Sekunde ein,
   nennt aber kein System.
5. **Alle 60 Ticks mit System-Hashes** kostet +16 % `Ir` (+34 % Wanduhr). Die System-Hashes am
   erkennenden Tick nennen aber nur das erste System, dessen Welt dort abweicht. Liegt die Ursache
   in einem früheren Tick des Fensters, was bei N = 60 fast immer so ist, ist das schlicht das erste
   System der Liste (Test `every_n_ticks_with_system_hashes_names_the_first_differing_system_but_not_as_the_cause`).

## Anforderungen

1. Regelmäßige Läufe bleiben billig: Harness-Standard-Suite unter 5 Minuten (PRD-0018), Golden-Master-
   Vergleich je Push, Plattformvergleich nightly.
2. Ein Bruch wird mit exaktem erstem Tick und verursachendem System gemeldet (PRD-0018 US-03).
3. Kein neues Hash-Layout: `state_hash` und alle Goldens bleiben (sonst Stufe I).
4. Aufzeichnen ändert keinen Zustands-Hash; System-Hashes hängen nicht von Executor oder `StageMode`
   ab (beides getestet).

## Betrachtete Optionen

### Option A: Je System je Tick, immer

**Positiv:** Jeder Bruch ist sofort exakt zugeordnet, ohne zweiten Lauf.
**Negativ:** 6- bis 12-fache Laufzeit bei 10.000 Bullets, wachsend mit jedem System; verfehlt
Anforderung 1. Ein Golden Master müsste `Ticks × Systeme × 8` Byte speichern (3.600 Ticks, 13 Systeme:
374 KB je Szene).

### Option B: Nur Zustands-Hash alle N Ticks

**Positiv:** Praktisch kostenlos (+3 % bei N = 60), Master winzig (8 Byte je Checkpoint).
**Negativ:** Meldet nur ein Fenster von N Ticks und kein System; verfehlt Anforderung 2.

### Option C: Alle N Ticks mit System-Hashes an den Checkpoints

**Positiv:** Billiger als A (+16 %).
**Negativ:** Die System-Angabe ist fast nie die Ursache (Befund 5); kostet fünfmal so viel wie B für
eine Angabe, die in die falsche Richtung zeigt.

### Option D: Zweistufig — erkennen alle N Ticks, eingrenzen je System

Regelmäßige Läufe zeichnen nur den Zustands-Hash alle N Ticks auf (wie B). Weicht ein Checkpoint ab,
laufen Referenz und Kandidat bis zum letzten übereinstimmenden Checkpoint ohne Beobachter und danach
nur über das Fenster bis zum erkennenden Checkpoint mit `PER_SYSTEM_PER_TICK`; `first_divergence`
über diese beiden Fenster ist exakt (Test
`detecting_every_n_ticks_then_tracing_only_the_window_per_system_finds_the_exact_system`).

**Positiv:** Regelmäßige Kosten wie B, Diagnose so genau wie A. Die Eingrenzung kostet bei N = 60 und
10.000 Bullets je Seite rund 60 × 1,86 ms ≈ 0,11 s plus den Vorlauf mit einfachen Schritten.
**Negativ:** Die Referenzseite muss für die Eingrenzung erneut laufen. Bei Vergleichen innerhalb
einer Engine-Version (Thread-Gate, Plattformvergleich, Wiederholungslauf) ist das trivial; bei einem
Golden-Master-Bruch nach einer Code-Änderung muss die Referenz aus ihrem aufgezeichneten Stand gebaut
werden (Replay-v2-Header: `engine_build`, `app.git`, `app.engine_pin`) — das kostet Bauzeit, aber nur
im Fehlerfall.

### Option E (nicht verfolgt): Billigeres oder aufgeteiltes Hashen

Spaltenweises Hashen ganzer Bytes-Blöcke statt einzelner Werte oder je Subsystem getrennte
Zustands-Hashes würden jeden Hash verbilligen. Beides ändert das Hash-Layout (Stufe I, alle Goldens
erneuert) bzw. braucht Eigentums-Metadaten je Komponente in `grimoire_ecs`. Kein P1-Bedarf, weil D die
Kosten bereits aus den regelmäßigen Läufen herausnimmt.

## Entscheidung (Vorschlag)

**Option D mit N = 60.**

- Regelmäßige Läufe (Harness WP7.4, Golden-Master-Vergleich WP7.5, Plattformvergleich nightly)
  zeichnen den Zustands-Hash alle 60 Ticks auf (`TraceGranularity::every(60)`, gleich
  `grimoire::DEFAULT_HASH_EVERY`). PRD-0018 nennt im Beispiel 600 Ticks; 60 kosten gemessen kaum mehr
  und grenzen zehnmal enger ein.
- Die Diagnose grenzt einen Bruch mit `PER_SYSTEM_PER_TICK` über das erste abweichende Fenster ein und
  meldet `Divergence` (Tick, System, Subsystem). Die Referenz läuft dafür erneut, aus demselben Stand
  oder aus dem im Replay-Header aufgezeichneten Build.
- Golden Master speichern keine System-Hashes. Sollte sich in WP7.5 zeigen, dass die Referenz im
  Spiel-CI nicht mit vertretbarem Aufwand neu gebaut werden kann, ist der Rückfall, beim bewussten
  Erneuern eines Masters zusätzlich einen `PER_SYSTEM_PER_TICK`-Trace abzulegen (Speicherbedarf wie
  Option A, Rechenkosten nur beim Erneuern). Das entscheidet WP7.5.
- Welt-Hashes gehören wegen Befund 2 nie in jeden Tick eines regelmäßigen Laufs.

## Konsequenzen

- **Vertrag:** §8.5 beschreibt `trace`, `SystemHasher`, `HashTrace`, `TraceGranularity`,
  `first_divergence` und `Divergence` (Stufe A, PO-Freigabe offen). Die API trägt jede Option; dieses
  ADR legt nur fest, welche Dichte Harness und Golden Master verwenden.
- **WP7.4/WP7.5:** Checkpoints alle 60 Ticks im `HarnessReport` und im Master; der Diff-Report
  (erster Tick, Subsystem, Diff-Replay) ruft die Eingrenzung auf, statt Daten dafür vorzuhalten.
- **Genauigkeit:** Das gemeldete System ist das erste in Listenreihenfolge, nach dem die Welt abweicht.
  Entsteht eine Abweichung zwischen zwei Schritten (etwa durch `replace_unit` oder `restore`), zeigt
  sie sich beim ersten System des folgenden Schritts; ein Swap steht ohnehin im Replay-Header.
  Systeme einer parallelen Stufe werden einzeln zugeordnet, weil der Hash nach dem jeweiligen Puffer
  entsteht.
- **Kosten sichtbar:** Die Spike-Zahlen stehen hier und im Artefakt des CI-Laufs; eine spätere
  Änderung am Hash-Layout (Option E) braucht ein eigenes ADR mit neuer Messung.
- **Nicht betroffen:** `state_hash`, Snapshots, Replays und alle bestehenden Goldens.
