# ADR-0009: Lizenz der Engine — Alle Rechte vorbehalten

- **Status:** Akzeptiert (PO-Entscheidung vom 2026-09-15). Nummer vorläufig: `main` endet bei 0006, die P1-Branches belegen 0007 und 0008; die endgültige Nummer steht beim Merge fest.
- **Datum:** 2026-09-15
- **Entscheider:** Lupus Malus Deviant (PO), vorbereitet durch Claude
- **Bezug:** Spiel-Repo `fiends-n-patrons`: `docs/adr/0012-lizenz-alle-rechte-vorbehalten.md` (dieselbe Entscheidung für beide Repos), `docs/plans/0001-phase-p0-fundament.md` (WP1.4), `docs/plans/0002-m0-eintritts-check.md` (P0-Rest R-09), `docs/prd/0001-vision-und-scope.md`, `docs/adr/0009-engine-pin-ueber-git-tag.md`; [CONTRIBUTING.md](../../CONTRIBUTING.md) (Release)

## Kontext

Grimoire war seit dem Anlegen ein privates GitHub-Repo. Plan 0001 im Spiel-Repo verlangt in WP1.4
einen dokumentierten Lizenzentscheid und nennt als Empfehlung „privat/All rights reserved“.
Festgehalten wurde der Entscheid nie. Das `README.md` schloss den Statusabsatz mit „Alle Rechte
vorbehalten“, nannte aber weder Rechteinhaber noch Jahr. Es gab keine `LICENSE`-Datei und kein
`license`- oder `license-file`-Feld in `Cargo.toml`. Der M0-Eintritts-Check führt das als P0-Rest R-09.

Am 2026-09-15 hat der PO entschieden, beide Repos als neue öffentliche GitHub-Repos mit bereinigter
Historie zu veröffentlichen. Die bisherigen privaten Repos bleiben als Archiv. Der Grund: Das
Kontingent an Actions-Minuten des privaten Tarifs ist fast aufgebraucht, öffentliche Repos erhalten
gehostete Standard-Runner kostenlos. Damit fällt die Prämisse „privat“ weg, und die Lizenzfrage muss vor dem Umschalten
beantwortet sein.

Zur Rechtslage, so wie die Quellen sie beschreiben (keine Rechtsberatung): Ohne Lizenz gilt das
Urheberrecht, niemand darf den Code vervielfältigen, verbreiten oder bearbeiten
([GitHub Docs: Licensing a repository](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/licensing-a-repository),
[choosealicense.com: No License](https://choosealicense.com/no-permission/)). Ein öffentliches Repo
erlaubt anderen GitHub-Nutzern nach den GitHub-Nutzungsbedingungen (Abschnitt D.5) nur, es auf GitHub
anzusehen und zu forken. Eine freie Lizenz lässt sich jederzeit nachträglich erteilen, für bereits
genommene Kopien aber nicht zurückholen.

Für öffentliche Rust-Bibliotheken empfehlen die Rust API Guidelines (C-PERMISSIVE) die Doppellizenz
`MIT OR Apache-2.0`. Grimoire hat jedoch genau einen Konsumenten, wird nicht auf crates.io
veröffentlicht (`publish = false`), und das Spiel pinnt Release-Tags über Git (Spiel-ADR-0009).

## Anforderungen

- Der Lizenzstand ist vor dem Umschalten auf öffentlich eindeutig und an einer auffindbaren Stelle festgehalten (R-09).
- Keine Rechte einräumen, die sich später nicht zurücknehmen lassen, solange der Vertrieb des Spiels offen ist (PRD-0001 hält einen Verkauf ab Phase 2 offen).
- GitHub und Cargo finden die Lizenzangabe maschinell: `LICENSE` im Wurzelverzeichnis, Feld im Manifest.
- Veröffentlichte Tags werden nicht verschoben (`CONTRIBUTING.md`, Abschnitt Release).
- Öffentliche Nightly- und Release-Binaries bleiben erlaubt (PO-Entscheidung vom 2026-09-15).

## Optionen

1. **Status quo: nur der Satz im README** — rechtlich ebenso restriktiv, aber ohne Rechteinhaber,
   Jahr und Datei. GitHub zeigt keine Lizenz an, R-09 bliebe offen.
2. **Alle Rechte vorbehalten, ausdrücklich (gewählt)** — `LICENSE` mit Copyright-Zeile und klarem
   Vorbehalt, `license-file` im Manifest. Räumt nichts ein und lässt jede spätere Lizenz offen.
3. **Permissiv (`MIT OR Apache-2.0`)** — im Rust-Ökosystem üblich, erlaubt Wiederverwendung und
   Beiträge ohne Rückfrage. Für genommene Kopien unwiderruflich; eine freie Weitergabe der Engine hat
   der PO nicht beschlossen.
4. **Source-available (etwa PolyForm Noncommercial oder BUSL)** — erlaubt eine eingeschränkte
   Nutzung. Bringt Bedingungen mit, die sorgfältig gewählt werden müssen, und löst kein aktuelles
   Problem.

## Entscheidung

Option 2.

- **Rechteinhaber** ist Lupus Malus Deviant, Jahr 2026. Die Datei `LICENSE` im Wurzelverzeichnis
  enthält einen englischen Vorbehaltstext ohne Erlaubnisse, einen Haftungsausschluss, den Hinweis,
  dass Fremdabhängigkeiten ihren eigenen Lizenzen unterliegen, und eine deutsche Kurzfassung.
  Maßgeblich ist der englische Text.
- **Manifest:** `license-file = "LICENSE"` in `[workspace.package]`; jede Crate erbt das Feld mit
  `license-file.workspace = true`, wie die übrigen geerbten Felder. Ein `license`-Feld entfällt, weil
  es für „Alle Rechte vorbehalten“ keinen SPDX-Bezeichner gibt. `publish = false` bleibt. Geprüft mit
  Cargo 1.98.1: `cargo metadata` meldet für jede Crate `license_file` `../../LICENSE` ohne Warnung,
  und `cargo package --list -p grimoire_core` führt `LICENSE` im Paket.
- **README:** ein Abschnitt „Lizenz“ ersetzt den Satz im Statusabsatz.
- **Geltungsbereich:** der gesamte Repo-Inhalt (Code, Shader, Doku, Daten, Build-Konfiguration) in
  allen Commits, Branches, Tags und Releases. Das gilt auch für `v0.1.0`: Der Tag enthält noch keine
  `LICENSE`-Datei, sein `README.md` sagt aber bereits „Alle Rechte vorbehalten“. Der Tag wird dafür
  nicht neu gesetzt; der erste Tag mit `LICENSE` ist `v0.1.1` (Scheduler, PO-Entscheidung P-7).
- **Neue Crates** (etwa `grimoire_exec` aus dem Scheduler-Branch) tragen `license-file.workspace = true`.
- Eine spätere Lizenzänderung ist ein neues ADR, das dieses ersetzt.

## Konsequenzen

- (+) R-09 ist für die Engine geschlossen. Der Lizenzstand ist vor der Veröffentlichung eindeutig,
  GitHub zeigt die `LICENSE`-Datei an.
- (+) Jede spätere Lizenz bleibt möglich und kann für alle bisherigen Stände erteilt werden, ohne
  Tags zu verschieben.
- (−) Außer dem Rechteinhaber darf niemand die Engine nutzen, bearbeiten oder weiterverbreiten. Forks
  auf GitHub bleiben nach den Nutzungsbedingungen möglich, räumen aber keine Nutzungsrechte ein.
- (−) Ein externer Pull Request bringt ohne Vereinbarung keine Rechteeinräumung mit. Deshalb werden
  Pull Requests von außen vorerst nicht angenommen; Issues für Hinweise und Fehlerberichte bleiben
  offen (PO-Entscheidung vom 2026-09-15, [CONTRIBUTING.md](../../CONTRIBUTING.md)).
- (−) Öffentliche Binaries räumen ebenfalls keine Rechte ein. Die Engine veröffentlicht bisher keine
  Binaries (Releases nur mit Notizen). Sobald sie es tut, verlangen die Lizenzen der gelinkten
  Fremd-Crates (MIT, BSD, ISC, Apache-2.0, Unicode-3.0) die Mitlieferung ihrer Lizenzhinweise.
- (−) Die Empfehlung C-PERMISSIVE der Rust API Guidelines wird bewusst nicht befolgt.
