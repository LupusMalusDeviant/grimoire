# ADR-0007: Sigil-Quelltextsyntax v1

- **Status:** Vorgeschlagen
- **Datum:** 2026-09-15
- **Autor:** Claude (Ausarbeitung, unbeaufsichtigter Lauf) im Auftrag von Lupus Malus Deviant (PO)
- **Konsultiert:** — (PO-Entscheidung in Sammelsitzung B ausstehend)

## Kontext und Problemstellung

Sigil ist die deklarative Daten-DSL für Bullet-Patterns (Projekt-ADR-0006). Quelltexte `.sigil` werden
offline kompiliert: Die Laufzeit lädt nur Binär-Units, der Parser liegt nicht im Laufzeitpfad
(Projekt-ADR-0007). Nach Plan 0002 WP1.3 gehört `grimoire_sigilc` mit der CLI `sigilc` in die
Determinismus-Menge. PRD-0004 OF-4.1 lässt offen, ob die Quelltextsyntax auf RON aufsetzt oder eine eigene
Grammatik bekommt. Das Kriterium sind die besten Fehlermeldungen und die Editier-Ergonomie im Tooling.
WP4.1 baut den Parser in P1 und verlangt einen verlustfreien Syntaxbaum, Diagnosen mit Knotenpfad sowie
einen Konformitätskorpus. Die Syntax prägt damit Compiler, Tooling-Suite (Projekt-ADR-0008),
Formatdokumentation und jeden Content bis zum Mini-Run.

Der Spike WP1.4 hat beide Kandidaten gemessen. Bericht:
[`docs/spikes/of-4.1-sigil-quelltextsyntax.md`](../spikes/of-4.1-sigil-quelltextsyntax.md), Rohdaten:
[`spikes/sigil-syntax/results.md`](../../spikes/sigil-syntax/results.md). Kurz:

- **Fehlermeldungen:** sigil 1 erreicht 119 von 120 Punkten, RON mit `ron` 0.12.2 und eigener Zusatzschicht
  96. RON verliert vor allem bei Syntaxfehlern und beim fehlenden Pflichtfeld.
- **Rundreise:** Einen Wert an einem Knotenpfad ohne Kommentarverlust setzen gelingt in beiden Syntaxen,
  aber nur mit einem selbst geschriebenen verlustfreien Baum. Über das `ron`-Crate gehen alle Kommentare
  verloren.
- **Länge:** sigil 1 braucht 56 % der Tokens von RON.
- **Aufwand:** 2 241 Zeilen eigener Parser gegen 630 Zeilen eigene Zusatzschicht für RON.
- **Generierbarkeit** durch Agenten ist nicht gemessen, weil Codex nicht verfügbar war.

Plan 0002 WP1.4 verlangt außerdem einen Versions-Header `sigil 1`, eine Migrationsregel und die Schließung
von OF-4.2 (f32 oder Festkomma) per Verweis auf ADR-0004.

**Kernfrage:** Welche Quelltextsyntax bekommt Sigil v1, damit Modder und Agenten Patterns mit präzisen
Fehlermeldungen schreiben und Werkzeuge einzelne Werte verlustfrei ändern können — und wie werden
künftige Syntaxversionen gekennzeichnet und migriert?

## Anforderungen

### Funktional

- Alle Sprachmerkmale aus PRD-0004 FR-01 bis FR-05, FR-08 und FR-10 bis FR-12 sind als Daten ausdrückbar, einschließlich Komposition mit Import und Parameter-Overrides.
- Einheiten (Ticks, Grad, Welteinheiten und ihre Raten) sind am Wert sichtbar und werden gegen das Zielfeld geprüft, auch in Overrides. Wallclock-Einheiten gibt es nicht, `beats` ist für v1 reserviert und wird abgelehnt.
- Jede Diagnose nennt Datei, Zeile, Spalte, Knotenpfad, Ursache, Fix-Hinweis und Code, als Text und als JSON (WP4.1).
- Ein Werkzeug setzt einen Wert an einem Knotenpfad, ohne Kommentare oder Formatierung außerhalb des Werts zu verändern (`sigilc set`, Tooling-Suite).
- `parse → fmt → parse` ergibt dasselbe Modell (Property-Test aus WP4.1).
- Jede Datei trägt ihre Syntaxversion. Eine Datei nicht unterstützter Version erzeugt genau eine Diagnose.
- Es gibt eine Migrationsregel zwischen Syntaxversionen.
- Agenten können Patterns generieren und statisch prüfen lassen (PRD-0004 US-02, Projekt-ADR-0006).

### Nicht-Funktional

- Fehlermeldungen sind menschen- und agentenlesbar, die Syntax ist öffentlich dokumentiert (PRD-0004 NFR, `docs/formats/sigil.md`, Modding-by-documentation).
- Determinismus: Dieselbe Quelle ergibt auf Windows x86_64, Linux x86_64 und macOS arm64 dieselbe Binär-Unit (ADR-0004).
- Robustheit: Keine Eingabe bringt Parser oder Compiler zum Panic (Review-Kriterium aus WP1.7).
- Diagnosequalität und Dateiformat hängen nicht stumm vom Verhalten einer Drittcrate-Version ab.
- Der eigene Code bleibt überschaubar und ist in WP4.1 ohne Verzug der P1-Stränge umsetzbar.
- Die Syntax ist unabhängig davon tragfähig, wie P-2 (Projekt-ADR-0010, Compiler-Hoheit in Rust) entschieden wird.

## Betrachtete Optionen

### Option 1: RON über das Crate `ron`, mit eigener Pfad- und Editierschicht

Idiomatisches serde-RON mit `#![enable(implicit_some)]`, PascalCase-Varianten und Einheiten als Newtypes
(`Ticks(20)`, `Deg(11.0)`). `ron` parst und deserialisiert. Eine eigene Schicht leitet Fix-Hinweise aus
den Fehlercodes ab, ein eigener verlustfreier Scanner liefert Knotenpfade, Positionen und `set`. Die
Version steht im Feld `version: 1`. Den Header `sigil 1` müsste ein Vorab-Scan etwa aus einer ersten
Kommentarzeile `// sigil 1` lesen, weil RON keine eigene Kopfzeile kennt. **Gemessen** im Spike.

**Positiv:**
- Kein eigener Parser für das Modell: serde derive liefert Schema-Pass und Typprüfung.
- RON ist im Rust-Umfeld verbreitet und öffentlich dokumentiert. Dass Agenten es gut treffen, liegt nahe, ist aber nicht gemessen.
- Kleinerer eigener Code: 630 Zeilen im Spike für Hinweise, Pfade, Positionen und `set`.
- `ron` 0.12.2 prüft Newtype-Namen, eine falsche Einheit (`Ticks` statt `Deg`) wird also erkannt.

**Negativ:**
- Syntaxfehler führen gemessen in die falsche Richtung. `3.5` statt Ganzzahl heißt „Expected comma“, eine fehlende Klammer erscheint als unbekanntes Feld `Bullet`, ein doppeltes Komma als fehlende Struktur `Key`. Ein fehlendes Pflichtfeld zeigt auf das Strukturende, 28 Zeilen unter dem Emitter.
- `ron` bricht beim ersten Fehler ab und trennt Syntax- und Schemafehler nicht sauber.
- Verlustfreies Editieren braucht einen zweiten Parser neben `ron`, der dieselbe Sprache anders liest. Jede Abweichung zwischen beiden ist eine eigene Fehlerquelle.
- Einheiten als Hülle kosten Tokens und Verschachtelung: 1 588 gegen 883 Tokens, ein Stützpunkt steht auf 24 statt 6 Leerzeichen Einrückung.
- Die Einheitenprüfung hängt an der `ron`-Version und daran, dass nichts über `ron::Value` läuft. Dort wird `Ticks(240)` stillschweigend zu `Deg(240.0)`; Overrides müssen deshalb als `RawValue` gehalten werden.
- Die Version wird erst nach der vollständigen Deserialisierung mit dem v1-Schema geprüft. Eine echte v2-Datei würde zuerst Schemafehler melden (nicht gemessen).

### Option 2: Eigene Grammatik „sigil 1“ mit verlustfreiem Syntaxbaum

Zeilenorientierte Grammatik (`corpus/grammar/sigil-1.ebnf`) mit dem Kopf `sigil 1` in Zeile 1.
Schlüsselwörter für Einträge (`meta`, `import`, `bullet`, `emitter`) und Blöcke (`block`, `modifier`,
`transform`), Felder als `name = wert`, Einheiten als Suffix (`20t`, `11deg`, `0.09u/t`). Die Grammatik
ist generisch; Feldnamen, Typen, Einheiten und Bereiche prüft das gemeinsame Schema. Ein handgeschriebener
Parser baut einen verlustfreien Syntaxbaum mit allen Tokens samt Leerraum und Kommentaren, setzt nach
Fehlern wieder auf und senkt in einen Wertebaum mit Spans ab. Darüber läuft ein serde-Deserializer.
**Gemessen** im Spike.

**Positiv:**
- Beste gemessene Diagnosen: 119/120 Punkte, alle zehn Fälle mit exakter Position und exaktem Knotenpfad, genau eine Diagnose je Fehler.
- Nach einem Syntaxfehler setzt der Parser wieder auf und meldet danach noch Schemafehler.
- Ein Parser für alles: Kompilieren, `fmt`, `set` und `migrate` arbeiten auf demselben verlustfreien Baum.
- Kompakter und flacher: 56 % der Tokens und 87 % der Zeilen von RON. Modifikatoren sind eigenständige Blöcke ohne Kommas über mehrere Ebenen.
- Einheiten stehen am Literal und bleiben in Overrides erhalten, unabhängig von einer Drittcrate.
- Der Versions-Header wird vor allem anderen geprüft; eine fremde Version ergibt genau eine Diagnose.
- Keine Fremdabhängigkeit im Parser, das Format bleibt unter eigener Kontrolle.

**Negativ:**
- Eigener Parser mit rund 2 241 Zeilen im Spike, etwa 3,6-mal so viel eigener Code wie die RON-Schicht. Er muss gewartet, dokumentiert und gegen beliebige Eingaben gehärtet werden (Fuzzing).
- Die Syntax ist neu: Agenten und Modder kennen sie nicht aus anderen Projekten. Ob Agenten sie gut treffen, ist nicht gemessen und das größte offene Risiko.
- Keine Editor-Unterstützung von Haus aus: Syntaxhervorhebung, Formatierer und später LSP sind Eigenbau.
- Die 30/30-Werte sind befangen, denn Grammatik, Soll-Meldungen und Parser stammen vom selben Autor. Sie zeigen, was erreichbar ist, nicht was ohne Aufwand entsteht.
- Lehnt der PO P-2 ab (R20), braucht auch die C#-Seite einen Parser für eine Grammatik ohne fremde Implementierung.

### Option 3: KDL 2.0 über das Crate `kdl`

KDL ist eine knotenorientierte Dokumentsprache: Knoten mit Argumenten, Eigenschaften und Kinderblöcken in
geschweiften Klammern, getrennt durch Zeilenumbrüche. Einheiten ließen sich als Typannotation schreiben,
etwa `key at=(t)20 mul=0.3`. **Nicht gemessen**, bewertet nach Spezifikation und Crate-Dokumentation;
Beispiele sind nicht gegen einen Parser geprüft.

**Positiv:**
- Die Spezifikation 2.0.0 ist seit 2024-12-21 final. Das Crate `kdl` (6.7.1) implementiert sie standardmäßig und bewahrt laut Dokumentation die Formatierung beim Editieren. Verlustfreies `set` käme also ohne eigenen Scanner.
- Diagnosen mit Spans über `miette` sind im Crate vorgesehen.
- Die Oberfläche ähnelt sigil 1: Blöcke in Klammern, ein Konstrukt je Zeile, Kommentare mit `//`, `/* */` und `/-` zum Auskommentieren ganzer Knoten.
- Es ist eine öffentliche, sprachübergreifende Spezifikation; das mildert R20 möglicherweise (C#-Implementierungen nicht geprüft).

**Negativ:**
- Nicht gemessen: Weder die Fehlermeldungen des Crates zu den zehn Korpusfällen noch die Generierbarkeit sind bekannt.
- Schema, Knotenpfade, Einheiten und Fix-Hinweise bleiben Eigenbau. KDL kennt kein serde-Schema mit Feldprüfung wie RON; der Schema-Pass liefe über das Dokumentmodell.
- Die Typannotation für zusammengesetzte Einheiten wie `u/t` bräuchte nach Spezifikation vermutlich Anführungszeichen (`("u/t")0.09`); das ist ungeprüft und liest sich schlechter als ein Suffix.
- KDL 1 und 2 sind inkompatibel (etwa `#true` statt `true`). Ob Werkzeuge und Trainingsdaten schon KDL 2 sprechen, ist offen.
- Das Format liegt unter fremder Kontrolle; Spezifikations- und Crate-Upgrades wären Migrationsanlässe.

### Option 4: TOML 1.1 über das Crate `toml_edit`

Patterns als TOML-Dokumente mit Tabellen-Arrays (`[[emitters]]`, `[[emitters.modifiers]]`) und
Inline-Tabellen, Einheiten als Zeichenketten (`"20t"`) oder im Feldnamen. **Nicht gemessen**, bewertet nach
Spezifikation und Crate-Dokumentation.

**Positiv:**
- Sehr verbreitet (Cargo); Modder und Agenten kennen TOML.
- `toml_edit` (0.25.15, Spezifikation 1.1.0) bewahrt laut Dokumentation Kommentare, Leerraum und die relative Reihenfolge beim Editieren.
- Seit TOML 1.1.0 dürfen Inline-Tabellen mehrere Zeilen umfassen und ein nachgestelltes Komma tragen.

**Negativ:**
- Tiefe Verschachtelung (Emitter → Modifikatoren → Stützpunkte, Transformationen mit Blöcken) wird entweder zu langen Tabellenpfad-Kaskaden oder zu verschachtelten Inline-Tabellen. Die Zugehörigkeit eines Blocks zu seinem Emitter ist bei Tabellen-Arrays nur über die Reihenfolge im Dokument sichtbar.
- Einheiten haben keinen eigenen Typ. Als Zeichenkette verliert der Parser die Typinformation, im Feldnamen (`delay_ticks`) wird eine falsche Einheit stumm (vgl. die Anmerkung zur fairen RON-Form im Korpus-README).
- `toml_edit` bewahrt laut Dokumentation die Reihenfolge gepunkteter Schlüssel nicht.
- Schema, Knotenpfade und Fix-Hinweise bleiben Eigenbau; die Diagnosequalität ist nicht gemessen.

## Vorschlag des Autors

**Option 2, die eigene Grammatik „sigil 1“** (vorläufig, die Bestätigung durch den PO steht aus).

PRD-0004 nennt Fehlermeldungen und Editier-Ergonomie als Kriterien, und bei beiden liegt sigil 1 gemessen
vorn. Der Hauptvorteil von RON, kein eigener Parser, trägt nicht so weit, wie es scheint: Plan 0002 verlangt
Knotenpfade in Diagnosen und verlustfreies Setzen von Werten, und dafür braucht auch RON einen eigenen
Scanner. Übrig bleibt ein Aufwandsunterschied von etwa Faktor 3,6 statt „null gegen alles“, und dazu das
Risiko zweier Parser für eine Sprache. KDL (Option 3) ist die ernsthafteste Alternative, weil das Crate
verlustfreies Editieren mitbringt. Es ist aber nicht gemessen, und Einheiten sowie Schema bleiben
Eigenbau. TOML (Option 4) passt schlecht zur tiefen Verschachtelung der Patterns.

Die Befangenheit des Spikes und die fehlende Generierbarkeitsmessung wiegen schwer. Der Autor schlägt
deshalb **Bedingungen vor der Abnahme** vor:

1. **Generierbarkeit nachholen.** Codex schreibt die Muster G1 bis G3 in beiden Syntaxen (Anleitung im
   Spike-Bericht, Abschnitt 4.3). Schneidet sigil 1 im ersten Versuch deutlich schlechter ab als RON und
   gleicht eine Rückmeldungsrunde mit den `check`-Diagnosen das nicht aus, sollte der PO Option 1 oder 3
   neu gewichten.
2. **Zweitbewertung** der Handbewertungen für die Fälle e02, e03, e04, e07 und e09 durch ein zweites Modell
   oder den PO.

**Versions-Header (für jede Option vorgeschlagen, in Option 1 als Vorab-Scan):**

- Zeile 1 beginnt in Spalte 1 mit `sigil <N>`, wobei `<N>` eine positive Ganzzahl ist. Vor dem Header
  stehen weder BOM noch Kommentar. v1-Dateien beginnen mit `sigil 1`.
- Der Header wird vor allen anderen Pässen geprüft. Fehlt er oder nennt er eine nicht unterstützte Version,
  entsteht genau eine Diagnose mit Fix-Hinweis (Header ergänzen, `sigilc migrate` oder ein neueres `sigilc`).
- Die Quelltextversion ist unabhängig von der Version des Binärformats `SigilUnit` (Vertrag WP1.2); beide
  werden getrennt gezählt.

**Migrationsregel (Vorschlag):**

1. **Inkompatible Änderungen erhöhen `<N>`.** Inkompatibel ist jede Änderung, nach der eine unter `N`
   gültige Datei ungültig wird oder eine andere Binär-Unit ergibt: Umbenennen oder Entfernen von Feldern
   und Arten, geänderte Defaults, geänderte Einheitensemantik, geänderte Grammatik.
2. **Additive Änderungen bleiben in `N`:** neue Arten, neue optionale Felder, deren Fehlen die bisherige
   Bedeutung behält. Ein älteres `sigilc` meldet solche Konstrukte als unbekannt und weist darauf hin, dass
   ein neueres `sigilc` nötig sein könnte.
3. **Übergangsfenster:** `sigilc` für `N+1` liest `N` weiter, mindestens bis zum Ende der folgenden Phase,
   und warnt dabei mit Migrationshinweis. Die Länge des Fensters legt der PO fest.
4. **`sigilc migrate`** schreibt eine Datei schrittweise von `N` auf `N+1` um, bei größeren Sprüngen als
   Kette. Es arbeitet auf dem verlustfreien Syntaxbaum: Kommentare und Formatierung außerhalb geänderter
   Knoten bleiben erhalten. Die Migration ist idempotent.
5. **Nachweis:** Für jede Datei des Konformitätskorpus ergibt die migrierte Fassung dieselbe Binär-Unit
   (Content-Hash) wie das Original. Ausgenommen sind nur Bedeutungsänderungen, die in den Migrationsnotizen
   der Version stehen; sie bekommen eigene Goldens. Replays und Golden Master beziehen sich auf Binär-Units,
   nicht auf Quelltext.
6. Jede Version hat einen eigenen Abschnitt in `docs/formats/sigil.md` mit Änderungsliste.

**OF-4.2 (f32 oder Festkomma), unabhängig von der Optionswahl:** geschlossen durch das akzeptierte
[ADR-0004](0004-deterministische-gleitkommaarithmetik.md). Bullets bewegen sich in `f32` nach dessen
Regeln, transzendente Funktionen laufen über `grimoire_core::math::dmath`. Festkomma gibt es in v1 nicht.
Der Fallback über den Typalias `SimVec` bleibt offen: Er käme nur über ein Folge-ADR, falls das
Annahmekriterium von ADR-0004 später bricht. Für die Syntax folgt daraus:

- Der Quelltext legt keinen Laufzeit-Zahlentyp fest. Literale sind dezimal, und der Compiler bildet sie auf
  die Darstellung der Binär-Unit ab. Ein späterer Wechsel auf Festkomma ändert Compiler und Binärformat,
  nicht die Quelltextsyntax.
- Einlesen der Literale und Umrechnen der Einheiten geschehen in `grimoire_sigilc` und nur mit Operationen,
  die nach ADR-0004 erlaubt sind (Regeln 1–3). Dass die Binär-Units plattformgleich sind, prüft ein
  Golden-Test über den Konformitätskorpus auf allen drei Plattformen.

## Entscheidung

**Gewählte Option:** offen — Vorschlag des Autors siehe oben; Entscheidung durch den PO in Sammelsitzung B (Plan 0002, P-10)

Versions-Header, Migrationsregel und die Schließung von OF-4.2 stehen im Vorschlag des Autors. Sie gelten
für jede Option und werden in Sammelsitzung B mit abgenommen.

## Konsequenzen

Die folgenden Punkte beschreiben die Folgen bei Annahme des Vorschlags (Option 2).

### Positiv

- Modder und Agenten bekommen Diagnosen, die auf das zu ändernde Token zeigen, Feld und Besitzer nennen und einen konkreten Ersatz vorschlagen; auch bei Syntaxfehlern.
- Kompilieren, `fmt`, `set` und `migrate` arbeiten auf einem einzigen verlustfreien Syntaxbaum. Die Tooling-Suite kann Werte schreiben, ohne Kommentare zu zerstören.
- Einheiten stehen sichtbar am Literal und werden in Overrides ohne Sonderfall geprüft.
- Das Format hängt nicht vom Verhalten einer Drittcrate-Version ab. Versionen und Migration liegen vollständig in eigener Hand.
- Der Spike-Korpus und die Soll-Diagnosen gehen direkt in den Konformitätskorpus aus WP4.1 über.
- OF-4.2 ist ohne Zusatzaufwand geschlossen; die Syntax bleibt von einem späteren Festkomma-Wechsel unberührt.

### Negativ

- Eigener Parser von geschätzt gut 2 000 Zeilen in `grimoire_sigilc`, dazu Pflege, Fuzzing und Formatdokumentation. Die Spike-Zahlen sind unoptimierter Prototyp-Code.
- Keine Editor-Unterstützung von Haus aus: Syntaxhervorhebung, Formatierer und später LSP sind Eigenbau.
- Agenten kennen die Syntax nicht aus anderen Projekten. Die Generierbarkeit hängt an Formatdoku, Beispielen und Diagnoseschleife und ist bis zur Nachmessung ein Risiko.
- Die gemessene Diagnosequalität ist befangen und muss sich an fremden Fehlerbildern erst bewähren. Das Wiederaufsetzen ist nur für die Korpus-Fehlerarten geprüft.
- Mehrere Schemafehler je Lauf meldet der Spike nicht; das bräuchte einen eigenen, nicht abbrechenden Schema-Pass statt serde derive.
- Lehnt der PO P-2 ab (R20), braucht die C#-Seite einen eigenen Parser für diese Grammatik.

### Folge-Entscheidungen

- **Vor der Abnahme:** Generierbarkeitsmessung mit Codex nachholen, Handbewertungen zweitbewerten (Bedingungen im Vorschlag).
- **P-2 / Projekt-ADR-0010** (Compiler-Hoheit in Rust): Bei Ablehnung gewinnt die Verfügbarkeit fremder Parser-Implementierungen an Gewicht (Option 3 oder 4).
- **Länge des Übergangsfensters** beim Lesen alter Syntaxversionen (Migrationsregel, Punkt 3).
- **Mehrfachbefunde im Schema-Pass:** ob `sigilc` alle Schemafehler einer Datei meldet, und wie (eigener Pass statt serde derive).
- **Endgültige Grammatik und Schema** in WP4.1 und WP4.2: Feldnamen, Arten, Bereiche; die Korpus-Annahmen sind vorläufig.
- **Editor-Unterstützung:** TextMate-Grammatik für Syntaxhervorhebung jetzt oder LSP später (Tooling-Suite, PRD-0016).
- **Falls Option 1 gewählt wird:** Overrides als `RawValue`, `ron` exakt pinnen, Upgrades gegen den Konformitätskorpus prüfen; Form des Versions-Headers (Vorab-Scan) festlegen.
- **`SimVec`-Fallback** (OF-4.2) nur über ein Folge-ADR zu ADR-0004.

### Review

**Reality-Check geplant für:** nach WP4.1, sobald `sigilc check` gegen den Konformitätskorpus läuft und die ersten Referenz-Patterns von Agenten stammen; spätestens an M4 (Ende P1)

## Weitere Informationen

**Nummer vorläufig:** 0007 ist die nächste freie Nummer auf diesem Branch
(`p1/wp1.4-sigil-syntax-spike`). Parallele P1-Branches können Engine-ADRs anlegen, etwa das
Crate-Map-ADR aus WP1.3. Die endgültige Nummer steht erst beim Merge fest; Verweise auf „Engine-ADR-0007“
sind bis dahin vorläufig.

### Scope

Gilt für die Quelltextsyntax aller `.sigil`-Dateien: Grammatik, Versions-Header, Migrationsregel, Knotenpfade
und Diagnoseformat an der Quelle, umgesetzt in `grimoire_sigilc` (`sigilc check`, `build`, `fmt`, `set`,
`migrate`). Nicht erfasst sind:

- das Binärformat `SigilUnit` (Vertrag WP1.2),
- die Laufzeit `grimoire_sigil`,
- die Frage, wer Parser und Compiler implementiert (Projekt-ADR-0010, P-2),
- das genaue Schema aus Feldnamen, Arten und Bereichen (WP4.1/WP4.2),
- andere Content-Formate außerhalb von `.sigil`.

### Tooling-Empfehlung

- Spike-Korpus (`spikes/sigil-syntax/corpus/`) samt `expected.json` als Grundstock des Konformitätskorpus aus WP4.1 übernehmen, ergänzt um mehrzeilige Fehlerbilder und echte v2-Dateien.
- Property-Tests `parse → fmt → parse` und „Verkettung aller Tokens = Quelldatei“ (Verlustfreiheit), dazu Fuzzing von Lexer und Parser; Ziel: kein Panic bei beliebiger Eingabe.
- `sigilc check --json` als Rückmeldungsschleife für Agenten; die Messung zur Generierbarkeit (`cargo run -- report`) als wiederholbarer Test mit neuen Aufgabentexten.
- Golden-Test über den Korpus: gleiche Binär-Unit auf Windows, Linux und macOS (OF-4.2, ADR-0004).
- Eine kleine TextMate-Grammatik für Syntaxhervorhebung im Editor; LSP erst, wenn die Tooling-Suite es braucht.

### Referenzen

- Spike-Bericht: [`docs/spikes/of-4.1-sigil-quelltextsyntax.md`](../spikes/of-4.1-sigil-quelltextsyntax.md)
- Messdaten: [`spikes/sigil-syntax/results.md`](../../spikes/sigil-syntax/results.md); Korpus und Grammatik: [`spikes/sigil-syntax/corpus/`](../../spikes/sigil-syntax/corpus/README.md)
- [ADR-0004](0004-deterministische-gleitkommaarithmetik.md) — `f32` mit Regeln und `dmath`, `SimVec` als Fallback; schließt OF-4.2
- Spiel-Repo: PRD-0004 (OF-4.1, OF-4.2, FR-01 bis FR-12, US-02), PRD-0016 (Tooling-Suite), Plan 0002 (WP1.3, WP1.4, WP1.6, WP4.1, P-2, P-10, R5, R20), Projekt-ADR-0006 (Sigil-Daten-DSL), ADR-0007 (Offline-Kompilierung), ADR-0008 (Avalonia-Tooling)
- `ron` 0.12.2 (gemessen): https://docs.rs/ron/0.12.2
- KDL-Spezifikation 2.0: https://kdl.dev/spec/ und Release-Hinweise https://github.com/kdl-org/kdl/releases (Final seit 2024-12-21; abgerufen 2026-09-15, nicht gemessen)
- Crate `kdl` 6.7.1 (Formatierung bleibt beim Editieren erhalten, `miette`-Diagnosen): https://docs.rs/kdl (abgerufen 2026-09-15, nicht gemessen)
- Crate `toml_edit` 0.25.15 (Kommentare, Leerraum und Reihenfolge bleiben erhalten; gepunktete Schlüssel nicht): https://docs.rs/toml_edit (abgerufen 2026-09-15, nicht gemessen)
- TOML 1.1.0 (mehrzeilige Inline-Tabellen, nachgestelltes Komma): https://github.com/toml-lang/toml/releases/tag/1.1.0 (abgerufen 2026-09-15)
