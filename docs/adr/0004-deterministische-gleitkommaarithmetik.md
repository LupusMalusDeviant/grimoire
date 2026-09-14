# ADR-0004: Deterministische Gleitkommaarithmetik (f32 mit Regeln und libm)

- **Status:** Vorgeschlagen - wird nach grünem CI-Lauf der Golden-Tests auf Windows, Linux und macOS akzeptiert
- **Datum:** 2026-09-14
- **Entscheider:** Lupus Malus Deviant (PO), vorbereitet durch Claude
- **Bezug:** Spiel-Repo PRD-0002 (OF-2.1), PRD-0018 (FR-04, FR-09), ADR-0005, Plan P0 WP6.1;
  [Crate-Verträge](../architektur/crate-vertraege.md) Abschnitte 3 und 4

## Kontext

ADR-0005 macht Determinismus zur Engine-Garantie: pro Plattform unverhandelbar, plattformübergreifend
(Windows x86_64, Linux x86_64, macOS arm64) das Ziel. Offen war OF-2.1: Reicht `f32` mit Regeln für
bitgleiche Ergebnisse auf allen drei Plattformen, oder braucht die Simulation Festkomma?

Ausgangslage aus der Rust-Float-Semantik (RFC 3514) und dem Code:

- Die IEEE-754-Grundoperationen (`+ - * / %`, `sqrt`, Runden, Casts) sind korrekt gerundet bzw. exakt
  definiert. Rust kontrahiert nie implizit zu FMA und kennt kein Fast-Math; x86_64 rechnet mit SSE2
  (kein x87), aarch64 mit NEON.
- Nicht festgelegt sind Vorzeichen und Payload erzeugter NaN, das Vorzeichen einer Null aus
  `f32::min`/`max` bei `(+0.0, -0.0)` und die Genauigkeit von `powi`.
- `f32::sin` & Co. rufen die C-Mathematikbibliothek der Plattform (UCRT, glibc, libSystem), deren
  Ergebnisse nicht korrekt gerundet und nicht plattformgleich sind.
- `grimoire_core::math::dmath` nutzt die Crate `libm` 0.2.16 (reines Rust). Laut Quelltext wählt sie
  auf x86_64 (SSE2) und aarch64 (NEON) nur für `sqrt`/`fma`/`rint` Hardwarebefehle; `sqrt` ist
  korrekt gerundet, `fma` wird nur von `cbrt` verwendet, das `dmath` nicht anbietet. Alle übrigen
  `dmath`-Funktionen laufen auf beiden Architekturen durch denselben generischen Rust-Code.

### Spike: Messungen

Werkzeuge im Engine-Repo: `crates/grimoire_core/tests/float_determinism.rs` (Eingaben aus einem
ganzzahligen LCG, Floats nur über `f32::from_bits`, je Arbeitslast mindestens 100.000 Iterationen) und
`crates/grimoire_core/tests/stable_hash_golden.rs` (friert den Hash-Algorithmus v1 ein). Die Probe
schreibt nach `<target>/float-probe/`: `core-`, `std-trig-`, `detail-` und `special-<os>-<arch>.txt`.
`<target>` ist das Elternverzeichnis von `CARGO_TARGET_TMPDIR` (berücksichtigt `CARGO_TARGET_DIR`),
ersatzweise `CARGO_MANIFEST_DIR/../../target`.

Gemessen am 2026-09-14 auf Windows 11 x86_64 (MSVC, rustc 1.98.1) und Linux x86_64 (WSL2 Ubuntu,
glibc, rustc 1.95.0; eigenständige Kopie von `grimoire_core` mit denselben `libm`-Quellen). macOS arm64
ist lokal nicht verfügbar und wird erst durch CI gemessen.

**Asserted Golden-Hashes** (identisch in allen vier Läufen: Windows/Linux × Debug/Release):

| Arbeitslast | Hash |
|-------------|------|
| `basic_hash` — Grundoperationen, `%`, Runden, Casts, `Vec2` inkl. `normalize_or_zero`/`lerp` | `0xd5965d26f7f427f2` |
| `dmath_hash` — alle `dmath`-Funktionen, Domänen plus 28 Randwerte | `0x6109792b432233ee` |
| `mini_sim_hash` — 256 Partikel × 400 Ticks mit `rotate`, `exp`, `atan2`, `sin`, `hypot` | `0xcf8c7a4958150c94` |
| `stable_hash_golden` — 62 Werte Hash-Algorithmus v1 | alle grün |

**`std`-Transzendentalfunktionen** (nur aufgezeichnet): `std_trig_hash` Windows `0xc362e1c66a97709b`
(Debug = Release); Linux `0x4e6cc5be56a3538f` (Debug) bzw. `0x4ae997d8f79db1d9` (Release). Auf Linux
weicht also schon `f32::tan` zwischen Debug und Release ab. Abweichungen gegenüber `dmath` bei
denselben Eingaben:

| Funktion | Windows (UCRT) | Linux (glibc) |
|----------|----------------|---------------|
| `sin` | 62 / 100.028 | 671 / 100.028 |
| `cos` | 81 / 100.028 | 502 / 100.028 |
| `tan` | 8.600 / 100.028 | 9.531 / 100.028 (Release 9.529) |
| `atan2` | 8.683 / 100.784 | 4.565 / 100.784 |
| `exp` | 4.093 / 100.028 | 4.094 / 100.028 |
| `ln` | 1.649 / 100.028 | 1.681 / 100.028 |
| `powf` | 6.771 / 100.784 | 6.754 / 100.784 |
| `hypot` | 4.587 / 100.784 | 4.587 / 100.784 |

**NaN-Bitmuster** (Windows und Linux identisch, Debug und Release identisch):

| Erzeugung | Bits |
|-----------|------|
| `0.0 / 0.0`, `inf - inf`, `sqrt(-1.0)`, `0.0 * inf` zur Laufzeit | `0xffc00000` |
| `dmath::ln(-1.0)`, `dmath::asin(2.0)`, `dmath::sqrt(-1.0)` | `0xffc00000` |
| `-(0.0 / 0.0)` zur Laufzeit | `0x7fc00000` |
| `const X: f32 = 0.0 / 0.0` (Konstantenfaltung), `f32::NAN` | `0x7fc00000` |
| `f64`: `0.0 / 0.0`, `sqrt(-1.0)` | `0xfff8000000000000` |

x86_64 liefert zur Laufzeit das negative Default-NaN, die Konstantenfaltung dagegen das positive, also
schon auf derselben Maschine zwei Muster. ARMv8 verwendet laut Architektur das positive Default-NaN
`0x7fc00000`; die CI-Datei `special-macos-aarch64.txt` bestätigt das.

**`min`/`max` mit `(+0.0, -0.0)`** (`min(+0,-0)`, `min(-0,+0)`, `max(+0,-0)`, `max(-0,+0)`):
Windows/1.98.1 Debug `-0 +0 -0 +0`, Release `-0 -0 -0 -0`; Linux/1.95.0 Debug `+0 -0 +0 -0`, Release
`+0 +0 +0 +0`. Das Ergebnis hängt also von Optimierungsstufe und Compiler-Version ab. Die asserted
Arbeitslast speist gleiche Operanden deshalb vorzeichenfrei ein.

**Lint-Probe** (temporäre Crate `crates/lintprobe`, danach vollständig gelöscht und nie committet;
`cargo clippy -p lintprobe -- -D warnings` mit clippy 1.98.1):

- Alle 16 ursprünglichen Einträge lösen auf. `f32::sin` usw. werden als Methodenaufruf (`x.sin()`),
  als Pfadaufruf (`f32::sin(x)`) und als Funktionszeiger (`let f: fn(f32) -> f32 = f32::sin`)
  gemeldet; `HashMap`/`HashSet` sowohl über `std::collections` als auch über
  `std::collections::hash_map::HashMap`.
- Ein absichtlich falscher Eintrag `f32::not_a_real_method` erzeugte **keine** Diagnose. Clippy
  ignoriert nicht auflösbare Pfade stillschweigend, ein Tippfehler schaltet eine Regel also unbemerkt
  ab.
- Lücken der ursprünglichen Konfiguration: `sinh cosh tanh asinh acosh atanh sin_cos exp2 exp_m1 ln_1p
  log log2 log10 cbrt powi` für `f32`, sämtliche `f64`-Pendants, `f64::mul_add`,
  `Instant::elapsed`/`SystemTime::elapsed` und `std::thread::spawn`. Nach der Ergänzung meldete die
  Probe 58 Fundstellen und flaggte jede darin verwendete Lücke: alle 15 `f32`-Ergänzungen,
  `f64::sin`, `f64::powf`, `f64::mul_add`, `Instant::elapsed` und `thread::spawn`. Die übrigen
  `f64`-Einträge und `SystemTime::elapsed` folgen demselben Pfadschema, wurden aber nicht einzeln
  aufgerufen.

**Laufzeit:** Die Probe-Testdatei läuft in 0,16 s (Windows Debug), 0,04 s (Windows Release) und
0,13 s (Linux Debug). Die gesamte Testsuite von `grimoire_core` braucht inkrementell 1,5 s.

## Anforderungen

- Gleicher Seed und gleiche InputFrames ergeben pro Plattform garantiert denselben Zustands-Hash,
  unabhängig von Debug/Release.
- Plattformübergreifend bitgleiche Hashes auf Windows x86_64, Linux x86_64 und macOS arm64 (Ziel).
- Maschinell geprüft: Golden-Tests in CI auf allen drei Plattformen, Probe-Dateien als Artefakt,
  Determinismus-Lint (PRD-0018 FR-09).
- Gameplay-, Kollisions- und Sigil-Code bleibt mit gewöhnlichem `f32`/`Vec2` schreibbar.
- Ein späterer Wechsel auf Festkomma bleibt lokal (Plan-Risiko R1, Typalias `SimVec`).

## Optionen

1. **`f32` mit Regeln und `libm` (vorgeschlagen)**
   - (+) Grundoperationen sind per IEEE-754 bitgleich; die Messung bestätigt das für x86_64 unter
     Windows und Linux bei verschiedenen Compiler-Versionen und Optimierungsstufen.
   - (+) Transzendente Funktionen laufen als reiner Rust-Code (`libm`), identisch auf x86_64 und aarch64.
   - (+) Normale Ergonomie; `Vec2`, Kollision und Sigil bleiben einfach; kein Umbau.
   - (−) Die Regeln sind nur teils lint-bar (NaN, Nullvorzeichen aus `min`/`max`, Payload-Inspektion)
     und brauchen Review.
   - (−) `libm` ist langsamer als Plattform-libm und muss bei Upgrades gegen die Golden-Hashes
     geprüft werden.
2. **Festkomma für Simulationspositionen** (z. B. `i32` 16.16 oder `i64` 32.32)
   - (+) Bitgleichheit folgt aus Ganzzahlarithmetik, unabhängig von FPU, NaN und Compiler.
   - (−) Eigene Trigonometrie (Tabellen/CORDIC), Sqrt, Überlaufbehandlung; Reichweite und Präzision
     müssen pro Größe abgewogen werden.
   - (−) Kollision, Sigil-Interpreter und Physik werden aufwendiger; Umrechnung an der Render-Grenze.
   - (−) Keine passende Abhängigkeit im Workspace; der gesamte Mathe-Unterbau wäre Eigenbau.
3. **Determinismus nur pro Plattform**
   - (+) Keine Einschränkungen über das Pro-Plattform-Maß hinaus; `std`-Mathe wäre erlaubt.
   - (−) Golden Master und Replays pro Plattform dreifach pflegen; plattformübergreifende Replays und
     Lockstep-Crossplay fallen weg.
   - (−) Selbst pro Plattform unsicher: Die Messung zeigt `f32::tan` auf Linux mit unterschiedlichen
     Ergebnissen in Debug und Release. Ein Verbot der `std`-Funktionen wäre also trotzdem nötig.

## Entscheidung

Vorgeschlagen ist **Option 1**. Die Simulationsseite (`core`, `ecs`, `sim`, `collide`, `sigil`)
rechnet mit `f32` nach diesen Regeln:

1. **Erlaubt:** `+ - * / %`, Negation, `sqrt`, `abs`, `floor`, `ceil`, `round`, `trunc`, `clamp`,
   `min`/`max` (mit Regel 5), Casts zwischen `f32` und Ganzzahlen.
2. **Transzendente Funktionen nur über `grimoire_core::math::dmath`.** Fehlt eine Funktion, wird sie
   dort auf Basis von `libm` ergänzt und von der Probe abgedeckt. `std`-Transzendentalfunktionen für
   `f32` und `f64` sind per `clippy.toml` gesperrt.
3. **Kein `mul_add`, kein `powi`** (gesperrt).
4. **NaN ist ein Fehler im Simulationszustand.** Code verzweigt nie auf NaN-Vorzeichen oder -Payload
   (`to_bits`, `total_cmp`, `is_sign_negative`, `copysign` auf möglichen NaN).
   `StableHasher::write_f32`/`write_f64` **kanonisieren NaN** vor dem Hashen (`0x7fc00000` bzw.
   `0x7ff8000000000000`). Begründung:
   - Rein bitgenaues Hashen würde Hashes schon auf einer Maschine von der Konstantenfaltung abhängig
     machen (gemessen: `0xffc00000` gegen `0x7fc00000`), zwischen x86 und ARM ohnehin.
   - Die Payload ist für Programme ohne Bit-Inspektion (Regel 4) unbeobachtbar. Ein Unterschied
     darin wäre ein falscher Alarm im Golden Master, keine echte Divergenz.
   - Divergenzen zwischen NaN und Zahl sowie zwischen `-0.0` und `+0.0` bleiben erkennbar, weil nur
     NaN kanonisiert wird.
   - Algorithmus-Version 1 ist mit dieser Regel definiert; vorher existierten keine Golden-Hashes.
     Die reine Verbotsvariante (bitgenau hashen, NaN verbieten) wurde verworfen, weil ein einziges
     durchgerutschtes NaN plattformabhängige Hashes ohne Verhaltensunterschied erzeugt hätte.
5. **Das Vorzeichen einer Null aus `min`/`max` bei gleichen Operanden darf nichts beeinflussen**
   (gemessen: abhängig von Optimierungsstufe und Compiler). Nicht lint-bar, Review-Regel.
6. **Keine Threads, keine Wanduhr** (gesperrt, wie in ADR-0005).
7. **`clippy.toml` ist in allen fünf Crates identisch.** Jeder neue Eintrag wird mit einer temporären
   Lint-Probe verifiziert, weil clippy nicht auflösbare Pfade stillschweigend ignoriert. Pfade für
   inhärente Float-Methoden haben die Form `f32::name`.

**Annahmekriterium:** `cargo test -p grimoire_core` ist in CI auf Windows x86_64, Linux x86_64 und
macOS arm64 mit denselben Konstanten grün, und die hochgeladenen `core-*.txt` sind identisch.
Weicht macOS arm64 ab, wird zuerst per `detail-*.txt` die betroffene Funktion bestimmt. Liegt die
Ursache in `libm` (Arch-Pfade), wird `libm` mit `force-soft-floats` oder eigener Implementierung
geprüft. Liegt sie in den Grundoperationen, entscheidet ein Folge-ADR zwischen Option 2 für
Positionen (über `SimVec`) und Option 3.

## Konsequenzen

- (+) Replays, Golden Master und Snapshots können plattformübergreifend verglichen werden, sobald CI
  macOS arm64 bestätigt.
- (+) Die Determinismus-Regeln sind größtenteils maschinell erzwungen. Die Lint-Probe zeigte, dass die
  ursprüngliche Konfiguration Lücken hatte (`powi`, `f64`, Hyperbelfunktionen, Threads); sie sind
  geschlossen.
- (+) Die Probe liefert bei einem Bruch pro Funktion einen Hash (`detail-*.txt`) und damit die
  Ursache.
- (−) Regeln 4 und 5 bleiben Review-Aufgaben. Eine Laufzeitprüfung auf NaN im Simulationszustand
  (z. B. Debug-Assertion in `Simulation::step`) ist nicht Teil dieses ADR.
- (−) Nicht auflösbare `clippy.toml`-Pfade bleiben still. Die Lint-Probe wird bisher nur manuell
  ausgeführt; ein dauerhafter CI-Check bräuchte eine Fixture außerhalb des Workspace.
- (−) Upgrades von `libm` und Toolchain ändern potenziell `dmath_hash`. Sie sind bewusste Commits,
  die die Golden-Tests auf allen drei Plattformen bestehen müssen.
- (−) CI muss `target/float-probe` hochladen und die `core-*.txt` der drei Plattformen vergleichen
  (PRD-0018 FR-04b).
