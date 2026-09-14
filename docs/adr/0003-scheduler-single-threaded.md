# ADR-0003: Single-threaded, strikt geordneter Scheduler mit Migrationspfad zur Parallelität

- **Status:** Vorgeschlagen
- **Datum:** 2026-09-14
- **Entscheider:** Lupus Malus Deviant (PO), vorbereitet durch Claude
- **Bezug:** Spiel-Repo PRD-0002 (FR-08, offene Frage OF-2.2), ADR-0004 (eigenes ECS), ADR-0005 (voll deterministische Simulation), Plan 0001 WP5.2/WP6.2, `docs/architektur/crate-vertraege.md` §7

## Kontext

`grimoire_ecs` braucht in P0 einen System-Scheduler. PRD-0002 lässt offen (OF-2.2), ob er
single-threaded deterministisch startet und später parallelisiert wird oder von Beginn an parallel
mit fester Ordnung entworfen wird. ADR-0005 macht Determinismus pro Plattform unverhandelbar:
gleicher Seed und gleiche InputFrames müssen bit-identische Zustands-Hashes liefern.
Parallelität ist dort nur „mit fester Reduktionsreihenfolge" erlaubt.

Die P0-Lasten sind überschaubar: eine Demo-Sim mit bewegten Entities, RNG und Bot-Inputs.
Das Bullet-Budget (10.000 Bullets) wird erst ab P1/P2 real. Die ECS-API (Queries mit
`&World`/`&mut World`, `CommandBuffer`, `System::run(&mut World)`) entsteht gerade erst, und
jedes Parallelisierungsmodell prägt diese API tief.

## Anforderungen

- Deterministische, explizit festgelegte Ausführungsreihenfolge der Systeme (ADR-0005).
- Bit-identische Zustands-Hashes über 10.000 Ticks im Doppellauf (P0-Gate WP5.6).
- Einfache, schnell lieferbare API für P0; keine Threads in Simulations-Crates (Vertrag §3). Die
  identischen `clippy.toml` sperren `thread::spawn`, `thread::Builder::spawn`/`spawn_scoped` und
  `thread::scope`; andere Wege, Threads zu starten (etwa über Drittcrates), bleiben Review-Aufgabe.
- Späteres Skalieren auf das Bullet-Budget, ohne dass Spiel-Systeme neu geschrieben werden müssen.
- Keine `unsafe`-Aliasing-Tricks im Fundament (Vertrag §2, Risiko R2 im Plan 0001).

## Optionen

1. **Parallel von Beginn an** — Systeme deklarieren Lese-/Schreibmengen, ein Executor verteilt
   konfliktfreie Systeme auf Threads, die Ergebnisse werden in fester Reihenfolge zusammengeführt.
   - (+) Kein späterer Umbau; Mehrkern-Leistung ab P1 sofort verfügbar.
   - (−) Hoher Vorab-Aufwand: Zugriffsdeklaration, Konfliktgraph, Thread-Pool, `Send`/`Sync`-Grenzen
     und vermutlich `unsafe` beim Aufteilen der Welt — genau im risikoreichsten Teil (R2).
   - (−) Determinismus schwerer zu beweisen und zu debuggen, solange die Grundlagen noch wackeln.
   - (−) Die API wird festgelegt, bevor echte Lastprofile existieren.
2. **Single-threaded für immer** — geordnete Systemliste, ein Thread, keine Migrationsvorsorge.
   - (+) Minimaler Aufwand, Determinismus trivial.
   - (−) Das Bullet-Budget könnte ab P2 einen Kern sprengen; ein Umbau träfe dann eine API ohne
     Zugriffsinformationen und damit alle Spiel-Systeme.
3. **Single-threaded jetzt, mit dokumentiertem Migrationspfad (gewählt)** — P0 bis P2 laufen
   strikt geordnet auf einem Thread. Die API wird so geschnitten, dass Zugriffsmengen und eine
   feste Zusammenführungsreihenfolge später ergänzt werden können, ohne bestehende Systeme zu brechen.
   - (+) Kleinster P0-Aufwand bei voller Determinismus-Garantie.
   - (+) Parallelisierung wird entschieden, wenn Messwerte (Profiler P1, Bullet-Benchmarks) vorliegen.
   - (−) Späterer Umbauaufwand im Scheduler bleibt bestehen, wird aber begrenzt.

## Entscheidung

Option 3. `Schedule` führt die Systeme auf dem aufrufenden Thread strikt in der Reihenfolge von
`add_system` aus, ohne implizite Umordnung. Systeme erhalten `&mut World`; verzögerte
Strukturänderungen laufen über `CommandBuffer` in Aufzeichnungsreihenfolge.

**Migrationspfad** (frühestens P2, nur mit Messbeleg und Folge-ADR):

1. **Zugriffsmengen einführen.** `System` erhält eine Methode mit Default-Implementierung, z. B.
   `fn access(&self) -> SystemAccess { SystemAccess::exclusive_world() }`. Bestehende Systeme
   bleiben damit gültig und exklusiv. Neue Systeme deklarieren Lese- und Schreibmengen über
   Registrierungsnummern (Komponenten-ID, Ressourcen-Slot), nie über `TypeId`-Reihenfolgen.
   Voraussetzung: `ComponentId` ist heute crate-intern und wird nicht re-exportiert; er (oder ein
   öffentlicher Zugriffsmengen-Typ) muss dafür erst Teil der öffentlichen API werden. Die bereits
   vorhandene Konfliktprüfung der Queries (doppelter mutabler sowie gleichzeitiger mutabler und
   lesender Zugriff) wird auf System-Ebene gehoben, und ein Debug-Modus prüft, dass Queries die
   Deklaration einhalten.
2. **Stufen statt Threads zuerst.** Der Scheduler bildet aus der festen Systemliste
   deterministische Stufen: benachbarte Systeme ohne Schreibkonflikt bilden eine Stufe. Die
   Aufteilung hängt nur von Liste und Zugriffsmengen ab und wird als Teil der Diagnose ausgegeben.
3. **Parallele Ausführung innerhalb einer Stufe.** Systeme einer Stufe dürfen parallel laufen,
   aber nur auf disjunkten Daten. Strukturänderungen landen in einem `CommandBuffer` **pro
   System**. Nach der Stufe werden die Puffer in der **Listenreihenfolge der Systeme**
   zusammengeführt und angewendet, nie in Fertigstellungsreihenfolge. Voraussetzung an
   `grimoire_sim`: Systeme ziehen Zufall ausschließlich über `derive_rng(seed, tick, stream)`
   (Vertrag §8, umgesetzt in `grimoire_sim::rng` und durch `tests/rng.rs` eingefroren) mit einem fest
   pro System vergebenen `stream`, nie über
   einen gemeinsam fortgeschalteten Generator. Nur dann sind die RNG-Ströme unabhängig von der
   Thread-Verteilung.
4. **Datenparallele Queries.** Parallele Iteration über eine Query zerlegt Archetypen und Zeilen
   in feste, von der Thread-Anzahl unabhängige Blöcke. Reduktionen (Summen, Min/Max, gesammelte
   Events) werden in Blockreihenfolge gefaltet.
5. **Beweis vor Freigabe.** Der Doppellauf-Hash-Test (WP5.6) läuft zusätzlich mit 1, 2 und N
   Threads. Alle Zustands-Hashes müssen identisch zum single-threaded Lauf sein. Ohne diesen
   Nachweis bleibt der Scheduler single-threaded.

## Konsequenzen

- (+) P0 liefert schnell einen einfachen, testbaren Scheduler; der Determinismus-Beweis hängt
  nicht an Thread-Verhalten.
- (+) Die ECS-API bleibt frei von `unsafe` beim Aufteilen der Welt, und Systeme erhalten keine
  `Send`/`Sync`-Zwänge über die bereits bestehenden hinaus: `system_fn` verlangt `Send`,
  `Component` und `Resource` verlangen `Send + Sync`, `Bundle` verlangt `Send`. Diese Grenzen sind
  damit für eine spätere Parallelisierung schon gesetzt.
- (+) Der Migrationspfad ist additiv (Default-Methode, pro-System-Puffer, feste
  Zusammenführungsreihenfolge) und bricht bestehende Spiel-Systeme nicht.
- (−) Bis zur Migration nutzt die Simulation nur einen Kern. Das Budget ab P2 muss gemessen werden
  (Timing-Test in `grimoire_ecs`, Profiler P1).
- (−) Systeme, die heute `&mut World` frei nutzen, laufen nach der Migration nur exklusiv, bis sie
  Zugriffsmengen deklarieren; Parallelgewinne erfordern also Nacharbeit an heißen Systemen.
- (−) Die Konfliktprüfung auf System-Ebene und der Mehr-Thread-Hash-Test sind zusätzliche
  Pflichtarbeit bei der Migration.
