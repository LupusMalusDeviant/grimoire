# Spike OF-4.1: Sigil-Quelltextsyntax — RON gegen eigene Grammatik „sigil 1“

- **Bezug:** Spiel-Repo Plan 0002 WP1.4, PRD-0004 OF-4.1 und OF-4.2, Projekt-ADR-0006 (Sigil-Daten-DSL) und
  ADR-0007 (Offline-Kompilierung); Engine-ADR-0004 (deterministische Gleitkommaarithmetik)
- **Status:** Spike-Bericht, **keine Entscheidung**. Der Vorschlag steht in
  [ADR-0007 „Sigil-Quelltextsyntax v1“](../adr/0007-sigil-quelltextsyntax-v1.md) (Nummer vorläufig bis zum
  Merge). Die Entscheidung trifft der PO in Sammelsitzung B (Plan 0002, P-10).
- **Stand:** 2026-09-15 (unbeaufsichtigter Lauf; Generierbarkeit mit Codex und Zweitbewertung am selben Tag nachgeholt)
- **Autor:** Claude (unbeaufsichtigter Lauf) im Auftrag von Lupus Malus Deviant (PO)
- **Vorläufig:** Alle Bewertungen und die Empfehlung sind vorläufig. Die Bestätigung durch den PO steht aus.
- **Messdaten:** [`spikes/sigil-syntax/results.md`](../../spikes/sigil-syntax/results.md), Stand nach der
  Nachmessung mit Codex vom 2026-09-15 (davor `8558c7d` nach dem Review, erste Fassung `c3239ad`). Reproduzierbar mit
  `cargo test` in `spikes/sigil-syntax/proto`.

## Kurzfassung

- Beide Prototyp-Parser lesen alle fünf Korpus-Muster ohne Befund und erzeugen dasselbe aufgelöste Modell.
- **Diagnosen:** Die eigene Grammatik „sigil 1“ trifft in allen zehn Fehlerfällen des Korpus Position und
  Knotenpfad. Sie erreicht 119 von 120 Punkten, RON mit `ron` 0.12.2 und eigener Zusatzschicht 102 von 120.
  Die Lücke entsteht ganz bei den Syntaxfehlern e02, e04 und e09, und dort an `ron` selbst. Fehlendes
  Pflichtfeld (e03) und falsche Einheit (e07) lagen in der ersten Fassung ebenfalls zurück (96 von 120).
  Das lag an der dünnen RON-Zusatzschicht, nicht an `ron`, und ist mit wenigen Zeilen behoben.
- **Zweitbewertung:** Codex hat Ursache und Fix-Hinweis blind nachbewertet. 37 von 40 Werten sind gleich,
  die drei Abweichungen betragen je einen Punkt. Mit Codex' Werten steht es 120 zu 104.
- **Außerhalb des Korpus:** Syntaxspezifische Sonden zeigen bei sigil 1 bis zu drei Diagnosen je Fehler,
  darunter falsche Folgebefunde. „Genau eine Diagnose“ gilt nur für die zehn Korpusfälle.
- **Rundreise:** Einen Wert an einem Knotenpfad setzen, ohne Kommentare zu verlieren, gelingt in
  **beiden** Syntaxen, aber nur mit einem selbst geschriebenen verlustfreien Baum. Der Weg über das
  `ron`-Crate verliert alle Kommentare und schreibt rund 40 Zeilen um.
- **Ergonomie:** sigil 1 braucht 87 % der Zeilen, 56 % der signifikanten Tokens und 69 % der Zeichen von RON.
  Die Token-Differenz stammt zu 40 % aus Trennkommas und zu 31 % aus Einheiten-Hüllen (Richtwert).
- **Aufwand:** Die sigil-Seite umfasst im Spike 2 241 Codezeilen. Die RON-Seite braucht für dieselben
  Pfade, Positionen und `set`-Operationen ebenfalls 713 Zeilen eigenen Code.
- **Generierbarkeit:** Codex hat die drei Aufgaben im ersten Versuch in beiden Syntaxen ohne Diagnose
  geschrieben, modellgleich zueinander und zu Claudes Fassung. Ein Nachteil von sigil 1 zeigt sich nicht.
  Die Messung liegt aber an der Decke (12 von 12 Dateien fehlerfrei) und trennt die Syntaxen kaum.
- **Empfehlung (vorläufig, unverändert):** eigene Grammatik „sigil 1“. Von den drei Bedingungen vor der
  Abnahme sind Generierbarkeit und Zweitbewertung erfüllt, und keine der beiden Messungen widerspricht der
  Empfehlung. Offen bleiben die syntaxspezifischen Fehlerfälle (Abschnitt 7).

## 1 Frage

PRD-0004 OF-4.1 fragt, ob die Quelltextsyntax von Sigil auf RON aufsetzt oder eine eigene Grammatik
bekommt. Die Kriterien sind die besten Fehlermeldungen und die Editier-Ergonomie im Tooling. Plan 0002
WP1.4 macht daraus vier prüfbare Kriterien:

1. **Editier-Ergonomie** für Menschen im Texteditor und in der Tooling-Suite (Projekt-ADR-0008).
2. **Text↔Parameter-Rundreise,** darunter das gezielte Setzen eines Werts an einem Knotenpfad ohne
   Kommentarverlust (Vorbild `sigilc set`).
3. **Generierbarkeit durch Agenten** (PRD-0004 US-02, Projekt-ADR-0006).
4. **Fehlermeldungen** mit Datei, Zeile, Spalte, Ursache und Fix-Hinweis.

Das Ergebnis soll ein Engine-ADR „Sigil-Quelltextsyntax v1“ sein. Es enthält den Versions-Header
`sigil 1` und eine Migrationsregel und schließt OF-4.2 per Verweis auf Engine-ADR-0004.

## 2 Methode

- **Ein Modell, zwei Oberflächen.** Das eigenständige Crate `spikes/sigil-syntax/proto` gehört nicht zum
  Engine-Workspace. Seine Abhängigkeiten sind exakt gepinnt: `ron` 0.12.2, `serde` 1.0.228 und
  `serde_json` 1.0.149. Beide Front-Ends deserialisieren in dieselben Rust-Typen mit `deny_unknown_fields`.
  RON läuft direkt über `ron` und serde. sigil 1 hat einen handgeschriebenen Lexer und Parser ohne
  Kombinator-Bibliothek. Der Parser baut einen verlustfreien Syntaxbaum, setzt nach Fehlern wieder auf und
  senkt den Baum in einen Wertebaum mit Spans ab. Darüber läuft ein eigener serde-Deserializer.
- **Gemeinsamer Validator.** Wertebereiche, Referenzen, Eindeutigkeit, Lesbarkeitsregeln, Kaskadentiefe
  und Komposition prüft für beide Syntaxen derselbe Code. Befunde dieser Stufe (e05, e06, e08, e10) sind
  deshalb in beiden Varianten gleich gut. Das folgt aus der Architektur und ist kein Verdienst der Syntax.
- **Fairness gegenüber RON.** Eine dünne eigene Schicht (`ron_front.rs`) leitet Fix-Hinweise aus den
  strukturierten Fehlercodes von `ron` ab. Ein eigener verlustfreier Scanner (`ron_cst.rs`) liefert
  Knotenpfade, Positionen für Validierungsbefunde und `set`. Ohne diese Schicht liefert `ron` nur
  Rohmeldung und Position.
- **Bewertung 0–3 je Fehlerfall.** Position und Knotenpfad werden automatisch gegen
  `corpus/errors/expected.json` bewertet. Ursache und Fix-Hinweis hat Claude von Hand bewertet; die Werte
  stehen als Konstante `RATINGS` in `src/measure.rs` und sind vorläufig. Die Skala ist in `results.md`
  unter „Methodik“ beschrieben. Codex hat beide Spalten blind zweitbewertet (`RATINGS_CODEX`,
  `results.md` 2.8).
- **Reproduktion.** `cargo test` wiederholt alle Messungen und prüft die generierten Tabellen in
  `results.md`. `cargo run -- report --write` erzeugt die Tabellen neu. Die CLI heißt `sigil-syntax-proto`
  und kennt `report`, `check` und `set`.

## 3 Korpus

Der Korpus liegt unter `spikes/sigil-syntax/corpus/` (Commit `1afbfaa`, siehe dessen README). Claude hat
ihn geschrieben, weil beide Codex-Läufe für den Entwurf nach dem Timeout ohne Ausgabe abbrachen.

- **Grammatik und Modell:** `grammar/sigil-1.ebnf` (ISO-EBNF mit Knotenpfad-Schema) und
  `grammar/schema-model.txt` (gemeinsames Datenmodell). Die Grammatik ist generisch: Sie kennt keine
  Feldnamen, Namen, Typen, Einheiten und Bereiche prüft erst das Schema.
- **Fünf Muster in beiden Syntaxen:**

  | Nr. | Muster | Inhalt |
  |---|---|---|
  | 01 | `ring-burst` | wiederholter Ring-Burst mit Beschleunigung |
  | 02 | `aimed-stream` | gezielter Drei-Wege-Strom mit Streuung und Sinus-Wobble |
  | 03 | `subemitter-cascade` | **komplex:** Sub-Emitter-Kaskade der Tiefe 3 mit Typwechsel nach Distanz |
  | 04 | `mirrored-spiral` | **komplex:** gespiegelte Spirale mit Rotation und Tempokurve aus 4 Stützpunkten |
  | 05 | `wave-line-composite` | Welle, Linie, Kurvenbahn, Umkehr per Ereignis, Behaviour, Import mit Overrides |

  Zusammen decken die Muster alle Bausteine, Modifikatoren, Transformationen, Trigger und Flags aus
  PRD-0004 FR-01 bis FR-05 ab. Dazu kommen Behaviour-Referenz, Metadaten, Kaskadentiefe am Limit und
  Komposition. Die Abdeckungsmatrix steht im Korpus-README. Die Ablehnungen aus FR-08 (Tiefe über 3,
  Zyklus) und `beats` enthält der bewertete Korpus nicht; sie stehen nur in den Sonden (Abschnitt 4.1).
- **Zehn Fehlerfälle in beiden Syntaxen:** Tippfehler im Schlüssel (e01), falscher Typ (e02), fehlendes
  Pflichtfeld (e03), nicht geschlossene Klammer (e04), Wert außerhalb des Bereichs (e05), unbekanntes
  Behaviour (e06), falsche Einheit (e07), doppelter Emitter-Name (e08), doppeltes Komma (e09) und falsche
  Version (e10). Jeder Fall verändert genau eine Stelle einer Musterkopie.
- **Faire RON-Form:** idiomatisches serde-RON mit `#![enable(implicit_some)]`, PascalCase-Varianten und
  Einheiten als Newtypes (`Ticks(20)`, `Deg(11.0)`). Die Einheit steckt also nicht im Feldnamen.

## 4 Messungen

### 4.1 Fehlermeldungen

Für jeden Fall zählt die erste Diagnose, verglichen mit `expected.json`. Die Tabellen je Fall stehen in
`results.md`, Abschnitte 2.1 bis 2.6.

**Punkte je Spalte (0–3 je Fall, höchstens 30):**

| Spalte | RON | sigil 1 | Bewertung |
|---|---:|---:|---|
| Position (Zeile:Spalte) | 29 | 30 | automatisch |
| Ursache | 24 | 30 | von Hand, vorläufig |
| Fix-Hinweis | 20 | 29 | von Hand, vorläufig |
| Knotenpfad | 29 | 30 | automatisch |
| **Summe (höchstens 120)** | **102** | **119** | |

In der ersten Fassung stand RON bei 96 (Position 27, Ursache 22, Hinweis 19, Pfad 28). Den Unterschied
machen e03 und e07; siehe „Ursachen trennen“ unten. Mit den Werten der Zweitbewertung durch Codex hat RON
beim Fix-Hinweis 22 statt 20 und sigil 1 30 statt 29 Punkte, zusammen 104 zu 120.

- **sigil 1:** Alle zehn Korpusfälle treffen Soll-Position und Soll-Knotenpfad, und jede dieser Dateien
  liefert genau eine Diagnose. Außerhalb des Korpus gilt das nicht (Sonden unten). Der eine fehlende Punkt
  ist der Fix-Hinweis bei e03, der keinen Beispielwert nennt.
- **RON: Rohmeldungen von `ron` 0.12.2 in den Fällen mit Abstand:**

  | Fall | Rohmeldung | Position Ist (Soll) | Befund |
  |---|---|---|---|
  | e02 | `Expected comma` | 43:25 (43:24) | `ron` liest `3` als Ganzzahl und stolpert über `.5`. Ein Typfehler erscheint als Syntaxfehler, ohne Hinweis. |
  | e03 | `Unexpected missing field named speed in Emitter` | 55:9 (27:19) | Die Position ist die schließende Klammer, 28 Zeilen unter dem Emitter; `bloom` wird nicht genannt. Die Zusatzschicht verlegt die Meldung über den Scanner auf `bloom` (27:19) und nennt den Emitter. |
  | e04 | `Unexpected field named Bullet in Bullet, expected one of name, silhouette …` | 33:9 (33:9) | Die öffnende Klammer bleibt unerwähnt. Der Hinweis zählt Felder auf und führt in die falsche Richtung. |
  | e07 | `Expected struct Deg but found Ticks` | 44:25 (44:25) | Die Position ist exakt, das Feld wird nicht genannt. Die Zusatzschicht ergänzt Feld und Wert (`spread: Deg(12.0)`). |
  | e09 | `Expected opening ( for struct Key` | 49:51 (49:51) | Die Position ist exakt, die Ursache irreführend. Der Pfad zeigt auf das nicht vorhandene `keys[3]`. |

- **Ursachen trennen:** Zu `ron` selbst gehören e02, e04 und e09. Dort sind Fehlerklasse oder Ursache
  falsch, und der Fehlercode enthält nicht, was eine bessere Meldung bräuchte. Diese drei Fälle machen die
  ganzen 17 Punkte Abstand aus. e03 und e07 dagegen waren Lücken der dünnen RON-Zusatzschicht: Der Scanner
  kannte den Pfad zum Besitzer schon, `ron_front.rs` nutzte ihn aber nicht. Die erste Fassung hatte der
  RON-Seite damit deutlich weniger Diagnoseaufwand gegeben als der sigil-Seite. Nach dem Review verlegt die
  Schicht das fehlende Feld auf den Emitter und nennt bei der falschen Einheit Feld und Wert, zusammen gut
  80 Zeilen in `ron_front.rs` (einschließlich des Rohaufrufs für `results.md` 2.2). Das bringt 6 Punkte.

- **Zweitbewertung (`results.md` 2.8):** Codex bekam Skala, eingebaute Änderung und vollständige Diagnose
  jeder Fehlerdatei, aber keine Werte von Claude, und lief read-only in einem leeren Verzeichnis. 37 von 40
  Werten stimmen überein, alle 40 liegen höchstens einen Punkt auseinander (Cohens Kappa ungewichtet 0,76,
  quadratisch gewichtet 0,95). Keine Abweichung beträgt 2 oder mehr. Codex gibt dem Fix-Hinweis bei e03 in
  beiden Syntaxen 3 statt 2 und dem RON-Hinweis bei e04 1 statt 0. Die Ursache 1 für die RON-Syntaxfehler
  e02, e04 und e09 bestätigt Codex; der Abstand liegt weiter ganz bei diesen drei Fällen (16 statt 17
  Punkte). Beide Bewerter sind Sprachmodelle, eine Bewertung durch den PO steht aus.
- **e05, e06, e08, e10:** Diese Befunde kommen aus dem gemeinsamen Validator und erreichen in beiden
  Syntaxen 3/3. Die RON-Positionen dafür liefert der eigene Scanner, nicht `ron`.
- **Phasen:** `ron` trennt Syntax und Schema nicht sauber. e04 und e09 (Soll: Syntax) landen im
  Schema-Pass, e02 (Soll: Schema) gilt als Syntaxfehler.
- **Einheiten:** Die Befürchtung aus dem Korpus-README ist widerlegt. `ron` 0.12.2 prüft Newtype-Namen,
  e07 bleibt also nicht stumm. **Aber** über `ron::Value` gelesen wird `Ticks(240)` zu `Seq([240])` und
  danach stillschweigend als `Deg(240.0)` akzeptiert. Nur `RawValue` behält die Namensprüfung. Overrides
  hält der Prototyp deshalb als `Box<RawValue>`.
- **Mehrere Fehler in einer Datei (Zusatzmessung):** RON bricht immer beim ersten Fehler ab. sigil 1 setzt
  nach einem Syntaxfehler wieder auf und meldet danach noch den Schemafehler (e03 + e09: 2 Diagnosen). Im
  Schema-Pass brechen beide beim ersten Befund ab, weil serde derive das tut (e02 + e07: 1 Diagnose).
- **Sonden außerhalb des Korpus (`results.md` 2.7):** Der Fehlerkorpus enthält nur Mutationen, die in beiden
  Syntaxen gleich aussehen. Zwölf Sonden ergänzen syntaxspezifische Fehlerbilder und die Ablehnungen aus
  FR-08. Sie sind nicht bewertet.
  - sigil 1: `count: 24` ergibt drei Diagnosen, `count = 24 start = 7.5deg` auf einer Zeile zwei. Beide
    enthalten einen falschen Befund „fehlendes Pflichtfeld“, obwohl das Feld dasteht. `radius = 0.25 u` und
    `delay = 30s` ergeben je zwei Diagnosen, `30s` ohne Hinweis. Eine fehlende Einheit (`speed = 0.05`) und
    `30beats` ergeben je eine gute Einzeldiagnose.
  - RON: Ein fehlender Newtype (`speed: 0.05`) heißt „Expected opening `(` for struct `UnitsPerTick`“, ein
    fehlender Strukturname „Expected identifier“, beide ohne Hinweis.
  - Beide: Kaskadenzyklus und Kaskadentiefe 4 meldet der gemeinsame Validator mit genau einer Diagnose.

So sieht ein Modder den Unterschied (Auszug aus `results.md` 2.6, Fall e02):

```text
error[parse/syntax]: Expected comma.
  --> corpus/errors/e02-wrong-type.ron:43:25
43 |                 count: 3.5,
   |                         ^
  = path: emitters.stream.block.count

error[schema/type]: Field `count` expects an integer, found the float `3.5`.
  --> corpus/errors/e02-wrong-type.sigil:39:13
39 |     count = 3.5
   |             ^
  = path: emitters.stream.block.count
  = help: Use a whole number, e.g. `count = 3`.
```

### 4.2 Text↔Parameter-Rundreise

Ein Wert wird über seinen Knotenpfad gesetzt und die Datei zurückgeschrieben. Drei Szenarien:

- `emitters.bloom.speed` in Muster 04,
- `emitters.bloom.modifiers[2].keys[1].mul` in Muster 04, also ein Stützpunkt der Tempokurve,
- der Override `emitters.finale.block.count` einer importierten Kachel in Muster 05.

Das Auftragsbeispiel `spiral_left` kommt im Korpus nicht vor, nur im Generierbarkeitsmuster G3. Der
Spiral-Emitter von Muster 04 heißt `bloom`.

| Methode | Kommentare vorher → nachher | Geänderte Zeilen | Zeilen vorher → nachher | Neu geparst ohne Befund | Modell wie erwartet |
|---|---|---|---|:-:|:-:|
| sigil 1, verlustfreier Syntaxbaum | 11 → 11 (04), 10 → 10 (05) | genau eine | unverändert | ja | ja |
| RON, eigener verlustfreier Scanner | 11 → 11 (04), 10 → 10 (05) | genau eine | unverändert | ja | ja |
| RON über das `ron`-Crate (Modell ändern, mit Strukturnamen und `implicit_some` neu serialisieren) | 11 → 0, 10 → 0 | −18/+40 (04, speed) | 58 → 80 (04) | ja | ja |

- Beide verlustfreien Wege erfüllen das Kriterium. Das prüft der Test
  `lossless_edits_change_exactly_one_line_and_keep_comments`, und `cargo run -- set <datei> <pfad> <wert>`
  macht dasselbe auf der Kommandozeile.
- Der Weg über das `ron`-Crate liefert ein richtiges Modell, verliert aber alle Kommentare und schreibt
  Default-Felder wie `offset`, `delay` und `role` aus. Für Werkzeuge, die Modder-Dateien bearbeiten, ist
  das unbrauchbar.
- **Folge:** Verlustfreies Editieren verlangt in beiden Syntaxen eigenen Code. Bei RON ist das ein zweiter
  Parser neben `ron`, der dieselbe Sprache anders liest; jede Abweichung zwischen beiden wäre eine eigene
  Fehlerquelle. Bei sigil 1 ist es derselbe Parser, der auch kompiliert.

### 4.3 Generierbarkeit durch Agenten

**Nachgeholt am 2026-09-15. Kein Unterschied messbar, aber mit geringer Trennschärfe.**

- **Aufgaben:** drei Musterbeschreibungen in Prosa unter `generability/briefs/`: G1 Pendelfächer,
  G2 Minenfeld und G3 Zwillingsspiralen `spiral_left`/`spiral_right` mit Tempokurve. Jede nennt alle Werte.
- **Codex:** Im unbeaufsichtigten Lauf brachen drei parallele Läufe und eine Wiederholung ab, weil Codex nicht
  verfügbar war. Später lief Codex (gpt-6-astra, Reasoning medium, read-only) je Muster genau einmal. Im Kontext standen
  nur Grammatik, Schema, das Beispielpaar 04 und der Aufgabentext; der Prompt verbietet ausdrücklich,
  Claudes Dateien, `results.md`, den Prototyp oder `docs/` zu öffnen. Laut Protokoll las Codex nur die
  erlaubten Dateien. Antworten unverändert unter `generability/codex/raw/`, die Codeblöcke daraus ohne
  Änderung als `generability/codex/<muster>.ron` und `.sigil`.
- **Claude:** die sechs Dateien aus dem unbeaufsichtigten Lauf, allein aus den Aufgabentexten geschrieben.

| Autor | Dateien | davon mit Diagnose | Diagnosen nach Art | RON ≡ sigil 1 je Muster | Modell gleich dem anderen Autor |
|---|---:|---:|---|:-:|:-:|
| Codex | 6 (3 RON, 3 sigil 1) | 0 | – | 3 von 3 | 6 von 6 |
| Claude | 6 (3 RON, 3 sigil 1) | 0 | – | 3 von 3 | 6 von 6 |

(Zusammenfassung von `results.md` Abschnitt 4; dort je Datei.)

- **Deutung:** Keiner der beiden Autoren macht in einer der beiden Syntaxen einen Fehler, und alle vier
  Fassungen je Muster ergeben dasselbe Modell. Codex' Dateien unterscheiden sich von Claudes nur in
  Formatierung und Kommentaren. Die Sorge, ein fremdes Modell treffe die neue Syntax im ersten Versuch
  deutlich schlechter als RON, bestätigt sich damit nicht.
- **Grenzen:** Die Messung liegt an der Decke. Die Aufgaben sind vollständig beziffert, Grammatik und ein
  vollständiges Beispielpaar lagen im Kontext, und es sind nur drei Muster und ein fremdes Modell. Ob Agenten
  sigil 1 aus einer knapperen Formatdoku, ohne Beispielpaar oder bei offen formulierten Aufgaben ebenso gut
  treffen, ist nicht gemessen. Einen kleineren Nachteil einer Syntax schließt das Ergebnis nicht aus.
- **Wiederholen:** Prompts und Aufruf in `spikes/sigil-syntax/generability/prompts/README.md`, danach
  `cargo run -- report --write`. Der Test `every_generability_file_is_present_and_clean` erwartet alle
  zwölf Dateien ohne Diagnose.

### 4.4 Editier-Ergonomie

**Länge über alle fünf Muster:**

| Maß | RON | sigil 1 | sigil 1 / RON |
|---|---:|---:|---:|
| Zeilen | 403 | 351 | 87 % |
| signifikante Tokens | 1 588 | 883 | 56 % |
| Zeichen ohne Leerraum und Kommentare | 4 600 | 3 166 | 69 % |

Die Token-Differenz von 705 stammt vor allem aus Trennkommas und Einheiten-Hüllen. Kommas machen 280 Tokens
aus (40 %), Einheiten-Hüllen 216 (31 %): `Ticks(20)` sind vier Tokens, `20t` ist eines. Die Kopfzeile
`#![enable(implicit_some)]` bringt 30 (4 %), der Rest aus Strukturnamen, `name:`-Feldern und Klammern 179
(25 %). Die Aufteilung erzeugt der Prototyp (`results.md` 5.1). Die Kommentarzeilen sind in beiden Varianten
gleich. Die Tokens zählt der jeweilige Scanner; Gesamtzahl und Aufteilung sind nur Richtwerte.

**Spirale mit Tempokurve (Muster 04, `speed_curve`):**

- Beide Fassungen haben 10 Zeilen. RON braucht 65 Tokens und 126 Zeichen, sigil 1 braucht 51 Tokens und
  108 Zeichen.
- In der vollständigen Datei steht ein Stützpunkt in RON auf 24 Leerzeichen Einrückung
  (`Sigil(` → `emitters: [` → `Emitter(` → `modifiers: [` → `SpeedCurve(` → `keys: [`), in sigil 1 auf 6.
- Wer Modifikatoren umordnet oder löscht, muss in RON Kommas und schließende Klammern über mehrere Ebenen
  richtig setzen. In sigil 1 sind Modifikatoren eigenständige Blöcke ohne Trennzeichen.
- Die Reihenfolge der Modifikatoren ist in beiden Varianten gleich gut sichtbar, und die Knotenpfade zählen
  gleich.

```text
RON                                            sigil 1
SpeedCurve(                                    modifier speed_curve {
    keys: [                                      keys = [
        (at: Ticks(0), mul: 1.6),                  (at = 0t, mul = 1.6),
        (at: Ticks(20), mul: 0.3),                 (at = 20t, mul = 0.3),
        (at: Ticks(60), mul: 0.3),                 (at = 60t, mul = 0.3),
        (at: Ticks(90), mul: 1.2),                 (at = 90t, mul = 1.2),
    ],                                           ]
    interp: Smooth,                              interp = smooth
),                                             }
```

(Kommentarzeile des Originals für die Gegenüberstellung weggelassen.)

**Fehlersicht eines Modders** (`results.md` 2.6): In den Korpusfällen zeigt sigil 1 auf das Token, das zu
ändern ist, und nennt Feld und Besitzer. In den meisten Fällen schlägt sie auch einen konkreten Ersatz vor;
Ausnahmen sind e03 (Hinweis ohne Beispielwert) und e08 (Umbenennen ohne konkreten Namen). Bei e04 nennt sie
die Zeile der öffnenden Klammer. RON-Syntaxfehler führen oft in die falsche Richtung (e02, e04, e09). Ein
fehlendes Feld meldet `ron` an der schließenden Klammer weit unter der Stelle, an der man suchen würde
(e03); die Zusatzschicht verlegt die Meldung auf den Emitter.

### 4.5 Implementierungsaufwand

Codezeilen des Spikes nach `rustfmt`, ohne Leer- und Kommentarzeilen, ohne Tests:

| Teil | Module | Codezeilen |
|---|---|---:|
| sigil-Seite | `sigil.rs` 1 476, `tree.rs` 765 | 2 241 |
| RON-Seite, trotzdem eigener Code | `ron_front.rs` 199, `ron_cst.rs` 514 | 713 |
| gemeinsam, unabhängig von der Syntax | `model.rs` 284, `check.rs` 962, `diag.rs` 170 | 1 416 |

Die sigil-Seite ist rund 3,1-mal so groß wie die RON-Seite (in der ersten Fassung 3,6 bei 630 Zeilen; die
Nacharbeit für e03 und e07 hat `ron_front.rs` um 83 Zeilen vergrößert). Die RON-Seite ist aber nicht null: Wer
Knotenpfade, Positionen für Validierungsbefunde oder `set` braucht, schreibt auch dort einen eigenen
Scanner. Der Vorteil „kein eigener Parser“ gilt für RON also nur ohne diese Anforderungen, und Plan 0002
verlangt sie (WP1.4 Rundreise, WP4.1 verlustfreier Syntaxbaum und Knotenpfade in Diagnosen).

## 5 Bewertung gegen die WP1.4-Kriterien

Vorläufige Einschätzung von Claude; die Bestätigung durch den PO steht aus.

| Kriterium | RON | sigil 1 | Beleg | Einschätzung |
|---|---|---|---|---|
| Fehlermeldungen | 102/120 (Zweitbewertung 104); Syntaxfehler oft irreführend (an `ron` selbst); nur erster Fehler; Hinweise und Positionen nur mit eigener Schicht | 119/120 im Korpus (Zweitbewertung 120); Wiederaufsetzen nach Syntaxfehlern; bei syntaxspezifischen Sonden Folgefehler | 4.1 | **Vorteil sigil 1 bei Syntaxfehlern, gemessen**, aber auf Soll-Texte hin geschrieben (Befangenheit, Abschnitt 6) |
| Rundreise und `set` ohne Kommentarverlust | erfüllt mit eigenem Scanner (zweiter Parser); über `ron` nicht erfüllt | erfüllt mit demselben Parser | 4.2 | **Gleichstand im Ergebnis**; bei RON bleibt das Risiko zweier Parser |
| Editier-Ergonomie | tiefere Verschachtelung, Einheiten als Hülle, Kommas über Ebenen | 56 % der Tokens, flache Blöcke, Einheiten als Suffix | 4.4 | **Vorteil sigil 1, gemessen** an Länge und Einrückung; nicht an echten Moddern |
| Generierbarkeit durch Agenten | G1–G3 im ersten Versuch fehlerfrei (Codex und Claude); vermutlich in Trainingsdaten vertreten (nicht geprüft) | G1–G3 im ersten Versuch fehlerfrei (Codex und Claude), mit Grammatik und Beispielpaar im Kontext | 4.3 | **Gleichstand, gemessen mit geringer Trennschärfe.** Das Risiko für sigil 1 ist nicht bestätigt, bei schwierigeren Bedingungen aber nicht ausgeräumt |
| Aufwand und Wartung | 713 Zeilen eigener Code plus Abhängigkeit von `ron`-Verhalten je Version | 2 241 Zeilen eigener Parser, dafür keine Fremdabhängigkeit im Parser | 4.5 | **Vorteil RON**, bei den geforderten Fähigkeiten etwa Faktor 3,1 statt „null gegen alles“ |

Zwei Befunde gelten unabhängig von der Wahl. Erstens liefern Schema- und Validierungsbefunde nur dann
Knotenpfade und gute Positionen, wenn ein eigener verlustfreier Baum existiert. Zweitens melden beide
Varianten mit serde derive nur den ersten Schemafehler je Lauf. Mehrfachbefunde bräuchten in beiden einen
eigenen, nicht abbrechenden Schema-Pass.

## 6 Grenzen des Spikes

- **Codex fiel im unbeaufsichtigten Lauf zunächst aus.** Beim Korpus-Entwurf lief er in den Timeout, bei Parser-Review und
  Generierbarkeit war er nicht verfügbar. Generierbarkeit und Zweitbewertung sind danach nachgeholt
  (4.1, 4.3); eine Zweitmodell-Prüfung des Parser-Codes gibt es weiterhin nicht. Die Generierbarkeitsmessung
  liegt an der Decke und trennt die Syntaxen kaum.
- **Befangenheit.** Claude hat Korpus, `expected.json`, Grammatik und beide Prototyp-Parser geschrieben.
  Die Meldungen von sigil 1 sind auf die Soll-Texte hin formuliert, die von `ron` nicht. Die 30/30 für
  sigil 1 zeigen, was eine eigene Grammatik **erreichen kann**, nicht, was sie ohne Aufwand liefert.
- **Handbewertungen.** Die Spalten Ursache und Fix-Hinweis sind Claudes subjektive, vorläufige Bewertung
  (`RATINGS` in `measure.rs`). Position und Knotenpfad sind automatisch bewertet. Codex hat beide Spalten
  blind zweitbewertet (`RATINGS_CODEX`); beide Bewerter sind Sprachmodelle, und die Skala stammt von Claude.
- **Parität von RON nur mit eigenem Scanner.** Pfade, Positionen und `set` erreicht RON nur über
  `ron_cst.rs`. Ohne den Scanner gibt `ron` nur Rohmeldung und Position (`results.md` 2.2).
- **Nur der erste Schemafehler.** Beide Varianten melden je Lauf nur den ersten Schemafehler, weil serde
  derive dort aufhört. Nur sigil 1 setzt nach Syntaxfehlern wieder auf.
- **Nur `ron` 0.12.2.** Andere Versionen verhalten sich anders, etwa bei der Prüfung von Newtype-Namen.
  Eine echte v2-Datei wurde nicht gemessen. Dass der Prototyp die Version in RON erst nach der vollständigen
  Deserialisierung prüft, ist ein Artefakt des Prototyps: Der im ADR vorgeschlagene Vorab-Scan der
  Kopfzeile ist nicht umgesetzt.
- **Nicht gemessen:** Parse-Geschwindigkeit, Hot-Reload, Editor-Unterstützung (LSP), sehr große Dateien und
  Rückmeldungen echter Modder. Das Wiederaufsetzen von sigil 1 ist nur an den Fehlerarten des Korpus
  bewertet; syntaxspezifische Fehlerbilder, Kaskadentiefe über 3, Zyklen und `beats` stehen nur als
  unbewertete Sonden in `results.md` 2.7.
- **Abweichungen vom Auftrag:**
  - Die Parser sind handgeschrieben, ohne Kombinator-Crate.
  - Der Rundreise-Pfad ist `emitters.bloom.*` statt `spiral_left`.
  - Die Messung mehrerer Fehler in einer Datei kam zusätzlich hinzu.
  - Fehlerkopien mit Import (e06, e08) werden gegen das Verzeichnis ihres Basismusters aufgelöst.
- **Code-Zustand:** `cargo clippy` meldet 11 Stilwarnungen (zusammenlegbares `if`, große `Err`-Variante,
  Einrückung von Doc-Listen); für den Spike bleiben sie stehen. `Cargo.lock` ist zur Reproduzierbarkeit
  eingecheckt. Engine-Workspace, Wurzel-`Cargo.toml` und CI sind unberührt. Der Branch ist ohne PR gepusht;
  ein Branch-Push löst keine CI aus, weil `ci.yml` bei `push` nur `main` beobachtet.
- **CI-Kosten eines späteren PR:** `ci.yml` nimmt unter `paths-ignore` nur `**/*.md` und `docs/**` aus. Der
  Spike enthält `.rs`-, `.ron`-, `.sigil`-, `.py`- und `.json`-Dateien sowie `Cargo.toml`/`Cargo.lock`
  unter `spikes/`; ein PR startet also die volle Drei-OS-Matrix, obwohl der Engine-Workspace unberührt ist.
  Ob `spikes/**` in `paths-ignore` gehört, entscheidet der PO. Zu prüfen ist dabei, ob übersprungene
  Pflicht-Checks den Merge blockieren.
- **Prosa-Zahlen.** Zahlen im Fließtext von `results.md` (etwa 29/30, 2 200 Zeilen, 87 %) sind von Hand
  aus den generierten Blöcken übernommen. `cargo test` prüft nur die generierten Blöcke.

## 7 Empfehlung

**Vorläufig, die Bestätigung durch den PO steht aus.** Der Autor empfiehlt die eigene Grammatik „sigil 1“
mit verlustfreiem Syntaxbaum als Quelltextsyntax v1. Die Nachmessung vom 2026-09-15 ändert die Empfehlung
nicht: Weder die Generierbarkeit noch die Zweitbewertung widersprechen ihr. Begründung und Gegenargumente stehen im
[ADR-Vorschlag](../adr/0007-sigil-quelltextsyntax-v1.md); dort sind auch KDL und TOML aus der Literatur
bewertet.

- **Dafür:** PRD-0004 nennt Fehlermeldungen und Editier-Ergonomie als Kriterien, und bei beiden liegt
  sigil 1 gemessen vorn, bei den Fehlermeldungen allerdings nur noch bei Syntaxfehlern. Das Setzen an
  Knotenpfaden erfordert in beiden Syntaxen eigenen Code, daher schrumpft der Aufwandsvorteil von RON auf
  etwa den Faktor 3,1. Bei RON bliebe zudem ein zweiter Parser neben `ron` zu pflegen.
- **Dagegen:** gut 2 200 Zeilen eigener Parser, der gewartet, dokumentiert und gegen beliebige Eingaben
  gehärtet werden muss. Keine Editor-Unterstützung von Haus aus. Und das Risiko, dass Agenten eine
  unbekannte Syntax schlechter treffen: in der Nachmessung nicht bestätigt, bei den dort sehr eindeutigen
  Aufgaben aber auch nicht ausgeschlossen.

**Bedingungen vor der Abnahme in Sammelsitzung B (Vorschlag):**

1. **Generierbarkeit nachholen — erfüllt (2026-09-15).** Codex hat G1 bis G3 in beiden Syntaxen im ersten
   Versuch ohne Diagnose geschrieben. sigil 1 schneidet nicht schlechter ab als RON; die Schwelle
   „deutlich schlechter“ für eine Neugewichtung von RON oder KDL ist nicht erreicht. Weil die Messung kaum
   trennt, bleibt die Frage nach einer schärferen Messung an den PO (Abschnitt 8).
2. **Zweitbewertung — erfüllt (2026-09-15)** durch Codex, blind gegenüber Claudes Werten: 37 von 40 Werten
   gleich, keine Abweichung von 2 oder mehr (4.1). Eine Bewertung durch den PO ersetzt das nicht.
3. **Syntaxspezifische Fehlerfälle** in den Fehlerkorpus aufnehmen und bewerten: für sigil 1 `:` statt `=`,
   mehrere Felder auf einer Zeile, Leerzeichen vor der Einheit und eine Wallclock-Einheit; für RON ein
   fehlender Newtype und ein fehlender Strukturname; für beide Kaskadentiefe über 3, ein Zyklus und `beats`.
   Die falschen Folgebefunde von sigil 1 sollten vorher behoben sein.

## 8 Offene Fragen für den PO (vorbereitet, nicht entschieden)

1. Wie viel eigener Parser-Code (Richtwert gut 2 200 Zeilen im Spike) ist die bessere Diagnose von
   Syntaxfehlern wert, wenn Schema- und Validierungsbefunde in beiden Varianten gleich gut erreichbar sind?
2. Ist verlustfreies Editieren (`sigilc set`, Schreiben aus der Tooling-Suite) Pflicht für v1? Plan 0002
   WP4.1 setzt es voraus. Wenn ja, braucht auch RON einen eigenen verlustfreien Parser.
3. Falls RON gewählt wird: Overrides sind als `RawValue` zu halten, weil `ron::Value` die
   Einheitenprüfung aushebelt.
4. Soll ein Lauf alle Schemafehler einer Datei melden? Das kostet in beiden Varianten einen eigenen
   Schema-Pass statt serde derive.
5. Genügt die nachgeholte Generierbarkeitsmessung (12 von 12 Dateien fehlerfrei, geringe Trennschärfe),
   oder soll vor der Entscheidung eine schärfere Messung laufen, etwa mit offen formulierten Aufgaben, nur
   mit Formatdoku ohne Beispielpaar oder mit weiteren Modellen?
