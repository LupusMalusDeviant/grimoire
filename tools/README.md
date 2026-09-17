# Grimoire-Werkzeuge (C#)

C#-Suite der Engine (Plan 0002 WP9.1, PO-Entscheid P-1): Sie liegt im Engine-Repo, weil Protokoll und
Formate mit der Engine versioniert sind. Sie hängt nur über Formatdokumente, Golden-Fixtures und
generierte Codecs an der Engine, nie über eine Cargo-Kante (Vertrag §1).

| Projekt | Inhalt |
|---|---|
| `src/Grimoire.Formats` | Debug-Protokoll v1 (Frames, Nachrichten, Handshake der Werkzeugseite) und Pack v1 (Leser, Schreiber, `AssetId`), dazu die aus `schema/*.gschema` generierten Codecs unter `Generated/` |
| `src/Grimoire.LiveLink` | Live-Link-Client: Verbindung, Handshake, Reconnect mit Backoff, typisierte Nachrichten, Hot-Swap, Degradieren auf Dateiarbeit bei Abriss (PRD-0016 FR-03) |
| `tests/Grimoire.Formats.Tests` | Konformanz gegen die Rust-Golden-Fixtures von `grimoire_debug` und `grimoire_assets`, Grenzen und feindliche Eingaben |
| `tests/Grimoire.LiveLink.Tests` | Client gegen ein Engine-Double nach Vertrag §13; TCP-Tests nur mit `GRIMOIRE_SOCKET_TESTS=1` |

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
