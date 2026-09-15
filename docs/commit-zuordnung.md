# Commit-Zuordnung

Die Git-Historie dieses Repos wurde bei der Veröffentlichung neu geschrieben und in das öffentliche
Repo neu eingespielt. Dadurch haben alle Commits aus der Zeit davor neue Commit-IDs. Die Dokumente in
diesem Repo nennen bereits die neuen IDs.

Verweise auf CI-Läufe aus der Zeit vor der Veröffentlichung, etwa die Läufe 34883182308 und
34893513991 in [Engine-ADR-0004](adr/0004-deterministische-gleitkommaarithmetik.md), zeigen auf das
nicht öffentliche Archiv-Repo. Im öffentlichen Repo gibt es diese Läufe nicht, und ihre Protokolle
nennen die alten Commit-IDs.

Die Tabelle ordnet jeder Commit-ID, die ein Dokument dieses Repos nennt, die alte ID zu. Stellen auf
den P1-Branches sind mit dem Branch-Namen angegeben.

| Alt | Neu | Erwähnt in |
|-----|-----|------------|
| `d7c66b2` | `0ef7696` | `docs/adr/0004-deterministische-gleitkommaarithmetik.md` (Lauf 34883182308); Testkommentar in `crates/grimoire_sim/tests/determinism.rs`, auf `p1/wp1.0-scheduler` und `p1/wp1.2-contracts-draft` in `crates/grimoire_sim/tests/scenario/mod.rs` |
| `53acf0b` | `b5ba0fe` | `docs/adr/0004-deterministische-gleitkommaarithmetik.md` (Lauf 34893513991, `chore(release): v0.1.0`) |
| `d806f05` | `70a7fb0` | `docs/architektur/entwurf-paralleler-scheduler.md` (`p1/wp1.0-scheduler`, `p1/wp1.2-contracts-draft`) |
| `d9e54e6` | `f5c9bf5` | `docs/adr/0008-crate-map-erweiterung-p1.md` und `docs/architektur/crate-vertraege.md` (`p1/wp1.2-contracts-draft`) |
| `c49e002` | `c3239ad` | `docs/spikes/of-4.1-sigil-quelltextsyntax.md` (`p1/wp1.4-sigil-syntax-spike`) |
| `b78502f` | `8558c7d` | `docs/spikes/of-4.1-sigil-quelltextsyntax.md` (`p1/wp1.4-sigil-syntax-spike`) |
| `2af98ab` | `1afbfaa` | `docs/spikes/of-4.1-sigil-quelltextsyntax.md` (`p1/wp1.4-sigil-syntax-spike`) |
