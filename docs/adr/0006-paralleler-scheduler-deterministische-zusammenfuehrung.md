# ADR-0006: Paralleler Scheduler mit deterministischer Zusammenführung

- **Status:** Akzeptiert (2026-09-14; PO-Entscheidung per Befragung — freigegeben ist die Umsetzung erst mit dem Hash-Gate aus Baustein 7)
- **Datum:** 2026-09-14
- **Autor:** Claude (Ausarbeitung) im Auftrag von Lupus Malus Deviant (PO)
- **Konsultiert:** Lupus Malus Deviant (PO; sechs Einzelentscheidungen per Befragung am 2026-09-14)
- **Ersetzt:** [0003-scheduler-single-threaded.md](0003-scheduler-single-threaded.md) (Vorschlag, abgelehnt)

## Kontext und Problemstellung

P1 soll 10.000 Sigil-Bullets, 100 Gegner-Proxies und eine Graze-Query pro Tick im Simulationsbudget
von höchstens 4 ms halten (Bullets ≤ 1,0 ms, Kollision ≤ 1,5 ms; Spiel-Repo PRD-0002 und Plan 0002).
Genau in P1 entstehen die heißesten Systeme der Engine: Sigil-Laufzeit, Kollisions-Broadphase und
Spieler-Proxy. ADR-0003 schlug vor, bis P2 single-threaded zu bleiben und erst mit Messbeleg zu
parallelisieren. Der PO lehnt das ab. ADR-0003 nennt selbst den Grund: Systeme, die frei `&mut World`
nutzen, laufen nach einer Migration nur exklusiv und brauchen Nacharbeit — und das träfe genau die
Systeme, die P1 schreibt. Die Entscheidung fällt deshalb jetzt. Ihre Umsetzung ist der erste P1-Schritt,
bevor parallele Agenten-Stränge Systeme gegen die Scheduler-API schreiben.

Technische Lage (Stand `v0.1.0`): `Schedule` führt `Box<dyn System>` strikt in `add_system`-Reihenfolge
mit `&mut World` aus, `CommandBuffer` wendet Befehle in Aufzeichnungsreihenfolge an. `Component`,
`Resource` und die internen Spalten- und Ressourcenspeicher sind `Send + Sync`; `&World` ist also ohne
`unsafe` zwischen Threads teilbar. Der Workspace verbietet `unsafe_code`, die identischen `clippy.toml`
der Determinismus-Crates sperren `std`-Threads. `derive_rng(seed, tick, stream)` liefert
reihenfolgeunabhängige Zufallsströme. Queries prüfen Zugriffskonflikte schon beim Erzeugen, `ComponentId`
ist aber crate-intern. Die goldenen Zustands-Hashes aus P0 (Engine-Tests und Spiel-Harness) sind auf
Windows, Linux und macOS identisch und müssen es bleiben.

Die Migrationsschritte 1–5 aus ADR-0003 (Zugriffsmengen, Stufen, Puffer pro System, Blöcke, Beweis vor
Freigabe) sind durchdacht. Dieses ADR übernimmt sie als Bausteine, schiebt sie aber nicht auf.

**Kernfrage:** Wie führt Grimoire Simulationssysteme ab P1 auf mehreren Kernen aus, sodass jeder
Zustands-Hash unabhängig von Thread-Anzahl, Plattform und Fertigstellungsreihenfolge bleibt — ohne
`unsafe` im Fundament?

## Anforderungen

### Funktional

- Zustands-Hashes nach jedem Tick sind bei 1, 2 und N Worker-Threads bit-identisch, auf Windows, Linux und macOS.
- Die Systemliste behält ihre Bedeutung: Jedes System sieht die Wirkungen aller vor ihm eingetragenen Systeme, auch bei paralleler Ausführung.
- Bestehende Systeme (`System::run(&mut World)`, `system_fn`) laufen ohne Änderung weiter; die P0-Goldens von Engine und Spiel bleiben unverändert.
- Große Queries (Bullets, Broadphase) lassen sich innerhalb eines Systems datenparallel bearbeiten, mit deterministischen Reduktionen (Summen, Min/Max, gesammelte Ereignisse).
- Zufall ist pro System und pro Datenblock unabhängig von der Thread-Verteilung.
- Die Thread-Anzahl ist einstellbar, 1 eingeschlossen (Tests, Fehlersuche, schwache Mobilgeräte).

### Nicht-Funktional

- Kein `unsafe` in Engine-Crates (Workspace-Lint, Vertrag §2, Risiko R2 aus Plan 0001).
- Leistung: Simulationsbudget ≤ 4 ms für `stress_10k` auf Referenz-Hardware; kleine Welten wie die P0-Demo werden nicht messbar langsamer als heute.
- Interface-First: Der Thread-Pool ist austauschbar, keine Drittcrate erscheint in der öffentlichen API von `grimoire_ecs` oder `grimoire_sim`.
- Threads entstehen an genau einer, maschinell prüfbaren Stelle; die Determinismus-Crates starten weiterhin selbst keine.
- Fehlersuche: Die Stufenaufteilung ist als Diagnose abrufbar; ein Zugriff außerhalb der Deklaration meldet sich im Debug-Build mit Systemnamen.
- Umsetzbar als abgegrenzter erster P1-Schritt, damit die P1-Stränge nicht lange warten.

## Betrachtete Optionen

### Option 1: Single-threaded mit Migrationspfad (ADR-0003)

P0 bis P2 laufen auf einem Thread in Listenreihenfolge; Parallelität kommt frühestens in P2, mit
Messbeleg und Folge-ADR.

**Positiv:**
- Kein Aufwand vor P1; Determinismus bleibt trivial.
- Die API wird erst festgelegt, wenn echte Lastprofile vorliegen.

**Negativ:**
- Die P1-Stränge schreiben ihre heißen Systeme gegen `&mut World`; nach der Migration laufen diese nur exklusiv, bis sie nachgearbeitet sind.
- Ob das Simulationsbudget auf einem Kern hält, bleibt bis P2 offen — ein Umbau träfe dann deutlich mehr Code.
- Mehrkern-Leistung fehlt genau in der Phase, die den 10k-Stresstest abnimmt.

### Option 2: Paralleler Scheduler mit aufgeteilter Welt (`unsafe`)

Systeme deklarieren Lese- und Schreibmengen. Der Scheduler gibt konfliktfreien Systemen gleichzeitig
disjunkte veränderliche Sichten auf die Welt (das Modell von Bevy), sodass auch schreibende Systeme
parallel laufen.

**Positiv:**
- Größtmögliche Parallelität: Systeme, die verschiedene Komponenten schreiben, laufen gleichzeitig und ohne Umweg über Befehlspuffer.
- Praxiserprobtes Modell mit bekannter API-Form.

**Negativ:**
- Braucht `unsafe` im risikoreichsten Teil des Fundaments; der Workspace-Lint müsste aufgeweicht werden, und die Soundness hängt an korrekten Zugriffsdeklarationen.
- Fehler zeigen sich als Datenrennen statt als Panic und sind schwer zu reproduzieren.
- Für Agenten-Reviews schwer zu prüfen.

### Option 3: Sicherer paralleler Scheduler mit verzögertem Schreiben

Systeme einer parallelen Stufe lesen die Welt nur und schreiben über je einen eigenen Befehlspuffer,
der nach der Stufe in Listenreihenfolge angewendet wird. Schwere Schreiblasten laufen als exklusives
System mit datenparalleler Query in festen Blöcken. Threads kommen aus rayon hinter einem eigenen
`Executor`-Trait.

**Positiv:**
- Kein `unsafe`: `&World` ist heute schon `Sync`.
- Determinismus folgt aus der Konstruktion: Die Welt ändert sich während einer Stufe nicht, Wirkungen werden in fester Reihenfolge angewendet, Blöcke hängen nicht von der Thread-Anzahl ab.
- Bestehende Systeme bleiben gültig (exklusiv); Parallelität wird Schritt für Schritt je System freigeschaltet.
- Deckt beide Lastarten ab: viele mittlere Systeme (Stufen) und wenige riesige Schleifen (Blöcke).

**Negativ:**
- Schreibende Systeme laufen nicht gleichzeitig: Wer Komponenten direkt schreibt, ist exklusiv, und Schreiben über Befehle kostet pro Befehl Allokation und Anwendungszeit.
- Vorab-Aufwand vor den P1-Strängen (Zugriffsdeklaration, Stufenbildung, Executor, Blöcke, Hash-Gate).
- Zusätzliche Drittabhängigkeit (rayon samt crossbeam) und echte Threads im Prozess.

### Option 4: Nur Datenparallelität innerhalb von Systemen

Die Systemliste bleibt sequentiell; einzelne Systeme verteilen nur ihre Query-Iteration in festen
Blöcken auf Threads.

**Positiv:**
- Kleinster Mehraufwand: keine Zugriffsdeklaration, keine Stufenbildung.
- Trifft die größte Einzellast (Bullet-Update) direkt.

**Negativ:**
- Alle übrigen Systeme (Emitter, KI, Kollisionsantworten, Spiel-Logik ab P2) bleiben auf einem Kern; der serielle Anteil begrenzt den Gewinn.
- Spätere System-Parallelität bräuchte wieder eine API-Änderung an allen Systemen — das Problem aus Option 1 in kleinerem Maß.

## Vorschlag des Autors

Option 3. Sie erfüllt die harten Anforderungen — Hash-Identität bei beliebiger Thread-Anzahl und kein
`unsafe` — durch Konstruktion statt durch Disziplin. Ihr Hauptnachteil gegenüber Option 2, dass
schreibende Systeme nicht gleichzeitig laufen, trifft die P1-Lasten wenig: Bullet-Update und
Broadphase sind je eine riesige Schleife über wenige Komponenten und profitieren von Blöcken; die vielen
kleineren Systeme lesen überwiegend und schreiben wenig. Zeigen Messungen später etwas anderes, bleibt
Option 2 als gezielte Erweiterung möglich.

Innerhalb von Option 3 empfiehlt der Autor rayon statt eines eigenen Pools: Der Work-Stealing-Pool ist
ausgereift, und der Trait hält ihn austauschbar. Zugriffe sollen explizit deklariert statt aus
Signaturen abgeleitet werden; das passt zu `&World`-Closures ohne Makros und ist im Debug-Build prüfbar.
Beginnen soll die Umsetzung als erster P1-Schritt, damit kein P1-System gegen die alte Form entsteht.

## Entscheidung

**Gewählte Option:** "Option 3: Sicherer paralleler Scheduler mit verzögertem Schreiben"

Den Ausschlag geben Hash-Identität ohne `unsafe` und der Zeitpunkt: Die P1-Systeme entstehen gleich
parallelfähig. Bewusst in Kauf genommen werden der Vorab-Aufwand vor den P1-Strängen, die
Drittabhängigkeit rayon und dass schreibende Systeme exklusiv laufen.

Verbindliche Bausteine (Namen sind Arbeitsnamen; die genaue API legt der Vertrags-PR fest):

1. **Zugriffsdeklaration.** Ein System deklariert seine Lesezugriffe und seine verzögerten
   Schreibzugriffe auf Komponenten und Ressourcen, und ob es Strukturbefehle (Spawn, Despawn, Insert,
   Remove) aufzeichnet. Identifiziert wird über Registrierungsnummern, nie über `TypeId`-Reihenfolgen.
   Ein System ohne Deklaration — das sind alle heutigen — läuft exklusiv mit `&mut World`. Debug-Builds
   prüfen, dass Zugriffe der Deklaration entsprechen.
2. **Stufen.** Der Scheduler zerlegt die feste Systemliste in Stufen, allein abhängig von Liste und
   Deklarationen. Ein System kommt nur dann in die Stufe seiner Vorgänger, wenn es nichts liest, was ein
   früheres System dieser Stufe schreibt, und kein früheres System der Stufe Strukturbefehle aufzeichnet;
   sonst beginnt eine neue Stufe. Exklusive Systeme bilden je eine eigene Stufe. Das Ergebnis ist damit
   gleich dem der streng sequentiellen Ausführung, bei der jeder Befehlspuffer direkt nach seinem System
   angewendet wird. Die Stufenaufteilung ist als Diagnose abrufbar.
3. **Verzögertes Schreiben.** Systeme einer parallelen Stufe erhalten `&World` und einen eigenen
   `CommandBuffer`. Nach der Stufe werden die Puffer in Listenreihenfolge angewendet, nie in
   Fertigstellungsreihenfolge. Während einer Stufe ist die Welt unveränderlich.
4. **Datenparallele Queries.** Ein System kann eine Query — auch eine veränderliche in einem exklusiven
   System — in Blöcke fester Größe zerlegen: Archetypen in Erzeugungsreihenfolge, darin dichte
   Reihenfolge. Die Blockgrenzen hängen nie von der Thread-Anzahl ab. Reduktionen und gesammelte
   Ergebnisse werden in Blockreihenfolge gefaltet; das ist für nicht assoziative `f32`-Summen nötig
   (ADR-0004).
5. **Zufall.** Systeme ziehen Zufall ausschließlich über `derive_rng(seed, tick, stream)` mit einem fest
   pro System vergebenen Strom; datenparallele Blöcke leiten ihren Strom deterministisch daraus ab. Ein
   gemeinsam fortgeschalteter Generator ist in parallelen Stufen und Blöcken verboten.
6. **Executor.** `grimoire_ecs` definiert einen `Executor`-Trait und liefert eine sequentielle
   Implementierung. Die rayon-Implementierung liegt in einer eigenen Crate außerhalb der
   Determinismus-Menge und nutzt einen eigenen Pool mit fester Thread-Anzahl statt des globalen.
   Ergebnisse werden nach Aufgabenindex zusammengeführt. Keine Determinismus-Crate hängt von rayon ab;
   die CI prüft das.
7. **Hash-Gate vor Freigabe.** Die goldenen Determinismus-Tests der Engine und die Spiel-Harness laufen
   mit 1, 2 und N Threads (N ≥ 3) auf Windows, Linux und macOS. Jeder Checkpoint-Hash muss mit dem
   1-Thread-Lauf und den bestehenden Goldens übereinstimmen. Ohne grünes Gate wird kein Release getaggt,
   das den parallelen Executor enthält.

Eine Aufteilung der Welt per `unsafe` (Option 2) ist nur über ein Folge-ADR mit Messbeleg möglich.

## Konsequenzen

### Positiv

- Die P1-Systeme (Sigil-Laufzeit, Broadphase, Spieler-Proxy) entstehen gleich parallelfähig; ein späterer Umbau aller Systeme entfällt.
- Determinismus unter Threads folgt aus der Konstruktion (unveränderliche Welt je Stufe, feste Anwendungsreihenfolge, feste Blöcke); das Hash-Gate prüft das Ergebnis.
- Ein Panic in einem System einer parallelen Stufe bricht den Tick ab, bevor ein Befehl dieser Stufe angewendet wurde; die Welt bleibt im Zustand vor der Stufe.
- Die P0-API bleibt gültig: Systeme ohne Deklaration laufen exklusiv wie heute, die Goldens bleiben unverändert.
- Der sequentielle Executor bleibt dauerhaft verfügbar — für Tests, Fehlersuche und schwache Geräte, mit identischen Hashes.
- Der Thread-Pool ist austauschbar, ohne Engine- oder Spiel-Systeme anzufassen.

### Negativ

- Vorab-Aufwand von geschätzt 6 Agenten-Tagen vor den parallelen P1-Strängen; M1 verschiebt sich entsprechend.
- Schreibende Systeme laufen nie gleichzeitig. Schreiben über Befehle kostet Allokation pro Befehl; heiße Schreiblasten müssen als exklusives System mit Blöcken formuliert werden.
- Zugriffsdeklarationen sind zusätzliche Pflichtangaben. Eine falsche Deklaration fällt im Debug-Build oder im Hash-Gate auf, nicht im Release-Build.
- Autoren müssen die Stufen-Semantik kennen: Befehle werden erst nach der Stufe sichtbar.
- Die API wird festgelegt, bevor echte Lastprofile existieren — das Bedenken aus ADR-0003 bleibt, gemildert durch Benches in P1.
- Neue Drittabhängigkeiten (rayon, crossbeam) und echte Threads: längere Builds, Pool-Overhead bei kleinen Welten, auf Mobilgeräten Wärme- und Akkufragen.
- Ändert sich eine Blockgröße, ändern sich reduktionsabhängige Hashes. Die Blockgröße ist deshalb Vertragsbestandteil, ihre Änderung erneuert Goldens.

### Folge-Entscheidungen

- Vertrags-PR in `docs/architektur/crate-vertraege.md`: §3 (einzige Thread-Quelle, Abhängigkeitsprüfung), §7 (Zugriffsdeklaration, Stufen, Executor, datenparallele Queries, Blockgröße, `Send`-Grenze für Systeme paralleler Stufen), §8 (Stromableitung je System und Block) sowie die Einstellung der Thread-Anzahl in der Fassade.
- Name und Abhängigkeitskanten der Executor-Crate im Crate-Map-ADR für P1.
- Öffentlicher Typ für Zugriffsmengen, und ob `ComponentId` dafür öffentlich wird.
- Konkrete Blockgröße, per Bench bestimmt.
- Standard-Thread-Anzahl je Plattform (Desktop jetzt, Mobile ab P4) und Verhältnis zum Render-Thread.
- Aufteilung der Welt per `unsafe` nur bei Messbeleg, als eigenes ADR.

### Review

**Reality-Check geplant für:** nach der Umsetzung (erstes P1-Release `v0.1.1`), spätestens 2026-11-09

## Weitere Informationen

### Scope

Gilt für `grimoire_ecs`, `grimoire_sim`, die Fassade und alle Simulationssysteme — Engine-Subsysteme wie
Sigil-Laufzeit und Kollision ebenso wie die Systeme eines Spiels. Nicht erfasst sind Render-Extraktion
und Render-Thread, Asset-Laden, Audio und Offline-Werkzeuge (Sigil-Compiler, Benches); dafür gelten
eigene Entscheidungen.

### Tooling-Empfehlung

- Ein Test-Executor, der die Aufgaben einer Stufe auf einem Thread in permutierter Reihenfolge ausführt, findet Abhängigkeiten von der Fertigstellungsreihenfolge reproduzierbar, ohne auf echte Nebenläufigkeit angewiesen zu sein.
- Die Stufenaufteilung des Standard-Schedules als eingefrorener Diagnose-Test: Eine Deklaration, die die Aufteilung ändert, fällt im Review auf.
- Benches mit 1 und N Threads für die P0-Demo und `stress_10k`, damit der Overhead bei kleinen Welten sichtbar bleibt.

### Referenzen

- [ADR-0003](0003-scheduler-single-threaded.md) — abgelehnter Vorschlag; seine Migrationsschritte 1–5 sind die Grundlage der Bausteine
- [ADR-0004](0004-deterministische-gleitkommaarithmetik.md) — `f32`-Regeln, nicht assoziative Reduktionen
- [Crate-Verträge](../architektur/crate-vertraege.md) §2, §3, §7, §8
- Spiel-Repo: PRD-0002 (FR-08, OF-2.2, Budgets), Plan 0002 (WP1, M0, R11), Projekt-ADR-0004 (eigenes ECS, Parallelität nur mit fester Reduktionsreihenfolge)
- rayon: https://docs.rs/rayon
