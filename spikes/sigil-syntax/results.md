# Sigil-Syntax-Spike: Messergebnisse RON gegen „sigil 1“ (OF-4.1)

- **Bezug:** Plan 0002 WP1.4 (Spiel-Repo), PRD-0004 OF-4.1, Projekt-ADR-0006 und ADR-0007, Korpus unter `corpus/`
- **Status:** Messbericht, **keine Entscheidung**. Die Syntaxwahl trifft das Engine-ADR „Sigil-Quelltextsyntax v1“; die Abnahme liegt beim PO (Sammelsitzung B, P-10).
- **Stand:** 2026-09-15 (unbeaufsichtigter Lauf, Claude)
- **Vorläufig:** Alle Bewertungen und Deutungen in diesem Bericht sind vorläufig; die Bestätigung durch den PO steht aus.

Alle Tabellen zwischen den Markierungen `BEGIN GENERATED` und `END GENERATED` erzeugt der Prototyp. Sie
lassen sich jederzeit neu erzeugen und werden von `cargo test` gegen den Code geprüft.

## Kurzbefund

1. **Beide Prototyp-Parser lesen alle fünf Korpus-Muster fehlerfrei** und erzeugen nach Auflösung der
   Komposition dasselbe Modell (Abschnitt 1). Der Korpus ist damit erstmals maschinell gegen einen echten
   RON-Parser und einen echten Parser für die eigene Grammatik geprüft.
2. **Diagnosen:** Die eigene Grammatik trifft in allen zehn Fehlerfällen des Korpus Soll-Position und
   Knotenpfad und liefert je Datei genau eine Diagnose. Das gilt nur für diese zehn Fälle: Syntaxspezifische
   Sonden wie `count: 24` oder zwei Felder auf einer Zeile ergeben zwei bis drei Diagnosen, darunter ein
   falsches „fehlendes Pflichtfeld“ (Abschnitt 2.7). RON mit `ron` 0.12.2 kommt auf 29/30 Punkte bei der
   Position, 24/30 bei der Ursache, 20/30 beim Fix-Hinweis und 29/30 beim Knotenpfad (Abschnitt 2.3), und
   das nur mit einer eigenen Zusatzschicht für Hinweise, Pfade und Positionen. Der verbleibende Abstand
   liegt ganz bei den Syntaxfehlern, und dort an `ron` selbst: `3.5` statt Ganzzahl meldet `ron` als
   „Expected comma“, eine fehlende Klammer als unbekanntes Feld `Bullet`, ein doppeltes Komma als fehlende
   Struktur `Key`. Fehlendes Pflichtfeld (e03) und falsche Einheit (e07) behebt die Zusatzschicht mit
   wenigen Zeilen; in der ersten Fassung fehlten diese Zeilen, und RON lag 6 Punkte tiefer (96/120).
3. **Einheiten:** Entgegen der Befürchtung im Korpus-README prüft `ron` 0.12.2 Newtype-Namen
   (`Ticks(12)` statt `Deg` wird erkannt). **Aber** `ron::Value` verliert die Namen: Über `ron::Value`
   gelesen wird `Ticks(240)` stillschweigend zu `Deg(240.0)`. Overrides müssen deshalb als `RawValue`
   gehalten und gegen das Zielfeld dekodiert werden (Abschnitt 2.4).
4. **Text→Parameter-Rundreise:** Verlustfrei gelingt sie in **beiden** Syntaxen, aber nur mit einem
   eigenen verlustfreien Baum (sigil 1: CST, RON: eigener Scanner). Der Weg über das `ron`-Crate
   (Modell ändern, neu serialisieren) verliert alle Kommentare (11 → 0) und schreibt rund 40 Zeilen um
   (Abschnitt 3).
5. **Länge:** sigil 1 braucht 87 % der Zeilen, 56 % der signifikanten Tokens und 69 % der Zeichen von RON
   (Abschnitt 5.1). Die Token-Differenz stammt zu 40 % aus Trennkommas und zu 31 % aus Einheiten-Hüllen;
   die Token-Zahl ist nur ein Richtwert.
6. **Aufwand:** Die eigene Grammatik kostet im Prototyp gut 2 200 Codezeilen (Lexer, CST, Parser mit
   Wiederaufsetzen, Deserializer mit Spans). Die RON-Seite braucht für gleichwertige Pfade, Positionen und
   verlustfreies Editieren ebenfalls rund 710 Zeilen eigenen Code; der Vorteil „kein eigener Parser“
   gilt für RON also nur, solange niemand Knotenpfade, Positionen für Validierungsbefunde oder
   `sigilc set` braucht (Abschnitt 5.4).
7. **Generierbarkeit ist nicht belastbar gemessen.** Codex war zweimal nicht verfügbar; es
   liegen nur die sechs Dateien von Claude vor (alle fehlerfrei, RON und sigil 1 jeweils modellgleich).
   Da Claude auch Grammatik, Schema und Aufgabentexte geschrieben hat, sagt das über fremde Autoren
   nichts aus (Abschnitt 4).

## Aufbau und Reproduktion

Der Prototyp ist das eigenständige Crate `proto/` mit eigener leerer `[workspace]`-Tabelle; es gehört
nicht zum Engine-Workspace, die Wurzel-`Cargo.toml` ist unverändert. Abhängigkeiten sind exakt gepinnt:
`ron = "=0.12.2"`, `serde = "=1.0.228"` (derive), `serde_json = "=1.0.149"` (nur zum Lesen von
`expected.json`). Der Parser für sigil 1 ist handgeschrieben, ohne Parser-Kombinator-Bibliothek.

```
cd spikes/sigil-syntax/proto
cargo test                                   # reproduziert alle Messungen, prüft results.md
cargo run -- report --write                  # erzeugt die Tabellen in results.md neu
cargo run -- check ../corpus/errors/e04-unbalanced-bracket.sigil
cargo run -- check --dir ../corpus/ron ../corpus/errors/e08-duplicate-emitter.ron
cargo run -- set ../corpus/sigil/04-mirrored-spiral.sigil emitters.bloom.speed 0.12u/t
```

| Modul | Inhalt |
|---|---|
| `src/model.rs` | gemeinsames Datenmodell nach `corpus/grammar/schema-model.txt`, serde derive |
| `src/ron_front.rs` | RON: `ron::Options::from_str`, Abbildung der `ron`-Fehlercodes auf Diagnosen und Hinweise |
| `src/ron_cst.rs` | RON: eigener verlustfreier Scanner (Offset → Knotenpfad, Knotenpfad → Position, `set`) |
| `src/sigil.rs` | sigil 1: Lexer, verlustfreier CST (alle Tokens inklusive Leerraum und Kommentaren), Parser mit Wiederaufsetzen, Absenkung, `set` |
| `src/tree.rs` | sigil 1: Wertebaum mit Spans und Knotenpfaden, serde-Deserializer darüber, Meldungstexte |
| `src/check.rs` | gemeinsame Pipeline, Komposition (Import, Overrides) und Validierung |
| `src/measure.rs` | alle Messungen und die Tabellen dieses Berichts |
| `tests/measurements.rs` | Tests, die die Messungen reproduzieren |

## Methodik

- **Ein Modell, zwei Oberflächen.** Beide Front-Ends deserialisieren in dieselben Rust-Typen mit
  `deny_unknown_fields`. RON geht direkt über `ron` und serde. sigil 1 senkt den verlustfreien Syntaxbaum
  in einen Wertebaum ab, über den ein eigener serde-Deserializer läuft. Der Schema-Pass (Feldnamen, Typen,
  Varianten, Pflichtfelder) ist damit in beiden Varianten derselbe serde-derive-Code; nur Einheiten prüft
  sigil 1 selbst (am Literal), RON über die Newtype-Namen.
- **Gemeinsamer Validator.** Wertebereiche, Referenzen, Eindeutigkeit, Lesbarkeitsregeln, Kaskadentiefe
  und Komposition laufen für beide Syntaxen im selben Code. Positionen holt er über Knotenpfade:
  bei sigil 1 aus dem Wertebaum, bei RON aus dem eigenen Scanner. Die Befunde e05, e06, e08 und e10 (RON)
  sind deshalb in beiden Varianten gleich gut; das ist ein Ergebnis der Architektur, kein Verdienst der
  Syntax.
- **Fairness gegenüber RON.** Die Fix-Hinweise für RON erzeugt eine dünne eigene Schicht aus den
  strukturierten Fehlercodes von `ron` (etwa „Did you mean“ aus `NoSuchStructField`), die Knotenpfade der
  eigene Scanner. Ohne diese Schicht liefert `ron` nur Rohmeldung und Position (Tabelle 2.2). Seit dem
  Review vom 2026-09-15 verlegt die Schicht außerdem ein fehlendes Pflichtfeld über den Scanner vom
  Strukturende auf den Besitzer und nennt bei einer falschen Einheit Feld und Wert. Vorher hatte die
  RON-Seite deutlich weniger Diagnoseaufwand bekommen als die sigil-Seite, und e03 und e07 lagen zusammen
  6 Punkte tiefer.
- **Soll-Werte** stammen aus `corpus/errors/expected.json`. Fehlerkopien, die importieren (e06, e08),
  werden gegen das Verzeichnis ihres Basismusters aufgelöst.
- **Bewertung 0–3.** *Position:* 3 exakt, 2 richtige Zeile, 1 andere Zeile, 0 keine (automatisch).
  *Knotenpfad:* 3 exakt, 2 Präfix des Soll-Pfads (Besitzer) oder umgekehrt, 1 anderer Pfad, 0 keiner
  (automatisch). *Ursache* und *Fix-Hinweis:* 0 keine oder falsch, 1 irreführend oder generisch,
  2 benennt das Problem ohne Kontext, 3 benennt das Problem mit Kontext (Feld, Art, Gegenstelle, konkreter
  Ersatz). Diese beiden Spalten hat Claude von Hand bewertet; die Werte stehen in
  `src/measure.rs` (`RATINGS`) und sind vorläufig.

## 1 Korpus: zwei Parser, ein Modell

<!-- BEGIN GENERATED: korpus -->
| Muster | Diagnosen RON | Diagnosen sigil 1 | Aufgelöste Modelle gleich | CST sigil 1 verlustfrei | RON-Scanner verlustfrei |
|---|---:|---:|:-:|:-:|:-:|
| `01-ring-burst` | 0 | 0 | ja | ja | ja |
| `02-aimed-stream` | 0 | 0 | ja | ja | ja |
| `03-subemitter-cascade` | 0 | 0 | ja | ja | ja |
| `04-mirrored-spiral` | 0 | 0 | ja | ja | ja |
| `05-wave-line-composite` | 0 | 0 | ja | ja | ja |
<!-- END GENERATED: korpus -->

„Aufgelöste Modelle gleich“ vergleicht die Modelle nach Import und Overrides (für 05 also inklusive des
aus 01 übernommenen Rings mit `delay` 240 Ticks und `count` 12). „Verlustfrei“ heißt: Die Verkettung aller
Tokens des Baums ergibt Byte für Byte die Quelldatei.

## 2 Diagnosen

### 2.1 Gemessene Diagnosen

Erste Diagnose je Datei, Ist gegen Soll.

<!-- BEGIN GENERATED: fehler -->
| Fall | Syntax | Diagnosen | Position Ist (Soll) | Phase Ist (Soll) | Meldung | Fix-Hinweis | Knotenpfad Ist (Soll) |
|---|---|---:|---|---|---|---|---|
| e01 | RON | 1 | 34:17 (34:17) | schema (schema) | Unexpected field named `cuont` in `Ring`, expected either `count` or `start` instead. | Did you mean `count`? Allowed fields of `Ring`: count, start. | `emitters.burst.block.cuont` (`emitters.burst.block.cuont`) |
| e01 | sigil 1 | 1 | 30:5 (30:5) | schema (schema) | Unknown field `cuont` in block `ring`. | Did you mean `count`? Allowed fields of block `ring`: count, start. | `emitters.burst.block.cuont` (`emitters.burst.block.cuont`) |
| e02 | RON | 1 | 43:25 (43:24) | parse (schema) | Expected comma. | – | `emitters.stream.block.count` (`emitters.stream.block.count`) |
| e02 | sigil 1 | 1 | 39:13 (39:13) | schema (schema) | Field `count` expects an integer, found the float `3.5`. | Use a whole number, e.g. `count = 3`. | `emitters.stream.block.count` (`emitters.stream.block.count`) |
| e03 | RON | 1 | 27:19 (27:19) | schema (schema) | Emitter `bloom` is missing the required field `speed`. | Add `speed: UnitsPerTick(<value>),` to Emitter `bloom`. | `emitters.bloom.speed` (`emitters.bloom.speed`) |
| e03 | sigil 1 | 1 | 23:9 (23:9) | schema (schema) | Emitter `bloom` is missing the required field `speed`. | Add a line `speed = <value>u/t`. | `emitters.bloom.speed` (`emitters.bloom.speed`) |
| e04 | RON | 1 | 33:9 (33:9) | schema (parse) | Unexpected field named `Bullet` in `Bullet`, expected one of `name`, `silhouette`, `palette`, `glow`, `radius`, `damage`, `flags`, `despawn_vfx`, `behaviour`, or `transforms` instead. | Allowed fields of `Bullet`: name, silhouette, palette, glow, radius, damage, flags, despawn_vfx, behaviour, transforms. | `bullets.seed` (`bullets.seed`) |
| e04 | sigil 1 | 1 | 30:1 (30:1) | parse (parse) | `bullet` cannot start a member inside `bullet seed`; the `{` opened at line 17 is not closed. | Insert `}` on its own line before this line. | `bullets.seed` (`bullets.seed`) |
| e05 | RON | 1 | 34:24 (34:24) | validate (validate) | `count` of block `Ring` must be in 1..=512, found 0; a block of 0 bullets fires nothing and has no defined angle step. | Use 1 to 512 bullets, e.g. `count: 24,`. | `emitters.burst.block.count` (`emitters.burst.block.count`) |
| e05 | sigil 1 | 1 | 30:13 (30:13) | validate (validate) | `count` of block `ring` must be in 1..=512, found 0; a block of 0 bullets fires nothing and has no defined angle step. | Use 1 to 512 bullets, e.g. `count = 24`. | `emitters.burst.block.count` (`emitters.burst.block.count`) |
| e06 | RON | 1 | 43:24 (43:24) | validate (validate) | Unknown behaviour `seek_target_weak2`; registered behaviours: orbit_parent, seek_target_weak. | Did you mean `seek_target_weak`? New behaviours must be registered in Rust first (FR-10). | `bullets.satellite.behaviour` (`bullets.satellite.behaviour`) |
| e06 | sigil 1 | 1 | 38:15 (38:15) | validate (validate) | Unknown behaviour `seek_target_weak2`; registered behaviours: orbit_parent, seek_target_weak. | Did you mean `seek_target_weak`? New behaviours must be registered in Rust first (FR-10). | `bullets.satellite.behaviour` (`bullets.satellite.behaviour`) |
| e07 | RON | 1 | 44:25 (44:25) | schema (schema) | Field `spread` expects `Deg(..)`, found `Ticks(..)`. | Write `spread: Deg(12.0),`. | `emitters.stream.block.spread` (`emitters.stream.block.spread`) |
| e07 | sigil 1 | 1 | 40:14 (40:14) | schema (schema) | Field `spread` expects an angle in `deg`, found the tick quantity `12t`. | Write `spread = 12deg`. | `emitters.stream.block.spread` (`emitters.stream.block.spread`) |
| e08 | RON | 1 | 66:19 (66:19) | validate (validate) | Duplicate emitter name `tide`; first defined at line 48. | Rename one of the emitters; emitter names must be unique within a unit. | `emitters[1].name` (`emitters[1].name`) |
| e08 | sigil 1 | 1 | 57:9 (57:9) | validate (validate) | Duplicate emitter name `tide`; first defined at line 41. | Rename one of the emitters; emitter names must be unique within a unit. | `emitters[1].name` (`emitters[1].name`) |
| e09 | RON | 1 | 49:51 (49:51) | schema (parse) | Expected opening `(` for struct `Key`. | – | `emitters.bloom.modifiers[2].keys[3]` (`emitters.bloom.modifiers[2].keys`) |
| e09 | sigil 1 | 1 | 44:29 (44:29) | parse (parse) | Unexpected `,` in list `keys`: expected a value or `]`. | Remove the extra comma; a single trailing comma is allowed. | `emitters.bloom.modifiers[2].keys` (`emitters.bloom.modifiers[2].keys`) |
| e10 | RON | 1 | 10:14 (10:14) | validate (validate) | Unsupported Sigil version 2; this compiler reads version 1. | Set `version: 1,` or migrate the file with a newer `sigilc`. | `version` (`version`) |
| e10 | sigil 1 | 1 | 1:7 (1:7) | parse (parse) | Unsupported Sigil version 2; this compiler reads `sigil 1`. | Change the header to `sigil 1` or migrate the file with a newer `sigilc`. | `version` (`version`) |
<!-- END GENERATED: fehler -->

### 2.2 Rohmeldungen von `ron`

Das liefert `ron` 0.12.2 ohne jede Zusatzschicht. Knotenpfade und Fix-Hinweise gibt es dort nicht.

<!-- BEGIN GENERATED: fehler-roh -->
| Fall | Rohmeldung `ron` 0.12.2 | Position laut `ron` |
|---|---|---|
| e01 | `` Unexpected field named `cuont` in `Ring`, expected either `count` or `start` instead `` | 34:17 |
| e02 | `` Expected comma `` | 43:25 |
| e03 | `` Unexpected missing field named `speed` in `Emitter` `` | 55:9 |
| e04 | `` Unexpected field named `Bullet` in `Bullet`, expected one of `name`, `silhouette`, `palette`, `glow`, `radius`, `damage`, `flags`, `despawn_vfx`, `behaviour`, or `transforms` instead `` | 33:9 |
| e05 | – (Befund des gemeinsamen Validators, nicht von `ron`) | – |
| e06 | – (Befund des gemeinsamen Validators, nicht von `ron`) | – |
| e07 | `` Expected struct `Deg` but found `Ticks` `` | 44:25 |
| e08 | – (Befund des gemeinsamen Validators, nicht von `ron`) | – |
| e09 | `` Expected opening `(` for struct `Key` `` | 49:51 |
| e10 | – (Befund des gemeinsamen Validators, nicht von `ron`) | – |
<!-- END GENERATED: fehler-roh -->

### 2.3 Qualität je Spalte (0–3)

<!-- BEGIN GENERATED: qualitaet -->
| Fall | Position RON | Position sigil 1 | Ursache RON | Ursache sigil 1 | Fix-Hinweis RON | Fix-Hinweis sigil 1 | Knotenpfad RON | Knotenpfad sigil 1 |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| e01 | 3 | 3 | 3 | 3 | 3 | 3 | 3 | 3 |
| e02 | 2 | 3 | 1 | 3 | 0 | 3 | 3 | 3 |
| e03 | 3 | 3 | 3 | 3 | 2 | 2 | 3 | 3 |
| e04 | 3 | 3 | 1 | 3 | 0 | 3 | 3 | 3 |
| e05 | 3 | 3 | 3 | 3 | 3 | 3 | 3 | 3 |
| e06 | 3 | 3 | 3 | 3 | 3 | 3 | 3 | 3 |
| e07 | 3 | 3 | 3 | 3 | 3 | 3 | 3 | 3 |
| e08 | 3 | 3 | 3 | 3 | 3 | 3 | 3 | 3 |
| e09 | 3 | 3 | 1 | 3 | 0 | 3 | 2 | 3 |
| e10 | 3 | 3 | 3 | 3 | 3 | 3 | 3 | 3 |
| **Summe (max. 30)** | **29** | **30** | **24** | **30** | **20** | **29** | **29** | **30** |
<!-- END GENERATED: qualitaet -->

Begründung der von Hand bewerteten Spalten, wo RON schlechter liegt oder lag. Nur e02, e04 und e09 liegen
noch unter sigil 1. Alle drei sind Syntaxfehler, bei denen `ron` Fehlerklasse oder Ursache verfehlt; der
Fehlercode enthält nicht, was eine Zusatzschicht für eine bessere Meldung bräuchte.

- **e02:** „Expected comma“ nennt weder Typ noch Feld und klassifiziert einen Typfehler als Syntaxfehler
  (Ursache 1). `ron` liest `3` als Ganzzahl und stolpert dann über `.5`. Einen Hinweis gibt der Fehlercode
  nicht her (0).
- **e03:** `ron` selbst nennt nur Feld und Strukturtyp `Emitter` und meldet am Strukturende, 28 Zeilen unter
  dem Emitter-Namen (Tabelle 2.2). Die Zusatzschicht verlegt die Meldung über den Scanner auf den Emitter
  `bloom`, nennt ihn und verweist auf die Position von `ron` (Ursache 3; vor dem Review Ursache 2 und
  Position 1). Der Hinweis nennt keinen Beispielwert (2), genau wie in sigil 1.
- **e04:** Die fehlende Klammer erscheint als „unbekanntes Feld `Bullet` in `Bullet`“; die öffnende
  Klammer bleibt unerwähnt (Ursache 1). Der Hinweis zählt die erlaubten Felder auf und führt damit in die
  falsche Richtung (0). Die Position stimmt nur, weil die nächste Struktur zufällig an der Soll-Stelle
  beginnt.
- **e07:** `ron` nennt nur die beiden Typen. Die Zusatzschicht ergänzt das Feld aus dem Knotenpfad und den
  Wert aus dem Quelltext (`spread: Deg(12.0)`), daher Ursache 3 und Hinweis 3 (vor dem Review je 2).
- **e09:** „Expected opening `(` for struct `Key`“ trifft die Position exakt, erklärt aber nicht, dass ein
  Komma doppelt steht (Ursache 1, Hinweis 0). Der Pfad zeigt auf das nicht vorhandene Element `keys[3]`
  (2).

### 2.4 Befunde zu den offenen Punkten des Korpus

- **e07 (Einheiten in RON):** widerlegt. `ron` 0.12.2 meldet `ExpectedDifferentStructName`; ein falscher
  Newtype bleibt also nicht stumm. Die Prüfung hängt aber an der `ron`-Version und daran, dass Werte nicht
  über `ron::Value` laufen:

<!-- BEGIN GENERATED: ron-value -->
- `ron::from_str::<ron::Value>("Ticks(240)")` ergibt `Ok(Seq([Number(U8(240))]))`.
- Dieser Wert als `Deg` gelesen: `Ok(Deg(240.0))`.
- Derselbe Text als `RawValue`, gelesen als `Deg`: Fehler `` Expected struct `Deg` but found `Ticks` `` an 1:1.
<!-- END GENERATED: ron-value -->

  Für Overrides ist `ron::Value` damit ungeeignet; der Prototyp hält sie als `Box<RawValue>` und dekodiert
  sie erst gegen das Zielfeld. In sigil 1 bleibt die Einheit im Literal und wird genauso gegen das
  Zielfeld geprüft.
- **e03 (fehlendes Feld):** für `ron` selbst bestätigt: Es meldet am Strukturende (55:9 statt 27:19,
  Tabelle 2.2). Die Zusatzschicht korrigiert das über den Scanner; das ist keine Grenze von RON.
- **e04 (Klammer):** bestätigt. `Bullet` wird als Feldname gelesen, der Öffner geht verloren.
- **e02, e01 (Knotenpfad):** bestätigt. `ron` liefert keine Pfade; der eigene Scanner rekonstruiert sie
  aus der Position und funktioniert auch auf nicht parsebaren Dateien.
- **e10 (Version):** Der Prototyp prüft in RON die Version erst nach der vollständigen Deserialisierung mit
  dem v1-Schema. Das ist ein Artefakt des Prototyps, keine Eigenschaft von RON: Ein Vorab-Scan der
  Kopfzeile (ADR-Vorschlag, Versions-Header) prüft sie auch in RON vor allem anderen; umgesetzt ist er
  nicht. Ohne ihn würde eine echte v2-Datei mit neuen Feldern zuerst Schemafehler melden; das ist **nicht
  gemessen**, weil der Korpus keine solche Datei enthält. sigil 1 prüft die Kopfzeile vor allem anderen
  und meldet genau einen Befund.
- **Phasen:** `ron` trennt Syntax- und Schemafehler nicht sauber. e04 und e09 (Soll: Syntax) landen im
  Schema-Pass, e02 (Soll: Schema) als Syntaxfehler.

### 2.5 Mehrere Fehler in einer Datei

Zusätzliche Messung: je zwei Fehlerfälle desselben Basismusters in einer Datei kombiniert.

<!-- BEGIN GENERATED: mehrfach -->
| Kombination | Syntax | Diagnosen | Meldungen (Position) |
|---|---|---:|---|
| e02 + e07 | RON | 1 | Expected comma. (43:25) |
| e02 + e07 | sigil 1 | 1 | Field `count` expects an integer, found the float `3.5`. (39:13) |
| e03 + e09 | RON | 1 | Expected opening `(` for struct `Key`. (48:51) |
| e03 + e09 | sigil 1 | 2 | Unexpected `,` in list `keys`: expected a value or `]`. (43:29); Emitter `bloom` is missing the required field `speed`. (23:9) |
<!-- END GENERATED: mehrfach -->

RON bricht beim ersten Fehler ab. sigil 1 setzt nach Syntaxfehlern wieder auf und meldet danach noch den
Schemafehler. Im Schema-Pass bricht aber auch sigil 1 beim ersten Befund ab, weil serde derive beim
ersten Fehler aufhört (e02 + e07). Mehrere Schemafehler pro Lauf bräuchte in beiden Varianten einen
eigenen, nicht abbrechenden Schema-Pass.

### 2.6 So sieht ein Modder die Fehler

Terminalausgabe des Prototyps (`cargo run -- check …`) für die Fälle, in denen sich die Syntaxen am
stärksten unterscheiden.

<!-- BEGIN GENERATED: modder -->
**e02, RON**

```text
error[parse/syntax]: Expected comma.
  --> corpus/errors/e02-wrong-type.ron:43:25
   |
43 |                 count: 3.5,
   |                         ^
  = path: emitters.stream.block.count
```

**e02, sigil 1**

```text
error[schema/type]: Field `count` expects an integer, found the float `3.5`.
  --> corpus/errors/e02-wrong-type.sigil:39:13
   |
39 |     count = 3.5
   |             ^
  = path: emitters.stream.block.count
  = help: Use a whole number, e.g. `count = 3`.
```

**e03, RON**

```text
error[schema/missing-field]: Emitter `bloom` is missing the required field `speed`.
  --> corpus/errors/e03-missing-field.ron:27:19
   |
27 |             name: "bloom",
   |                   ^
  = note: `ron` reports the end of the struct at 55:9
  = path: emitters.bloom.speed
  = help: Add `speed: UnitsPerTick(<value>),` to Emitter `bloom`.
```

**e03, sigil 1**

```text
error[schema/missing-field]: Emitter `bloom` is missing the required field `speed`.
  --> corpus/errors/e03-missing-field.sigil:23:9
   |
23 | emitter bloom {
   |         ^
  = path: emitters.bloom.speed
  = help: Add a line `speed = <value>u/t`.
```

**e04, RON**

```text
error[schema/unknown-field]: Unexpected field named `Bullet` in `Bullet`, expected one of `name`, `silhouette`, `palette`, `glow`, `radius`, `damage`, `flags`, `despawn_vfx`, `behaviour`, or `transforms` instead.
  --> corpus/errors/e04-unbalanced-bracket.ron:33:9
   |
33 |         Bullet(
   |         ^
  = path: bullets.seed
  = help: Allowed fields of `Bullet`: name, silhouette, palette, glow, radius, damage, flags, despawn_vfx, behaviour, transforms.
```

**e04, sigil 1**

```text
error[parse/unclosed-brace]: `bullet` cannot start a member inside `bullet seed`; the `{` opened at line 17 is not closed.
  --> corpus/errors/e04-unbalanced-bracket.sigil:30:1
   |
30 | bullet shard {
   | ^
  = note: opening brace at 17:13
  = path: bullets.seed
  = help: Insert `}` on its own line before this line.
```

**e07, RON**

```text
error[schema/unit]: Field `spread` expects `Deg(..)`, found `Ticks(..)`.
  --> corpus/errors/e07-wrong-unit.ron:44:25
   |
44 |                 spread: Ticks(12), // total spread of the volley around the aim line
   |                         ^
  = path: emitters.stream.block.spread
  = help: Write `spread: Deg(12.0),`.
```

**e07, sigil 1**

```text
error[schema/unit]: Field `spread` expects an angle in `deg`, found the tick quantity `12t`.
  --> corpus/errors/e07-wrong-unit.sigil:40:14
   |
40 |     spread = 12t // total spread of the volley around the aim line
   |              ^
  = path: emitters.stream.block.spread
  = help: Write `spread = 12deg`.
```

**e09, RON**

```text
error[schema/type]: Expected opening `(` for struct `Key`.
  --> corpus/errors/e09-comma-misuse.ron:49:51
   |
49 |                         (at: Ticks(20), mul: 0.3),,
   |                                                   ^
  = path: emitters.bloom.modifiers[2].keys[3]
```

**e09, sigil 1**

```text
error[parse/comma]: Unexpected `,` in list `keys`: expected a value or `]`.
  --> corpus/errors/e09-comma-misuse.sigil:44:29
   |
44 |       (at = 20t, mul = 0.3),,
   |                             ^
  = path: emitters.bloom.modifiers[2].keys
  = help: Remove the extra comma; a single trailing comma is allowed.
```
<!-- END GENERATED: modder -->

### 2.7 Sonden außerhalb des Fehlerkorpus

<!-- BEGIN GENERATED: sonden -->
| Sonde | Fehlerbild | Syntax | Diagnosen | Meldungen (Position) | Fix-Hinweis der ersten Meldung |
|---|---|---|---:|---|---|
| p01 | RON-Gewohnheit: `:` statt `=` | sigil 1 | 3 | Expected a field `name = value` or a nested `block`, `modifier` or `transform` inside `block ring`, found `count`. (30:5); Unexpected character `:`. (30:10); Block `ring` is missing the required field `count`. (29:9) | Fields are written `name = value`, one per line. |
| p02 | zwei Felder auf einer Zeile | sigil 1 | 2 | Expected end of line, found `start`. (30:16); Block `ring` is missing the required field `start`. (29:9) | Write one member per line. |
| p03 | Leerzeichen vor der Einheit | sigil 1 | 2 | Expected end of line, found `u`. (17:17); Field `radius` expects a distance in `u`, found the plain number `0.25`. (17:12) | Write one member per line. |
| p04 | Wallclock-Einheit `30s` | sigil 1 | 2 | Expected a value for `delay`, found `30s`. (25:11); Unknown unit `s`. (25:13) | – |
| p05 | Einheit fehlt | sigil 1 | 1 | Field `speed` expects a speed in `u/t`, found the plain number `0.05`. (28:11) | Add the unit: `speed = 0.05u/t`. |
| p06 | Newtype fehlt | RON | 1 | Expected opening `(` for struct `UnitsPerTick`. (32:19) | – |
| p07 | Strukturname fehlt | RON | 1 | Expected identifier. (33:20) | – |
| p08 | reservierte Einheit `beats` | sigil 1 | 1 | The unit in `30beats` is reserved: beat time is not available in sigil 1. (25:11) | Use ticks (`t`) until the beat clock arrives (P2). |
| p08 | reservierte Einheit `beats` | RON | 1 | Field `delay` expects `Ticks(..)`, found `Beats(..)`. (29:20) | Write `delay: Ticks(30),`. |
| p09 | Kaskadenzyklus (`mote` platzt in `seed`) | sigil 1 | 1 | Transform cycle: seed -> shard -> ember -> mote -> seed. (90:12) | Break the cycle; a bullet may not turn back into an earlier type. |
| p09 | Kaskadenzyklus (`mote` platzt in `seed`) | RON | 1 | Transform cycle: seed -> shard -> ember -> mote -> seed. (102:21) | Break the cycle; a bullet may not turn back into an earlier type. |
| p10 | Kaskadentiefe 4 (`dust` platzt in neues `grain`) | sigil 1 | 1 | Cascade depth 4 starting at emitter `seeds` exceeds the v1 maximum of 3. (108:12) | Remove a `burst` or `become_emitter` stage. |
| p10 | Kaskadentiefe 4 (`dust` platzt in neues `grain`) | RON | 1 | Cascade depth 4 starting at emitter `seeds` exceeds the v1 maximum of 3. (122:21) | Remove a `burst` or `become_emitter` stage. |
<!-- END GENERATED: sonden -->

Der Fehlerkorpus enthält nur Mutationen, die in beiden Syntaxen gleich aussehen. Die Sonden ergänzen
Fehlerbilder, die es nur in einer Syntax gibt, und die Ablehnungen aus FR-08 (Kaskadentiefe über 3,
Zyklus) sowie `beats`. Jede Sonde ändert eine Kopie eines Korpus-Musters im Speicher (`PROBES` in
`src/measure.rs`). Sie haben keine Soll-Werte und gehen nicht in die Punkte von Abschnitt 2.3 ein.

- **sigil 1:** `:` statt `=` (p01) ergibt drei Diagnosen, zwei Felder auf einer Zeile (p02) zwei. Beide
  enden mit einem falschen Befund „fehlendes Pflichtfeld“, obwohl das Feld dasteht, weil der Parser den Rest
  der Zeile verwirft. Leerzeichen vor der Einheit (p03) und `30s` (p04) ergeben je zwei Diagnosen, `30s`
  ohne Hinweis. Nur die fehlende Einheit (p05) und `beats` (p08) ergeben eine gute Einzeldiagnose. Die
  Aussage „genau eine Diagnose je Datei“ gilt also nur für die zehn Korpusfälle.
- **RON:** Ein fehlender Newtype (p06) meldet `ron` als „Expected opening `(` for struct `UnitsPerTick`“,
  ein fehlender Strukturname (p07) als „Expected identifier“, beide ohne Hinweis. `Beats(30)` (p08) fängt
  die Zusatzschicht wie e07 ab.
- **FR-08:** Zyklus (p09) und Tiefe 4 (p10) meldet der gemeinsame Validator in beiden Syntaxen mit genau
  einer Diagnose. Vorher lief dieser Code in keinem Test und keinem Fehlerfall.

## 3 Text→Parameter-Rundreise

Ein Wert wird über seinen Knotenpfad gesetzt und der Text zurückgeschrieben (Vorbild `sigilc set`).
Geprüft wird, ob Kommentare und Formatierung erhalten bleiben, ob die neue Datei fehlerfrei parst und ob
das Modell genau die erwartete Änderung zeigt. Der Auftrag nannte als Beispiel `spiral_left`; im Korpus
heißt der Spiral-Emitter `bloom` (Muster 04), `spiral_left` kommt nur im Generierbarkeitsmuster G3 vor.

<!-- BEGIN GENERATED: roundtrip -->
| Szenario (Knotenpfad) | Methode | Kommentare vorher → nachher | Zeilen −/+ (Diff) | Zeilen vorher → nachher | Neu geparst ohne Diagnose | Modell wie erwartet |
|---|---|:-:|:-:|:-:|:-:|:-:|
| Emitter-Geschwindigkeit (`emitters.bloom.speed`) | sigil 1: verlustfreier CST | 11 → 11 | −1/+1 | 50 → 50 | ja | ja |
| Emitter-Geschwindigkeit (`emitters.bloom.speed`) | RON: eigener verlustfreier Scanner | 11 → 11 | −1/+1 | 58 → 58 | ja | ja |
| Emitter-Geschwindigkeit (`emitters.bloom.speed`) | RON: `ron`-Crate (Modell ändern, neu serialisieren) | 11 → 0 | −18/+40 | 58 → 80 | ja | ja |
| Tempokurve, 2. Stützpunkt (`emitters.bloom.modifiers[2].keys[1].mul`) | sigil 1: verlustfreier CST | 11 → 11 | −1/+1 | 50 → 50 | ja | ja |
| Tempokurve, 2. Stützpunkt (`emitters.bloom.modifiers[2].keys[1].mul`) | RON: eigener verlustfreier Scanner | 11 → 11 | −1/+1 | 58 → 58 | ja | ja |
| Tempokurve, 2. Stützpunkt (`emitters.bloom.modifiers[2].keys[1].mul`) | RON: `ron`-Crate (Modell ändern, neu serialisieren) | 11 → 0 | −17/+39 | 58 → 80 | ja | ja |
| Override einer importierten Kachel (`emitters.finale.block.count`) | sigil 1: verlustfreier CST | 10 → 10 | −1/+1 | 76 → 76 | ja | ja |
| Override einer importierten Kachel (`emitters.finale.block.count`) | RON: eigener verlustfreier Scanner | 10 → 10 | −1/+1 | 90 → 90 | ja | ja |
| Override einer importierten Kachel (`emitters.finale.block.count`) | RON: `ron`-Crate (Modell ändern, neu serialisieren) | 10 → 0 | −18/+36 | 90 → 108 | ja | ja |
<!-- END GENERATED: roundtrip -->

- Beide verlustfreien Wege ändern genau eine Zeile und behalten alle Kommentare.
- Der Weg über das `ron`-Crate liefert zwar ein richtiges Modell, verliert aber alle Kommentare und
  formatiert die Datei um: Default-Felder werden ausgeschrieben (etwa `offset`, `delay`, `role`), Listen
  und Strukturen neu umbrochen. Für Werkzeuge, die Modder-Dateien bearbeiten (Tooling-Suite nach ADR-0008,
  `sigilc set`), ist das unbrauchbar.
- **Folge für den Vergleich:** Verlustfreies Editieren verlangt in beiden Syntaxen eigenen Code. Bei RON
  ist das ein zweiter Parser neben `ron`, der dieselbe Sprache anders liest; Abweichungen zwischen beiden
  wären eine eigene Fehlerquelle. Bei sigil 1 ist es derselbe Parser.

## 4 Generierbarkeit

**Vorgehen.** Drei neue Muster sind als Prosa beschrieben (`generability/briefs/`): G1 Pendelfächer
(Fächer, Rotation, Sinus-Auslenkung), G2 Minenfeld (Linie, Burst, Typwechsel nach Distanz, Despawn-VFX),
G3 Zwillingsspiralen (zwei Spiral-Emitter `spiral_left`/`spiral_right` mit Tempokurve, Reverse auf
Ereignis, Behaviour, gezielter dritter Emitter). Laut Plan sollte Codex (read-only) jedes Muster in beiden
Syntaxen schreiben und Claude unabhängig davon als zweiter Autor.

**Was geschah.** Alle vier Codex-Aufrufe (drei parallel, eine Wiederholung) brachen sofort ab, weil
Codex nicht verfügbar war. Nach den Regeln für unbeaufsichtigte Läufe ging die Arbeit ohne Codex
weiter. Claude hat seine sechs Dateien aus den Aufgabentexten geschrieben; eine Codex-Fassung, die er hätte
sehen können, gab es nicht.

<!-- BEGIN GENERATED: generierbarkeit -->
| Autor | Muster | Syntax | Datei | Diagnosen | Arten (Phase/Art × Anzahl) | RON ≡ sigil 1 |
|---|---|---|:-:|---:|---|:-:|
| claude | `g1-pendulum-fan` | RON | ja | 0 | – | ja |
| claude | `g1-pendulum-fan` | sigil 1 | ja | 0 | – | ja |
| claude | `g2-mine-field` | RON | ja | 0 | – | ja |
| claude | `g2-mine-field` | sigil 1 | ja | 0 | – | ja |
| claude | `g3-twin-spirals` | RON | ja | 0 | – | ja |
| claude | `g3-twin-spirals` | sigil 1 | ja | 0 | – | ja |
| codex | `g1-pendulum-fan` | RON | fehlt | – | – | – |
| codex | `g1-pendulum-fan` | sigil 1 | fehlt | – | – | – |
| codex | `g2-mine-field` | RON | fehlt | – | – | – |
| codex | `g2-mine-field` | sigil 1 | fehlt | – | – | – |
| codex | `g3-twin-spirals` | RON | fehlt | – | – | – |
| codex | `g3-twin-spirals` | sigil 1 | fehlt | – | – | – |
<!-- END GENERATED: generierbarkeit -->

**Deutung.** Null Fehler bei Claude beweisen nur, dass Grammatik, Schema und Prototyp in sich stimmig sind.
Claude hat Grammatik, Schema, Aufgabentexte und Prüfer selbst geschrieben; das ist keine Messung der
Generierbarkeit durch einen fremden Autor. Aussagekräftig wird die Tabelle erst mit den Codex-Dateien.

**Nachholen.** Sobald Codex verfügbar ist, die Prompts aus `generability/prompts/` (unverändert aus dem
unbeaufsichtigten Lauf übernommen, Aufruf im README dort) erneut an Codex geben, die Antworten
als `generability/codex/g1-pendulum-fan.ron`, `….sigil` usw. ablegen und `cargo run -- report --write`
ausführen. Die Messung zählt dann Diagnosen nach Phase und Art und prüft, ob RON- und sigil-Fassung
desselben Autors dasselbe Modell ergeben.

## 5 Ergonomie

### 5.1 Länge

<!-- BEGIN GENERATED: laenge -->
| Muster | Zeilen RON / sigil 1 | davon Kommentarzeilen | signifikante Tokens RON / sigil 1 | Zeichen ohne Leerraum und Kommentare RON / sigil 1 |
|---|---|---|---|---|
| `01-ring-burst` | 45 / 37 | 4 / 4 | 170 / 88 | 514 / 332 |
| `02-aimed-stream` | 71 / 63 | 5 / 5 | 288 / 160 | 850 / 590 |
| `03-subemitter-cascade` | 139 / 125 | 13 / 13 | 533 / 303 | 1542 / 1091 |
| `04-mirrored-spiral` | 58 / 50 | 6 / 6 | 237 / 141 | 611 / 431 |
| `05-wave-line-composite` | 90 / 76 | 8 / 8 | 360 / 191 | 1083 / 722 |
| **Summe** | **403 / 351** (87 %) | | **1588 / 883** (56 %) | **4600 / 3166** (69 %) |

Prozentwerte: sigil 1 relativ zu RON.
<!-- END GENERATED: laenge -->

<!-- BEGIN GENERATED: token-arten -->
| Tokenart (alle fünf Muster) | RON | sigil 1 | Differenz | Anteil an der Differenz |
|---|---:|---:|---:|---:|
| Trennkommas `,` | 298 | 18 | 280 | 40 % |
| Einheiten-Hüllen (`Ticks` `(` `)` usw., je 3 Tokens) | 216 | 0 | 216 | 31 % |
| Kopfzeile (`#![enable(implicit_some)]` bzw. `sigil 1`) | 40 | 10 | 30 | 4 % |
| übrige (Namen, Werte, Klammern, `:`/`=`, Schlüsselwörter) | 1034 | 855 | 179 | 25 % |
| **Summe** | **1588** | **883** | **705** | **100 %** |
<!-- END GENERATED: token-arten -->

Die Token-Differenz stammt vor allem aus Trennkommas (40 %) und Einheiten-Hüllen (31 %): `Ticks(20)` sind
in RON vier Tokens, `20t` in sigil 1 eines, und RON trennt jedes Feld und jedes Listenelement mit einem
Komma. Die Kopfzeile `#![enable(implicit_some)]` zählt acht Tokens je Datei. Der Rest verteilt sich auf
`name:`-Felder, Strukturnamen und Klammern. Anführungszeichen um offene Bezeichner ändern die Token-Zahl
nicht, nur die Zeichenzahl. Kommentarzeilen sind in beiden Varianten gleich. Die Aufteilung ordnet Tokens
nach ihrem Text zu (`token_kinds` in `src/measure.rs`) und ist wie die Gesamtzahl nur ein Richtwert.

### 5.2 Lesbarkeit: Spirale mit Tempokurve (Muster 04)

<!-- BEGIN GENERATED: spirale -->
RON (10 Zeilen, 65 signifikante Tokens, 126 Zeichen):

```ron
SpeedCurve(
    // Tempo curve over bullet age: fast launch, near stop, hold, release.
    keys: [
        (at: Ticks(0), mul: 1.6),
        (at: Ticks(20), mul: 0.3),
        (at: Ticks(60), mul: 0.3),
        (at: Ticks(90), mul: 1.2),
    ],
    interp: Smooth,
),
```

sigil 1 (10 Zeilen, 51 signifikante Tokens, 108 Zeichen):

```text
modifier speed_curve {
  // Tempo curve over bullet age: fast launch, near stop, hold, release.
  keys = [
    (at = 0t, mul = 1.6),
    (at = 20t, mul = 0.3),
    (at = 60t, mul = 0.3),
    (at = 90t, mul = 1.2),
  ]
  interp = smooth
}
```
<!-- END GENERATED: spirale -->

Beobachtungen (vorläufige Einschätzung von Claude):

- Die Zeilenzahl ist gleich. Der Unterschied liegt in der Dichte je Zeile: In RON trägt jeder
  Stützpunkt die Einheit als Hülle (`at: Ticks(20)`), in sigil 1 als Suffix (`at = 20t`).
- Im vollständigen Muster steht ein Stützpunkt in RON auf 24 Leerzeichen Einrückung
  (`Sigil(` → `emitters: [` → `Emitter(` → `modifiers: [` → `SpeedCurve(` → `keys: [`), in sigil 1 auf 6.
  Der Auszug oben ist auf die Einrückung des Modifikators gekürzt.
- RON verlangt beim Umordnen oder Löschen von Modifikatoren saubere Kommas und schließende Klammern über
  mehrere Ebenen; in sigil 1 sind Modifikatoren eigenständige Blöcke ohne Trennzeichen.
- Die Reihenfolge der Modifikatoren (wirksam in geschriebener Reihenfolge) ist in beiden Varianten gleich
  gut sichtbar. Die Knotenpfade `modifiers[2].keys[1]` zählen in beiden gleich.

### 5.3 Fehlersicht eines Modders

- In sigil 1 zeigt in den zehn Korpusfällen jede Meldung auf das Token, das der Modder ändern muss, und
  nennt Feld und Besitzer (Abschnitt 2.6). In den meisten Fällen schlägt sie auch einen konkreten Ersatz
  vor; Ausnahmen sind e03 (Hinweis ohne Beispielwert) und e08 (Umbenennen ohne konkreten Namen). Eine
  fehlende Klammer wird mit Zeile des Öffners gemeldet. Bei syntaxspezifischen Fehlerbildern entstehen
  Folgefehler (Abschnitt 2.7).
- In RON sind Schema- und Validierungsbefunde brauchbar, sobald die Zusatzschicht Pfade, Positionen und
  Hinweise liefert; das gilt auch für das fehlende Feld, das `ron` selbst an der schließenden Klammer meldet
  (e03). Syntaxfehler führen dagegen oft in die falsche Richtung (e02, e04, e09).
- Beide Varianten melden im Schema-Pass nur den ersten Fehler (Abschnitt 2.5).

### 5.4 Implementierungsaufwand im Prototyp

<!-- BEGIN GENERATED: aufwand -->
| Modul | Zweck | Codezeilen (ohne Leer- und Kommentarzeilen) |
|---|---|---:|
| `sigil.rs` | sigil 1: Lexer, verlustfreier CST, Parser mit Wiederaufsetzen, Absenkung, `set` | 1476 |
| `tree.rs` | sigil 1: Wertebaum, serde-Deserializer mit Spans, Meldungstexte | 765 |
| `ron_front.rs` | RON: `ron`-Aufruf, Fehlercodes → Diagnose und Hinweis | 199 |
| `ron_cst.rs` | RON: verlustfreier Scanner für Positionen, Knotenpfade und `set` | 514 |
| `model.rs` | gemeinsam: Datenmodell | 284 |
| `check.rs` | gemeinsam: Pipeline, Komposition, Validierung | 962 |
| `diag.rs` | gemeinsam: Diagnose, Darstellung | 170 |
<!-- END GENERATED: aufwand -->

Die Zahlen beschreiben Spike-Code nach `rustfmt`, nicht optimiert und ohne Tests. Die sigil-Seite
(`sigil.rs`, `tree.rs`, 2 241 Zeilen) ist rund 3,1-mal so groß wie die RON-Seite (`ron_front.rs`,
`ron_cst.rs`, 713 Zeilen). Vor dem Review waren es 630 Zeilen und Faktor 3,6; `ron_front.rs` ist um die
Verlegung fehlender Felder, die Feldnamen bei falscher Einheit und den Rohaufruf für Tabelle 2.2 gewachsen. Die
RON-Seite ist aber nicht null: Knotenpfade, Positionen für Validierungsbefunde, Hinweise und
verlustfreies `set` brauchen auch dort eigenen Code. Der gemeinsame Teil (Modell, Validierung,
Diagnosedarstellung) ist unabhängig von der Syntaxwahl.

## Grenzen der Messung

- **Befangenheit.** Claude hat Korpus, Soll-Diagnosen, Grammatik und beide Prototyp-Parser geschrieben.
  Die Meldungen von sigil 1 sind auf die Soll-Texte hin formuliert, die von `ron` nicht. Die 30/30 für
  sigil 1 zeigen, was eine eigene Grammatik **erreichen kann**, nicht, was sie ohne Aufwand liefert.
- **Keine Zweitmodell-Prüfung.** Codex fiel in dieser Stufe ganz aus (nicht verfügbar), ebenso die
  unabhängige Zweitfassung der Generierbarkeitsmuster.
- **Handbewertungen** der Spalten Ursache und Fix-Hinweis sind subjektiv und vorläufig.
- **Nur `ron` 0.12.2.** Andere Versionen verhalten sich anders, etwa bei der Prüfung von Newtype-Namen.
- **Wiederaufsetzen** des sigil-Parsers ist für die Fehlerarten des Korpus ausgelegt. Syntaxspezifische
  Fehlerbilder sind nur als Sonden gemessen und zeigen Folgefehler (Abschnitt 2.7); andere (etwa nicht
  geschlossene Listen über viele Zeilen) sind nicht gemessen.
- **FR-08 und `beats`:** Kaskadentiefe über 3, Zyklus und `beats` stehen nur in den Sonden, nicht im
  bewerteten Fehlerkorpus; der Korpus deckt die Kaskadentiefe nur am Limit ab (Muster 03).
- **Nicht gemessen:** Parse-Geschwindigkeit, Hot-Reload, Editor-Unterstützung (LSP), echte v2-Dateien,
  Verhalten bei sehr großen Dateien, Rückmeldungen echter Modder.
- Die Metrik „signifikante Tokens“ zählt Tokens der jeweiligen Scanner und ist nur als Richtwert
  vergleichbar.

## Vorbereitung für das ADR (keine Entscheidung)

Offene Fragen an den PO, die die Messung schärft:

1. **Gewicht der Diagnosequalität:** Wie viel eigener Parser-Code (Richtwert gut 2 200 Zeilen im Spike)
   ist die bessere Syntaxfehler-Diagnose wert, wenn Schema- und Validierungsbefunde in beiden Varianten
   gleich gut erreichbar sind?
2. **Verlustfreies Editieren:** Ist `sigilc set` bzw. das Schreiben aus der Tooling-Suite (ADR-0008) eine
   Anforderung für v1? Falls ja, braucht auch RON einen eigenen verlustfreien Parser, und der eigene Code von
   RON wächst auf etwa ein Drittel dessen von sigil 1.
3. **Komposition:** Falls RON gewählt wird, sind Overrides als `RawValue` zu halten; `ron::Value` hebelt die
   Einheitenprüfung aus (Abschnitt 2.4).
4. **Mehrfachbefunde:** Soll ein Lauf alle Schemafehler einer Datei melden? Das ist in beiden Varianten
   Zusatzaufwand (eigener Schema-Pass statt serde derive).
5. **Generierbarkeit:** Die Codex-Messung sollte vor der Entscheidung nachgeholt werden (Abschnitt 4).
