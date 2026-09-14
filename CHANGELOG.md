# Changelog

Alle nennenswerten Änderungen an Grimoire. Format nach [Keep a Changelog](https://keepachangelog.com/de/1.1.0/),
Versionierung nach [SemVer](https://semver.org/lang/de/). Einträge entstehen aus Conventional Commits.

## [Unreleased]

Phase P0 — Fundament.

### Added
- Workspace-Gerüst mit 13 Crates gemäß Crate-Map, gepinnte Toolchain 1.98.1, Crate-Verträge (`docs/architektur/crate-vertraege.md`).
- `grimoire_core`: stabiler 64-Bit-Hasher (`StableHasher`, Algorithmus v1, eingefroren durch Golden-Tests), `StableHash` mit `impl_stable_hash!`, deterministische Mathematik (`dmath` über `libm`, inklusive `min`/`max`, und `Vec2`); Float-Determinismus-Sonde (`tests/float_determinism.rs`) mit Golden-Hashes und CI-Artefakt.
- Determinismus-Lint: identische `clippy.toml` in `core`, `ecs`, `sim`, `collide`, `sigil` sperrt `HashMap`/`HashSet`, Wanduhr (`now`/`elapsed`), Threads, `f32`/`f64`-Transzendentalfunktionen, `powi`, `mul_add` sowie `min`/`max`.
- `grimoire_platform`: winit-Desktop-Runner (`run_desktop`) mit `AppHandler`-Lebenszyklus, Abbildung auf `PlatformEvent`/`RawInputEvent` über physische Tasten, Headless-Runner (`run_headless`) mit `ManualClock`, `SystemClock`, `StdFileSystem` mit atomarem Schreiben, `MemoryFileSystem`; Fensterplatzierung per `MonitorChoice` und `focus_on_open` samt Umgebungsvariablen `GRIMOIRE_WINDOW_MONITOR`/`GRIMOIRE_WINDOW_FOCUS`; Beispiel `window`.
- `grimoire_gpu`: wgpu-Kontext für Fenster und offscreen, Window-Surface mit Wiederherstellung bei `Lost`/`Outdated`, RGBA-Readback, optionaler Software-Fallback und erzwungener CPU-Adapter über `GRIMOIRE_GPU_ADAPTER=software`.
- `grimoire_render`: `WgpuRenderer` mit instanziertem Sprite-Pass in einem Draw-Call (kantengeglättete Kreise und Rechtecke, Rotation, Alpha-Blending in linearem Raum), wachsender Instanzpuffer, `Camera2D`, `NullRenderer`; Offscreen-Tests, Beispiel `instancing` mit 10.000 Sprites.
- `grimoire_ecs`: deterministisches Archetyp-ECS (Entity-Generationen mit FIFO-Wiederverwendung, Queries mit `&T`/`&mut T`/`Option`/`With`/`Without` und Alias-Prüfung, Ressourcen, `CommandBuffer`, geordneter single-threaded `Schedule`), `World::stable_hash`, Snapshots; Property-Tests gegen ein Referenzmodell, allokationsfreie Query-Iteration.
- `grimoire_sim`: exakter ganzzahliger `FixedTimestep` mit Tick-Limit, `dropped_time` und `alpha`, `SimRng` (PCG32, Version 1) mit `derive_rng`, `InputFrame`/`TickInput`, Binärformat `InputLog` (`GRIMREPL` v1), `Simulation` mit `state_hash`, Snapshot/Restore und `replay`; Determinismus-Gate mit 2.000 Entities über 10.000 Ticks und goldenem Endhash.
- `grimoire` (Fassade): `App`/`AppBuilder`, `GamePlugin` (`build`, `extract`, `on_frame`, `window_created`), `FrameStats`, `InputMap` mit WASD-/Pfeiltasten-Preset und Fokusverlust-Freigabe, eine Hauptschleife für Desktop (`run`) und headless (`run_headless_frames` mit `NullRenderer`), reiner Simulationslauf `run_headless`, `GrimoireError`, `prelude`; Tests für Determinismus, Plugin-Lebenszyklus, Frame-Timing und Eingabe; Beispiel `sim_loop` mit 5.000 Entities auf Spiralbahnen und Interpolation.
- CI: Format, Standalone-Gate (keine `fnp_`-Referenzen), rustdoc mit `-D warnings`, Test-Matrix Windows/Linux/macOS mit Clippy, Beispiel-Builds und plattformübergreifendem Vergleich der Float-Sonden; Nightly-Tests im Release-Profil; Release-Workflow für Tags `vX.Y.Z` mit git-cliff.
- Dokumentation: Engine-ADRs 0001 (winit), 0002 (GPU-Kapselungsgrenze), 0003 (Scheduler, vorgeschlagen), 0004 (f32 mit Regeln und `libm`, vorgeschlagen), 0005 (`grimoire_core` als Blatt-Crate); `CONTRIBUTING.md`, `cliff.toml`.

### Changed
- `grimoire`: hängt in P0 nur noch von `core`, `ecs`, `platform`, `render` und `sim` ab; die Platzhalter-Crates folgen mit ihrer API.
- Determinismus-Lint gilt auch für die Fassade `grimoire` (Hauptschleife, `InputMap`, Tests und Beispiel `sim_loop`): sie trägt dieselbe `clippy.toml`, jetzt sechs identische Kopien.

### Fixed
- `grimoire_core`: `StableHasher` speist jedes NaN kanonisch ein (Bitmuster sind nicht portabel).
- `grimoire_render`: Surface wird bei `Lost` neu erzeugt, GPU-Validierungsfehler kommen aus `render` zurück, Kreisränder werden nicht mehr abgeschnitten.
- `grimoire_sim`: `dropped_time` sättigt exakt bei `Duration::MAX`; `f32::min`/`max` durch `dmath` ersetzt (Werte und Golden-Hashes unverändert).
- `grimoire_platform`: Lebenszyklus als testbare Zustandsmaschine, Temp-Dateinamen beim atomaren Schreiben gekürzt.
- `grimoire_core`/`grimoire_sim`: NaN im Simulationszustand fällt in Debug-Builds auf (`StableHasher::saw_nan`, Debug-Assertion in `Simulation::state_hash`); Hashwerte und Golden-Hashes unverändert.
- `grimoire_ecs`: `CommandBuffer::spawn` lehnt Bundles mit doppeltem Komponententyp schon beim Aufzeichnen ab, statt `apply` mittendrin abzubrechen.
- `grimoire_platform`: Die Desktop-Schleife lastet keinen Kern mehr aus, solange nichts präsentiert werden kann (minimiertes oder verdecktes Fenster, wiederholt nicht verfügbare Surface), sondern rendert dann alle 100 ms; neu sind `PlatformEvent::Occluded` und `PlatformContext::frame_not_presented`, die Fassade meldet `RenderError::SurfaceLost` darüber.
- `grimoire_render`: Offscreen-Tests ohne GPU-Adapter bestehen nicht mehr unbemerkt: Das Überspringen erscheint als GitHub-Actions-Warnung im Log, und mit `GRIMOIRE_REQUIRE_GPU_ADAPTER=1` (in der CI unter Windows und Linux gesetzt) scheitern sie.
- `grimoire_sim`: `InputLog::to_bytes` verweigert `tick_rate_hz == 0`, das `from_bytes` nie laden könnte; das Determinismus-Gate listet bei Abweichung alle Checkpoint-Hashes.
- `grimoire`: Kurze Tastendrücke gehen nicht mehr verloren. `InputState` merkt jeden Druck vor (`is_active`, `clear_presses`), bis ein Frame mit mindestens einem Tick ihn abgetastet hat, auch wenn die Taste vorher schon losgelassen wurde oder Frames ohne Tick dazwischen lagen; neue Tests decken Tipps, mehrere Ticks pro Frame mit wechselnder Eingabe und die Weiterleitung von `Resized` an den Renderer ab.
- CI: `cargo test` läuft in CI, Nightly und Release mit `--no-fail-fast`, damit ein rotes Test-Binary die Float-Sonden und Golden-Tests der folgenden nicht mehr unterdrückt. Der Float-Vergleich prüft jetzt die `core-*.txt` (`basic_hash`, `dmath_hash`) aller Plattformen und schlägt bei Abweichung, unlesbarer Probe oder trotz grüner Tests fehlender Probe fehl; der Probe-Upload scheitert ohne Dateien, und ein Selbsttest prüft das Vergleichsskript.
