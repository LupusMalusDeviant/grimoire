# ADR-0010: Benchmark-Strategie für das Regressions-Gate (OF-17.3)

- **Status:** Akzeptiert (2026-09-15; PO-Entscheidung in Sammelsitzung B)
- **Datum:** 2026-09-15
- **Autor:** Claude (Ausarbeitung, unbeaufsichtigter Lauf) im Auftrag von Lupus Malus Deviant (PO)
- **Konsultiert:** — (PO-Entscheidung in Sammelsitzung B erfolgt; dieses ADR ist der Messnachweis zur vorbereiteten
  Empfehlung im Spiel-Repo, `docs/plans/0002-vorbereitung-of-17.3.md`)
- **Bezug:** Spiel-Repo PRD-0017 FR-03, PRD-0018; Plan 0002 WP6.1/WP6.2/WP6.5–6.7, OF-17.3; die
  Vorbereitung `docs/plans/0002-vorbereitung-of-17.3.md` (Kandidaten K1–K7, Versuchsprotokoll,
  Entscheidungsverfahren); Spike-Branch `p1/wp6.1-bench-spike`, Crate `spikes/bench-noise`
  (`spikes/bench-noise/README.md` nennt jede Abweichung vom vorbereiteten Protokoll); CI-Läufe
  34995502352 (Linux, 10 Wiederholungen) und 34997032190 (Linux erneut + Windows/macOS-Vergleich)

## Kontext und Problemstellung

Plan 0002 WP6.1 verlangt einen Spike zu OF-17.3, bevor WP6.2 ein Regressions-Gate baut: Wie laut ist
ein Benchmark auf gehosteten Runnern, und welche Metrik erträgt einen scharfen `>10 %`-Bruch, ohne
Fehlalarme zu erzeugen (R10)? Die Vorbereitung nennt sieben Kandidaten (K1–K7) und ein
Versuchsprotokoll mit zwei Messblöcken auf drei Betriebssystemen. Dieser Spike führt eine
kostengünstigere, aber ausreichende Fassung davon aus (Begründung und vollständige Abweichungsliste
in `spikes/bench-noise/README.md`): zwei P0-förmige Benches (`ecs_query_10k`, `sim_step_600`), vier
statt sechs Kandidaten geprüft (K1 Wanduhr, K4 Instruktionszählung; K3 A/B-im-Job und K5 `perf`
wurden nicht gemessen), vier statt sechs Einspeisungs-Varianten (`baseline`, `+5 %`, `+12 %`,
`+20 %`, kein `+15 %`, keine cache-feindliche Variante), 10 statt 30 Wanduhr-Stichproben je Job, ein
Messfenster statt zwei Blöcken an verschiedenen Tagen, kein Zwei-Commit-Build-Pfad-Nachweis. Für K4
liest dieser Spike die `summary:`-Zeile von Callgrind direkt statt über `gungraun`/`iai-callgrind` zu
gehen — für eine einmalige Zahl pro Lauf spart das eine ungeprüfte Rust-API/JSON-Schema-Abhängigkeit;
fällt K4 wie gemessen aus, schlägt dieses ADR `gungraun` für das echte `grimoire_bench` in WP6.2 vor,
wo seine Regressionswerkzeuge den Mehraufwand wert sind.

**Messaufbau.** Je Bench vier Varianten (0/+5/+12/+20 % zusätzliche, identische Arbeit innerhalb des
Messbereichs, als Laufzeitparameter — ein Build deckt alle Varianten ab). Linux: 10 sequentielle
Wiederholungen (`max-parallel: 1`) je Metrik, in zwei unabhängigen Pushes (Lauf 34995502352 und
erneut in 34997032190) — damit liegen sowohl Job-zu-Job- als auch Lauf-zu-Lauf-Werte vor. Windows und
macOS: nur Wanduhr, nur `baseline`/`+20 %`, 3 Wiederholungen (Hard Rule: diese OS nur zum
Wanduhr-Vergleich, Nebenläufigkeit moderat halten).

## Anforderungen

### Funktional

- Eine Metrik muss auf Linux einen `>10 %`-Bruch scharf erkennen (10/10 Treffer bei `+12 %`/`+20 %`)
  und darf `+5 %` nicht auslösen — ohne dass Job-zu-Job-Rauschen selbst über 10 % liegt (Rauschband
  `B`, plan §3.3/§3.5).
- Windows und macOS liefern mindestens eine Wanduhr-Einordnung für den Trend (nicht zwingend
  gate-fähig).
- Ein Negativnachweis zeigt, dass der Vergleicher eine eingespeiste Regression tatsächlich als
  Fehler meldet (Exitcode-Tabelle, plan §5) — hier über das `gate_decision`-Unit-Test-Set, das nur in
  CI läuft (`cargo test`, Job `aggregate`/`linux-aggregate`, beide Läufe grün).

### Nicht-funktional

- Exakte Ganzzahlvergleiche an der 10-%-Grenze (kein Gleitkomma-Rundungsfehler, plan §4.3).
- Keine Drittabhängigkeit im Spike, deren API/Schema ungeprüft ist (Begründung für den rohen
  Callgrind-Weg statt `gungraun`).
- Modester Ressourcenverbrauch: `max-parallel: 1` auf Linux, Windows/macOS nur mit 3 statt 10
  Wiederholungen, kein zusätzlicher Druck auf eine parallel laufende 3-OS-CI eines anderen PR.

## Betrachtete Optionen

### Option 1: K1 — Wanduhr-Median als Gate-Metrik

**Gemessen** (Median aus 10 bzw. 15 Stichproben je Job, robuster VK = `1,4826·MAD/Median`):

| Bench | OS | Lauf | Jobs | Robuster VK | Klassischer VK | Spannweite/Median | Rauschband B |
|---|---|---|---|---|---|---|---|
| `ecs_query_10k` | Linux | 34995502352 | 10 | 17,6 % | 24,5 % | 57,6 % | 105,0 % |
| `ecs_query_10k` | Linux | 34997032190 | 10 | 38,6 % | 31,9 % | 71,2 % | 129,2 % |
| `sim_step_600` | Linux | 34995502352 | 10 | 17,4 % | 23,1 % | 54,9 % | 94,9 % |
| `sim_step_600` | Linux | 34997032190 | 10 | 32,3 % | 28,5 % | 59,5 % | 95,5 % |
| `ecs_query_10k` | Windows | 34997032190 | 3 | 1,1 % | 4,1 % | 7,5 % | 7,6 % |
| `ecs_query_10k` | macOS | 34997032190 | 3 | 7,5 % | 14,2 % | 28,2 % | 29,7 % |
| `sim_step_600` | Windows | 34997032190 | 3 | 0,4 % | 13,1 % | 24,6 % | 24,7 % |
| `sim_step_600` | macOS | 34997032190 | 3 | 0,0 % | 0,8 % | 1,4 % | 1,4 % |

**Positiv:**
- Billig, keine Zusatzabhängigkeit, läuft auf allen drei Betriebssystemen.
- Der einzige Weg zu absoluten Millisekunden-Budgets (WP6.7) und zu N-Thread-Benches.
- Auf den zwei Stichproben von Windows und macOS (nur 3 Jobs) ungewöhnlich ruhig — aber 3 Jobs
  beweisen nichts; das ist ein erster Hinweis, kein Nachweis.

**Negativ:**
- Auf Linux ein Rauschband von 95–129 % zwischen den zehn Jobs *desselben* Laufs — mehr als das
  Zehnfache der 10-%-Schwelle, die es gaten soll. `+12 %` wird in nur 5 von 10 Jobs als Regression
  erkannt, `+20 %` ebenfalls nur in 5 von 10 — eine Münzwurf-Erkennungsrate bei einer klaren,
  eingespeisten Verdopplung fast bis zum Rauschband.
- Das Rauschband ist selbst nicht stabil: Der zweite Linux-Lauf (34997032190) zeigt ein *höheres*
  Rauschband als der erste (34995502352), obwohl Bench, Variante und Runner-Typ identisch sind — ein
  fester Schwellenwert für „wie unruhig ist die Wanduhr heute" wäre selbst ein bewegliches Ziel.
- Als scharfes Gate auf Linux **ungeeignet** — deckungsgleich mit der Literatur, die die Vorbereitung
  zitiert (Reichelt et al., Criterion-FAQ), nur mit einem deutlich größeren gemessenen Band als dort
  berichtet. Ein Grund dafür ist eine bewusste Sparmaßnahme dieses Spikes: nur 2 statt der im
  Vorschlag vorgesehenen 3 Sekunden Aufwärmphase — das lässt CPU-Taktrampen und Caching-Effekte in
  die gemessenen Stichproben durchschlagen. Ein größeres Aufwärmbudget würde das Band vermutlich
  senken, aber angesichts der Literaturwerte kaum unter die 10-%-Schwelle.

### Option 2: K4 — Callgrind-Instruktionen (`Ir`) als Gate-Metrik

**Gemessen** (ein `Ir`-Wert je Job, Linux, `ubuntu-24.04`):

| Bench | Lauf | Jobs | Basis-`Ir` (Median) | Robuster VK | Rauschband B |
|---|---|---|---|---|---|
| `ecs_query_10k` | 34995502352 | 10 | 39.298.827 | 0,0 % | 0,0 % |
| `ecs_query_10k` | 34997032190 | 10 | 39.298.827 | 0,0 % | 0,0 % |
| `sim_step_600` | 34995502352 | 10 | 63.691.093 | 0,0 % | 0,0 % |
| `sim_step_600` | 34997032190 | 10 | 63.691.093 | 0,0 % | 0,0 % |

Job-zu-Job **und** Lauf-zu-Lauf identisch, ganzzahlig, in allen vier Kombinationen — nicht nur nahe
null, sondern exakt null bis auf eine einzige abweichende Wiederholung je Bench (rep 10 in beiden
Läufen weicht in der zehnten Nachkommastelle des Anteils ab, siehe `cv-report.json`; die
Basis-`Ir`-Werte selbst sind unverändert).

**Erkennung der eingespeisten Regression** (Kandidat gegen Median der zehn `baseline`-Jobs,
exakter Ganzzahlvergleich `10·Ir_Kandidat > 11·Ir_Basis`):

| Bench | Variante (nominell) | Ist-Anteil | Rot (von 10) |
|---|---|---|---|
| `ecs_query_10k` | +5 % | +4,33 % | 0/10 (Warnung, nie Rot) |
| `ecs_query_10k` | +12 % | +10,40 % | 10/10 |
| `ecs_query_10k` | +20 % | +17,33 % | 10/10 |
| `sim_step_600` | +5 % | +3,41 % | 0/10 (Warnung) |
| `sim_step_600` | +12 % | +8,19 % | 0/10 (Warnung, **nicht** Rot) |
| `sim_step_600` | +20 % | +13,64 % | 10/10 |

**Positiv:**
- Rauschen ist nicht klein, sondern **exakt null** — job- und laufübergreifend identisch. Callgrind
  zählt deterministisch, unabhängig von CPU-Takt, Nachbarlast oder Zeitpunkt des Laufs.
- Trennt `+5 %` (nie Rot) sauber von `+12 %`/`+20 %` (immer Rot) bei `ecs_query_10k` — genau das
  Verhalten, das ein `>10 %`-Gate braucht.
- Der Selbsttest (plan §5) läuft als `cargo test` in beiden CI-Läufen grün: die `gate_decision`-Regel
  behandelt `+10,0 %` als Warnung (nicht Rot), `+10,01 %` als Rot, exakt an der Ganzzahlgrenze.

**Negativ:**
- **Die Einspeisung ist nicht kalibriert** — genau die Vorarbeit, die die Vorbereitung verlangt
  („der Anteil wird lokal so kalibriert, dass `Ir` um den Nennwert steigt, … vor dem ersten Lauf
  festgeschrieben", §3.1), hat dieser Spike ausgelassen (Zeitbudget). Folge: Der feste
  Prozess-Anlaufanteil (10.000-Entity-Aufbau plus Programmstart, im ganzen Prozess mitgezählt, da
  dieser Spike bewusst nicht auf Callgrind-Client-Requests umschaltet) verwässert die nominelle
  Rundenzahl-Erhöhung, und bei `sim_step_600` kostet eine zusätzliche *lesende* Tick-Runde spürbar
  weniger als eine reguläre Tick-Runde mit Schedule-Overhead. `+12 %` nominell landet bei nur
  `+10,4 %` (`ecs`) bzw. `+8,2 %` (`sim`, unter der Schwelle) Ist-Anteil. **Vor dem produktiven Gate
  in WP6.2 muss die Einspeisung wie im Vorschlag vorgesehen lokal kalibriert werden** — dieser
  Befund ist keine Schwäche der Metrik, sondern ein offener Punkt der Spike-Durchführung.
- Nur Linux (Valgrind unterstützt weder Windows noch `macos-latest`/arm64 planmäßig).
- Nur einfädige Benches (Valgrind serialisiert Threads); N-Thread-Benches bleiben auf der Wanduhr.
- `Ir` ist keine Zeit — Cache-, Sprung- und SIMD-Effekte sieht das Standard-Callgrind nicht (ohne
  `--cache-sim`), Parallelisierungsgewinne gar nicht.
- Dieser Spike liest die rohe `summary:`-Zeile statt `gungraun`/`iai-callgrind`: für WP6.2 fehlen
  damit dessen Regressions-Flags, Baseline-Verwaltung und Editor-Integration — die dort nachgeholt
  werden müssten, falls `gungraun` gewählt wird.

### Option 3: K3 — A/B im selben Job (nicht gemessen)

Von der Vorbereitung als „vermutlich beste Wanduhr-Option" genannt (§2), aber in diesem Spike nicht
umgesetzt (Zeitbudget; siehe `spikes/bench-noise/README.md`). Bleibt ein offener Punkt für WP6.2,
falls die Wanduhr für N-Thread-Benches oder Budget-Trends schärfer als reiner K1 werden soll, als
dort schon vorgesehen.

## Entscheidung

**Gewählte Option:** 2 — Callgrind-Instruktionen (`Ir`) werden die scharfe Gate-Metrik für
einfädige P0/P1-Benches auf Linux, mit exaktem Ganzzahlvergleich `10·Ir_Kandidat > 11·Ir_Basis` für
Rot und dem gleitenden Warnschwellenwert `w = 3 %` bei `B ≤ 1 %`, sonst `min(3·B, 9 %)` (plan §4.3),
gegen eine **akzeptierte Basis** (nie den letzten Push, plan §4.2). Die Wanduhr (K1) bleibt **Trend**
auf allen drei Betriebssystemen — sie geht nicht in ein hartes Gate ein, weil das gemessene
Rauschband (95–129 % auf Linux, mit zwei Läufen bestätigt) das um mehr als das Zehnfache übersteigt,
was ein `>10 %`-Gate erträgt. Absolute Millisekunden-Budgets (WP6.5/WP6.7) bleiben aus demselben
Grund Trend mit Budgetlinie, umgerechnet über den Faktor aus Messsitzung 1, nie ein hartes
CI-Gate auf geteilten Runnern — deckungsgleich mit dem in der Vorbereitung skizzierten Vorschlag
(§4.4), hier mit echten statt angenommenen Zahlen unterlegt.

**Vor der Umsetzung in WP6.2** sind zwei Dinge offen, die dieses ADR nicht selbst schließt (siehe
Folge-Entscheidungen): die Kalibrierung der Einspeisung (oben, Option 2 „Negativ“) und die
Entscheidung `gungraun` gegen die rohe Callgrind-Zeile für das produktive `grimoire_bench`.

**Nachtrag (2026-09-15, PO-Entscheidung Sammelsitzung B):** Angenommen; Millisekunden-Budgets sind auf
gemeinsam genutzten Runnern nie ein hartes CI-Gate. Zusätzlich entschieden:

- **P-12 (Trendablage):** Trenddaten liegen auf einem eigenen Datenzweig `bench-trends` in diesem Repo;
  CI hängt nach Pushes auf `main` JSON an und bekommt nur für diesen Zweig Schreibzugriff.
- **Self-hosted-Runner-Rückfall:** Sollten gehostete Runner je nicht ausreichen, ist der PC des PO der
  Rückfall-Runner — abweichend von der Empfehlung des Autors, hier ehrlich vermerkt. Er ist jetzt
  **nicht** eingerichtet. Bei einer Einrichtung gelten zwingend: nie für `pull_request`- oder
  Fork-Events, nur `workflow_dispatch` oder Push auf `main`, ein eigenes Label, ein ephemerer bzw.
  just-in-time-Runner, nie während der PO spielt, und die Einrichtung selbst nur mit ausdrücklichem
  PO-Ja.

## Konsequenzen

### Positiv

- Ein Gate, das auf Linux beweisbar nicht auf Umgebungsrauschen anspringt (0,0 % Rauschband, zwei
  unabhängige Zehner-Läufe) und eine `+12 %`/`+20 %`-Regression zuverlässig erkennt, sobald die
  Einspeisung kalibriert ist.
- Die Wanduhr wird nicht verworfen, sondern auf die Rolle beschränkt, in der ihr gemessenes Verhalten
  sie noch trägt: Trend, Budget-Näherung, N-Thread-Vergleich — nie ein Ja/Nein-Gate.
- Der exakte Ganzzahlvergleich und die `gate_decision`-Selbsttests sind bereits geschrieben und in
  zwei CI-Läufen grün; WP6.2 kann sie fast unverändert in `grimoire_bench` übernehmen.
- Windows/macOS-Zahlen (wenn auch nur 3 Wiederholungen) zeigen keinen Widerspruch zu „Wanduhr bleibt
  Trend" — im Gegenteil, macOS/Windows wirkten hier ruhiger als Linux, was aber mit 3 Jobs nicht
  belastbar ist.

### Negativ

- Kein Nachweis für N-Thread-Benches oder absolute Millisekundenbudgets aus diesem Spike; beide
  hängen weiterhin an Messsitzung 1 auf der Referenz-Hardware (unverändert gegenüber der
  Vorbereitung).
- Die Einspeisungs-Kalibrierung fehlt; ein Team, das diese Zahlen ungeprüft für das produktive Gate
  übernimmt, gated de facto auf einer anderen Prozentschwelle als angenommen (siehe Beispiel
  `sim_step_600` `+12 %`).
- K3 (A/B im selben Job) wurde nicht gemessen; ob es die Wanduhr für einen Zusatznutzen (etwa
  N-Thread-Trend) genug beruhigt, bleibt offen.
- Nur zwei Linux-Läufe und je drei Windows-/macOS-Jobs — der Pilot-Charakter aus plan §3 gilt
  unverändert: Das beweist keine Fehlalarmquote, das bestätigt erst der Warnmodus in WP6.2 (plan
  §4.5).
- `gungraun` wurde nicht erprobt; ein späterer Umstieg von der rohen Callgrind-Zeile könnte
  überraschend anders instrumentieren (z. B. Client-Request-Toggling, das die Anlaufkosten aus der
  Messung nimmt) und müsste erneut gegen diese Zahlen validiert werden.

### Folge-Entscheidungen

- WP6.2 kalibriert die Einspeisungs-Prozentsätze empirisch je Bench, bevor der Selbsttest
  (`GRIMOIRE_BENCH_INJECT_REGRESSION`) produktiv geschaltet wird.
- WP6.2 entscheidet `gungraun`/`iai-callgrind` gegen eine eigene, schlanke Callgrind-Ansteuerung wie
  in diesem Spike (Kriterium: braucht `grimoire_bench` dessen eingebaute Regressions-Flags und
  Baseline-Verwaltung, oder genügt der eigene Vergleicher aus plan §4.3?).
- K3 (A/B im selben Job) bleibt eine offene Option für WP6.2, falls die Wanduhr für Budget-Trend oder
  N-Thread-Vergleich schärfer werden soll, als K1 es hier zeigt.
- Die Trendablage (P-12: Datenzweig gegen Actions-Artefakte gegen GitHub Pages) ist eine
  PO-Entscheidung und bleibt offen (unverändert gegenüber der Vorbereitung §8).
- Windows/macOS bleiben nightly/Trend-only für die Wanduhr; ein härterer Anspruch dort bräuchte
  eigene, mit mehr als drei Wiederholungen abgesicherte Zahlen.

### Review

**Reality-Check geplant für:** sobald WP6.2 `grimoire_bench` mit kalibrierter Einspeisung und dem
Warnmodus auf `main` scharf schaltet (plan §4.5: mindestens 20 Gate-Läufe über mindestens 14 Tage,
kein Fehlalarm).

## Weitere Informationen

### Scope

Gilt für einfädige P0/P1-Benchmarks der Engine (`grimoire_ecs`, `grimoire_sim` und, ab WP6.5,
`grimoire_collide`). N-Thread-Benches (Engine-ADR-0006), GPU-/Render-Kosten und absolute
Millisekundenbudgets sind nicht erfasst — dafür bleibt die Wanduhr als Trend, mit Umrechnung über
Messsitzung 1, wie in der Vorbereitung vorgesehen.

### Abweichungen vom vorbereiteten Vorschlag

Vollständige Liste mit Begründung in `spikes/bench-noise/README.md`. Kurzfassung: ein Messfenster
statt zwei Blöcken an verschiedenen Tagen; vier statt sechs Kandidaten (K1, K4 gemessen; K3, K5 nicht);
vier statt sechs Einspeisungs-Varianten (kein `+15 %`, keine cache-feindliche Variante); kein
Zwei-Commit-Build-Pfad-Nachweis; 15 statt 30 Wanduhr-Stichproben, 2 statt 3 Sekunden Aufwärmphase;
rohe Callgrind-`summary:`-Zeile statt `gungraun`.

### Referenzen

- Vorbereitung: Spiel-Repo `docs/plans/0002-vorbereitung-of-17.3.md` (Kandidaten, Versuchsprotokoll,
  Entscheidungsverfahren, Quellen Q1–Q42)
- Plan 0002 WP6.1/WP6.2/WP6.5–6.7
- Spike-Crate `spikes/bench-noise/` (Cargo.toml, `src/lib.rs`, `src/bin/*.rs`,
  `scripts/measure_ir.sh`, `README.md`) auf Branch `p1/wp6.1-bench-spike`
- CI-Läufe: 34995502352 (Linux, 10 Wiederholungen, Commit `11014dc`), 34997032190 (Linux erneut +
  Windows/macOS, Commit `51638cc`); Rohdaten und `cv-report.json` als Lauf-Artefakte 30 Tage
  aufbewahrt
- Valgrind/Callgrind: https://valgrind.org/docs/manual/cl-manual.html
- `gungraun` (vormals `iai-callgrind`): https://github.com/gungraun/gungraun

## Nachtrag (WP6.2-Vorbereitung, 2026-09-15)

Ergänzung zu den beiden unter „Vor der Umsetzung in WP6.2" offen gelassenen Punkten (Kalibrierung
der Einspeisung; `gungraun` gegen die rohe Callgrind-Zeile). Beide sind jetzt gemessen; der Text
oben (Optionen, Entscheidung, Konsequenzen) bleibt unverändert.

### Kalibrierte Einspeisung

**Methode:** `bench_noise::run_calibration_units` (Spike-Crate, Branch `p1/wp6.1-bench-spike`)
ersetzt "N weitere ganze Runden/Ticks der echten Bench-Arbeit" durch eine homogene, feingranulare
Einheit ohne den fixen Pro-Runde/Pro-Tick-Overhead, der die alte Einspeisung verwässert hat.
`scripts/calibrate_injection.sh` misst auf dem Runner, im selben Job wie die Zielmessung: einen
Basislauf (0 Einheiten) und einen Referenzlauf (2.000.000 Einheiten), daraus die Steigung (`Ir` je
Einheit), löst daraus die Einheitenzahl für die Zielprozente und misst das Ergebnis nach —
durchgehend exakte 64-Bit-Ganzzahlarithmetik wie das Gate selbst (Plan §4.3). `src/bin/
calibration_check.rs` prüft automatisiert, ob jede Zielprozentzahl auf ±1 Prozentpunkt trifft und
ob das exakte Ganzzahl-Gate (`gate_decision`) wie erwartet urteilt; der neue CI-Job `calibration`
bricht bei Abweichung ab.

**Gemessen** (CI-Lauf 35022362362, `ubuntu-24.04`, Commit `15ccb08`; bit-genau identisch
reproduziert in Lauf 35020976939, Commit `92e2fa2`):

| Bench | Ziel % | Basis-`Ir` | Einheiten | End-`Ir` | Gemessen % | \|Δ\| pp | Gate | Erwartet |
|---|---|---|---|---|---|---|---|---|
| `ecs_query_10k` | +5 % | 74.179.857 | 285.306 | 77.888.899 | 5,00 % | 0,00 | warn | nicht rot |
| `ecs_query_10k` | +12 % | 74.179.857 | 684.735 | 83.081.476 | 12,00 % | 0,00 | rot | rot |
| `ecs_query_10k` | +20 % | 74.179.857 | 1.141.224 | 89.015.863 | 20,00 % | 0,00 | rot | rot |
| `sim_step_600` | +5 % | 97.293.940 | 374.206 | 102.158.682 | 5,00 % | 0,00 | warn | nicht rot |
| `sim_step_600` | +12 % | 97.293.940 | 898.095 | 108.969.239 | 12,00 % | 0,00 | rot | rot |
| `sim_step_600` | +20 % | 97.293.940 | 1.496.824 | 116.752.746 | 20,00 % | 0,00 | rot | rot |

Alle sechs Punkte treffen ihr Ziel auf 0,00 Prozentpunkte (weit innerhalb der ±1-pp-Vorgabe) und
urteilen exakt wie gefordert — insbesondere `sim_step_600` `+12 %`, das mit der alten,
unkalibrierten Einspeisung nur `+8,19 %` maß und **nicht** rot wurde (offener Punkt oben, Option 2
„Negativ"): mit der kalibrierten Einspeisung wird es korrekt rot.

Ein Zusatzbefund aus dem Vergleichsjob (unten) relativiert, wie stabil der alte, unkalibrierte
Nennwert-Anteil selbst ist: **derselbe** unkalibrierte `+12 %`-Einspeisungscode maß im Job
`gungraun-vs-raw` auf `ubuntu-latest` für `sim_step_600` nur `+5,36 %` — weder die ursprünglich
beobachteten `+8,19 %` noch `+12 %`. Der Pool mischt CPU-Modelle (Vorbereitung §2), und Callgrinds
feste Guest-CPUID schaltet je nach Host unterschiedliche `hwcaps` frei (Vorbereitung §2 K4) — ein
plausibler, hier nicht weiter verifizierter Erklärungsansatz. Das bestätigt den gewählten Ansatz:
Kalibrierung muss **im selben Lauf** wie die Zielmessung geschehen, nicht einmalig ermittelt und
fest verdrahtet werden.

### gungraun gegen die rohe Callgrind-Zeile

**Aufbau:** `benches/gungraun_bench.rs` (neue `[dev-dependencies]`-Abhängigkeit
`gungraun = "0.19.4"`, ausschließlich im Spike-Crate, nie im Engine-Workspace oder
`grimoire_bench`) misst dieselben zwei Benches (`baseline`, nominelle `+12 %`) über
`#[library_benchmark]`/`library_benchmark_group!`/`main!`. Der CI-Job `gungraun-vs-raw`
(`ubuntu-latest`, gepinnte Toolchain 1.98.1) führt beide Pfade zweimal im selben Job aus, damit der
Vergleich nicht durch Umgebungsdrift verfälscht wird.

**Setup-Aufwand:**

- Roh: `scripts/measure_ir.sh` (~45 Zeilen Bash) + `src/bin/ir_probe.rs`; keine zusätzliche
  Cargo-Abhängigkeit, keine zusätzliche Action.
- `gungraun`: zieht 19 zusätzliche Crates (u. a. `syn`, `gungraun-macros`, `gungraun-runner`,
  `indexmap`, `hashbrown`, `bincode-next`); zusätzlich die Action `gungraun/setup-gungraun` (per
  SHA gepinnt). **Reale Stolperfalle angetroffen:** `runner-version: auto` schlug im ersten
  Versuch fehl (Lauf 35020976939, Job „gungraun vs. raw Callgrind": „Unable to detect
  gungraun-runner version"). Die Action liest `Cargo.lock`, um die passende
  `gungraun-runner`-Version zu erkennen, aber `defaults.run.working-directory:
  spikes/bench-noise` gilt nur für `run:`-Schritte, nicht für `uses:`-Actions — die Erkennung lief
  am Repo-Root (Engine-Workspace ohne `gungraun`-Abhängigkeit) ins Leere. Behoben durch
  `runner-version: "0.19.4"` explizit passend zum Lockfile.

**CI-Zeit** (derselbe Job, `ubuntu-latest`, zwei Wiederholungen je Pfad):

| Pfad | Lauf 1 | Lauf 2 |
|---|---|---|
| roh (`measure_ir.sh`, 2 Benches × 4 Varianten = 8 Callgrind-Läufe) | 5 s | 5 s |
| `gungraun` (2 Benches × 2 Varianten = 4 Callgrind-Läufe) | 16 s | 5 s |

Job `gungraun-vs-raw` insgesamt (Checkout, Toolchain, Valgrind, zwei Builds, beide Pfade zweimal):
79 s. Zum Vergleich: Job `calibration` (Checkout, Toolchain, Valgrind, ein Build, sechs
Kalibrierungsziele über zehn Callgrind-Läufe): 42 s. `gungraun`s erster Lauf brauchte trotz halb so
vieler Varianten gut dreimal so lang wie der volle rohe Lauf — einmaliger Kompilierkosten für die
Proc-Macro-Kette (`syn`, `gungraun-macros`) bei kaltem Cache; im zweiten (warmen) Lauf gleichauf
mit dem rohen Pfad.

**Stabilität:** Innerhalb desselben Jobs zeigte der rohe Pfad auf `ubuntu-latest` eine winzige,
aber reale Lauf-zu-Lauf-Abweichung (`ecs_query_10k`-Basis 74.179.329 → 74.179.967,
`sim_step_600`-Basis 97.293.411 → 97.294.049; je ≈ 0,0009 %) — kleiner als je zuvor gemessen, aber
ungleich null, anders als das exakte 0,0-%-Band der ursprünglichen 10er-Matrix auf
`ubuntu-24.04` (dort wich ebenfalls nur eine von zehn Wiederholungen in der zehnten
Nachkommastelle ab, siehe oben). `gungraun`s eigener Vergleich beider Läufe meldete für alle vier
Benchmarks wörtlich „No change" (bitgenau gleiche `Ir`). Beide Abweichungen liegen weit unterhalb
der 10-%-Schwelle und sind nicht gate-relevant; `gungraun`s eingebauter Lauf-gegen-Lauf-Vergleich
zeigt das aber ohne Zusatzwerkzeug an.

**Toolchain/Image:** Beide Pfade bauten und liefen anstandslos mit der gepinnten Toolchain 1.98.1
und Valgrind 3.22.0 (`apt`/`system`-Strategie für beide, für einen fairen Vergleich) auf
`ubuntu-latest` — keine Inkompatibilität, abgesehen von der oben genannten
`runner-version`-Stolperfalle.

**Setup-Ausschluss-Nuance:** `gungraun`s `#[bench::id(ausdruck)]`-Argumentausdrücke laufen vor
Callgrinds Standard-Einstiegspunkt und schließen daher Aufbaukosten aus der Messung aus — anders
als `ir_probe`, das den ganzen Prozess zählt. Im selben Job gemessen macht das bei `ecs_query_10k`
≈ 6,9 % des rohen Basiswerts aus (74.179.329 roh gegen 69.055.375 `gungraun`), bei `sim_step_600`
nur ≈ 1,5 % (97.293.411 gegen 95.823.888) — der 600-Tick-Rumpf dominiert dort ohnehin gegenüber
dem Aufbau von 2.000 Entities. Das verkleinert die Verwässerung durch Fixkosten etwas, ersetzt die
Kalibrierung oben aber nicht: `gungraun`s eigene *unkalibrierte* `+12 %`-Einspeisung maß `+12,00 %`
bei `ecs_query_10k` (durch den Ausschluss zufällig sehr nah dran) und nur `+5,44 %` bei
`sim_step_600` (nahe am rohen Pfad in demselben Job, `+5,36 %`, siehe oben) — WP6.2 braucht die
kalibrierte Einspeisung unabhängig von der Werkzeugwahl.

### Entscheidung: roh bleibt, vorerst

**Empfehlung:** Für das produktive `grimoire_bench` in WP6.2 **bei der rohen
Callgrind-`summary:`-Zeile dieses Spikes bleiben**, `gungraun` (noch) nicht einführen — abweichend
von der eingangs skizzierten Tendenz dieses ADR („schlägt dieses ADR gungraun … vor, wo seine
Regressionswerkzeuge den Mehraufwand wert sind"), jetzt mit echten statt angenommenen Zahlen
unterlegt:

- Die eigentlich harte Arbeit — akzeptierte Basis statt letztem Push (§4.2), exakter
  Ganzzahlvergleich, Warn-/Rot-Schwellen, Exitcode-Tabelle (§4.3) — ist bereits geschrieben,
  unit-getestet und in drei CI-Läufen grün. `gungraun`s eingebaute Regressionsprüfung vergleicht
  gegen den *eigenen letzten lokalen Lauf*, nicht gegen eine verwaltete akzeptierte Basis auf
  einem Datenzweig (P-12) — sie ersetzt diese Vergleicherlogik nicht, sie käme zusätzlich dazu.
- `gungraun` kostet: 19 zusätzliche Crates (Audit-Fläche für ein determinismus-empfindliches
  Engine-Repo, `crate-vertraege.md` §2 Regel 4), eine zusätzliche gepinnte Action mit einer
  bereits angetroffenen Konfigurationsfalle, und einen realen Kaltstart-Zeitaufschlag beim ersten
  Lauf nach einer Cache-Invalidierung.
- Der Setup-Ausschluss-Vorteil ist real, aber ungleichmäßig (6,9 % gegenüber 1,5 % zwischen den
  zwei Benches) und löst die Kalibrierungsfrage nicht.
- Was `gungraun` bringt und die rohe Zeile nicht — Lauf-gegen-Lauf-Diffing ohne Zusatzwerkzeug,
  DHAT/Massif/Cachegrind, lokale Entwicklerergonomie (`cargo bench` statt manuell Valgrind
  aufrufen) — wird für die P0-Benches in WP6.2 (nur `Ir`, nur Gate) nicht gebraucht. Es wird
  relevant, sobald WP6.5+ Kollisions- oder N-Thread-Arbeit Cache-Simulation, DHAT oder
  Multi-Tool-Auswertung braucht; dann lohnt sich ein erneuter Blick mit denselben Kriterien.

**Offen für WP6.2 selbst:** Die Kalibrierung hier lief gegen den Spike-eigenen
`run_calibration_units`; WP6.2 muss dieselbe Methode (oder eine äquivalente) in `grimoire_bench`
selbst verdrahten, nicht die konkreten Einheitenzahlen aus dieser Tabelle übernehmen — die
Steigung ist bench- und hostabhängig (siehe oben, `sim_step_600` driftete zwischen zwei Läufen).

### Referenzen (Nachtrag)

- CI-Läufe: 35020976939 (Kalibrierung grün, `gungraun-vs-raw` rot durch Setup-Fehler, Commit
  `92e2fa2`), 35022362362 (alle Jobs grün nach Fix, Commit `15ccb08`) auf Branch
  `p1/wp6.1-bench-spike`; Rohdaten (`calibration-*.jsonl`, `ir-raw-ubuntu-latest-run*.jsonl`,
  `gungraun-run*.txt`, `target/gungraun/**/callgrind.*.out`) als Lauf-Artefakte
  `wp62-calibration` und `wp62-gungraun-vs-raw`, 30 Tage aufbewahrt
- Spike-Ergänzung: `spikes/bench-noise/src/lib.rs` (`run_calibration_units`),
  `src/bin/ir_probe.rs` (`calibrated`-Modus), `src/bin/calibration_check.rs`,
  `scripts/calibrate_injection.sh`, `benches/gungraun_bench.rs`, CI-Jobs `calibration` und
  `gungraun-vs-raw`
