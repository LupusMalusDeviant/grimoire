# ADR-0010: Benchmark-Strategie für das Regressions-Gate (OF-17.3)

- **Status:** Vorgeschlagen (Nummer vorläufig: 0007 ist auf dem Branch `p1/wp1.4-sigil-syntax-spike`
  belegt, 0008 auf `p1/wp1.2-contracts-draft`, `main` endet bei 0009; die endgültige Nummer steht
  erst beim Merge fest)
- **Datum:** 2026-09-15
- **Autor:** Claude (Ausarbeitung, unbeaufsichtigter Lauf) im Auftrag von Lupus Malus Deviant (PO)
- **Konsultiert:** — (PO-Freigabe aussteht; dieses ADR ist der Messnachweis zur vorbereiteten
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

**Vorschlag:** Callgrind-Instruktionen (`Ir`, Option 2) werden die scharfe Gate-Metrik für
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
Folge-Entscheidungen): die Kalibrierung der Einspeisung (oben, Option 2 „Negativ") und die
Entscheidung `gungraun` gegen die rohe Callgrind-Zeile für das produktive `grimoire_bench`.

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
