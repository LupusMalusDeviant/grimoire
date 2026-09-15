# Entwurf: Paralleler Scheduler (Engine-ADR-0006, Plan 0002 WP1.0)

Entwurf zu WP1.0 — Vertragsänderung, PO-Freigabe ausstehend

Grundlage ist der Entwurf **minimal-api**, ergänzt um Teile aus **proof-first** (`set`, `catch_unwind` je
Aufgabe, `StageMode::Isolated`, Prüfung der Befehle nach dem Systemlauf, `derive_block_rng` über
`derive_rng`, Identitätsprüfung der `clippy.toml`, Allokations- und Zähl-Executor-Tests,
`PermutedExecutor::reversed`) und aus **codex** (festes N = 4, keine Migration skalarer Reduktionen,
Panic mit kleinstem Index). Geprüft gegen grimoire `70a7fb0`. Die sieben verbindlichen Bausteine von
[ADR-0006](../adr/0006-paralleler-scheduler-deterministische-zusammenfuehrung.md) sind die
Spezifikation; die Migrationsschritte 1–5 aus ADR-0003 sind nur Hintergrund.

Der verbindliche Vertragstext steht in [crate-vertraege.md](crate-vertraege.md) (§1, §3, §7, §8, §9,
§10). Dieses Dokument begründet und erläutert ihn.

## 0. Belege

Vor der Umsetzung hat eine Probe (Kopien von `core`, `ecs`, `sim`, gepatchtes `ecs`, `grimoire_exec` mit
rayon 1.12.0) belegt:

- Build, Clippy mit `-D warnings` (Debug, Release, `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`) und
  Tests grün unter der unveränderten Determinismus-`clippy.toml`, ohne `unsafe`.
- Die bestehenden Tests `world`, `model` und `alloc` von `grimoire_ecs` bestehen gegen das gepatchte ECS:
  `REFERENCE_WORLD_HASH` bleibt, Query-Iteration bleibt allokationsfrei.
- Die parallele Form des P0-Szenarios reproduziert jeden Checkpoint und `GOLDEN_FINAL_HASH` mit
  `SequentialExecutor`, `StageMode::Isolated`, `PermutedExecutor::new(1)` und `reversed()` sowie mit echten
  Pools aus 1, 2 und 4 Threads, auch mit verschachtelten Blöcken in einer parallelen Stufe, in Debug und
  Release.
- Der Debug-Kontext erreicht rayon-Worker; eine nicht deklarierte Lesung in einem Block bricht mit
  Systemnamen ab. Eine panickende Stufe lässt den Welt-Hash unverändert.
- Offline scheitert `cargo tree -p grimoire -e normal,build,dev --target all` mit Exit 101; das CI-Skript
  muss deshalb bei Cargo-Fehlern scheitern, statt still zu bestehen.

Stolperstein aus der Probe: Importe, die nur `#[cfg(debug_assertions)]`-Tests nutzen, müssen selbst
`cfg`-geschützt sein, sonst scheitert Clippy im Release-Build.

## 1. Crates und Kanten

- `grimoire_ecs` (Determinismus-Menge): neu `access.rs`, `executor.rs`, `debug_access.rs` (ganzes Modul nur
  mit `debug_assertions`); erweitert `schedule.rs`, `command.rs`, `query.rs`, `world.rs`, `lib.rs`.
- `grimoire_sim` (Determinismus-Menge): `rng.rs` erhält `derive_block_rng`; `simulation.rs` ändert sich nur
  in der Dokumentation.
- **Neu `crates/grimoire_exec`**, außerhalb der Determinismus-Menge und ohne `clippy.toml`. Abhängigkeiten
  `grimoire_ecs`, `rayon`, `thiserror`; Dev-Abhängigkeiten `grimoire_core`, `grimoire_sim`, `grimoire`,
  `proptest`. Workspace: `grimoire_exec` unter den Engine-Crates, `rayon = "1.12.0"` unter den
  Drittabhängigkeiten. `Cargo.lock` erhält rayon 1.12.0, rayon-core 1.13.0, crossbeam-deque 0.8.7,
  crossbeam-epoch 0.9.20 und either 1.18.0 (crossbeam-utils 0.8.23 war schon gesperrt).
- `grimoire` (Fassade, Determinismus-Menge): keine Kante zu `grimoire_exec`, auch nicht als
  Dev-Abhängigkeit.
- **Strenger Baustein 6:** Keine Determinismus-Crate hat eine normale, Build- oder Dev-Abhängigkeit auf
  rayon oder `grimoire_exec`. Die Thread-Pool-Tests liegen in `grimoire_exec/tests` und binden die
  Engine-Szenarien per `#[path]` ein.

## 2. API von `grimoire_ecs`

Alles ist additiv: `System`, `system_fn`, `add_system` und alle P0-Signaturen bleiben unverändert.

### 2.1 Zugriffsdeklaration

```rust
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Access { /* components: Vec<Declared>, resources: Vec<Declared>, structural: bool */ }
impl Access {
    #[must_use] pub fn new() -> Self;
    #[must_use] pub fn read<C: Component>(self) -> Self;            // &C, Option<&C>, World::get::<C>
    #[must_use] pub fn write<C: Component>(self) -> Self;           // CommandBuffer::set::<C>
    #[must_use] pub fn read_resource<R: Resource>(self) -> Self;    // World::resource::<R>
    #[must_use] pub fn write_resource<R: Resource>(self) -> Self;   // insert_resource/remove_resource::<R>
    #[must_use] pub fn structural(self) -> Self;                    // spawn, despawn, insert, remove
}
```

- `write` schließt `read` nicht ein. Doppelte Deklarationen werden ignoriert.
- Ohne Deklaration: das Query-Element `Entity`, `With<T>`/`Without<T>`, `World::is_alive`,
  `World::entity_count`, `World::executor`. Mitgliedschaft und Existenz ändern sich nur durch
  Strukturbefehle, und diese schließen eine Stufe für jedes spätere System.
- **Registrierungsnummern:** Jeder `Schedule` hat zwei private Register (Komponenten, Ressourcen), je eine
  `BTreeMap<TypeId, u32>` plus die Typnamen. Nummern entstehen in Reihenfolge der ersten Deklaration über
  die `add_parallel_system`-Aufrufe. Die Stufenregel vergleicht nur sortierte `u32`-Listen; keine Welt wird
  berührt, nichts registriert, kein Hash ändert sich. `ComponentId` bleibt crate-intern.

### 2.2 Parallele Systeme und Schedule

```rust
pub trait ParallelSystem: Send {
    fn name(&self) -> &str;
    fn access(&self) -> Access;                                   // genau einmal, von add_parallel_system
    fn run(&mut self, world: &World, commands: &mut CommandBuffer);
}
pub fn parallel_system_fn<F>(name: &'static str, access: Access, f: F) -> impl ParallelSystem
where F: FnMut(&World, &mut CommandBuffer) + Send + 'static;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StageMode { #[default] Grouped, Isolated }

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Stage<'s> { pub exclusive: bool, pub systems: Vec<&'s str>, pub reason: String }
impl fmt::Display for Stage<'_> // "{exclusive|parallel} [{Namen mit ", "}] ({Grund})"

impl Schedule {                                                    // zusätzlich zu P0
    pub fn add_parallel_system(&mut self, system: impl ParallelSystem + 'static) -> &mut Self;
    pub fn set_stage_mode(&mut self, mode: StageMode) -> &mut Self;
    #[must_use] pub fn stage_mode(&self) -> StageMode;
    #[must_use] pub fn stages(&self) -> Vec<Stage<'_>>;
}
```

- Intern `enum Entry { Exclusive(Box<dyn System>), Parallel(ParallelEntry) }`; `ParallelEntry` hält das
  System, die aufgelösten sortierten Lese-/Schreiblisten, das Strukturflag, einen eigenen, jeden Tick
  wiederverwendeten `CommandBuffer`, einen Panic-Slot und (nur Debug) den Zugriffskontext.
- Die Stufen werden bei jedem `add_system`, `add_parallel_system` und `set_stage_mode` neu geplant (O(n)).
  `run` plant nie.
- `system_names`, `len`, `is_empty` und `Debug` umfassen beide Arten in Listenreihenfolge. `Schedule`
  bleibt `!Send`.

### 2.3 Stufenregel (ein Vorwärtsdurchlauf, sortiert nie um)

Die Einträge werden in Listenreihenfolge durchlaufen; für die offene parallele Stufe merkt sich der Planer
den ersten Schreiber je Komponenten- und Ressourcennummer und das erste strukturelle Mitglied.

1. **Exklusiver Eintrag:** offene Stufe schließen, `exclusive [name] (exclusive system)` anlegen.
2. **Paralleler Eintrag**, Grund für eine neue Stufe, der erste Treffer gilt:
   1. keine offene Stufe und noch keine Stufe: `first stage`;
   2. keine offene Stufe: `follows an exclusive system`;
   3. Modus `Isolated`: `isolated stage mode`;
   4. die offene Stufe hat ein strukturelles Mitglied `a`: `` `a` records structural commands ``;
   5. die erste deklarierte Komponentenlesung (aufsteigende Nummer) mit Schreiber `a` in der offenen
      Stufe: ``reads component `{type_name}`, written by `a` ``;
   6. dasselbe für Ressourcen: ``reads resource `{type_name}`, written by `a` ``.
3. Mit Grund beginnt eine neue parallele Stufe, sonst tritt der Eintrag der offenen bei. Danach gehen seine
   Schreibzugriffe und sein Strukturflag in die offene Stufe ein.

Folgen: Lesen/Lesen, Schreiben/Schreiben und früheres Lesen/späteres Schreiben teilen eine Stufe. Ein
strukturelles System kann einer Stufe beitreten, ist aber ihr letztes Mitglied; höchstens ein
strukturelles System je Stufe. Typnamen stammen aus `std::any::type_name`, dienen nur der Diagnose und
sind nur unter der gepinnten Toolchain stabil.

### 2.4 Ausführung

`Schedule::run(&mut self, world: &mut World)` behält die P0-Signatur.

- **Exklusive Stufe:** `system.run(world)`. Ein Schedule nur aus exklusiven Systemen ist exakt die
  P0-Schleife ohne Allokation.
- **Parallele Stufe:**
  1. `let shared: &World = world;` und je Eintrag eine Closure `move || run_task(entry, shared)`.
  2. `run_task` betritt im Debug-Build den Kontext und führt
     `catch_unwind(AssertUnwindSafe(|| { system.run(world, commands); /* Debug: validate_commands */ }))`
     aus; ein `Err` landet im Panic-Slot.
  3. Genau eine Aufgabe läuft direkt, ohne Executor. Sonst `Vec<&mut (dyn FnMut() + Send)>` sammeln und
     `shared.executor().run(&mut refs)` aufrufen — zwei Allokationen je Stufe mit mehreren Systemen.
  4. Nachdem alle Aufgaben zurückgekehrt sind, werden die Panics in Listenreihenfolge eingesammelt. Gibt es
     einen, werden alle Puffer der Stufe geleert (crate-internes `CommandBuffer::clear`) und der Panic mit
     dem kleinsten Index per `resume_unwind` weitergereicht. Nichts aus der Stufe ist angewendet.
  5. Sonst werden die Puffer in Listenreihenfolge angewendet.
- Ein Panic in einem exklusiven System läuft unverändert weiter.

### 2.5 Ergänzungen an `CommandBuffer`

```rust
impl CommandBuffer {                                                 // zusätzlich zu P0
    pub fn set<C: Component>(&mut self, entity: Entity, value: C);   // ersetzt vorhandenes C; sonst übersprungen; nie Archetypwechsel
    pub fn insert_resource<R: Resource>(&mut self, resource: R);     // wie World::insert_resource
    pub fn remove_resource<R: Resource>(&mut self);                  // wie World::remove_resource, Wert verworfen
    pub fn append(&mut self, other: &mut CommandBuffer);             // Befehle von other in Reihenfolge anhängen, other leer
}
```

- `set` wendet `if let Some(slot) = world.get_mut::<C>(entity) { *slot = value; }` an.
- Crate-intern `clear()`. Nur im Debug-Build hält jeder Puffer parallel zu den Befehlen ihre Art
  (`Structural(&'static str)` für spawn/despawn/insert/remove, `Component { type_id, name }` für `set`,
  `Resource { type_id, name }` für die Ressourcenbefehle); `append` verschiebt sie mit, `apply` und `clear`
  leeren sie.
- Vom Nutzer erzeugte Puffer verhalten sich wie in P0. Keine `modify`-artigen Closure-Befehle.

### 2.6 Executor

```rust
pub trait Executor: Send + Sync {
    /// Worker-Threads; nur Diagnose, beeinflusst nie Stufen, Blöcke oder Zustand.
    fn threads(&self) -> usize;
    /// Führt jede Aufgabe genau einmal aus und kehrt zurück, wenn alle beendet sind. Reihenfolge,
    /// Gleichzeitigkeit und Thread sind unbestimmt. Verschachtelte Aufrufe dürfen nicht verklemmen.
    /// Aufgaben von grimoire_ecs fangen ihre Panics selbst; ein Executor verschluckt nie einen Panic.
    fn run(&self, tasks: &mut [&mut (dyn FnMut() + Send)]);
}
#[derive(Clone, Copy, Debug, Default)] pub struct SequentialExecutor;   // Indexreihenfolge, aufrufender Thread
#[derive(Debug)] pub struct PermutedExecutor { seed: u64, calls: AtomicU64, reversed: bool }
impl PermutedExecutor {
    #[must_use] pub fn new(seed: u64) -> Self;   // je Aufruf Fisher–Yates aus SplitMix64 über (Seed, Aufrufzähler), nur Ganzzahlen
    #[must_use] pub fn reversed() -> Self;       // immer vom letzten zum ersten
}
```

Ergebnisse laufen nie durch den Executor. `grimoire_ecs` besitzt die Ergebnis- und Panic-Slots je Aufgabe
und führt sie nach Aufgabenindex zusammen. Der Trait ist objektsicher, leiht die Aufgaben vom Stack, braucht
keine Box je Aufgabe, keine Sperren und kein `unsafe`.

### 2.7 Executor an der Welt

```rust
impl World {
    pub fn set_executor(&mut self, executor: Arc<dyn Executor>);
    #[must_use] pub fn executor(&self) -> &dyn Executor;   // Standard: &SequentialExecutor (statisch, ohne Allokation)
}
```

Feld `executor: Option<Arc<dyn Executor>>`. Der Executor ist kein Simulationszustand: `stable_hash`
ignoriert ihn, `snapshot`/`duplicate` speichern `None`, `restore` nimmt den aktuellen heraus, ersetzt den
Zustand und setzt ihn wieder ein. `World::new` allokiert nichts Neues; `World: Send + Sync` bleibt und ist
per Test fixiert. Parallele Stufen und `par_blocks*` nutzen `world.executor()`, sodass exklusive Systeme und
verschachtelte Blöcke in parallelen Systemen den Executor ohne zusätzliche Parameter finden.

### 2.8 Datenparallele Queries in festen Blöcken

```rust
pub const QUERY_BLOCK_SIZE: usize = 1024;                   // Vertragskonstante, vorläufig bis zum P1-Bench
pub struct QueryBlock<'w, Q: Query> { /* index, len, remaining, fetch */ }
impl<'w, Q: Query> QueryBlock<'w, Q> {
    #[must_use] pub fn index(&self) -> usize;               // Position in der globalen Blockreihenfolge
    #[must_use] pub fn len(&self) -> usize;                 // Zeilen des Blocks (konstant)
    #[must_use] pub fn is_empty(&self) -> bool;
}
impl<'w, Q: Query> Iterator for QueryBlock<'w, Q> { type Item = Q::Item<'w>; /* size_hint exakt */ }
impl World {
    pub fn par_blocks<Q: ReadOnlyQuery, T: Send>(&self, f: impl Fn(QueryBlock<'_, Q>) -> T + Sync) -> Vec<T>;
    pub fn par_blocks_mut<Q: Query, T: Send>(&mut self, f: impl Fn(QueryBlock<'_, Q>) -> T + Sync) -> Vec<T>;
}
```

Versiegelte interne Änderungen, von außen unsichtbar: `Element::Fetch<'w>: Send` und
`QueryInternal::Fetch<'w>: Send`; neu `split_fetch(fetch, at)` (für `Entity`/`&T` per
`as_slice().split_at`, für `&mut T` per `into_slice().split_at_mut`, `Option` teilt `Some` und liefert bei
`None` zweimal `None`, Filter liefern `((), ())`, Tupel teilen elementweise) und `for_each_access` für die
Debug-Prüfung.

Ablauf von `par_blocks_mut` (`par_blocks` analog mit `&self` und `fetch_shared`):

1. `World` zerlegen, `Q::check_access()` und `init_state`.
2. Für jeden passenden, nicht leeren Archetyp `fetch_exclusive` und davon wiederholt
   `min(Rest, QUERY_BLOCK_SIZE)` Zeilen von vorn abspalten; jeder Block erhält `index = blocks.len()`.
3. Bei höchstens einem Block `f` direkt ohne Executor.
4. Sonst je Block ein Slot `{ block, output, panic }` und eine Aufgabe, die im Debug-Build den Kontext des
   aufrufenden Threads betritt, den Block nimmt und `f` unter `catch_unwind` ausführt. Nach `executor.run`
   die Ergebnisse in Indexreihenfolge einsammeln, den Panic mit dem kleinsten Blockindex weiterreichen; ein
   Slot ohne Ergebnis und ohne Panic bricht mit `executor did not run block task {index}` ab.
5. Blöcke überspannen nie Archetypen. Grenzen hängen nur von Erzeugungsreihenfolge der Archetypen,
   Zeilenzahlen und `QUERY_BLOCK_SIZE` ab. Allokationen hängen von der Blockzahl ab, nie von der
   Zeilenzahl.

Reduktionen faltet der Aufrufer über den zurückgegebenen `Vec` in Reihenfolge. Eine bestehende skalare
Faltung über eine ganze Query wird nicht auf Blöcke umgestellt: Das ändert die Klammerung und damit Hashes.

### 2.9 Debug-Prüfung der Zugriffe (nur `debug_assertions`)

- Thread-lokaler Stapel `RefCell<Vec<Arc<AccessContext>>>`. `enter(ctx)` legt auf und liefert einen Guard,
  dessen `Drop` per `try_with` abräumt — balanciert auch beim Abwickeln und unter rayon-Work-Stealing.
  `current()` klont das oberste `Arc`; während Nutzercode läuft, ist keine `RefCell`-Ausleihe aktiv.
- Ohne Kontext sind alle Haken wirkungslos; exklusive Systeme werden also nicht geprüft.
  - `World::query::<Q>` und `World::par_blocks::<Q>`: jeder Elementzugriff (`&T`, `Option<&T>`) braucht
    `read::<T>()`;
  - `World::get::<C>` braucht `read::<C>()`, `World::resource::<R>` braucht `read_resource::<R>()`;
  - `World::stable_hash` und `World::snapshot` brechen als Lesen der ganzen Welt ab.
- Nach `ParallelSystem::run`, noch in der Aufgabe und vor jeder Anwendung, prüft `validate_commands` die
  Arten im Puffer: Strukturbefehle brauchen `structural()`, `set` braucht `write::<C>()`, Ressourcenbefehle
  brauchen `write_resource::<R>()`. Angehängte Puffer sind abgedeckt.
- Meldungen (Tests prüfen Präfixe):
  - ``system `{name}` reads component `{type}` without declaring it (Access::read, World::{query|par_blocks|get})``
  - ``system `{name}` reads resource `{type}` without declaring it (Access::read_resource)``
  - ``system `{name}` reads the whole world (World::{stable_hash|snapshot})``
  - ``system `{name}` records CommandBuffer::{spawn|despawn|insert|remove} without declaring structural commands (Access::structural)``
  - ``system `{name}` writes component `{type}` without declaring it (Access::write)``
  - ``system `{name}` writes resource `{type}` without declaring it (Access::write_resource)``
- Hilfsfunktionen nur für die Prüfung tragen `#[cfg(debug_assertions)]`, damit Release-Builds warnungsfrei
  bleiben.

### 2.10 `lib.rs`

```rust
pub use access::Access;
pub use executor::{Executor, PermutedExecutor, SequentialExecutor};
pub use query::{QUERY_BLOCK_SIZE, Query, QueryBlock, QueryIter, QueryIterMut, ReadOnlyQuery, With, Without};
pub use schedule::{ParallelSystem, Schedule, Stage, StageMode, System, parallel_system_fn, system_fn};
```

Crate-Dokumentation und Moduldokumentation von `schedule.rs` verweisen auf Stufen und ADR-0006.

## 3. `grimoire_sim`

```rust
/// Generator des datenparallelen Blocks `block` im Systemstrom `stream`:
/// derive_rng(seed, tick, splitmix64(splitmix64(stream) ^ block)). Reine Funktion der Argumente.
#[must_use]
pub const fn derive_block_rng(seed: u64, tick: u64, stream: u64, block: u64) -> SimRng;
```

- Re-Export aus `lib.rs`. `ALGORITHM_VERSION` bleibt 1, weil sich keine bestehende Ausgabe ändert.
- Stromkonvention (vorläufig): Engine-Crates nutzen Stromkonstanten mit gesetztem Bit 63, Spiele solche
  ohne.
- `Simulation` erhält keine neue API: `sim.world_mut().set_executor(..)`; `restore` behält den Executor
  über `World::restore`; `step` bleibt `schedule.run(&mut world)`.
- Nach einem Panic in `step` ist die Simulation nur per `restore` weiterverwendbar: `Tick` ist nicht
  erhöht, `TickInput` bereits ersetzt.

## 4. `grimoire_exec`

```rust
#[derive(Debug)]
pub struct ThreadPoolExecutor { pool: rayon::ThreadPool, threads: usize }
impl ThreadPoolExecutor {
    /// Eigener Pool mit genau `threads` Workern `grimoire-sim-{i}`, nie der globale Pool.
    pub fn new(threads: usize) -> Result<Self, ExecError>;
}
impl Executor for ThreadPoolExecutor {
    fn threads(&self) -> usize { self.threads }
    fn run(&self, tasks: &mut [&mut (dyn FnMut() + Send)]) {
        self.pool.install(|| tasks.par_iter_mut().with_max_len(1).for_each(|task| task()));
    }
}
#[derive(Debug, thiserror::Error)] #[non_exhaustive]
pub enum ExecError { ZeroThreads, Pool(String) }   // rayon-Fehler als Text; kein rayon-Typ in der API
pub const GATE_THREADS_ENV: &str = "GRIMOIRE_GATE_THREADS";
pub fn gate_executors() -> Vec<(String, Arc<dyn Executor>)>;   // Pools mit 1, 2 und N Threads, N = 4 oder Umgebung ≥ 3
```

`install` aus einem Worker desselben Pools läuft direkt, verschachtelte `par_blocks` verklemmen daher nicht.
Kein `available_parallelism`: N ist fest, damit das Gate nicht von der Kernzahl des Runners abhängt.

## 5. Fassade `grimoire`

```rust
impl AppBuilder {
    /// Executor für parallele Stufen und datenparallele Queries (Standard: SequentialExecutor).
    /// Wird direkt nach Simulation::new und vor dem build jedes Plugins an der Welt gesetzt.
    #[must_use] pub fn executor(mut self, executor: Arc<dyn Executor>) -> Self;
}
```

- Feld `executor: Option<Arc<dyn Executor>>` (`None` heißt Standard der Welt); `Debug` zeigt
  `executor_threads`. `LoopSettings` erhält dasselbe Feld; `run_headless` und `GameLoop::init` setzen den
  Executor direkt nach `Simulation::new`.
- Prelude zusätzlich `Access, Executor, ParallelSystem, QueryBlock, SequentialExecutor,
  parallel_system_fn` (ECS) und `derive_block_rng` (Sim); die `fnp_*`-Crates wurden auf Namenskollisionen
  geprüft.
- Die Fassade erzeugt keine Threads. Spiele, die mehrere Threads nutzen, binden `grimoire_exec` neben der
  Fassade ein und übergeben `ThreadPoolExecutor` an `AppBuilder::executor`. Eine Fassaden-Feature, die rayon
  zieht, würde Baustein 6 brechen, sobald sie aktiv ist.

## 6. CI und `clippy.toml`

- Neues Skript `.github/scripts/check-thread-source.sh`: Identität der sechs `clippy.toml` per SHA-256; je
  Crate mit `clippy.toml` `cargo tree --locked -p <crate> -e normal,build,dev --target all`, dessen Ausgabe
  zuerst erfasst wird (ein scheiterndes `cargo tree` bricht den Schritt ab, statt still zu bestehen), und
  Suche nach `rayon`, `rayon-core`, `grimoire_exec`; Positivkontrolle, dass `grimoire_exec` rayon zeigt.
- `ci.yml`, Job `docs` (Ubuntu, Toolchain und Cache vorhanden): neuer Schritt vor `cargo doc`. Keine neuen
  Jobs, keine Umbenennung, keine Matrixänderung; Tests, Clippy und Doku von `grimoire_exec` laufen in den
  bestehenden Workspace-Schritten, das 1/2/N-Hash-Gate also in der bestehenden Testmatrix auf drei
  Betriebssystemen.
- Lokal braucht `--target all` Registry-Downloads für die Fassade und scheitert offline; lokal läuft das
  Skript ohne `--target all`, die vollständige Prüfung in der CI.
- `clippy.toml` (sechs identische Dateien): nur der Begründungstext der vier `std::thread`-Einträge und der
  Kopfkommentar ändern sich. Pfade bleiben, eine Lint-Probe ist daher nicht nötig.

## 7. Vertragstext

Die Änderungen an `crate-vertraege.md` (§1 Kante und Hinweis zu `grimoire_exec`, §3 einzige Thread-Quelle,
Ausführungsunabhängigkeit, Reduktionen, Zufall und Review-Punkte, §7 API und Semantik, §8
`derive_block_rng`, Ströme, `Simulation`, Hash-Gate, §9 Fassade, neuer §10 `grimoire_exec`, bisheriger §10
wird §11) und der CHANGELOG-Eintrag unter `[Unreleased]` sind Teil desselben Entwurfs-PRs.

## 8. Testplan

### 8.1 `grimoire_ecs` (ohne Threads; Determinismus-Lints gelten)

1. `tests/stages.rs`: Tabellentests mit eingefrorenem `Display` (leerer Schedule, nur exklusive Systeme,
   disjunkte Leser, Komponenten- und Ressourcen-Lesen-nach-Schreiben trennt, Schreiben/Schreiben und
   früheres Lesen/späteres Schreiben teilen, strukturelles letztes Mitglied, exklusives System dazwischen,
   gleicher Rust-Typ als Komponente und Ressource ist verschieden, `Isolated`, Neuplanung nach `add_*` und
   `set_stage_mode`, Registrierung berührt keine Welt).
2. Proptest `grouped_equals_isolated`: 1–8 parallele Systeme mit zufälligem Zugriff auf Komponenten A/B/C
   und Ressourcen R1/R2 samt Strukturflag, dazwischen 0–2 exklusive Systeme, 0–300 Entities. Systeme leiten
   alles aus deklarierten Lesungen ab. Nach jedem von drei Ticks gleicher Hash für `Isolated` + sequentiell,
   `Grouped` + sequentiell, `Grouped` + `PermutedExecutor::new(1..=3)` und `Grouped` + `reversed`.
   Generator in `tests/support/random_schedule.rs`, von `grimoire_exec` per `#[path]` mitgenutzt.
3. `tests/blocks.rs` (Proptest) über Zeilenzahlen {0, 1, B−1, B, B+1, 3B+7} in 1–3 Archetypen mit
   Swap-Removes: lückenlose Indizes, `len ≤ B`, kein Block über Archetypgrenzen, Verkettung gleich
   `query`-Reihenfolge, `par_blocks_mut` gleich `query_mut`, `f32`-Blocksummen bitgleich zur manuellen
   Faltung für jeden Executor, Alias-Queries panicken wie `query_mut`, Panic mit kleinstem Blockindex,
   Zähl-Executor wird bei höchstens einem Block nie aufgerufen.
4. `tests/parallel.rs`: `set`, Ressourcenbefehle, `append`; Panic-Semantik der Stufe; Executoren inklusive
   verschachtelter Aufrufe; `restore` behält den Executor, Hash und Snapshot sind unabhängig von ihm,
   `World: Send + Sync`.
5. `tests/debug_access.rs` (nur Debug): ein Test je Regel aus 2.9, nicht deklarierte Lesung in einem Block
   unter `PermutedExecutor`, angehängte Strukturbefehle, exklusive Systeme ungeprüft, keine Fehlalarme für
   `With`/`Without`/`Entity`/`entity_count`.
6. `tests/alloc.rs`: Tick nur aus exklusiven Systemen 0 Allokationen, Stufe aus drei Systemen ohne Befehle
   ≤ 2, `par_blocks_mut` über 10.000 Entities ≤ 32.

### 8.2 `grimoire_sim`

7. `rng.rs`: Formel von `derive_block_rng`, verschiedene erste Ausgaben je Block, eingefrorener
   Referenzvektor (Seed 42, Tick 7, Strom 1, Blöcke 0/1), bestehende Vektoren unverändert.
8. Commit 1 verschiebt das P0-Szenario nach `tests/scenario/mod.rs`; `GOLDEN_FINAL_HASH` samt Doku bleibt
   Byte für Byte.
9. Parallele Form des P0-Szenarios (`build_parallel_simulation`): Tests je Executor (sequentiell,
   isoliert, permutiert 1 und 2, rückwärts) gegen jeden Checkpoint und `GOLDEN_FINAL_HASH`, Fortsetzung
   nach P0-Snapshot unter `PermutedExecutor::new(3)`, eingefrorene Stufenaufteilung.
10. Neues Parallelszenario (`tests/parallel_scenario/mod.rs`, `tests/parallel_determinism.rs`): etwa
    12.000 Entities in mindestens drei Archetypen, 1.500 Ticks, Checkpoints alle 100 Ticks, Systeme
    `hazard`, `sense`, `heat`, `census`, `tag`, `steer`, `integrate`, `emit`. `GOLDEN_PARALLEL_FINAL_HASH`
    gemessen mit `Isolated` + sequentiell, vorläufig bis zur grünen CI auf drei Betriebssystemen.

### 8.3 Fassade

11. `tests/common/mod.rs` erhält `ParallelScenario` (exklusive Mover mit `par_blocks_mut` und
    `derive_block_rng`, paralleler `player` mit `write_resource::<Player>`); `headless.rs` prüft
    Executor-Unabhängigkeit in `run_headless` und in der Frame-Schleife, auch für das P0-Szenario.

### 8.4 `grimoire_exec` (echte Threads)

12. Unit-Tests: `new(0)`, `threads()`, jede Aufgabe genau einmal (1, 7, 1.000 Aufgaben; 1, 2, 4 Threads),
    verschachteltes `run`, Panic einer fremden Aufgabe läuft weiter, `gate_executors` über eine reine
    Parse-Hilfsfunktion.
13. `tests/hash_gate.rs` mit `#[path]`-Modulen: (a) parallele P0-Form, (b) Parallelszenario, (c)
    Fassaden-Szenarien, je mit 1, 2 und N Threads; (d) Zufalls-Schedules (32 Fälle) auf 4 Threads; (e)
    Debug-Prüfung auf Workern.
14. `tests/timing.rs` (`#[ignore]`, nur manuell im Release und in einer Messsitzung): Blöcke gegen
    `query_mut`, Schrittkosten, Verteilungsaufwand einer Zwei-System-Stufe.

Spiel-Repo (nicht in WP1.0, erst nach PO-Freigabe, Alpha-Tag und Pin-Anhebung): `fnp_sim_harness` prüft
`0x5270_20ae_cf76_4ca7` mit jedem Executor aus `gate_executors()`; `fnp_app` übergibt einen
`ThreadPoolExecutor`.

## 9. Commit-Reihenfolge

Worktree `_wt/p1-scheduler`, Branch `p1/wp1.0-scheduler`, nur Entwurfs-PR.

1. `refactor(sim): move the golden scenario into a shared test module`
2. `feat(ecs): add the Executor trait with sequential and permuted executors`
3. `feat(ecs): add access declarations and deferred component and resource commands`
4. `feat(ecs): run parallel stages with per-system command buffers`
5. `feat(ecs): add data-parallel queries in fixed blocks`
6. `feat(ecs): check declared access in debug builds`
7. `feat(sim): derive per-block random streams`
8. `test(sim): gate the golden scenario in parallel form and add a parallel golden`
9. `feat(exec): add grimoire_exec with a thread-pool executor`
10. `feat(grimoire): let AppBuilder choose the simulation executor`
11. `test(exec): gate golden hashes with 1, 2 and N threads`
12. `ci: check that no determinism crate depends on rayon`
13. `chore(lint): point the thread-ban reasons to engine ADR-0006`
14. `docs(architektur): specify the parallel scheduler contract`

Vertragsänderungen bleiben bis zur PO-Freigabe unvermerged (WP1.7); kein Tag `v0.2.0-alpha.1` vor der
Freigabe.

## 10. Vorläufige Entscheidungen (PO-Bestätigung ausstehend)

| Thema | Entscheidung |
|-------|--------------|
| Identifikation der Zugriffe | typbasierter `Access`-Builder, schedule-lokale Registrierungsnummern; `ComponentId` bleibt intern |
| Ort des Executors | optionales Feld an `World`, kein Simulationszustand |
| Blockgröße | `QUERY_BLOCK_SIZE = 1024`, bis der P1-Bench sie bestimmt |
| Stromkonvention | Engine-Ströme mit gesetztem Bit 63, Spiel-Ströme ohne |
| Executor-Crate | Name `grimoire_exec`, festes N = 4 im Gate |
| Abhängigkeitsstrenge | auch keine Dev-Abhängigkeit einer Determinismus-Crate auf rayon oder `grimoire_exec` |
| Thread-Einstellung der Fassade | nur `AppBuilder::executor`, keine Fassaden-Feature |
| Umfang des Hash-Gates | P0-Szenario in paralleler Form, neues Parallelszenario, Fassaden-Szenarien; Goldens von `ecs`/`core` nicht unter Threads dupliziert |
