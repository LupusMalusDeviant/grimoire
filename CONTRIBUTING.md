# Mitwirken an Grimoire

Verbindliche Arbeitsregeln für Menschen und Coding-Agenten. Architektur: [README](README.md) und
[docs/architektur/crate-vertraege.md](docs/architektur/crate-vertraege.md). Produkt- und
Architekturentscheidungen liegen im Spiel-Repo (ADR-0002 Engine im eigenen Repo, PRD-0017 CI,
PRD-0018 Teststrategie).

## Grundsätze

- **Eigenständig:** Kein Crate, kein Test, kein Beispiel referenziert Spiel-Code (`fnp_*`). Die CI
  prüft das bei jedem Lauf (Standalone-Gate, PRD-0002 FR-01).
- **Schichten:** Abhängigkeiten zeigen nur nach unten (Fassade → Engine → Plattform).
- **Gepinnte Werkzeuge:** Rust kommt aus `rust-toolchain.toml` (1.98.1 mit rustfmt und clippy);
  Drittanbieter-Versionen stehen exakt in `[workspace.dependencies]`. Upgrades sind eigene Commits.

## Commits

[Conventional Commits](https://www.conventionalcommits.org/de/v1.0.0/), Nachricht auf **Englisch**,
Imperativ, Betreff höchstens 72 Zeichen:

```text
<typ>(<scope>): <beschreibung>

<optionaler Rumpf: warum, nicht was>

<optionale Footer>
```

| Typ | Wofür | Abschnitt in den Release-Notes |
|-----|-------|--------------------------------|
| `feat` | neue Fähigkeit | Neue Funktionen |
| `fix` | Fehlerbehebung | Fehlerbehebungen |
| `perf` | Performance ohne Verhaltensänderung | Performance |
| `refactor` | Umbau ohne Verhaltensänderung | Umbauten |
| `docs` | Dokumentation | Dokumentation |
| `test` | Tests, Golden-Master | Tests |
| `build`, `ci` | Cargo, Abhängigkeiten, Workflows | Build und CI |
| `style` | reine Formatierung | Formatierung |
| `chore` | Wartung | Wartung |
| `revert` | Rücknahme | Zurückgenommen |

- **Scope** ist der Crate-Name ohne Präfix (`core`, `ecs`, `sim`, `render`, `platform`, …) oder ein
  Querschnittsthema (`ci`, `release`, `deps`).
- **Inkompatible Änderungen** tragen ein `!` hinter Typ/Scope **und** einen Footer
  `BREAKING CHANGE: <was bricht, wie migrieren>`. Sie erscheinen in den Release-Notes immer, markiert
  mit **[inkompatibel]**.
- `chore(release): vX.Y.Z` ist für Release-Commits reserviert und fehlt in den Release-Notes.
- **Agenten-Commits** enden mit einer Leerzeile und dem Trailer des tatsächlich arbeitenden Modells:

```text
feat(ecs): add archetype move on component insert

Moves the entity row between archetype tables instead of rebuilding them.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
```

## Lokale Pflichtprüfungen vor jedem Push

Alles, was die CI prüft, läuft vorher lokal. Die Beispiele öffnen Fenster; in der CI werden sie
nur gebaut, lokal startet man sie bei Bedarf von Hand.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --examples --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
git grep -n -e fnp_ -- Cargo.toml ':(glob)crates/**/Cargo.toml' ':(glob)crates/**/*.rs'   # muss leer bleiben
```

PowerShell: `$env:RUSTDOCFLAGS = '-D warnings'; cargo doc --workspace --no-deps --locked`.

`--locked` verlangt ein aktuelles `Cargo.lock`. Wer eine Abhängigkeit oder Version ändert,
committet das Lockfile mit.

**Achtung, `[patch]` im Elternordner:** Ist in `<Arbeitsordner>\.cargo\config.toml` der
Spiel-Patch für `grimoire` aktiv (siehe `CONTRIBUTING.md` des Spiels), gilt er auch für dieses Repo
und alle Worktrees unter `<Arbeitsordner>\`. Cargo trägt den hier ungenutzten Patch dann als
`[[patch.unused]]` in `Cargo.lock` ein: Jeder Befehl mit `--locked` bricht mit „cannot update the
lock file“ ab, und ohne `--locked` wird das Lockfile verändert (am 2026-09-14 mit Cargo 1.98.1
nachgeprüft). Vor den Pflichtprüfungen den Patch auskommentieren; ein Lockfile mit
`[[patch.unused]]` nie committen.

## CI im Überblick

| Workflow | Auslöser | Inhalt |
|----------|----------|--------|
| `ci.yml` | Push auf `main`, Pull Request, manuell | `fmt`, `standalone-gate`, `docs` (rustdoc mit `-D warnings`), `test` auf Windows/Linux/macOS (clippy, Tests, Offscreen-Tests mit erzwungenem CPU-Adapter unter Windows/Linux, Beispiele bauen, Float-Probe als Artefakt `float-probe-<os>`), `float-compare` (Vergleich im Job-Summary) |
| `nightly.yml` | täglich 02:17 UTC, manuell | Tests im Release-Modus auf drei Systemen plus Float-Vergleich; geplante Läufe entfallen, wenn `main` 24 h nicht bewegt wurde (Push oder Merge laut Aktivitäts-API, nicht Commit-Datum) |
| `release.yml` | Tag `vX.Y.Z` | Versionsprüfung, Testsuite (Linux), Release-Notes per git-cliff, GitHub-Release (nur Quelltext) |

- Commits, die nur Markdown oder `docs/` ändern, lösen `ci.yml` nicht aus. **Achtung Branch-Schutz:**
  Ein per Pfadfilter übersprungener Workflow meldet keinen Status. Pull Requests, die ausschließlich
  Doku ändern, bleiben dann bei Pflicht-Checks auf „Expected“ stehen und brauchen einen manuellen
  Lauf (`gh workflow run ci.yml --ref <branch>`) oder einen Admin-Merge.
- Ein neuer Push auf denselben Pull Request bricht dessen laufende CI ab. Läufe auf `main` werden
  **nie** abgebrochen; jeder `main`-Commit bekommt ein Ergebnis.
- GPU-Tests überspringen sich ohne Adapter und schreiben dann eine `::warning::`-Zeile ins Log. Linux
  bekommt per `mesa-vulkan-drivers` (lavapipe) ein Software-Vulkan, Windows-Runner haben WARP; dort setzt
  die CI `GRIMOIRE_REQUIRE_GPU_ADAPTER=1`, sodass ein fehlender Adapter die Tests scheitern lässt. Lokal
  lässt sich dasselbe mit `GRIMOIRE_GPU_ADAPTER=software GRIMOIRE_REQUIRE_GPU_ADAPTER=1 cargo test` prüfen.
  Unter Windows und Linux führt die CI zusätzlich `cargo test --workspace --test offscreen` mit
  `GRIMOIRE_GPU_ADAPTER=software` und `GRIMOIRE_REQUIRE_GPU_ADAPTER=1` aus und belegt so den erzwungenen
  CPU-Adapter (WARP bzw. lavapipe), auf den sich lokale Testläufe verlassen.
- **Float-Probe:** Die Core-Tests schreiben `target/float-probe/std-trig-<os>-<arch>.txt` und
  `core-<os>-<arch>.txt`. `float-compare` stellt die `std_trig_hash`-Werte nebeneinander. Eine
  Abweichung zwischen Systemen ist ein **Hinweis** (Eingabe für OF-2.1), kein roter Lauf; rot werden
  die Golden-Assertions in den Tests selbst.
- **Kosten:** In privaten Repos zählen Linux-Minuten einfach, Windows doppelt, macOS zehnfach. Teure
  Zusatzjobs gehören in die Nightly, nicht in `ci.yml`.

## CI-Überwachung (Pflicht)

**Ein Push ohne Nachsehen zählt nicht als fertig.** Jeder Push, der eine CI auslöst, wird überwacht,
bis der Lauf durch ist:

```bash
git push
run_id=$(gh run list --commit "$(git rev-parse HEAD)" --limit 1 --json databaseId --jq '.[0].databaseId')
gh run watch "$run_id" --exit-status
```

(Direkt nach dem Push kann es ein paar Sekunden dauern, bis der Lauf gelistet ist. Interaktiv genügt
`gh run watch --exit-status` mit Auswahl.)

Ist der Lauf rot, wird **zuerst analysiert**, bevor irgendetwas anderes passiert:

```bash
gh run view "$run_id" --log-failed
```

Die Ursache wird benannt und einer Kategorie zugeordnet: **Code** (unsere Änderung),
**Vorrichtung** (Workflow, Cache, Runner-Konfiguration) oder **fremd** (Dienst gestört, Zeitfehler,
Runner-Image geändert). Auch eine fremde Ursache wird festgehalten, zusammen mit der Antwort, ob der
Test das künftig aushalten soll. „Rot, schaue ich später an“ gibt es nicht: Zwei Commits später
steckt die Ursache unter fremden Änderungen.

## Golden-Master und Referenzwerte

Golden-Werte (Zustands-Hashes, `std_trig_hash`, später Replays und Render-Snapshots) sind
eingefrorene Erwartungen; die Tests schlagen bei jeder Abweichung fehl. Erneuert wird **nur bewusst**:

1. **Ursache verstehen und benennen.** Ist die Abweichung gewollt (Verhaltensänderung), ein Fehler
   oder plattformabhängig? Nur der erste Fall rechtfertigt eine Erneuerung.
2. **Eigener Commit, direkt nach der verursachenden Änderung, im selben Push/PR:**
   `test(golden): renew <was> after <warum>`. Der Rumpf nennt für jeden Wert *alt → neu* und die
   Begründung. Keine anderen Änderungen im selben Commit.
3. **Nie für eine einzelne Plattform.** Weicht nur ein System ab, ist das ein Determinismus-Befund
   (ADR-0005, OF-2.1) und wird als Issue oder ADR behandelt, nicht per Golden-Update versteckt.
4. **Nie, um CI grün zu bekommen.** Agenten erneuern Golden-Werte nicht eigenmächtig; die
   Entscheidung trifft der PO.
5. Ändert sich ein Hash-Algorithmus oder deterministisches Verhalten (etwa `StableHasher`-Version,
   `dmath`, RNG-Streams, Sim-Reihenfolge), werden Replays und Golden-Master des Spiels ungültig: Das
   ist eine **inkompatible Änderung** (siehe SemVer) mit CHANGELOG-Eintrag.

## Release

Voraussetzung: Der letzte CI-Lauf auf `main` ist grün.

1. **Version heben** in `Cargo.toml` unter `[workspace.package]` (`version = "X.Y.Z"`), dann
   `cargo check --workspace`, damit `Cargo.lock` die neuen Crate-Versionen enthält.
2. **CHANGELOG pflegen:** `[Unreleased]` in `## [X.Y.Z] - JJJJ-MM-TT` umbenennen, einen neuen leeren
   `[Unreleased]`-Abschnitt anlegen, inkompatible Änderungen mit Migrationshinweis aufführen. Einen
   Entwurf liefert `git cliff --unreleased --tag vX.Y.Z --strip header` (falls git-cliff lokal
   installiert ist).
3. **Commit und Push:** `chore(release): vX.Y.Z`, pushen, CI-Lauf überwachen (siehe oben).
4. **Tag setzen und pushen:**
   ```bash
   git tag -a vX.Y.Z -m "Grimoire vX.Y.Z"
   git push origin vX.Y.Z
   ```
5. **Release-Lauf überwachen:**
   ```bash
   run_id=$(gh run list --workflow release.yml --limit 1 --json databaseId --jq '.[0].databaseId')
   gh run watch "$run_id" --exit-status
   gh release view vX.Y.Z
   ```

Scheitert die Versionsprüfung, wurde falsch getaggt: Tag lokal und remote löschen
(`git tag -d vX.Y.Z`, `git push origin :refs/tags/vX.Y.Z`), korrigieren, neu taggen. Das ist nur
erlaubt, solange kein Release zu dem Tag existiert. **Ein veröffentlichter Tag wird nie verschoben
oder gelöscht**, denn das Spiel pinnt ihn (ADR-0009 im Spiel-Repo). Fehler danach behebt eine neue
Patch-Version.

## SemVer-Politik

- **Vor 1.0 (`0.MINOR.PATCH`):** Eine inkompatible Änderung hebt **MINOR** (`0.3.2` → `0.4.0`) und
  braucht einen CHANGELOG-Eintrag mit Migrationshinweis. Kompatible Funktionen und Fehlerbehebungen
  heben **PATCH**.
- **Ab 1.0:** reguläres SemVer (inkompatibel → MAJOR, Funktion → MINOR, Fehlerbehebung → PATCH).
- **Als inkompatibel gilt:** Entfernen oder Signaturänderung öffentlicher API der `grimoire*`-Crates;
  geänderte deterministische Ergebnisse (Hash-Werte, RNG-Streams, Sim-Reihenfolge), weil sie Replays
  und Golden-Master des Spiels brechen; geänderte Formate (Snapshots, Replays, Packs); ein höheres
  `rust-version`; geänderte Standard-Features.
- Das Spiel pinnt exakte Tags; auch ein Patch-Release erreicht es erst durch einen bewussten
  Upgrade-Commit.

## Versionen der GitHub Actions

Die Actions von GitHub selbst sind auf Major-Tags gepinnt (`actions/checkout@v7`,
`actions/upload-artifact@v7`, `actions/download-artifact@v8`). Actions von Drittanbietern sind auf den
vollständigen Commit-SHA gepinnt, mit der Version als Kommentar dahinter, etwa
`Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6 # v2.9.2` und
`orhun/git-cliff-action@3d96a18cc4ec17e9dc69ddcc424ccafaf1f78ce2 # v4.9.0`: Ein Tag lässt sich nachträglich
auf anderen Code umhängen, ein SHA nicht.

Aktuelle Stände prüfen mit `gh api repos/<owner>/<repo>/releases/latest --jq .tag_name`; den Commit-SHA
eines Tags liefert `gh api repos/<owner>/<repo>/commits/<tag> --jq .sha` (auch bei annotierten Tags). Ein
Versionswechsel einer SHA-gepinnten Action ersetzt SHA und Kommentar in allen Workflows (`ci.yml`,
`nightly.yml`, `release.yml`) gemeinsam. Ein Major-Wechsel ist ein eigener `ci:`-Commit, nachdem die
Release-Notes auf Breaking Changes gelesen wurden.
