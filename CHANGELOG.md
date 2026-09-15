# Changelog

Alle nennenswerten Änderungen an Grimoire. Format nach [Keep a Changelog](https://keepachangelog.com/de/1.1.0/),
Versionierung nach [SemVer](https://semver.org/lang/de/). Einträge entstehen aus Conventional Commits.

## [Unreleased]

### Added
- `grimoire_render`: Render-Vertrag v1 (WP2.2, additiv, Vertragsänderung wartet auf PO-Freigabe V-20): `Camera25D` (Neigung, FOV, Ziel, Look-Ahead-Parameter, `screen_to_ground`/`ground_to_screen` als Strahl-Ebene-Schnitt ohne NaN an Horizont oder bei parallelem Strahl), `MeshInstance` (Mesh-/Material-Handle, Transform, Ebene), `PbrMaterial` (glTF-Metallic-Roughness-kompatibel, optionale Basisfarbe-/Normalen-/ORM-Texturen, `AlphaMode`), `PointLight`, `DirectionalLight` (Key-Light), `AmbientLight` (flach oder Hemisphäre), `BulletLightCap` (PRD-0003 Regel 5 / FR-15). Neues Modul `grimoire_render::stage3d`; `StageFrame`/`StageStats` wachsen additiv; `NullRenderer` validiert und zählt die neuen Kanäle; `grimoire_render` hängt neu (additiv, bereits erlaubte Kante) von `grimoire_core` ab.

## [0.1.1] - 2026-09-15

Erster P1-Schritt (WP1.0): paralleler Scheduler. Rein additiv, alle P0-Goldens unverändert; nach der
P1-Versionsregel (PO-Entscheidung P-7) eine Patch-Version.

### Added
- Paralleler Scheduler nach Engine-ADR-0006 (Vertragsänderung, vom PO freigegeben): Zugriffsdeklaration `Access`, parallele Systeme (`ParallelSystem`, `parallel_system_fn`, `Schedule::add_parallel_system`) mit Stufenbildung, Diagnose `Schedule::stages` und Referenzmodus `StageMode::Isolated`; `CommandBuffer::set`, `insert_resource`, `remove_resource` und `append`; Trait `Executor` mit `SequentialExecutor` und dem Test-Executor `PermutedExecutor`, Executor an der `World`; datenparallele Queries `World::par_blocks`/`par_blocks_mut` mit `QUERY_BLOCK_SIZE = 1024`; Debug-Prüfung der deklarierten Zugriffe; `derive_block_rng`; `AppBuilder::executor`; neue Crate `grimoire_exec` mit `ThreadPoolExecutor` (rayon 1.12.0) und `gate_executors`; Hash-Gate mit 1, 2 und N Threads (`grimoire_exec/tests/hash_gate.rs`) samt neuem Parallelszenario und goldenem Endhash; CI-Prüfung, dass keine Determinismus-Crate von rayon abhängt. Entwurf: `docs/architektur/entwurf-paralleler-scheduler.md`.

### Changed
- Determinismus-Lint: Die Begründung der Thread-Sperren in den sechs `clippy.toml` verweist auf Engine-ADR-0006 (einzige Thread-Quelle `grimoire_exec`); die gesperrten Pfade sind unverändert.
- `grimoire_sim`: Das P0-Determinismusszenario liegt in `tests/scenario/mod.rs` und wird zusätzlich in paralleler Form geprüft; `GOLDEN_FINAL_HASH` ist unverändert.
- Lizenz: `LICENSE` („Alle Rechte vorbehalten“, Rechteinhaber Lupus Malus Deviant, Engine-ADR-0009) und `license-file` in allen Crates; Autor in den Manifesten ist „Lupus Malus Deviant“.
- Das Repository ist öffentlich. Die Historie wurde bei der Veröffentlichung neu geschrieben; frühere Commit-IDs ordnet `docs/commit-zuordnung.md` zu. Der Tag `v0.1.0` zeigt auf den neu geschriebenen Commit mit unverändertem Inhalt.

### Dokumentation
- Engine-ADR-0006 (paralleler Scheduler) akzeptiert, ADR-0003 (single-threaded) abgelehnt.
- `CONTRIBUTING.md`: Beiträge von außen werden derzeit nicht angenommen; P1-Versionsregel `0.1.x`; `main` ist per Ruleset gegen Force-Push und Löschen geschützt.

## [0.1.0] - 2026-09-14

Phase P0 — Fundament. Erste Version, gegen die das Spiel *Fiends n Patrons* pinnt.

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
- Dokumentation: Die Crate-Verträge nennen die `Vec2`-Konstanten, den Re-Export `raw_window_handle` als SemVer-Kopplung und die bewusste Einengung von PRD-0002 FR-14 in P0 (keine `fixed_update`-Phase, keine Eingabeaufzeichnung im Fensterlauf) samt geplanten Hooks; die umgesetzten Abschnitte heißen nicht mehr „Zu implementieren". ADR-0003 berücksichtigt die Thread-Sperren im Lint und das umgesetzte `derive_rng`.

### Fixed
- `grimoire_core`: `StableHasher` speist jedes NaN kanonisch ein (Bitmuster sind nicht portabel).
- `grimoire`/`grimoire_platform`: Beenden per Cmd+Q unter macOS verliert kein Spielende mehr. AppKit beendet den Prozess dort direkt nach `AppHandler::shutdown`, ohne dass `run_desktop` zurückkehrt; `run_desktop`, `AppHandler::shutdown`, `App::run` und die Crate-Verträge dokumentieren das. Die Hauptschleife der Fassade implementiert `shutdown` und ruft die neue optionale Methode `GamePlugin::shutdown` genau einmal je Plugin in Registrierungsreihenfolge auf (nicht, wenn der Renderer nicht erzeugt werden konnte), auch nach einem Render-Fehler oder der Exit-Taste; Fehler darin werden geloggt. `shutdown` läuft bei jedem geordneten Ende der Schleife, nicht aber, wenn das Betriebssystem den Prozess beendet (Sitzungsende unter Windows, `SIGTERM`/`SIGINT`, Strg+C in einer Konsole).
- `grimoire_render`: Ein fehlgeschlagenes `resize` (etwa über dem Texturlimit des Geräts oder bei Speichermangel) lässt das Fenster nicht mehr dauerhaft schwarz, während `render` Erfolg meldet. `WgpuRenderer` behält den Fehler und liefert ihn aus jedem folgenden `render` als `OutOfMemory` bzw. `Backend`, bis ein weiteres `resize` ihn ersetzt; die Fassade beendet den Lauf damit. Keine Wiederholung mit begrenzter Größe; neuer Offscreen-Test mit einer Breite über dem Texturlimit.
- `grimoire_render`: Surface wird bei `Lost` neu erzeugt, GPU-Validierungsfehler kommen aus `render` zurück, Kreisränder werden nicht mehr abgeschnitten.
- `grimoire_sim`: `dropped_time` sättigt exakt bei `Duration::MAX`; `f32::min`/`max` durch `dmath` ersetzt (Werte und Golden-Hashes unverändert).
- `grimoire_platform`: Lebenszyklus als testbare Zustandsmaschine, Temp-Dateinamen beim atomaren Schreiben gekürzt.
- `grimoire_core`/`grimoire_sim`: NaN im Simulationszustand fällt in Debug-Builds auf (`StableHasher::saw_nan`, Debug-Assertion in `Simulation::state_hash`); Hashwerte und Golden-Hashes unverändert.
- `grimoire_ecs`: `CommandBuffer::spawn` lehnt Bundles mit doppeltem Komponententyp schon beim Aufzeichnen ab, statt `apply` mittendrin abzubrechen.
- `grimoire_platform`: Die Desktop-Schleife lastet keinen Kern mehr aus, solange nichts präsentiert werden kann (minimiertes oder verdecktes Fenster, wiederholt nicht verfügbare Surface), sondern rendert dann alle 100 ms; neu sind `PlatformEvent::Occluded` und `PlatformContext::frame_not_presented`, die Fassade meldet `RenderError::SurfaceLost` darüber.
- `grimoire_render`: Offscreen-Tests ohne GPU-Adapter bestehen nicht mehr unbemerkt: Das Überspringen erscheint als GitHub-Actions-Warnung im Log, und mit `GRIMOIRE_REQUIRE_GPU_ADAPTER=1` (in der CI unter Windows und Linux gesetzt) scheitern sie.
- `grimoire_sim`: `InputLog::to_bytes` verweigert `tick_rate_hz == 0`, das `from_bytes` nie laden könnte; das Determinismus-Gate listet bei Abweichung alle Checkpoint-Hashes.
- `grimoire`: Kurze Tastendrücke gehen nicht mehr verloren. `InputState` merkt jeden Druck vor (`is_active`, `clear_presses`), bis ein Frame mit mindestens einem Tick ihn abgetastet hat, auch wenn die Taste vorher schon losgelassen wurde oder Frames ohne Tick dazwischen lagen; neue Tests decken Tipps, mehrere Ticks pro Frame mit wechselnder Eingabe und die Weiterleitung von `Resized` an den Renderer ab.
- Dokumentation: `CONTRIBUTING.md` („Versionen der GitHub Actions") gibt die Pins wieder, wie die Workflows sie setzen: GitHubs eigene Actions auf Major-Tags, `Swatinem/rust-cache` und `orhun/git-cliff-action` auf den vollständigen Commit-SHA mit Versionskommentar, samt Befehl zum Ermitteln des SHA eines Tags.
- CI: Der erzwungene CPU-Adapter (`GRIMOIRE_GPU_ADAPTER=software`), auf den sich lokale Testläufe verlassen, lief in der CI nie. Die Test-Jobs unter Windows und Linux führen die Offscreen-Tests jetzt zusätzlich (`cargo test --workspace --test offscreen`, damit die Artefakte des Testschritts wiederverwendet werden) mit `GRIMOIRE_GPU_ADAPTER=software` und `GRIMOIRE_REQUIRE_GPU_ADAPTER=1` aus und belegen so WARP bzw. lavapipe; macOS bleibt unverändert (kein CPU-Adapter).
- CI: `cargo test` läuft in CI, Nightly und Release mit `--no-fail-fast`, damit ein rotes Test-Binary die Float-Sonden und Golden-Tests der folgenden nicht mehr unterdrückt. Der Float-Vergleich prüft jetzt die `core-*.txt` (`basic_hash`, `dmath_hash`) aller Plattformen und schlägt bei Abweichung, unlesbarer Probe oder trotz grüner Tests fehlender Probe fehl; der Probe-Upload scheitert ohne Dateien, und ein Selbsttest prüft das Vergleichsskript.
