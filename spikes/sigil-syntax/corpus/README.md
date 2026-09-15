# Sigil-Syntax-Korpus (Spike OF-4.1)

- **Bezug:** Plan 0002 WP1.4 (Spiel-Repo `docs/plans/0002-phase-p1-sichtbarer-kern.md`), PRD-0004 OF-4.1, Projekt-ADR-0006 und ADR-0007
- **Status:** Entwurf für den Spike, **keine Entscheidung**. Welche Syntax Sigil v1 bekommt, entscheidet das Engine-ADR „Sigil-Quelltextsyntax v1“; die Abnahme liegt beim PO (Sammelsitzung B, P-10).
- **Stand:** 2026-09-15

## Zweck

Der Korpus ist die gemeinsame Eingabe für den Syntax-Vergleich RON gegen eigene Grammatik:
fünf Patterns in beiden Syntaxen mit gleicher Bedeutung und zehn typische Autorenfehler, jeweils
in beiden Syntaxen. Die Prototyp-Parser des Spikes laufen über diese Dateien. Ihre Meldungen
werden mit den Soll-Diagnosen in `errors/expected.json` verglichen (Datei, Zeile, Spalte,
Ursache, Fix-Hinweis). Später wird daraus der Konformitätskorpus nach WP4.1.

## Entstehung

Laut Plan des unbeaufsichtigten Laufs sollte Codex (gpt-6-astra, read-only) den Entwurf liefern. Beide Läufe brachen
nach dem 580-s-Timeout **ohne Ausgabe** ab: der erste mit Dokument-Recherche, der zweite mit
schlankem Prompt ohne Dateizugriff. Nach den Regeln für unbeaufsichtigte Läufe ging die Arbeit ohne Codex weiter.
Claude hat Grammatik, Datenmodell, Patterns und Fehlerfälle selbst geschrieben. Eine
Zweitmodell-Prüfung fehlt deshalb; die Prüfung übernehmen die Skripte unter `tools/` (siehe unten).

## Aufbau

```
corpus/
├── grammar/
│   ├── sigil-1.ebnf         eigene Grammatik „sigil 1“ (ISO-EBNF), Knotenpfad-Schema
│   └── schema-model.txt     gemeinsames Datenmodell als Rust/serde-Skizze + Abbildung RON ↔ sigil 1
├── ron/                     5 Patterns in RON
├── sigil/                   dieselben 5 Patterns in „sigil 1“
├── errors/
│   ├── eNN-<fehler>.ron     10 Fehlerfälle, je eine Kopie eines Patterns mit genau einem Fehler
│   ├── eNN-<fehler>.sigil   derselbe Fehler in der eigenen Grammatik
│   └── expected.json        Soll-Diagnosen je Fall und Syntax
└── tools/
    ├── check_corpus.py      Äquivalenz-, Lesbarkeits- und Abdeckungsprüfung
    └── make_errors.py       erzeugt errors/ aus den Patterns (Positionen berechnet, nicht getippt)
```

## Gemeinsames Modell und Annahmen

Die folgenden Festlegungen braucht der Vergleich. Sie sind **vorläufig** (Bestätigung durch den
PO steht aus) und gelten nur für den Spike. Die endgültigen Regeln entstehen in WP4.

- **Versionierung:** In „sigil 1“ steht in Zeile 1 ab Spalte 1 der Kopf `sigil 1`. In RON trägt die Wurzelstruktur `Sigil(version: 1, …)` die Version.
- **Einheiten:** Zeit in Sim-Ticks (`30t` bzw. `Ticks(30)`), Winkel in Grad (`deg`), Strecken in Welteinheiten (`u`), dazu `u/t`, `u/t2`, `deg/t`. `beats` ist reserviert und wird in v1 abgelehnt (WP4.2). Wallclock-Einheiten gibt es nicht.
- **Winkel:** 0° ist die +x-Achse, positiv gegen den Uhrzeigersinn, 270° zeigt zum unteren Bildschirmrand. Sub-Emitter und Bursts richten sich relativ zur Flugrichtung des auslösenden Bullets aus.
- **Transformationen hängen an Bullet-Typen**, wie im Konzeptdiagramm von PRD-0004 (Bullet-Spezifikation → Trigger/Transformationen). Modifikatoren hängen an Emittern und wirken in der geschriebenen Reihenfolge.
- **Kaskadentiefe:** höchstens 3. Gezählt werden erzeugende Transformationen (`burst`, `become_emitter`) entlang der längsten Kette ab einem Root-Emitter; `change_type` und `reverse` zählen nicht.
- **Registry und Ereignisse:** Die Behaviours `orbit_parent` und `seek_target_weak` gelten als registriert, die Ereignisse `phase_end` und `player_parry` als bekannt.
- **Lesbarkeit (PRD-0003 Regeln 3 und 4):** Die Bullet-Typen einer Unit haben paarweise verschiedene Silhouetten; Paletten liegen im gegnerischen Raum `enemy.*`.
- **Wertebereiche:** Anzahlen 1..=512, `glow` 0..=1, `radius` > 0 und ≤ 8, `speed` > 0 und ≤ 4; weitere Bereiche in `grammar/schema-model.txt`.
- **Kommas:** Nachgestellte Kommas sind in beiden Syntaxen erlaubt, in RON ohnehin. Der Fehlerfall „Komma-Missbrauch“ ist deshalb ein **doppeltes Komma** in einer Liste. Das ist eine bewusste Abweichung vom Auftragsbeispiel „trailing comma misuse“: Ein einzelnes nachgestelltes Komma wäre in RON gar kein Fehler.
- **Faire RON-Form:** idiomatisches serde-RON mit `#![enable(implicit_some)]`, `deny_unknown_fields`, PascalCase-Varianten und Einheiten als Newtypes. Die Alternative, die Einheit im Feldnamen zu tragen (`delay_ticks: 30`), wurde nicht gewählt: Damit ließe sich Fall e07 in RON gar nicht formulieren, ein falscher Betrag bliebe stumm. Das ist selbst ein Befund für den Vergleich.

Die eigene Grammatik ist bewusst **generisch**: Sie kennt keine Feldnamen, das Schema prüft Namen,
Typen, Einheiten und Bereiche. Damit unterscheiden sich beide Varianten nur in der Oberfläche und
teilen dasselbe Knotenpfad-Schema (Beschreibung im Kopf von `grammar/sigil-1.ebnf`, etwa
`emitters.burst.block.count` oder `emitters.bloom.modifiers[2].keys[1].mul`).

## Patterns

| Nr. | Datei | Inhalt |
|-----|-------|--------|
| 01 | `01-ring-burst` | wiederholter Ring-Burst mit Beschleunigung |
| 02 | `02-aimed-stream` | gezielter Drei-Wege-Strom mit Streuung und Sinus-Wobble, dazu ein geseedeter Streu-Emitter |
| 03 | `03-subemitter-cascade` | **komplex:** Sub-Emitter-Kaskade der Tiefe 3 mit Typwechsel nach Distanz und abschließendem Burst |
| 04 | `04-mirrored-spiral` | **komplex:** gespiegelte Spirale (3 Faltungen, 6 Arme) mit Rotation und Tempokurve aus 4 Stützpunkten |
| 05 | `05-wave-line-composite` | Welle, Linie, Kurvenbahn, Richtungsumkehr per Ereignis, Behaviour-Referenz, Import von 01 mit Parameter-Overrides |

## Abdeckung

Die Matrix erzeugt `tools/check_corpus.py`. „✓“ heißt, der Baustein steht im Quelltext des
Patterns. In 05 kommt der Ring zusätzlich über den Import von 01 vor.

| Merkmal (PRD-0004) | 01 | 02 | 03 | 04 | 05 |
|--------------------|:--:|:--:|:--:|:--:|:--:|
| **Bausteine (FR-01)** | | | | | |
| Ring | ✓ | | ✓ | | (Import) |
| Spirale | | | | ✓ | |
| Fächer | | | ✓ | | |
| Aimed | | ✓ | | | |
| Welle | | | | | ✓ |
| Linie | | | | | ✓ |
| Zufalls-Streuung (geseedet) | | ✓ | | | |
| **Modifikatoren (FR-02)** | | | | | |
| Tempo-Kurve `speed_curve` | | | | ✓ | |
| Winkelrotation `rotate` | | | | ✓ | |
| Beschleunigung `accelerate` | ✓ | | | | |
| Kurvenbahn `curve` | | | | | ✓ |
| Sinus-Offset `sine_offset` | | ✓ | | | |
| Spiegelung/Symmetrie `mirror` | | | | ✓ | |
| **Transformationen (FR-03)** | | | | | |
| Platzen `burst` | | | ✓ | | |
| Typwechsel `change_type` | | | ✓ | | |
| Richtungsumkehr `reverse` | | | | | ✓ |
| Sub-Emitter `become_emitter` | | | ✓ | | |
| **Trigger** | | | | | |
| nach Zeit | | | ✓ | | |
| nach Distanz | | | ✓ | | |
| nach Ereignis | | | | | ✓ |
| **Flags (FR-04)** | | | | | |
| `smashable` | ✓ | | ✓ | | |
| `reflectable` | | | | | ✓ |
| `env_active` | | | | | ✓ |
| `grazeable` | ✓ | ✓ | ✓ | ✓ | ✓ |
| **Weiteres** | | | | | |
| Visual-Metadaten Silhouette/Palette/Glow (FR-05) | ✓ | ✓ | ✓ | ✓ | ✓ |
| Despawn-VFX-Hook für Clear (FR-12) | ✓ | | ✓ | | |
| Behaviour-Referenz (FR-10) | | | | | ✓ |
| Metadaten Name/Schwierigkeit/Dichte (FR-11) | ✓ | ✓ | ✓ | ✓ | ✓ |
| Patron-Zugehörigkeit (FR-11) | | ✓ | | | ✓ |
| Kaskadentiefe am Limit (FR-08) | | | ✓ | | |
| Komposition: Import + Overrides | | | | | ✓ |
| Emitter-Offset | | ✓ | | | ✓ |
| `repeat = forever` | | ✓ | | | |
| Emitter-Rolle `sub` | | | ✓ | | |

Nicht abgedeckt, weil kein Sprachmerkmal: FR-06 (SoA-Pool), FR-07 (Kollision), FR-09 (Hot-Swap)
und die Clear-Auslösung selbst (FR-12; der Korpus deckt nur den VFX-Hook ab). Beat-Einheiten
fehlen bewusst, sie sind in v1 abgelehnt.

## Fehlerkorpus

Jeder Fall verändert eine Kopie genau eines Patterns an genau einer Stelle, in beiden Syntaxen
gleich. Zeile:Spalte bezeichnen die **Soll-Position** des Diagnose-Tokens (1-basiert); sie stammen
aus `make_errors.py` und sind nicht gemessen. Ursache, Fix-Hinweis, Knotenpfad und Anmerkungen
stehen in `errors/expected.json`.

| ID | Fehler | Basis | Phase RON / sigil | Position RON | Position sigil |
|----|--------|-------|-------------------|--------------|----------------|
| e01 | Tippfehler im Schlüssel (`cuont`) | 01 | schema / schema | 34:17 `cuont` | 30:5 `cuont` |
| e02 | Falscher Typ (`3.5` statt Ganzzahl) | 02 | schema / schema | 43:24 `3.5` | 39:13 `3.5` |
| e03 | Fehlendes Pflichtfeld `speed` | 04 | schema / schema | 27:19 `"bloom"` | 23:9 `bloom` |
| e04 | Nicht geschlossene Klammer | 03 | parse / parse | 33:9 `Bullet` (Öffner 18:15) | 30:1 `bullet` (Öffner 17:13) |
| e05 | Wert außerhalb des Bereichs (`count` 0) | 01 | validate / validate | 34:24 `0` | 30:13 `0` |
| e06 | Unbekannte Behaviour-ID | 05 | validate / validate | 43:24 `"seek_target_weak2"` | 38:15 `seek_target_weak2` |
| e07 | Falsche Einheit (Ticks statt Grad) | 02 | schema / schema | 44:25 `Ticks` | 40:14 `12t` |
| e08 | Doppelter Emitter-Name | 05 | validate / validate | 66:19 `"tide"` (erste Def. 48:19) | 57:9 `tide` (erste Def. 41:9) |
| e09 | Doppeltes Komma in Liste | 04 | parse / parse | 49:51 `,` | 44:29 `,` |
| e10 | Falscher Versions-Header | 03 | validate / parse | 10:14 `2` | 1:7 `2` |

## Offene Punkte für die Messung

Die folgenden Aussagen über RON sind **nicht verifiziert**. Sie beschreiben das erwartete Verhalten
von ron/serde und müssen mit dem Prototyp-Parser gemessen werden; in `expected.json` stehen sie als
`note`.

- **e07:** Ob `Ticks(12)` an einer `Deg`-Stelle abgelehnt wird, hängt davon ab, ob die verwendete ron-Version Newtype-Strukturnamen prüft. Andernfalls wird der Wert stumm als `Deg(12.0)` gelesen, und es erscheint keine Diagnose.
- **e03:** serde meldet fehlende Felder erst am Strukturende. Die RON-Position zeigt deshalb vermutlich auf die schließende Klammer statt auf den Emitter-Namen.
- **e04:** RON hat keine Item-Schlüsselwörter zur Resynchronisation. `Bullet` wird als Feldname gelesen, vermutlich mit der Meldung „unknown field“ oder „expected ':'“; der Öffner geht dabei verloren.
- **e02, e01:** Die serde-Meldungen nennen Typ bzw. erlaubte Felder, aber keinen Knotenpfad. Den muss eine eigene Span-Schicht liefern.
- **e10:** In RON ist die Version ein normales Feld. Eine echte v2-Datei erzeugt zuerst Schemafehler, außer der Compiler liest `version:` vorab.
- **Overrides (05):** `overrides` ist in RON eine Map auf `ron::Value`. Vermutlich verliert `Value` die Newtype-Namen (`Ticks(240)`), dann ist die Einheitenprüfung dort nicht mehr möglich. In „sigil 1“ bleiben die Einheiten im Literal erhalten.
- **Eigene Grammatik:** Die Soll-Meldungen für „sigil 1“ sind ebenso ungemessen; es gibt noch keinen Parser.

## Werkzeuge

Aus dem Wurzelverzeichnis des Engine-Repos ausführen (Python 3.10 oder neuer, keine Abhängigkeiten):

```
python spikes/sigil-syntax/corpus/tools/check_corpus.py   # Exit-Code 0 = äquivalent und vollständig abgedeckt
python spikes/sigil-syntax/corpus/tools/make_errors.py    # errors/ neu erzeugen, nachdem ein Pattern geändert wurde
```

`check_corpus.py` ist **kein Parser**. Das Skript reduziert beide Varianten auf Token-Ströme und
vergleicht sie:

- Werte mit Einheiten
- Feldnamen
- Kommentartexte in Reihenfolge

Dazu prüft es die Lesbarkeitsregeln und die Abdeckung. Die Wirksamkeit ist nachgewiesen: Auf einer
Kopie mit drei eingebauten Abweichungen (ein Wert, ein Kommentar, eine Flag-Reihenfolge) schlug das
Skript dreimal an. Syntaktische Gültigkeit und Semantik (etwa Kaskadentiefe, Zyklen, Importauflösung)
prüft es nicht; das bleibt Aufgabe der Prototyp-Parser.
