# ADR-0008: Crate-Map-Erweiterung P1

- **Status:** Vorgeschlagen (Nummer vorläufig: 0006 ist vergeben, 0007 belegt der Vorschlag auf dem Branch `p1/wp1.4-sigil-syntax-spike`; parallele P1-Branches legen Engine-ADRs an, die endgültige Nummer steht erst beim Merge fest)
- **Datum:** 2026-09-15
- **Autor:** Claude (Ausarbeitung, unbeaufsichtigter Lauf) im Auftrag von Lupus Malus Deviant (PO)
- **Konsultiert:** — (PO-Entscheidung ausstehend; vorgesehen mit dem Vertrags-PR aus Plan 0002 WP1.2/WP1.3, spätestens in Sammelsitzung B)

## Kontext und Problemstellung

In P0 besteht die Engine aus 13 Crates (Crate-Verträge §1). Engine-ADR-0006 fügt auf dem Branch
`p1/wp1.0-scheduler` die Executor-Crate `grimoire_exec` hinzu und verschiebt ihren Namen und ihre Kanten
ausdrücklich in „das Crate-Map-ADR für P1“ (ADR-0006, Folge-Entscheidungen). P1 bringt außerdem Bausteine, für die
die P0-Karte keinen Platz hat:

- **Sigil-Compiler.** Quelltexte werden offline kompiliert, die Laufzeit lädt nur Binär-Units (Projekt-ADR-0007 im
  Spiel-Repo: Parser nicht im Laufzeitpfad). Plan 0002 WP1.5 schlägt vor, dass `grimoire_sigilc` mit der CLI
  `sigilc` die einzige Implementierung von Parser, Validator und Compiler ist (P-2, Sammelsitzung B). Die Ausgabe
  des Compilers geht in Content-Hash, Golden Master und Replays ein. Rechnet er Konstanten mit
  plattformabhängigen Funktionen vor, entstehen je Build-Maschine andere Units (Plan-Risiko R4). Die
  Crate-Beschreibung von `grimoire_sigil` nennt heute noch „parser, compiler and runtime interpreter“.
- **Bullet-Pool als ECS-Ressource.** Die Sigil-Laufzeit hält bis zu 10.000 Bullets als SoA-Ressource
  (`Clone + StableHash`, Crate-Verträge §3 und §7) und registriert Systeme gegen die Scheduler-API aus ADR-0006.
  `grimoire_sigil` hängt heute an `grimoire_sim` und `grimoire_core`, nicht an `grimoire_ecs`.
- **Benchmarks.** Die Uhr- und Thread-Sperren der Determinismus-`clippy.toml` gelten für `--all-targets`, also
  auch für `benches/` der Simulations-Crates (§3). Benchmarks brauchen die Wanduhr und laut ADR-0006
  (Tooling-Empfehlung) Läufe mit 1 und N Threads.
- **Hot-Reload ohne C#.** Der P1-Nachweis „Hot-Reload via Dev-Link“ soll auch ohne C#-Suite gelingen: Eine
  Rust-CLI `grimoire-link` kompiliert eine `.sigil`-Datei und schickt die Unit über den Debug-Link.
- **Trennungen.** `grimoire_render` soll `grimoire_sigil` nicht kennen, Sigil liefert neutrale Visual-Kennungen, und
  die Fassade bildet sie ab. `grimoire_debug` soll ohne Kanten zu `grimoire_ecs` und `grimoire_render` bleiben:
  Profiler-Anbindung, Uhrzugriff und Overlay liegen in der Fassade (Plan 0002 WP1.2).
- **C#-Suite.** Empfohlen ist `grimoire/tools/` (P-1, Sammelsitzung A), ein Verzeichnis außerhalb von Cargo, das
  das Standalone-Gate heute nicht prüft.

**Technische Lage** (Branch `p1/wp1.0-scheduler`, Kopf `f5c9bf5`, an den `Cargo.toml` der Crates geprüft):

- Normale Kanten:
  - `grimoire_sigil` → `grimoire_core`, `grimoire_sim`
  - `grimoire_collide` → `grimoire_core`, `grimoire_ecs`
  - `grimoire_assets` und `grimoire_debug` → `grimoire_platform`
  - Fassade → `core`, `ecs`, `platform`, `render`, `sim`
  - `grimoire_exec` → `grimoire_ecs`, `rayon`, `thiserror`; Dev-Kanten auf `grimoire`, `grimoire_core`, `grimoire_sim`, `proptest`
- Keine Crate der Determinismus-Menge hat eine Dev-Kante auf eine Engine-Crate.
- Sechs identische `clippy.toml` liegen in `core`, `ecs`, `sim`, `collide`, `sigil` und der Fassade.
- `.github/scripts/check-thread-source.sh` prüft zweierlei:
  - Keine dieser Crates hängt von rayon, rayon-core oder `grimoire_exec` ab (normal, Build und Dev, alle Targets).
  - Die Dateien sind identisch.
- Das Standalone-Gate durchsucht `Cargo.toml` und `crates/**`.
- Cargo verhindert Zyklen normaler Kanten, aber keine Schichtverletzung wie `grimoire_render → grimoire_sigil`.
  PRD-0002 FR-03 verlangt eine erzwungene Layer-Regel.

**Kernfrage:** Welche Crates und Kanten kommen in P1 hinzu, damit Compiler, Benchmarks und Werkzeuge weder den
Laufzeitpfad noch die Determinismus-Menge verunreinigen — und wie wird die Karte maschinell geprüft?

## Anforderungen

### Funktional

- Der Sigil-Compiler ist als Bibliothek (für `grimoire-link` und Tests) und als CLI `sigilc` nutzbar; keine Laufzeit-Crate hängt von ihm ab.
- Die Sigil-Laufzeit hält ihren Pool als ECS-Ressource und schreibt Systeme gegen die Scheduler-API aus ADR-0006.
- Benchmarks messen Wanduhrzeit mit 1 und N Threads und schreiben JSON, ohne Ausnahmen in Determinismus-Crates.
- Eine Rust-CLI kompiliert eine `.sigil`-Datei und schickt die Unit über den Debug-Link an eine laufende Engine.
- `grimoire_render` und `grimoire_sigil` kennen einander nicht; `grimoire_debug` kennt weder ECS noch Render.
- Name und Kanten der Executor-Crate aus ADR-0006 sind festgehalten, einschließlich ihrer Dev-Kanten.
- Jede erlaubte Kante steht in einer Tabelle und ist in der CI prüfbar.

### Nicht-Funktional

- Standalone: Der Workspace baut und testet ohne Spiel (PRD-0002 FR-01), auch wenn `tools/` hinzukommt.
- Determinismus: Binär-Units sind auf Windows, Linux und macOS byte-identisch (ADR-0004); keine Determinismus-Crate hängt von rayon ab (ADR-0006).
- Die P0-API bleibt unverändert; neue Crates erscheinen in der Fassade nur additiv.
- CI-Laufzeit: Die Standard-Push-CI bleibt unter 15 Minuten je Plattform (PRD-0017 NFR, Plan 0002 OP-5); GitHub-Actions-Minuten sind knapp (OP-2).
- So wenige neue Crates wie möglich, jede mit genau einer Rolle.
- Laufzeit-Crates bleiben ab P4 für Mobile-Targets kompilierbar; Werkzeug-Crates müssen das nicht.

## Betrachtete Optionen

### Option 1: Werkzeuge in bestehenden Crates

Der Compiler liegt hinter einem Feature `compiler` in `grimoire_sigil`, Benchmarks liegen als `benches/` in den
jeweiligen Crates, `grimoire-link` ist ein Binary von `grimoire_debug`.

**Positiv:**
- Keine neuen Crates, kürzere Workspace-Liste.
- Compiler und Decoder teilen Typen ohne zusätzliche Kante.

**Negativ:**
- Parser und Compiler lägen im Laufzeit-Crate, getrennt nur durch ein Feature. Feature-Unifikation im Workspace zieht sie in Tests und Werkzeugläufen mit; der Grundsatz „Parser nicht im Laufzeitpfad“ wird verwässert.
- Benches in Determinismus-Crates bräuchten für jede Zeitmessung `#[allow(clippy::disallowed_methods)]`. Ein Bench mit N Threads bräuchte eine Dev-Kante auf `grimoire_exec`, die ADR-0006 (Baustein 6, strenge Auslegung) verbietet.
- `grimoire-link` in `grimoire_debug` bräuchte `grimoire_debug → grimoire_sigil` zum Kompilieren, genau die Kante, die der Profiler-Schnitt vermeiden soll.

### Option 2: Eigene Werkzeug-Crates, Compiler in der Determinismus-Menge

Neue Crates:
- `grimoire_sigilc` (→ `grimoire_sigil`, `grimoire_sim`, `grimoire_ecs`, `grimoire_core`; mit identischer `clippy.toml`)
- `grimoire_bench` (außerhalb der Determinismus-Menge, ohne `clippy.toml`)
- `grimoire_link` (außerhalb; → `grimoire_debug`, `grimoire_sigilc`)

Dazu kommen die Laufzeitkante `grimoire_sigil → grimoire_ecs` und `grimoire_exec` wie auf dem Branch. Das ist der
Schnitt aus Plan 0002 WP1.3.

**Positiv:**
- Der Laufzeitpfad bleibt frei von Parser und Compiler; keine Laufzeit-Crate hängt an einem Werkzeug.
- Die Determinismus-Lints gelten für den Compiler: std-Trigonometrie, `HashMap`-Reihenfolgen und eigene Threads fallen schon im lokalen Clippy-Lauf auf, nicht erst im 3-OS-Identitätsvergleich der Units (WP4.4), der Minuten kostet.
- Benchmarks dürfen Uhr und Threads nutzen, ohne Ausnahmen in Simulations-Crates.
- `grimoire_debug` bleibt schlank; `grimoire-link` bündelt Compiler und Transport außerhalb der Engine-Laufzeit.

**Negativ:**
- Drei neue Crates bedeuten längere Builds und mehr CI-Zeit (OP-5).
- Der Compiler unterliegt allen Sperren: kein paralleles Kompilieren, keine `HashMap` im Parser.
- Statt sechs gibt es sieben identische `clippy.toml`; ihr Kopfkommentar ändert sich in allen gleichzeitig.
- Neutrale Visual- und Palettenraum-Kennungen existieren in Sigil und Render als eigene Typen (`BulletVisual` in Sigil, Felder und Konstanten von `BulletInstance` in Render) und werden in der Fassade abgebildet.

### Option 3: Eigene Werkzeug-Crates, Compiler außerhalb der Determinismus-Menge

Wie Option 2, aber `grimoire_sigilc` ohne `clippy.toml`. Die Plattformidentität der Units sichern allein das
Identitäts-Gate (WP4.4) und goldene Unit-Hashes.

**Positiv:**
- Freie Wahl von Containern und paralleler Kompilierung im Compiler.
- Eine `clippy.toml` weniger.

**Negativ:**
- Plattformabhängige Konstanten (etwa `f32::sin` beim Umrechnen von Grad) fallen erst im 3-OS-Vergleich auf; bis dahin entstehen je Build-Maschine andere Units und Content-Hashes (R4).
- Das Gate kostet je Lauf Minuten auf drei Betriebssystemen (OP-2); lokal auf einem System bleibt der Fehler unsichtbar.
- Widerspricht dem Plan-Umfang zu PRD-0018 FR-09 („Determinismus-Lints auch für `sigilc`“).

### Option 4: Wie Option 2, plus gemeinsame Format-Crate

Eine zusätzliche Crate `grimoire_formats` hält Unit-, Pack-, Replay- und Nachrichtentypen sowie neutrale Visual- und
Palettenraum-Kennungen. Sigil, Render, Assets, Debug und die Werkzeuge hängen daran.

**Positiv:**
- Eine Typquelle für Formate und Kennungen, keine Abbildung in der Fassade.
- Natürlicher Zielort für generierten Schema-Code (Projekt-ADR-0011, OF-16.2).

**Negativ:**
- Eine Hub-Crate, die fast alles berührt: Jede Formatänderung baut die halbe Engine neu und betrifft Crates mit unterschiedlichem Determinismus-Status.
- Nimmt die Codegen-Entscheidung aus Sammelsitzung B vorweg.
- `grimoire_render` bekäme über die gemeinsamen Kennungen eine indirekte Kopplung an Sigil-Konzepte.
- Kein gemessener Nutzen in P1.

## Vorschlag des Autors

**Option 2** (vorläufig, die Bestätigung durch den PO steht aus).

Option 2 erfüllt die harten Anforderungen durch Konstruktion statt durch Disziplin. Der Laufzeitpfad kann den
Compiler gar nicht erreichen. Die Determinismus-Lints wirken dort, wo plattformabhängige Units entstehen würden, und
finden die Fehler lokal und ohne CI-Minuten. Benchmarks bekommen Uhr und Threads, ohne Ausnahmen in
Simulations-Crates zu streuen. Die Nachteile — drei Crates mehr, ein sequentieller Compiler, eine siebte
`clippy.toml` — sind Aufwand, kein Risiko für Determinismus oder Standalone-Garantie.

Option 1 scheitert an zwei Kanten, die dieses ADR gerade verhindern soll (`debug → sigil`, Dev-Kante auf
`grimoire_exec`). Option 3 verlagert einen lokal findbaren Fehler in ein teures 3-OS-Gate. Option 4 kann sinnvoll
werden, sobald ADR-0011 den Schema-Codegen entschieden hat; heute nähme sie diese Entscheidung vorweg.

## Entscheidung

**Gewählte Option:** offen — Vorschlag des Autors: Option 2; Entscheidung durch den PO mit dem Vertrags-PR aus
WP1.2/WP1.3, spätestens in Sammelsitzung B (Plan 0002)

Bausteine des Vorschlags (Namen sind Arbeitsnamen; die verbindliche Kantentabelle steht in Crate-Verträge §1):

1. **Laufzeitkanten.**
   - Neu ist `grimoire_sigil → grimoire_ecs`; die Kanten zu `grimoire_sim` (Tick, Zufallsströme) und `grimoire_core` bleiben.
   - `grimoire_collide` bleibt bei `grimoire_ecs` und `grimoire_core`.
   - Die Fassade hängt in P1 normal an `grimoire_collide`, `grimoire_sigil`, `grimoire_assets` und `grimoire_debug`. `grimoire_collide` und `grimoire_sigil` kommen schon an M1 mit den WP1.3-Skeletten hinzu, weil der Spieler-Proxy ihre Typen braucht (vorläufig, Crate-Verträge §9.1).
   - `grimoire_audio` und `grimoire_ui` bleiben Platzhalter ohne Kante bis zu einem P2-Crate-Map-ADR.
2. **`grimoire_sigilc`.** Die neue Crate (Bibliothek und Binary `sigilc`) hängt an `grimoire_sigil`, `grimoire_sim`, `grimoire_ecs` und `grimoire_core`, gehört zur Determinismus-Menge und trägt die identische `clippy.toml` (sieben Dateien). Keine Laufzeit-Crate hängt von ihr ab, auch nicht als Dev-Abhängigkeit; Laufzeit-Tests nutzen eingecheckte Unit-Fixtures. `UnitId`s leitet sie nach der Regel von `AssetId::from_path` über `StableHasher` ab, ohne Kante zu `grimoire_assets`. Die Kanten zu `grimoire_sim` und `grimoire_ecs` braucht `sigilc simulate` (Plan 0002 WP5.6): Es führt den Laufzeit-Interpreter über `grimoire_sigil::install` auf einer `Simulation` aus (vorläufig, PO-Bestätigung ausstehend). Beide Crates gehören zur Determinismus-Menge; die Regel „`sigilc` hängt nur an Crates dieser Menge“ bleibt gewahrt.
3. **`grimoire_bench`.** Die neue Crate liegt außerhalb der Determinismus-Menge und hat keine `clippy.toml`. Sie darf von jeder Laufzeit-Crate und von `grimoire_exec` abhängen; keine Engine-Crate hängt von ihr ab. Ihre Bibliothek enthält die JSON-Schema-Typen für Bench-Ergebnisse und Golden Master (Crate-Verträge §15). Wanduhrwerte gelangen nie in Zustands- oder Subsystem-Hashes.
4. **`grimoire_link`.** Die neue Crate (Binary `grimoire-link`) liegt außerhalb der Determinismus-Menge. Sie hängt an `grimoire_debug` mit Feature `tcp` und an `grimoire_sigilc` und darf weitere Laufzeit-Crates nutzen; nichts hängt von ihr ab. In P1 ist sie kein Release-Artefakt (P-14). Die feste `tcp`-Kante vereinigt sich in jeden `--workspace`-Lauf; die Auslieferungskonfiguration (Fassade ohne `debug-link`, `grimoire_debug` ohne `tcp`) prüft deshalb ein paketgewählter CI-Schritt (Crate-Verträge §2 Regel 14).
5. **`grimoire_exec`** wird festgehalten, wie ADR-0006 und der Branch es umsetzen:
   - normale Kante zu `grimoire_ecs`, Drittcrates `rayon` und `thiserror`
   - Dev-Kanten zu `grimoire`, `grimoire_sim` und `grimoire_core` für das Hash-Gate
   - außerhalb der Determinismus-Menge; keine Crate dieser Menge hängt von ihr ab, bei keiner Kantenart
6. **Trennungen.**
   - `grimoire_render` und `grimoire_sigil` kennen einander nicht; neutrale Kennungen sind je Crate eigene Typen, die Fassade bildet sie ab.
   - `grimoire_debug` hat keine Kante zu Simulations- oder Render-Crates.
   - `grimoire_assets` kennt `grimoire_sigil` nicht.
   - `grimoire_core` nimmt keine Format-, Render- oder Sigil-Typen auf (Scope von ADR-0005).
7. **Kantenarten.**
   - Die Richtung „nur nach unten“ gilt für normale und Build-Kanten.
   - Dev-Kanten von Crates außerhalb der Determinismus-Menge dürfen für Gate-Tests nach oben zeigen.
   - Dev-Kanten von Crates der Determinismus-Menge zeigen nur auf Engine-Crates dieser Menge.
   - Kanten zu `grimoire_core` sind jeder Crate erlaubt.
   - Zwischen Werkzeug-Crates gibt es nur `grimoire_link → grimoire_sigilc`.
8. **Außerhalb von Cargo.** `tools/` (P-1, vorläufig) ist kein Workspace-Mitglied und hat keine Cargo-Kante. Das Standalone-Gate prüft zusätzlich `tools/**`.
9. **Prüfung.** Ein CI-Kanten-Check vergleicht die Kanten jedes Workspace-Mitglieds (normal, Build, Dev) mit einer eingecheckten Positivliste, die §1 entspricht; eine Positivkontrolle stellt sicher, dass die Abfrage greift. Die Identitätsprüfung der `clippy.toml` gilt für sieben Dateien. Die Umsetzung kommt mit den Skeletten in WP1.3.

Determinismus-Menge nach diesem Vorschlag: `grimoire_core`, `grimoire_ecs`, `grimoire_sim`, `grimoire_collide`,
`grimoire_sigil`, `grimoire_sigilc`, Fassade `grimoire`.

## Konsequenzen

Die folgenden Punkte beschreiben die Folgen bei Annahme des Vorschlags (Option 2).

### Positiv

- Parser und Compiler sind strukturell aus dem Laufzeitpfad ausgeschlossen; eine versehentliche Kante scheitert am Kanten-Check.
- Plattformabhängige Operationen im Compiler fallen lokal im Clippy-Lauf auf, bevor Units in Content-Hashes, Replays oder Golden Master gelangen.
- Benchmarks messen mit Uhr und 1 bzw. N Threads, ohne dass Simulations-Crates Ausnahmen tragen.
- `grimoire-link` belegt Hot-Reload ohne C#-Suite und ohne neue Kanten in der Engine-Laufzeit.
- Die Layer-Regel aus PRD-0002 FR-03 wird maschinell geprüft, nicht nur durch Cargo-Zyklenfreiheit.
- Die Executor-Crate aus ADR-0006 ist vollständig eingeordnet; die strenge Auslegung von Baustein 6 bleibt prüfbar.

### Negativ

- Drei zusätzliche Crates verlängern Builds und die Engine-CI (OP-5), während GitHub-Actions-Minuten knapp sind (OP-2).
- `sigilc` kompiliert sequentiell und ohne `HashMap`; große Content-Mengen brauchen mehr Zeit.
- Visual- und Palettenraum-Kennungen existieren doppelt und werden in der Fassade abgebildet; eine Abweichung der Wertebereiche fällt nur durch Tests auf.
- Der Kopfkommentar aller sieben `clippy.toml`, die Kommentare in `check-thread-source.sh` und die Crate-Beschreibung von `grimoire_sigil` müssen angepasst werden.
- Die Architektur-Übersicht in PRD-0002 und die Codebase-Map in PRD-0000 §4 (Spiel-Repo) sind danach veraltet (Sigil-Parser in der Laufzeit-Crate, keine Werkzeug-Crates).
- Werkzeug-Crates dürfen weitere Laufzeit-Crates ohne ADR nutzen. Das ist bewusst locker; eine Kante in Gegenrichtung bleibt aber verboten.

### Folge-Entscheidungen

- **P-2 / Projekt-ADR-0010** (Compiler-Hoheit): Bei Ablehnung gäbe es einen zweiten Unit-Produzenten außerhalb der Determinismus-Menge; dieses ADR müsste dann festlegen, wie dessen Units gegen `sigilc` geprüft werden (R20).
- **Format-Crate (Option 4)** nach der Entscheidung zu Projekt-ADR-0011 (Schema-Codegen) neu bewerten.
- **Ort der JSON-Schema-Typen:** Bibliothek von `grimoire_bench` (Crate-Verträge §15) oder eine eigene Crate, falls Anwendungs-Harnesses nicht von einer Werkzeug-Crate abhängen sollen.
- **Client-Transport von `grimoire-link`** im Engine-ADR „Debug-Link v1“ (Crate-Verträge §13 legt nur den Server-Transport fest).
- **Kanten von `grimoire_audio` und `grimoire_ui`** in einem P2-Crate-Map-ADR.
- **CI-Aufnahme** der Werkzeug-Crates in die Standard-Matrix oder in eigene Jobs nach dem ersten Lauf (OP-5).
- **Veröffentlichung** von `sigilc`- und `grimoire-link`-Binaries frühestens mit der Distribution in P3 (P-14).
- **Ausweitung des Standalone-Gates** auf `tools/**` (P-1) und gegebenenfalls auf `docs/formats/**` (P-9 Option B).
- **Nachziehen** der Crate-Beschreibung von `grimoire_sigil` (WP1.3) und der PRD-Stellen im Spiel-Repo (WP11.6).

### Review

**Reality-Check geplant für:** nach WP1.3, sobald die Skelette kompilieren und der Kanten-Check grün ist; erneut
nach dem ersten CI-Lauf mit `grimoire_sigilc` und `grimoire_bench` (OP-5); spätestens an M1

## Weitere Informationen

### Scope

Gilt für alle Cargo-Kanten des Engine-Workspaces in P1 (normal, Build, Dev), für die Zugehörigkeit zur
Determinismus-Menge und für das Verhältnis von `tools/` zum Workspace. Nicht erfasst sind:

- die öffentlichen APIs der Crates (Crate-Verträge §2a ff.),
- Binär- und Nachrichtenformate (`docs/formats/`),
- die innere Struktur der C#-Solution,
- Drittabhängigkeiten (Crate-Verträge §2 Regel 4),
- Kanten von `grimoire_audio` und `grimoire_ui` (P2).

### Tooling-Empfehlung

- Kanten-Check als Skript neben `check-thread-source.sh`:
  - `cargo metadata --format-version 1 --locked` liefert je Workspace-Mitglied die Engine-Abhängigkeiten mit Kantenart.
  - Der Vergleich läuft gegen eine eingecheckte Positivliste; jede Abweichung wird mit Crate, Ziel und Kantenart als `::error::` gemeldet.
  - Eine Positivkontrolle (eine bekannte Kante muss gefunden werden) verhindert ein stilles Bestehen.
- Die Identitätsprüfung der `clippy.toml` meldet zusätzlich die erwartete Anzahl (sieben), damit eine fehlende Datei in einer neuen Determinismus-Crate auffällt.
- Standalone-Gate um `tools/**` erweitern, sobald `tools/` existiert. Dieses Repo soll keine Namen oder Crates eines Spiels enthalten; die Gate-Suche darf deshalb nicht auf Dokumente ausgeweitet werden, die das Suchmuster selbst erklären.
- Ein Test in `grimoire_sigilc` baut die eingecheckten Unit-Fixtures neu und vergleicht sie byteweise, damit Laufzeit-Tests ohne Dev-Kante auf den Compiler aktuell bleiben.

### Referenzen

- [ADR-0004](0004-deterministische-gleitkommaarithmetik.md) — `f32`-Regeln, `dmath`; Grund für den Compiler in der Determinismus-Menge
- [ADR-0005](0005-grimoire-core-als-blatt-crate.md) — `grimoire_core` als Blatt-Crate; Scope-Grenze für gemeinsame Typen
- [ADR-0006](0006-paralleler-scheduler-deterministische-zusammenfuehrung.md) — Executor-Crate, strenge Abhängigkeitsregel, Scope ohne Werkzeuge und Asset-Laden
- ADR-0007 (Vorschlag, Branch `p1/wp1.4-sigil-syntax-spike`) — Sigil-Quelltextsyntax v1, Umsetzung in `grimoire_sigilc`
- [Crate-Verträge](../architektur/crate-vertraege.md) §1, §2 (Regeln 12–15), §2a, §3, §15
- Spiel-Repo: PRD-0002 (FR-01, FR-02, FR-03, FR-15), PRD-0004, PRD-0016 (FR-10), PRD-0017 (NFR), PRD-0018 (FR-09); Plan 0002 (WP1.2, WP1.3, WP1.5, WP4.3, WP4.4, WP6.2, WP8.5, OP-2, OP-5, R4, R20, P-1, P-2, P-14); Projekt-ADR-0007 (Offline-Kompilierung)
- `.github/scripts/check-thread-source.sh`, `.github/workflows/ci.yml` (Standalone-Gate, Job `docs`)
