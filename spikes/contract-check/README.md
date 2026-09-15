# Vertrags-Compile-Check (Spike)

Dieser Ordner prüft, ob die Rust-Blöcke aus `docs/architektur/crate-vertraege.md` (Entwurf WP1.2) als
Signaturen übersetzbar und untereinander stimmig sind. Er prüft **nur Signaturen**: Rümpfe sind Platzhalter
(meist `unimplemented!()`), Semantik, Byte-Layouts und Leistung prüft er nicht.

**Das ist nicht WP1.3.** Hier entstehen keine Vertrags-Skelette im Workspace der Engine, keine
Null-Implementierungen und keine Konformanz-Suiten. WP1.3 baut die echten Crates unter `crates/` neu; dieser
Ordner ist danach nur noch Referenz und kann entfallen.

## Aufbau

- Eigener Workspace (`[workspace]` in `Cargo.toml`), kein Mitglied des Wurzel-Workspaces. Die Wurzel-CI (rustfmt,
  Clippy, Tests, `check-thread-source.sh`, Standalone-Gate) erfasst ihn nicht.
- `*_p1`-Crates spiegeln P1-Ergänzungen bestehender Crates und hängen per relativem Pfad an den echten Crates.
- Neue Crates (`grimoire_collide`, `grimoire_sigil`, `grimoire_assets`, `grimoire_debug`, `grimoire_sigilc`,
  `grimoire_link`, `grimoire_bench`) tragen die Namen aus dem Vertrag.
- `p0_callers` enthält Aufrufer im Stil von P0 und fremde Implementierungen der P1-Traits, damit Additivität und
  Implementierbarkeit von außen mitgeprüft werden.

## Aufruf

Aus diesem Ordner, mit eigenem Zielverzeichnis (nicht `target/` der Wurzel, um veraltete Artefakte zu vermeiden):

```bash
CARGO_TARGET_DIR=<temporäres Verzeichnis> cargo check --workspace --all-targets
CARGO_TARGET_DIR=<temporäres Verzeichnis> cargo clippy --workspace --all-targets -- -D warnings
```

## Features

- `expect-fail-stage-info`, `expect-fail-camera-field` (`p0_callers`): müssen **fehlschlagen**. Sie belegen Grenzen,
  die der Vertrag bewusst setzt (Ausgabetyp ohne Konstruktor, §2 Regel 13; kein Kamerafeld vor WP2.2, §9.2).
  `--all-features` schlägt deshalb absichtlich fehl.
- `wp2-2-camera` (`grimoire_render_p1`, `grimoire_p1`): Platzhalter `Camera25D` und `sample_aim`, die der Vertrag
  erst mit WP2.2 festlegt.
- `tcp`, `fixtures`, `debug-link`, `conformance`: wie im Vertrag (§2 Regel 14).

## Nicht abgebildet

- Dev-Kanten des Hash-Gates (`grimoire_exec/tests/hash_gate.rs`, §1, §11.7) und der Kanten-Check der CI.
- `sha2`, Prüfsummen und `content_hash` (Stub), Kodierung und Kürzung im Debug-Protokoll.
- Laufzeitverhalten: Nur ein Test (`read_limited` mit `u64::MAX`) läuft wirklich.
