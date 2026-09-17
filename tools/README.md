# Grimoire-Werkzeuge (C#)

C#-Suite der Engine (Plan 0002 WP9.1, PO-Entscheid P-1): Sie liegt im Engine-Repo, weil Protokoll und
Formate mit der Engine versioniert sind. Sie hängt nur über Formatdokumente, Golden-Fixtures und
generierte Codecs an der Engine, nie über eine Cargo-Kante (Vertrag §1).

| Projekt | Inhalt |
|---|---|
| `src/Grimoire.Formats` | Debug-Protokoll v1 (Frames, Nachrichten, Handshake der Werkzeugseite) und Pack v1 (Leser, Schreiber, `AssetId`), dazu die aus `schema/*.gschema` generierten Codecs unter `Generated/` |
| `src/Grimoire.LiveLink` | Live-Link-Client: Verbindung, Handshake, Reconnect mit Backoff, typisierte Nachrichten, Hot-Swap, Degradieren auf Dateiarbeit bei Abriss (PRD-0016 FR-03) |
| `src/Grimoire.AssetCompiler` | Asset-Compiler `grimoire-ac`: Content-Discovery, `sigilc`-Orchestrierung, Pack und Manifest, `watch --push` (Plan 0002 WP9.2) |
| `tests/Grimoire.Formats.Tests` | Konformanz gegen die Rust-Golden-Fixtures von `grimoire_debug` und `grimoire_assets`, Grenzen und feindliche Eingaben |
| `tests/Grimoire.LiveLink.Tests` | Client gegen ein Engine-Double nach Vertrag §13; TCP-Tests nur mit `GRIMOIRE_SOCKET_TESTS=1` |
| `tests/Grimoire.AssetCompiler.Tests` | Discovery, Pack-Erzeugung und Watch-Schleife gegen einen Compiler-Doppelgänger, dazu der WP4.1-Konformitätskorpus gegen das echte `sigilc` |

## Bauen und testen

Aus diesem Verzeichnis, mit dem in `global.json` gepinnten SDK:

```bash
dotnet build Grimoire.Tools.slnx
dotnet test --solution Grimoire.Tools.slnx
```

Die Socket-Tests laufen nur mit `GRIMOIRE_SOCKET_TESTS=1`; das setzt ausschließlich die CI
(`.github/workflows/tools.yml`, Windows, Linux und macOS). Lokal werden sie übersprungen.

## Festlegungen

- **SDK:** `global.json` pinnt .NET SDK `10.0.111` exakt (PO-Entscheid P-13: .NET 10 LTS, exakt gepinnt).
  Das ist das Feature-Band 1xx, das erste und am längsten gewartete Band des LTS-Release; die höheren
  Bänder bringen neuere MSBuild- und NuGet-Komponenten für neuere Visual-Studio-Versionen mit, die die Suite
  nicht braucht. `dotnet test` läuft über Microsoft.Testing.Platform (ebenfalls `global.json`).
- **Pakete:** zentral und exakt in `Directory.Packages.props`, aufgelöst in `packages.lock.json`; die CI
  stellt im Locked Mode wieder her. Die Bibliotheken haben keine Drittabhängigkeiten.
- **Warnungen** sind Fehler, öffentliche Typen brauchen XML-Dokumentation (wie `missing_docs`).
- **Engine-Version:** `Directory.Build.props` liest die Workspace-Version aus `../Cargo.toml`; der Handshake
  vergleicht sie byteweise mit `ENGINE_VERSION` (Vertrag §13). Werkzeuge werden deshalb je Engine-Tag neu
  gebaut. Der Build-Hash kommt wie in der Engine aus `GRIMOIRE_BUILD_HASH`, sonst `unknown`.
- **Generierter Code:** `cargo run -p grimoire_schemagen -- generate` im Repo-Wurzelverzeichnis schreibt
  `src/Grimoire.Formats/Generated/*.g.cs` neben den Rust-Codecs; `.github/scripts/check-schemagen-drift.sh`
  macht die CI rot, wenn der eingecheckte Stand abweicht. Nie von Hand ändern.
- **Fixtures:** Die Testprojekte kopieren die Golden-Fixtures beim Bauen aus `crates/` (auch die
  Byte-Literale von `grimoire_debug/tests/handshake_golden.rs`); es gibt keine zweite Kopie, die abweichen
  könnte.
- **Pfade:** alle relativ; die Suite baut an jedem Ort, an dem das Repo liegt.

## Asset-Compiler `grimoire-ac`

Sigil-Hoheit liegt bei `grimoire_sigilc` (Projekt-ADR-0010): `grimoire-ac` enthält keine Grammatik, keine
Validierungsregel und keinen Interpreter, sondern entdeckt Content, normalisiert Pfade, ruft
`sigilc build --json` und schreibt Pack und Manifest. Die Diagnosen des Compilers werden unverändert
durchgereicht — in JSON als dieselben Objekte, im Textmodus in der Form von `docs/formats/sigil.md` §6.1.
Ein Test vergleicht diese Ausgabe byteweise mit dem, was `sigilc check` selbst druckt.

```bash
# Alles unter content/ bauen; Pack und Zwischenstände landen unter packs/ (Build-Artefakt)
grimoire-ac build content/ --sigilc ../engine/target/release/sigilc

# Maschinenlesbar, mit Zeitmessung auf stderr
grimoire-ac build content/ --json --timings

# Eine Datei kompilieren und in die laufende Engine schieben
grimoire-ac push content/sigil/ring.sigil --root content --token "$GRIMOIRE_DEBUG_TOKEN"

# Beim Arbeiten: jede Änderung kompilieren und swappen
grimoire-ac watch --push content/
```

- **Exit-Codes** wie `sigilc` (sigil.md §13.1): `0` alles gut, `1` der Lauf fand Probleme (Diagnose,
  abgelehnter Swap), `2` der Lauf war so nicht durchführbar (Bedienfehler, fehlendes `sigilc`, nicht
  lesbarer Pfad). Jede Diagnose verhindert das Pack — ein Pack passt zu seinen Quellen vollständig oder
  wird nicht geschrieben.
- **Content-Discovery** nimmt den Baum, wie er ist: jede `.sigil`-Datei unterhalb der Wurzel ist eine
  Quelle, jede andere Datei wird als übersprungen gemeldet, versteckte Verzeichnisse bleiben unberührt.
  Nichts wird an einem festen Ort erwartet.
- **Eigene Diagnosen** (Codes `AC****`, damit kein Code zwei Bedeutungen hat): `AC0001` Pfad ist kein
  `AssetPath` (Vertrag §12) und muss umbenannt werden, `AC0002` zwei Dateien mit einem Asset-Pfad,
  `AC0003` die Unit ist nicht lesbar oder nennt eine andere Kennung als ihr Pfad, `AC0004` das Pack
  überschreitet eine Grenze aus Vertrag §12, `AC0005` das `sigilc`-Binary hat eine andere Version als
  dieser Build (`--allow-version-mismatch` erlaubt es), `AC0006` keine Quelldatei gefunden
  (`--allow-empty` erlaubt es).
- **Pack-Identität:** Der Pack-Eintrag trägt den Asset-Pfad der Quelle mit Endung `.sigil`, seine
  `AssetId` ist damit die `UnitId` der Unit (Vertrag §11.1, §12); die Artversion ist die
  `SigilUnit`-Formatversion aus dem Unit-Kopf. Der Compiler-Name im Manifest ist `grimoire-ac`, die
  Compiler-Version die des `sigilc`, das die Unit-Bytes erzeugt hat. Gleiche Quellen ergeben ein
  byte-gleiches Pack; Zeitmessungen stehen nur auf stderr, nie im Pack oder im Bericht.
- **Berichtsdokument** `--json` (`grimoire.ac.build`, Schema-Version 1): feste Schlüsselreihenfolge,
  Kennungen und Hashes als Kleinbuchstaben-Hex, keine Zeitstempel, die Diagnosen des Compilers unverändert
  eingebettet.
- **Messung:** Die Zeit eines vollständigen Pack-Rebuilds messen ausschließlich CI-Runner
  (`.github/scripts/measure-pack-rebuild.sh`, Ziel < 60 s aus PRD-0016), nie der Entwicklungsrechner.
